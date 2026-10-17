//! Lo que hace un nodo de Quantic en cada vuelta de su bucle: publicar las
//! transferencias que entraron por la API, limpiar el mempool cuando cambia la
//! punta, minar en segundo plano y publicar lo minado. El bucle del nodo lo
//! llama una vez por vuelta; así el de Zenith queda como estaba.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use zoka_quantic::orchard::circuit::ProvingKey;
use zoka_quantic::tx::QuanticTransaction;
use zoka_quantic::QuanticAddress;

use super::{mempool, miner, network_for_chain};
use crate::network::message::ZokaMessage;
use crate::network::p2p::Node;
use crate::storage::RocksDbStorage;
use crate::types::Block;

/// Acciones por bloque que arma el minero. El consenso admite 512, pero un
/// bloque viaja entero por gossip (1 MB, y el JSON lleva cada transacción en
/// hex): 96 acciones dejan el bloque por debajo de unos 850 KB.
pub const MINER_MAX_BLOCK_ACTIONS: usize = 96;
/// Cuánto busca el minero sobre una plantilla antes de rearmarla (para sumar
/// lo que llegó al mempool y actualizar la hora).
pub const TEMPLATE_LIFETIME: Duration = Duration::from_secs(20);

/// La clave de prueba de Ironwood (2,8 s en la PC), una sola vez por proceso.
pub fn proving_key() -> &'static ProvingKey {
    static KEY: OnceLock<ProvingKey> = OnceLock::new();
    KEY.get_or_init(zoka_quantic::verify::proving_key)
}

/// Transferencias que la API ya admitió, para publicarlas desde el bucle.
static OUTBOUND: OnceLock<mpsc::UnboundedSender<Vec<u8>>> = OnceLock::new();

/// Admite una transferencia que llegó por la API y la deja lista para
/// publicarla. Devuelve su id.
pub fn submit_from_api(
    storage: &RocksDbStorage,
    chain_id: &str,
    bytes: &[u8],
) -> Result<String, mempool::MempoolError> {
    let tx = QuanticTransaction::from_bytes(bytes).map_err(|e| mempool::MempoolError::Decode(e.to_string()))?;
    let txid = mempool::global()
        .lock()
        .map_err(|_| mempool::MempoolError::Storage("mempool lock poisoned".into()))?
        .insert(storage, chain_id, tx, now_secs())?;
    if let Some(outbound) = OUTBOUND.get() {
        let _ = outbound.send(bytes.to_vec());
    }
    Ok(txid.to_hex())
}

/// Una transferencia que llegó por la red. El gossip ya la reenvió; acá solo
/// decide si entra al mempool.
pub fn accept_network_transaction(storage: &RocksDbStorage, chain_id: &str, tx_hex: &str) -> Option<String> {
    let bytes = hex::decode(tx_hex).ok()?;
    let tx = QuanticTransaction::from_bytes(&bytes).ok()?;
    let mut pool = mempool::global().lock().ok()?;
    match pool.insert(storage, chain_id, tx, now_secs()) {
        Ok(txid) => Some(format!("Quantic transaction {} entered the mempool.", &txid.to_hex()[..12])),
        Err(mempool::MempoolError::AlreadyKnown) => None,
        Err(error) => Some(format!("Ignored Quantic transaction: {error}")),
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Clone, Debug)]
pub struct QuanticMiningConfig {
    pub address: QuanticAddress,
    pub threads: usize,
    /// Pausa entre bloques propios (0: seguido).
    pub interval: Duration,
    /// Pares mínimos para minar (0 en una red local o en la autoridad).
    pub min_peers: usize,
}

type MiningJob = tokio::task::JoinHandle<Result<Option<Block>, String>>;

pub struct QuanticRuntime {
    chain_id: String,
    mining: Option<QuanticMiningConfig>,
    job: Option<MiningJob>,
    stop: Arc<AtomicBool>,
    next_mine_at: Instant,
    last_tip: Option<String>,
    outbound: mpsc::UnboundedReceiver<Vec<u8>>,
}

/// Lo que pasó en una vuelta.
#[derive(Default, Debug)]
pub struct TickOutcome {
    pub events: Vec<String>,
    /// La punta avanzó con un bloque propio (ya publicado).
    pub mined: Option<(u64, String)>,
}

impl QuanticRuntime {
    /// Una sola por proceso: la API publica por el canal de esta.
    pub fn new(chain_id: &str, mining: Option<QuanticMiningConfig>) -> Result<Self, String> {
        let network = network_for_chain(chain_id).ok_or_else(|| format!("{chain_id} is not a Quantic chain"))?;
        if let Some(config) = &mining {
            if config.address.network() != network {
                return Err(format!(
                    "the mining address is for {}, this node follows {chain_id}",
                    config.address.network()
                ));
            }
            if super::consensus_params(network).premine.is_none() && network != zoka_quantic::Network::Regtest {
                return Err(format!(
                    "{chain_id} has no genesis yet: mining would create one nobody announced"
                ));
            }
        }
        let (sender, outbound) = mpsc::unbounded_channel();
        OUTBOUND
            .set(sender)
            .map_err(|_| "the Quantic runtime already exists in this process".to_string())?;
        Ok(Self {
            chain_id: chain_id.to_string(),
            mining,
            job: None,
            stop: Arc::new(AtomicBool::new(false)),
            next_mine_at: Instant::now(),
            last_tip: None,
            outbound,
        })
    }

    pub fn is_mining(&self) -> bool {
        self.mining.is_some()
    }

    pub async fn tick(&mut self, node: &mut Node, storage: &Arc<RocksDbStorage>) -> TickOutcome {
        let mut outcome = TickOutcome::default();

        // 1. Lo que entró por la API sale a la red.
        while let Ok(bytes) = self.outbound.try_recv() {
            let message = ZokaMessage::QuanticTransaction {
                tx_hex: hex::encode(&bytes),
            };
            if let Err(error) = node.publish_message(&message) {
                outcome.events.push(format!("Quantic transaction not published yet: {error}"));
            }
        }

        // 2. Punta nueva: el mempool se limpia y el minero deja la plantilla vieja.
        let tip = storage.get_last_block_hash().ok().filter(|hash| !hash.is_empty());
        if tip != self.last_tip {
            self.last_tip = tip;
            self.stop.store(true, Ordering::Relaxed);
            if let Ok(mut pool) = mempool::global().lock() {
                let removed = pool.revalidate(storage, &self.chain_id);
                if removed > 0 {
                    outcome.events.push(format!("{removed} Quantic transaction(s) left the mempool."));
                }
            }
        }

        // 3. El trabajo de minería que terminó.
        if self.job.as_ref().is_some_and(|job| job.is_finished()) {
            let job = self.job.take().expect("checked above");
            match job.await {
                Ok(Ok(Some(block))) => match crate::state::chain::apply_block(storage, &self.chain_id, &block) {
                    Ok(head) => {
                        let payload = serde_json::to_vec(&block).unwrap_or_default();
                        let _ = node.publish_block_response(block.header.block_height, block.header.hash.clone(), payload);
                        let _ = node.publish_block_announce(
                            head.height,
                            head.hash.clone(),
                            head.cumulative_work.to_string(),
                            None,
                        );
                        let _ = node.publish_sync_state(head.height, head.hash.clone());
                        outcome.events.push(format!(
                            "Mined Quantic block {} ({} transfer(s)).",
                            head.height,
                            block.quantic_transactions.len().saturating_sub(1)
                        ));
                        outcome.mined = Some((head.height, head.hash));
                        if let Some(config) = &self.mining {
                            self.next_mine_at = Instant::now() + config.interval;
                        }
                    }
                    Err(error) => outcome.events.push(format!("Mined Quantic block was refused: {error}")),
                },
                Ok(Ok(None)) => {}
                Ok(Err(error)) => {
                    outcome.events.push(format!("Quantic mining stopped: {error}"));
                    self.next_mine_at = Instant::now() + Duration::from_secs(10);
                }
                Err(error) => outcome.events.push(format!("Quantic mining task failed: {error}")),
            }
        }

        // 4. Un trabajo nuevo, si toca.
        if let Some(config) = self.mining.clone() {
            let ready = self.job.is_none()
                && Instant::now() >= self.next_mine_at
                && node.connected_peer_count() >= config.min_peers;
            if ready {
                self.stop = Arc::new(AtomicBool::new(false));
                let stop = Arc::clone(&self.stop);
                let storage = Arc::clone(storage);
                let chain_id = self.chain_id.clone();
                let transactions = mempool::global()
                    .lock()
                    .map(|pool| pool.select(MINER_MAX_BLOCK_ACTIONS))
                    .unwrap_or_default();
                self.job = Some(tokio::task::spawn_blocking(move || {
                    mine_once(&storage, &chain_id, &config, transactions, &stop)
                }));
            }
        }
        outcome
    }
}

impl Drop for QuanticRuntime {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Arma una plantilla y busca el nonce hasta resolverlo, vencer la plantilla
/// o que cambie la punta. `None`: no se resolvió (se rearma en la próxima vuelta).
fn mine_once(
    storage: &RocksDbStorage,
    chain_id: &str,
    config: &QuanticMiningConfig,
    transactions: Vec<QuanticTransaction>,
    stop: &AtomicBool,
) -> Result<Option<Block>, String> {
    let timestamp = now_secs();
    let (mut block, _) = miner::build_template(storage, chain_id, &config.address, proving_key(), transactions, timestamp)?;
    // La misma regla con la que el nodo acepta el bloque.
    let difficulty = crate::mining::difficulty::expected_for_next_block_for_chain(storage, chain_id)?;
    match miner::solve_pow_parallel(chain_id, &mut block, difficulty, config.threads, TEMPLATE_LIFETIME, stop)? {
        miner::PowOutcome::Solved => Ok(Some(block)),
        miner::PowOutcome::Interrupted => Ok(None),
    }
}
