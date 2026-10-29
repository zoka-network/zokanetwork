//! Servidor Stratum de Zoka Quantic (`docs/QUANTIC-POOL-DE-MINERIA.md`): XMRig,
//! y cualquier minero `rx/0` que hable el protocolo de pools de Monero, mina
//! contra este nodo.
//!
//! - `login` con una dirección de Quantic, completa o corta (la corta se
//!   resuelve con el directorio del nodo): el coinbase le paga a esa. Si el
//!   login no es una dirección, a la de `--quantic-mine-to` del nodo.
//! - Cada conexión tiene su propio extranonce (los 32 bits de arriba del nonce
//!   del encabezado), así que recorre su propio espacio sin tocar el coinbase.
//! - `job`: el bloque de trabajo de 76 bytes con el nonce en cero, la semilla
//!   de RandomX de la época, la altura y el objetivo de los aciertos (16 veces
//!   más fácil que el de la red, para ver que el minero trabaja).
//! - `submit`: el nodo recalcula el hash. Si no coincide con el del minero, el
//!   formato no es el que el minero cree y se rechaza; si alcanza el objetivo
//!   de la red, el bloque va al runtime, que lo aplica y lo publica.
//!
//! Escucha en `127.0.0.1` salvo que se pida otra cosa: abrirlo a la red local
//! es una decisión de quien corre el nodo.
//!
//! El protocolo es uno solo; lo que cambia con la cadena (con qué dirección
//! cobra un login, el bloque de trabajo y qué hacer con uno encontrado) es un
//! [`Backend`]: el de Ironwood está acá y el del pool STARK en `stark_stratum`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use num_bigint::BigUint;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use zoka_quantic::address::Recipient;
use zoka_quantic::{Network, QuanticAddress};

use super::{difficulty, mempool, miner, network_for_chain};
use crate::mining::pow::{self, PowEngine};
use crate::storage::RocksDbStorage;
use crate::types::{Block, Signature};

/// Los aciertos son tantas veces más fáciles que un bloque.
pub const SHARE_FACTOR_BITS: usize = 4;
/// Una plantilla se rehace al cambiar la punta o pasado este tiempo (para
/// sumar transferencias del mempool y mover la hora).
pub const TEMPLATE_REFRESH: Duration = Duration::from_secs(30);
/// Direcciones de cobro distintas a la vez: cada una necesita su coinbase
/// (una prueba Halo 2) en cada plantilla.
pub const MAX_PAYOUT_ADDRESSES: usize = 16;
/// Trabajos viejos que todavía aceptan aciertos (los que llegan tarde).
const RECENT_JOBS: usize = 8;
/// Una línea de JSON más larga que esto no es de un minero.
const MAX_LINE_BYTES: usize = 8 * 1024;
/// Conexiones a la vez. Abierto a la red local, nadie llena el nodo de
/// sesiones: la que pasa el tope se cierra al aceptarla.
pub const MAX_CONNECTIONS: usize = 256;

/// Un bloque que encontró un minero del Stratum, para que lo publique el runtime.
pub struct FoundBlock {
    pub block: Block,
    pub reward_atoms: u64,
}

static FOUND: OnceLock<Mutex<Vec<FoundBlock>>> = OnceLock::new();

/// Los bloques encontrados desde la última vez (los toma el runtime).
pub fn take_found() -> Vec<FoundBlock> {
    FOUND
        .get()
        .and_then(|found| found.lock().ok().map(|mut found| std::mem::take(&mut *found)))
        .unwrap_or_default()
}

fn push_found(found: FoundBlock) {
    if let Ok(mut list) = FOUND.get_or_init(Default::default).lock() {
        list.push(found);
    }
}

pub struct StratumConfig {
    pub bind: SocketAddr,
    pub default_address: Option<QuanticAddress>,
}

/// Lo que el Stratum necesita de una cadena. Todo corre en hilos de bloqueo
/// (`spawn_blocking`): puede esperar una prueba o un candado.
pub trait Backend: Send + Sync + 'static {
    fn chain_id(&self) -> &str;
    fn storage(&self) -> &RocksDbStorage;
    /// A qué dirección cobra un login, codificada: la clave de sus trabajos.
    fn payout_for(&self, login: &str) -> Result<String, String>;
    /// El bloque de `payout` sobre la punta, sin prueba de trabajo, y lo que
    /// cobra (subsidio y comisiones; el premine no cuenta).
    fn template(&self, payout: &str) -> Result<(Block, u64), String>;
    /// Un bloque que alcanzó el objetivo de la red, con su prueba de trabajo.
    /// Devuelve si cuenta como hallado: el backend STARK lo importa en el
    /// momento y solo cuenta el que queda canónico (una segunda solución del
    /// mismo trabajo queda como rama lateral).
    fn found(&self, block: Block, reward_atoms: u64) -> bool;
    /// Cada cuánto se rehace una plantilla aunque la punta no cambie.
    fn template_refresh(&self) -> Duration {
        TEMPLATE_REFRESH
    }
    /// Direcciones de cobro distintas a la vez (cada una, una plantilla).
    fn max_payout_addresses(&self) -> usize {
        MAX_PAYOUT_ADDRESSES
    }
}

/// La cadena de Ironwood: el mempool y el runtime del nodo de siempre.
struct IronwoodBackend {
    chain_id: String,
    network: Network,
    storage: Arc<RocksDbStorage>,
    default_address: Option<QuanticAddress>,
}

impl Backend for IronwoodBackend {
    fn chain_id(&self) -> &str {
        &self.chain_id
    }

    fn storage(&self) -> &RocksDbStorage {
        &self.storage
    }

    fn payout_for(&self, login: &str) -> Result<String, String> {
        let login = login.trim();
        let address = match Recipient::decode_for(login, self.network) {
            Ok(Recipient::Full(full)) => full,
            Ok(Recipient::Short(short)) => super::directory::lookup(&self.storage, &short)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "this node does not know that short address: publish it first".to_string())?,
            // Parece una dirección de Zoka Quantic pero no es de esta red (o
            // está mal copiada): cobrarle a la del nodo sería minar para otro
            // sin que el minero se entere.
            Err(error) if looks_like_quantic_address(login) => {
                return Err(format!("that is not a {} address: {error}", self.network.address_prefix()))
            }
            Err(_) => self
                .default_address
                .clone()
                .ok_or_else(|| "login with your Zoka Quantic address".to_string())?,
        };
        Ok(address.encode())
    }

    /// Una prueba Halo 2 del coinbase, alrededor de un segundo.
    fn template(&self, payout: &str) -> Result<(Block, u64), String> {
        let payout = QuanticAddress::decode_for(payout, self.network).map_err(|e| e.to_string())?;
        let transactions = mempool::global()
            .lock()
            .map(|pool| pool.select(super::runtime::MINER_MAX_BLOCK_ACTIONS))
            .unwrap_or_default();
        let (block, validated) = miner::build_template(
            &self.storage,
            &self.chain_id,
            &payout,
            super::runtime::proving_key(),
            transactions,
            super::runtime::now_secs(),
        )?;
        let pays_the_premine = block.header.block_height == super::GENESIS_HEIGHT
            && super::consensus_params(self.network).premine.is_some();
        let reward_atoms = if pays_the_premine {
            0
        } else {
            validated.minted.saturating_add(validated.fees)
        };
        Ok((block, reward_atoms))
    }

    /// Lo importa el runtime después: se cuenta al encolarlo, como siempre.
    fn found(&self, block: Block, reward_atoms: u64) -> bool {
        push_found(FoundBlock { block, reward_atoms });
        true
    }
}

struct Job {
    id: String,
    block: Block,
    reward_atoms: u64,
    network_target: BigUint,
    share_target: BigUint,
    seed: Vec<u8>,
    tip: String,
    created: Instant,
}

impl Job {
    fn blob(&self, chain_id: &str, extranonce: u32) -> Vec<u8> {
        let header = &self.block.header;
        pow::quantic_pow_blob_without_nonce(
            chain_id,
            &header.prev_hash,
            &header.merkle_root,
            &header.state_root,
            header.block_height,
            header.timestamp,
            extranonce,
        )
    }

    fn notification(&self, chain_id: &str, extranonce: u32) -> Value {
        // XMRig lee el objetivo de 8 bytes como un entero little endian y lo
        // compara con los 8 bytes de arriba del hash.
        let target64 = u64::try_from(&self.share_target >> 192usize).unwrap_or(u64::MAX);
        json!({
            "blob": hex::encode(self.blob(chain_id, extranonce)),
            "job_id": self.id,
            "target": hex::encode(target64.to_le_bytes()),
            "algo": "rx/0",
            "height": self.block.header.block_height,
            "seed_hash": hex::encode(&self.seed),
        })
    }
}

struct Hub {
    chain_id: String,
    backend: Arc<dyn Backend>,
    /// El trabajo vigente de cada dirección de cobro.
    current: Mutex<HashMap<String, Arc<Job>>>,
    /// Los últimos trabajos por id, para los aciertos que llegan tarde.
    recent: Mutex<VecDeque<Arc<Job>>>,
    /// Conexiones activas por dirección de cobro.
    miners: Mutex<HashMap<String, usize>>,
    seen: Mutex<HashSet<(String, u64)>>,
    next_extranonce: AtomicU32,
    next_job: AtomicU64,
    engine: Mutex<Option<(Vec<u8>, Box<dyn PowEngine>)>>,
    jobs: broadcast::Sender<String>,
    accepted: AtomicU64,
    rejected: AtomicU64,
    blocks: AtomicU64,
}

impl Hub {
    fn new(backend: Arc<dyn Backend>) -> Self {
        let (jobs, _) = broadcast::channel(64);
        Self {
            chain_id: backend.chain_id().to_string(),
            backend,
            current: Mutex::new(HashMap::new()),
            recent: Mutex::new(VecDeque::new()),
            miners: Mutex::new(HashMap::new()),
            seen: Mutex::new(HashSet::new()),
            next_extranonce: AtomicU32::new(rand::random::<u32>() | 1),
            next_job: AtomicU64::new(1),
            engine: Mutex::new(None),
            jobs,
            accepted: AtomicU64::new(0),
            rejected: AtomicU64::new(0),
            blocks: AtomicU64::new(0),
        }
    }

    /// Arma el trabajo de `payout` sobre la punta actual.
    fn build_job(&self, payout: &str) -> Result<Arc<Job>, String> {
        let (block, reward_atoms) = self.backend.template(payout)?;
        let storage = self.backend.storage();
        let parent = storage
            .get_block(&block.header.prev_hash)
            .map_err(|e| format!("{e:?}"))?
            .unwrap_or_else(|| crate::state::chain::genesis_parent_for(&self.chain_id));
        let compact = super::compact_target_for_child(storage, &self.chain_id, &parent)
            .map_err(|e| e.to_string())?;
        let network_target = difficulty::from_compact(compact);
        // Más fácil que un bloque, pero nunca más difícil: en una red con
        // dificultad por debajo del piso (regtest) el tope no puede ganarle.
        let share_target = (&network_target << SHARE_FACTOR_BITS)
            .min(difficulty::max_target())
            .max(network_target.clone());
        let seed = pow::epoch_seed(&self.chain_id, block.header.block_height);
        let job = Arc::new(Job {
            id: format!("{:x}", self.next_job.fetch_add(1, Ordering::Relaxed)),
            tip: block.header.prev_hash.clone(),
            block,
            reward_atoms,
            network_target,
            share_target,
            seed,
            created: Instant::now(),
        });
        if let Ok(mut recent) = self.recent.lock() {
            recent.push_back(Arc::clone(&job));
            while recent.len() > RECENT_JOBS * self.backend.max_payout_addresses() {
                recent.pop_front();
            }
        }
        Ok(job)
    }

    fn job_for(&self, payout: &str) -> Result<Arc<Job>, String> {
        if let Some(job) = self.current.lock().ok().and_then(|jobs| jobs.get(payout).cloned()) {
            return Ok(job);
        }
        let job = self.build_job(payout)?;
        if let Ok(mut jobs) = self.current.lock() {
            jobs.insert(payout.to_string(), Arc::clone(&job));
        }
        Ok(job)
    }

    fn find_job(&self, id: &str) -> Option<Arc<Job>> {
        self.recent.lock().ok()?.iter().rev().find(|job| job.id == id).cloned()
    }

    fn hash(&self, seed: &[u8], blob: &[u8]) -> Result<Vec<u8>, String> {
        let mut engine = self.engine.lock().map_err(|_| "pow engine lock poisoned".to_string())?;
        if engine.as_ref().map(|(cached, _)| cached.as_slice()) != Some(seed) {
            let algorithm = crate::economics::pow_algorithm_for_chain(&self.chain_id);
            let built = pow::build_engine(algorithm, seed).map_err(|e| e.to_string())?;
            *engine = Some((seed.to_vec(), built));
        }
        let (_, engine) = engine.as_ref().expect("built above");
        engine.calculate_hash(blob).map_err(|e| e.to_string())
    }

    /// Arma el verificador de la época de `seed` antes del primer acierto: el
    /// dataset de RandomX (uno por proceso, el mismo que valida los bloques)
    /// tarda decenas de segundos y XMRig esperaría la respuesta todo ese
    /// tiempo. No espera el candado: si está tomado, alguien ya lo arma o lo usa.
    fn warm(self: &Arc<Self>, seed: &[u8]) {
        let ready = match self.engine.try_lock() {
            Ok(engine) => engine.as_ref().map(|(cached, _)| cached.as_slice()) == Some(seed),
            Err(_) => true,
        };
        if !ready {
            let hub = Arc::clone(self);
            let seed = seed.to_vec();
            tokio::task::spawn_blocking(move || {
                if let Err(error) = hub.hash(&seed, &[0; pow::QUANTIC_POW_BLOB_BYTES]) {
                    tracing::warn!(target: "zoka.stratum", "pow verifier not built: {error}");
                }
            });
        }
    }

    /// Un acierto. Devuelve si encontró un bloque.
    fn submit(&self, job_id: &str, extranonce: u32, nonce_hex: &str, result_hex: &str) -> Result<bool, String> {
        let job = self.find_job(job_id).ok_or_else(|| "unknown or stale job".to_string())?;
        let nonce_bytes: [u8; 4] = hex::decode(nonce_hex.trim())
            .ok()
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or_else(|| "nonce must be 4 bytes of hex".to_string())?;
        let nonce = (u64::from(extranonce) << 32) | u64::from(u32::from_le_bytes(nonce_bytes));
        if let Ok(mut seen) = self.seen.lock() {
            if !seen.insert((job.id.clone(), nonce)) {
                return Err("duplicate share".to_string());
            }
            if seen.len() > 100_000 {
                seen.clear();
            }
        }
        let header = &job.block.header;
        let blob = pow::build_quantic_pow_input(
            &self.chain_id,
            &header.prev_hash,
            &header.merkle_root,
            &header.state_root,
            header.block_height,
            header.timestamp,
            nonce,
        );
        let hash = self.hash(&job.seed, &blob)?;
        if !result_hex.trim().is_empty() && hex::encode(&hash) != result_hex.trim().to_ascii_lowercase() {
            return Err("the result does not match the node's hash of that nonce".to_string());
        }
        if !difficulty::hash_meets_target(&hash, &job.share_target) {
            return Err("low difficulty share".to_string());
        }
        if !difficulty::hash_meets_target(&hash, &job.network_target) {
            return Ok(false);
        }
        let mut block = job.block.clone();
        block.header.nonce = nonce;
        block.header.seal = Signature(hash);
        block.header.hash = block.compute_hash();
        if self.backend.found(block, job.reward_atoms) {
            self.blocks.fetch_add(1, Ordering::Relaxed);
        }
        Ok(true)
    }
}

/// Empieza como una dirección de alguna red de Zoka Quantic (`zq1`, `zqtest1`,
/// `zqregtest1`). Un login así que no se decodifica en esta red es un error del
/// minero, no un nombre de trabajador.
pub(crate) fn looks_like_quantic_address(login: &str) -> bool {
    [Network::Mainnet, Network::Testnet, Network::Regtest]
        .iter()
        .any(|network| login.starts_with(network.address_prefix()))
}

/// Lo que muestra `GET /quantic/stratum`.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct StratumStatus {
    /// Dónde se le pidió escuchar (`--stratum`). Con esto y sin `listening`
    /// ni `error`, el Stratum está arrancando: se abre al final del arranque
    /// del nodo, cerca de un minuto después.
    pub configured: Option<String>,
    pub listening: Option<String>,
    /// Por qué no se pudo abrir, si falló.
    pub error: Option<String>,
    pub miners: usize,
    pub accepted_shares: u64,
    pub rejected_shares: u64,
    pub blocks_found: u64,
}

static HUB: OnceLock<Arc<Hub>> = OnceLock::new();
static CONFIGURED: OnceLock<String> = OnceLock::new();
static FAILED: OnceLock<String> = OnceLock::new();

/// El nodo arrancó con `--stratum`. Se marca antes de lanzar el servidor, para
/// que la API distinga «arrancando» de «sin Stratum».
pub fn mark_configured(bind: SocketAddr) {
    let _ = CONFIGURED.set(bind.to_string());
}

/// El servidor no pudo arrancar: la API lo dice en vez de quedar «arrancando».
pub fn mark_failed(error: &str) {
    let _ = FAILED.set(error.to_string());
}

pub fn status() -> StratumStatus {
    let mut status = StratumStatus {
        configured: CONFIGURED.get().cloned(),
        listening: LISTENING.get().cloned(),
        error: FAILED.get().cloned(),
        ..StratumStatus::default()
    };
    if let Some(hub) = HUB.get() {
        status.miners = hub.miners.lock().map(|miners| miners.values().sum()).unwrap_or(0);
        status.accepted_shares = hub.accepted.load(Ordering::Relaxed);
        status.rejected_shares = hub.rejected.load(Ordering::Relaxed);
        status.blocks_found = hub.blocks.load(Ordering::Relaxed);
    }
    status
}

static LISTENING: OnceLock<String> = OnceLock::new();

/// Escucha mineros de la cadena de Ironwood hasta que se apague el nodo.
pub async fn serve(config: StratumConfig, storage: Arc<RocksDbStorage>, chain_id: String) -> Result<(), String> {
    let network = network_for_chain(&chain_id).ok_or_else(|| format!("{chain_id} is not a Quantic chain"))?;
    let backend = IronwoodBackend {
        chain_id,
        network,
        storage,
        default_address: config.default_address,
    };
    serve_backend(config.bind, Arc::new(backend)).await
}

/// Escucha mineros de la cadena de `backend` hasta que se apague el nodo.
pub async fn serve_backend(bind: SocketAddr, backend: Arc<dyn Backend>) -> Result<(), String> {
    // XMRig calcula RandomX (rx/0): en una cadena que hashea con otra cosa
    // (regtest usa SHA3 para ir rápido) ningún acierto coincidiría.
    let chain_id = backend.chain_id();
    if !crate::economics::chain_uses_randomx(chain_id) {
        return Err(format!(
            "stratum: {chain_id} does not use RandomX (regtest hashes with SHA3); XMRig can only mine the Quantic testnet or mainnet"
        ));
    }
    let listener = TcpListener::bind(bind)
        .await
        .map_err(|e| format!("stratum: cannot listen on {bind}: {e}"))?;
    let _ = LISTENING.set(bind.to_string());
    serve_listener(listener, backend).await
}

/// El servidor sobre un puerto ya abierto. No mira si la cadena usa RandomX:
/// eso lo hace [`serve_backend`] (las pruebas usan regtest, con su SHA3).
pub(crate) async fn serve_listener(listener: TcpListener, backend: Arc<dyn Backend>) -> Result<(), String> {
    let hub = Arc::new(Hub::new(backend));
    let _ = HUB.set(Arc::clone(&hub));
    let next = hub.backend.storage().get_chain_height().unwrap_or(0).saturating_add(1);
    hub.warm(&pow::epoch_seed(&hub.chain_id, next));
    tokio::spawn(refresh_templates(Arc::clone(&hub)));
    if let Ok(bind) = listener.local_addr() {
        tracing::info!(target: "zoka.stratum", "Stratum listening on {bind}");
    }
    let slots = Arc::new(tokio::sync::Semaphore::new(MAX_CONNECTIONS));
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(error) => {
                tracing::warn!(target: "zoka.stratum", "accept failed: {error}");
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            }
        };
        let Ok(slot) = Arc::clone(&slots).try_acquire_owned() else {
            tracing::debug!(target: "zoka.stratum", "miner {peer}: too many connections");
            drop(stream);
            continue;
        };
        let hub = Arc::clone(&hub);
        tokio::spawn(async move {
            if let Err(error) = session(hub, stream).await {
                tracing::debug!(target: "zoka.stratum", "miner {peer}: {error}");
            }
            drop(slot);
        });
    }
}

/// Rehace las plantillas de las direcciones con mineros cuando cambia la punta
/// o pasa [`TEMPLATE_REFRESH`], y avisa a las conexiones.
async fn refresh_templates(hub: Arc<Hub>) {
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        // Una cadena vacía no tiene punta guardada (dice "0"): sus plantillas
        // van sobre el padre sintético del génesis. Sin esto, cada vuelta las
        // daba por viejas y rehacía una prueba por segundo.
        let storage = hub.backend.storage();
        let tip = if storage.get_chain_height().unwrap_or(0) == 0 {
            crate::state::chain::genesis_parent_for(&hub.chain_id).header.hash
        } else {
            storage.get_last_block_hash().unwrap_or_default()
        };
        let refresh = hub.backend.template_refresh();
        let active: Vec<String> = hub
            .miners
            .lock()
            .map(|miners| miners.iter().filter(|(_, count)| **count > 0).map(|(address, _)| address.clone()).collect())
            .unwrap_or_default();
        for key in active {
            let stale = hub
                .current
                .lock()
                .ok()
                .and_then(|jobs| jobs.get(&key).cloned())
                .is_none_or(|job| job.tip != tip || job.created.elapsed() >= refresh);
            if !stale {
                continue;
            }
            let builder = Arc::clone(&hub);
            let payout = key.clone();
            match tokio::task::spawn_blocking(move || builder.build_job(&payout)).await {
                Ok(Ok(job)) => {
                    hub.warm(&job.seed);
                    if let Ok(mut jobs) = hub.current.lock() {
                        jobs.insert(key.clone(), job);
                    }
                    let _ = hub.jobs.send(key);
                }
                Ok(Err(error)) => tracing::warn!(target: "zoka.stratum", "template not built: {error}"),
                Err(error) => tracing::warn!(target: "zoka.stratum", "template task failed: {error}"),
            }
        }
    }
}

async fn send(writer: &mut tokio::net::tcp::OwnedWriteHalf, message: &Value) -> Result<(), String> {
    let mut line = message.to_string();
    line.push('\n');
    writer.write_all(line.as_bytes()).await.map_err(|e| e.to_string())
}

async fn session(hub: Arc<Hub>, stream: TcpStream) -> Result<(), String> {
    let _ = stream.set_nodelay(true);
    let extranonce = hub.next_extranonce.fetch_add(1, Ordering::Relaxed);
    let mut payout: Option<String> = None;
    let result = session_loop(&hub, stream, extranonce, &mut payout).await;
    // Pase lo que pase, el minero deja de contar para su direccion de cobro.
    if let Some(key) = payout {
        if let Ok(mut miners) = hub.miners.lock() {
            if let Some(count) = miners.get_mut(&key) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    miners.remove(&key);
                    if let Ok(mut jobs) = hub.current.lock() {
                        jobs.remove(&key);
                    }
                }
            }
        }
    }
    result
}

async fn session_loop(
    hub: &Arc<Hub>,
    stream: TcpStream,
    extranonce: u32,
    payout: &mut Option<String>,
) -> Result<(), String> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    let mut jobs = hub.jobs.subscribe();
    let session_id = format!("{extranonce:08x}");
    // Mientras se contesta un acierto, los avisos de trabajo se acumulan, y
    // cada uno manda el trabajo vigente: XMRig, al ver dos veces el mismo, se
    // desconecta («duplicate job received, reconnect»).
    let mut sent_job: Option<String> = None;
    loop {
        tokio::select! {
            line = lines.next_line() => {
                let line = match line {
                    Ok(Some(line)) => line,
                    Ok(None) => return Ok(()),
                    Err(error) => return Err(error.to_string()),
                };
                if line.len() > MAX_LINE_BYTES {
                    return Err("line too long".to_string());
                }
                let request: Value = serde_json::from_str(&line).map_err(|_| "not JSON".to_string())?;
                let id = request.get("id").cloned().unwrap_or(Value::Null);
                let params = request.get("params").cloned().unwrap_or(Value::Null);
                let reply = match request.get("method").and_then(Value::as_str).unwrap_or("") {
                    "login" => {
                        let reply = login(hub, id, &params, extranonce, &session_id, payout).await?;
                        if let Some(job_id) = reply.pointer("/result/job/job_id").and_then(Value::as_str) {
                            sent_job = Some(job_id.to_string());
                        }
                        reply
                    }
                    "submit" => {
                        let job_id = params.get("job_id").and_then(Value::as_str).unwrap_or("").to_string();
                        let nonce = params.get("nonce").and_then(Value::as_str).unwrap_or("").to_string();
                        let result = params.get("result").and_then(Value::as_str).unwrap_or("").to_string();
                        let checker = Arc::clone(hub);
                        let checked = tokio::task::spawn_blocking(move || checker.submit(&job_id, extranonce, &nonce, &result))
                            .await
                            .map_err(|e| e.to_string())?;
                        match checked {
                            Ok(found) => {
                                hub.accepted.fetch_add(1, Ordering::Relaxed);
                                if found {
                                    tracing::info!(target: "zoka.stratum", "a Stratum miner found a block");
                                }
                                json!({"id": id, "jsonrpc": "2.0", "error": null, "result": {"status": "OK"}})
                            }
                            Err(error) => {
                                hub.rejected.fetch_add(1, Ordering::Relaxed);
                                tracing::debug!(target: "zoka.stratum", "share rejected: {error}");
                                json!({"id": id, "jsonrpc": "2.0", "error": {"code": -1, "message": error}})
                            }
                        }
                    }
                    "keepalived" => json!({"id": id, "jsonrpc": "2.0", "error": null, "result": {"status": "KEEPALIVED"}}),
                    _ => json!({"id": id, "jsonrpc": "2.0", "error": {"code": -1, "message": "unknown method"}}),
                };
                send(&mut writer, &reply).await?;
            }
            changed = jobs.recv() => {
                let Ok(changed) = changed else { continue };
                if payout.as_deref() != Some(changed.as_str()) {
                    continue;
                }
                let job = hub.current.lock().ok().and_then(|jobs| jobs.get(&changed).cloned());
                if let Some(job) = job.filter(|job| sent_job.as_deref() != Some(job.id.as_str())) {
                    let notice = json!({"jsonrpc": "2.0", "method": "job", "params": job.notification(&hub.chain_id, extranonce)});
                    send(&mut writer, &notice).await?;
                    sent_job = Some(job.id.clone());
                }
            }
        }
    }
}

async fn login(
    hub: &Arc<Hub>,
    id: Value,
    params: &Value,
    extranonce: u32,
    session_id: &str,
    payout: &mut Option<String>,
) -> Result<Value, String> {
    let login = params.get("login").and_then(Value::as_str).unwrap_or("").to_string();
    let resolver = Arc::clone(hub);
    let built = tokio::task::spawn_blocking(move || {
        let key = resolver.backend.payout_for(&login)?;
        // El tope antes de armar la plantilla: cada dirección nueva cuesta una.
        let busy = resolver
            .miners
            .lock()
            .map(|miners| miners.len() >= resolver.backend.max_payout_addresses() && !miners.contains_key(&key))
            .unwrap_or(true);
        if busy {
            return Err("too many payout addresses on this node".to_string());
        }
        let job = resolver.job_for(&key)?;
        Ok::<_, String>((key, job))
    })
    .await
    .map_err(|e| e.to_string())?;
    let (key, job) = match built {
        Ok(built) => built,
        Err(error) => return Ok(json!({"id": id, "jsonrpc": "2.0", "error": {"code": -1, "message": error}})),
    };
    hub.warm(&job.seed);
    if payout.as_deref() != Some(key.as_str()) {
        let mut miners = hub.miners.lock().map_err(|_| "miners lock poisoned".to_string())?;
        if miners.len() >= hub.backend.max_payout_addresses() && !miners.contains_key(&key) {
            return Ok(json!({"id": id, "jsonrpc": "2.0", "error": {"code": -1, "message": "too many payout addresses on this node"}}));
        }
        if let Some(previous) = payout.as_ref() {
            if let Some(count) = miners.get_mut(previous) {
                *count = count.saturating_sub(1);
            }
        }
        *miners.entry(key.clone()).or_insert(0) += 1;
        *payout = Some(key);
    }
    Ok(json!({
        "id": id, "jsonrpc": "2.0", "error": null,
        "result": {
            "id": session_id,
            "job": job.notification(&hub.chain_id, extranonce),
            "extensions": ["algo"],
            "status": "OK"
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    // El objetivo de 8 bytes que recibe XMRig es el de los aciertos corrido 192
    // bits, en little endian: un hash que pasa el filtro de XMRig pasa la regla
    // de la red para aciertos.
    #[test]
    fn the_xmrig_target_is_the_top_64_bits_of_the_share_target() {
        let share = difficulty::target_for_bits(10);
        let target64 = u64::try_from(&share >> 192usize).unwrap();
        let mut hash = [0xffu8; 32];
        hash[24..32].copy_from_slice(&(target64 - 1).to_le_bytes());
        assert!(u64::from_le_bytes(hash[24..32].try_into().unwrap()) < target64);
        assert!(difficulty::hash_meets_target(&hash, &share));
    }

    // Un login que no es dirección (el nombre de un trabajador) cobra a la del
    // nodo; uno que empieza como dirección de Quantic y no es de esta red, no.
    #[test]
    fn a_login_that_starts_like_an_address_is_never_taken_for_a_worker_name() {
        for login in ["zq1abc", "zqtest1abc", "zqregtest1abc"] {
            assert!(looks_like_quantic_address(login), "{login}");
        }
        for login in ["x", "worker1", "rig-01", "zka1abc", ""] {
            assert!(!looks_like_quantic_address(login), "{login}");
        }
    }
}
