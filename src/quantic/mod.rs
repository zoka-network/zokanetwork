//! Zoka Quantic dentro del nodo (etapa 5, segunda parte; diseño en
//! `docs/ETAPA5-perfil-quantic-nodo.md`).
//!
//! Un bloque de Quantic es un `Block` de siempre con sus transacciones en
//! `quantic_transactions` (el coinbase primero) y ningún campo de Zenith salvo
//! `private_nullifiers`, que lleva los nullifiers de todas ellas para que el
//! índice de gastados de siempre (que ya resuelve las reorganizaciones) proteja
//! también a Quantic. `state_root` compromete las raíces de txid, de
//! autorización y del árbol de notas.
//!
//! El estado propio de Quantic se guarda por hash de bloque, así que validar un
//! bloque de cualquier rama solo necesita a sus ancestros: la frontera del árbol
//! después de cada bloque y una ficha corta (altura, padre, raíz del árbol y
//! compromisos de su coinbase) para las anclas y la maduración.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use zoka_quantic::orchard::circuit::VerifyingKey;
use zoka_quantic::orchard::note::ExtractedNoteCommitment;
use zoka_quantic::orchard::tree::MerkleHashOrchard;
use zoka_quantic::state::{
    block_commitments, check_block_shape, decode_tree, encode_tree, maturing_coinbase_height,
    ConsensusParams, NoteCommitmentTree, Premine, QuanticBlock,
};
use zoka_quantic::tx::QuanticTransaction;
use zoka_quantic::Network;

use crate::storage::RocksDbStorage;
use crate::types::Block;

pub use zoka_quantic as protocol;

pub mod api;
pub mod difficulty;
pub mod directory;
pub mod light;
pub mod mempool;
pub mod miner;
pub mod runtime;
pub mod stratum;
pub mod wallet_file;
pub mod wallet_sync;
#[cfg(feature = "quantic-pq")]
pub mod pq_local;
#[cfg(feature = "quantic-pq")]
pub mod stark;
#[cfg(feature = "quantic-pq")]
pub mod stark_cli;
#[cfg(feature = "quantic-pq")]
pub mod stark_api;
#[cfg(feature = "quantic-pq")]
pub mod stark_net;
#[cfg(feature = "quantic-pq")]
pub mod stark_node;
#[cfg(feature = "quantic-pq")]
pub mod stark_peers;
#[cfg(feature = "quantic-pq")]
pub mod stark_stratum;

/// El nodo numera desde 1 (la altura 0 es el génesis sintético de cada
/// cadena), así que el génesis de Quantic, con el premine, es el bloque 1.
pub const GENESIS_HEIGHT: u64 = 1;
const STATE_ROOT_PREFIX: &str = "quantic-v1";
const TREE_KEY_PREFIX: &str = "quantic:tree:";
const META_KEY_PREFIX: &str = "quantic:meta:";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QuanticBlockError {
    #[error("{0} is not a Zoka Quantic chain")]
    NotQuantic(String),
    #[error("a Quantic block cannot carry Zenith data ({0})")]
    ZenithField(&'static str),
    #[error("a Quantic block needs at least its coinbase")]
    Empty,
    #[error("height {0} is out of range")]
    Height(u64),
    #[error("transaction {index} does not decode: {error}")]
    Decode { index: usize, error: String },
    #[error("{0}")]
    Consensus(String),
    #[error("private_nullifiers do not list exactly the block's nullifiers")]
    Nullifiers,
    #[error("state_root does not commit to this block's roots")]
    StateRoot,
    #[error("this is not the genesis block of the network")]
    NotTheGenesis,
    #[error("block time {time} is not after the median {median} of the previous blocks")]
    TimeTooOld { time: u64, median: u64 },
    #[error("block time {time} is more than two hours ahead of this node's clock")]
    TimeTooNew { time: u64 },
    #[error("missing Quantic state of ancestor {0}")]
    MissingAncestor(String),
    #[error("Quantic storage: {0}")]
    Storage(String),
}

fn consensus(error: zoka_quantic::state::ConsensusError) -> QuanticBlockError {
    QuanticBlockError::Consensus(error.to_string())
}

fn storage_error(error: impl std::fmt::Debug) -> QuanticBlockError {
    QuanticBlockError::Storage(format!("{error:?}"))
}

pub fn network_for_chain(chain_id: &str) -> Option<Network> {
    if chain_id == crate::economics::QUANTIC_PQ_REGTEST_CHAIN_ID {
        return Some(Network::Regtest);
    }
    Network::ALL.into_iter().find(|n| n.chain_id() == chain_id)
}

/// La red cuyas reglas de dificultad sigue una cadena. Las de STARK v2 toman
/// las de su par v1 (regtest fija, testnet con ASERT) y nada más: la testnet v2
/// no está en [`network_for_chain`] para que nunca le llegue el génesis de la v1.
fn difficulty_network(chain_id: &str) -> Option<Network> {
    match crate::economics::stark_pool_network(chain_id) {
        Some(1) => Some(Network::Mainnet),
        Some(2) => Some(Network::Testnet),
        Some(_) => Some(Network::Regtest),
        None => network_for_chain(chain_id),
    }
}

/// Raíz de la wallet del equipo en regtest. **Pública a propósito**: regtest
/// es una red local de pruebas sin valor, y así una prueba o una red local
/// pueden gastar su premine. Nunca se usa en la testnet ni en mainnet.
pub const REGTEST_TEAM_ROOT: [u8; 32] = *b"zoka-quantic-regtest-team-root!!";
/// En regtest el coinbase madura en 2 bloques (60 en las redes públicas).
pub const REGTEST_COINBASE_MATURITY: u32 = 2;

/// El génesis fijo de una red pública, si ya existe.
pub fn fixed_genesis(network: Network) -> Option<crate::economics::QuanticGenesis> {
    match network {
        Network::Mainnet => crate::economics::QUANTIC_MAINNET_GENESIS,
        Network::Testnet => crate::economics::QUANTIC_TESTNET_GENESIS,
        Network::Regtest => None,
    }
}

/// El premine anunciado de cada red: el de su génesis fijo en las públicas.
fn premine(network: Network) -> Option<Premine> {
    if let Some(genesis) = fixed_genesis(network) {
        let recipient = zoka_quantic::QuanticAddress::decode_for(genesis.premine_address, network)
            .expect("the compiled premine address is a valid address of its network");
        return Some(Premine {
            recipient,
            value: zoka_quantic::state::PREMINE_ATOMS,
        });
    }
    match network {
        Network::Regtest => {
            static REGTEST: OnceLock<Option<Premine>> = OnceLock::new();
            REGTEST
                .get_or_init(|| {
                    let team = zoka_quantic::QuanticSpendingKey::from_root(&REGTEST_TEAM_ROOT, Network::Regtest, 0).ok()?;
                    Some(Premine {
                        recipient: team.full_viewing_key().address(),
                        value: zoka_quantic::state::PREMINE_ATOMS,
                    })
                })
                .clone()
        }
        Network::Mainnet | Network::Testnet => None,
    }
}

pub fn consensus_params(network: Network) -> ConsensusParams {
    let mut params = ConsensusParams::new(network, premine(network));
    params.genesis_height = GENESIS_HEIGHT as u32;
    if network == Network::Regtest {
        params.coinbase_maturity = REGTEST_COINBASE_MATURITY;
    }
    params
}

/// La clave de verificación de Ironwood (1,9 s en la PC), una sola vez.
pub fn verifying_key() -> &'static VerifyingKey {
    static KEY: OnceLock<VerifyingKey> = OnceLock::new();
    KEY.get_or_init(zoka_quantic::verify::verifying_key)
}

/// Las tres raíces que `state_root` compromete.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuanticRoots {
    pub txid_root: [u8; 32],
    pub auth_root: [u8; 32],
    pub tree_root: [u8; 32],
}

impl QuanticRoots {
    pub fn to_state_root(&self) -> String {
        format!(
            "{STATE_ROOT_PREFIX}:{}:{}:{}",
            hex::encode(self.txid_root),
            hex::encode(self.auth_root),
            hex::encode(self.tree_root)
        )
    }

    pub fn parse(state_root: &str) -> Option<Self> {
        let mut parts = state_root.split(':');
        if parts.next()? != STATE_ROOT_PREFIX {
            return None;
        }
        let mut next = || -> Option<[u8; 32]> {
            let part = parts.next()?;
            if part.len() != 64 || part.bytes().any(|b| b.is_ascii_uppercase()) {
                return None;
            }
            hex::decode(part).ok()?.try_into().ok()
        };
        let roots = Self {
            txid_root: next()?,
            auth_root: next()?,
            tree_root: next()?,
        };
        parts.next().is_none().then_some(roots)
    }
}

/// Ficha de un bloque de Quantic ya validado.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct QuanticBlockMeta {
    height: u64,
    prev_hash: String,
    #[serde(default)]
    timestamp: u64,
    tree_root: String,
    coinbase_commitments: Vec<String>,
}

fn read_meta(storage: &RocksDbStorage, hash: &str) -> Result<Option<QuanticBlockMeta>, QuanticBlockError> {
    let Some(bytes) = storage.get_raw(&format!("{META_KEY_PREFIX}{hash}")).map_err(storage_error)? else {
        return Ok(None);
    };
    serde_json::from_slice(&bytes).map(Some).map_err(storage_error)
}

fn read_tree(storage: &RocksDbStorage, hash: &str) -> Result<Option<NoteCommitmentTree>, QuanticBlockError> {
    let Some(bytes) = storage.get_raw(&format!("{TREE_KEY_PREFIX}{hash}")).map_err(storage_error)? else {
        return Ok(None);
    };
    decode_tree(&bytes)
        .map(Some)
        .ok_or_else(|| QuanticBlockError::Storage(format!("corrupt Quantic tree for {hash}")))
}

fn commitment_from_hex(text: &str) -> Option<ExtractedNoteCommitment> {
    let bytes: [u8; 32] = hex::decode(text).ok()?.try_into().ok()?;
    Option::from(ExtractedNoteCommitment::from_bytes(&bytes))
}

/// Las transacciones de Quantic de un bloque, decodificadas sin perdonar nada.
pub fn decode_block(block: &Block) -> Result<QuanticBlock, QuanticBlockError> {
    let height = u32::try_from(block.header.block_height)
        .map_err(|_| QuanticBlockError::Height(block.header.block_height))?;
    let mut txs = block
        .quantic_transactions
        .iter()
        .enumerate()
        .map(|(index, bytes)| {
            QuanticTransaction::from_bytes(bytes).map_err(|e| QuanticBlockError::Decode {
                index,
                error: e.to_string(),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if txs.is_empty() {
        return Err(QuanticBlockError::Empty);
    }
    let coinbase = txs.remove(0);
    Ok(QuanticBlock {
        height,
        coinbase,
        transactions: txs,
    })
}

/// Un bloque de Quantic no lleva nada de Zenith.
pub fn validate_shape(block: &Block) -> Result<(), QuanticBlockError> {
    if !block.transactions.is_empty() {
        return Err(QuanticBlockError::ZenithField("transactions"));
    }
    if !block.private_tx_hashes.is_empty() {
        return Err(QuanticBlockError::ZenithField("private_tx_hashes"));
    }
    if !block.private_tx_bodies.is_empty() {
        return Err(QuanticBlockError::ZenithField("private_tx_bodies"));
    }
    if !block.private_fee_claims.is_empty() {
        return Err(QuanticBlockError::ZenithField("private_fee_claims"));
    }
    if block.private_coinbase.is_some() {
        return Err(QuanticBlockError::ZenithField("private_coinbase"));
    }
    if block.quantic_transactions.is_empty() {
        return Err(QuanticBlockError::Empty);
    }
    Ok(())
}

/// `private_nullifiers` de un bloque de Quantic: todos sus nullifiers, en hex,
/// ordenados y sin repetir (como en Zenith).
pub fn nullifier_list(nullifiers: &[[u8; 32]]) -> Vec<String> {
    let mut list: Vec<String> = nullifiers.iter().map(hex::encode).collect();
    list.sort();
    list.dedup();
    list
}

/// Lo que dejó un bloque válido.
#[derive(Clone, Debug)]
pub struct ValidatedBlock {
    pub roots: QuanticRoots,
    pub tree: NoteCommitmentTree,
    pub fees: u64,
    /// Premine o subsidio: lo que se suma a la emisión.
    pub minted: u64,
    pub coinbase_commitments: Vec<ExtractedNoteCommitment>,
}

/// Recorre la rama hacia atrás desde `start` (incluido).
struct Ancestors<'a> {
    storage: &'a RocksDbStorage,
    next: Option<String>,
}

impl Iterator for Ancestors<'_> {
    type Item = Result<(String, QuanticBlockMeta), QuanticBlockError>;

    fn next(&mut self) -> Option<Self::Item> {
        let hash = self.next.take()?;
        match read_meta(self.storage, &hash) {
            Ok(Some(meta)) => {
                if meta.height > GENESIS_HEIGHT {
                    self.next = Some(meta.prev_hash.clone());
                }
                Some(Ok((hash, meta)))
            }
            Ok(None) => Some(Err(QuanticBlockError::MissingAncestor(hash))),
            Err(error) => Some(Err(error)),
        }
    }
}

fn ancestors<'a>(storage: &'a RocksDbStorage, parent: &Block) -> Ancestors<'a> {
    Ancestors {
        storage,
        next: (parent.header.block_height >= GENESIS_HEIGHT).then(|| parent.header.hash.clone()),
    }
}

/// Las raíces finales de los últimos `window` bloques de la rama que termina
/// en `tip_hash` (incluido): las anclas válidas para el bloque que sigue.
pub fn recent_roots_after(
    storage: &RocksDbStorage,
    tip_hash: &str,
    window: usize,
) -> Result<std::collections::HashSet<[u8; 32]>, QuanticBlockError> {
    let mut roots = std::collections::HashSet::new();
    let walk = Ancestors {
        storage,
        next: Some(tip_hash.to_string()),
    };
    for ancestor in walk.take(window) {
        let (_, meta) = ancestor?;
        let root: [u8; 32] = hex::decode(&meta.tree_root)
            .ok()
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or_else(|| QuanticBlockError::Storage("corrupt tree root".into()))?;
        roots.insert(root);
    }
    Ok(roots)
}

/// Lo que un bloque necesita de su rama: las raíces finales de los bloques
/// anteriores dentro de la ventana de anclas y el árbol que deja.
struct BranchState {
    recent_roots: std::collections::HashSet<[u8; 32]>,
    tree: NoteCommitmentTree,
}

fn branch_state(
    storage: &RocksDbStorage,
    params: &ConsensusParams,
    block: &QuanticBlock,
    parent: &Block,
) -> Result<BranchState, QuanticBlockError> {
    // Anclas: la raíz final de alguno de los `anchor_window` bloques anteriores
    // de esta misma rama. Maduración: el coinbase del ancestro que corresponde.
    let maturing_height = maturing_coinbase_height(params, block.height).map(u64::from);
    let mut recent_roots = std::collections::HashSet::new();
    let mut maturing = Vec::new();
    for (index, ancestor) in ancestors(storage, parent).enumerate() {
        let (_, meta) = ancestor?;
        if index < params.anchor_window {
            let root: [u8; 32] = hex::decode(&meta.tree_root)
                .ok()
                .and_then(|bytes| bytes.try_into().ok())
                .ok_or_else(|| QuanticBlockError::Storage("corrupt tree root".into()))?;
            recent_roots.insert(root);
        }
        if Some(meta.height) == maturing_height {
            maturing = meta
                .coinbase_commitments
                .iter()
                .map(|c| commitment_from_hex(c))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| QuanticBlockError::Storage("corrupt coinbase commitment".into()))?;
        }
        let anchors_done = index + 1 >= params.anchor_window;
        let maturing_done = maturing_height.is_none_or(|h| meta.height <= h);
        if anchors_done && maturing_done {
            break;
        }
    }
    let mut tree = if parent.header.block_height >= GENESIS_HEIGHT {
        read_tree(storage, &parent.header.hash)?
            .ok_or_else(|| QuanticBlockError::MissingAncestor(parent.header.hash.clone()))?
    } else {
        NoteCommitmentTree::empty()
    };
    for cmx in block_commitments(params, block.height, &maturing, block) {
        tree.append(MerkleHashOrchard::from_cmx(&cmx))
            .map_err(|_| QuanticBlockError::Consensus("the note commitment tree is full".into()))?;
    }
    Ok(BranchState { recent_roots, tree })
}

fn roots_of(block: &QuanticBlock, tree: &NoteCommitmentTree) -> QuanticRoots {
    QuanticRoots {
        txid_root: block.txid_root(),
        auth_root: block.auth_root(),
        tree_root: zoka_quantic::orchard::Anchor::from(tree.root()).to_bytes(),
    }
}

/// El bloque génesis embebido en el binario, si la red ya tiene uno: así
/// cada nodo nuevo arranca con él sin depender de pares.
pub fn embedded_genesis_block(network: Network) -> Option<Block> {
    const TESTNET: &str = include_str!("../../docs/quantic-testnet-genesis.v1.json");
    const MAINNET: &str = include_str!("../../docs/quantic-mainnet-genesis.v1.json");
    let raw = match network {
        Network::Testnet => TESTNET,
        Network::Mainnet => MAINNET,
        Network::Regtest => return None,
    };
    let block: Block = serde_json::from_str(raw).ok()?;
    let expected = fixed_genesis(network)?;
    (block.header.hash == expected.block_hash && block.header.block_height == GENESIS_HEIGHT).then_some(block)
}

/// Si la base está vacía y la red tiene génesis embebido, lo aplica.
pub fn import_embedded_genesis(storage: &RocksDbStorage, chain_id: &str) -> Result<bool, String> {
    let Some(network) = network_for_chain(chain_id) else {
        return Ok(false);
    };
    if storage.get_chain_height().map_err(|e| format!("{e:?}"))? > 0 {
        return Ok(false);
    }
    let Some(block) = embedded_genesis_block(network) else {
        return Ok(false);
    };
    crate::state::chain::apply_block(storage, chain_id, &block).map_err(|e| e.to_string())?;
    Ok(true)
}

/// Bloques cuya mediana de hora tiene que superar la de uno nuevo.
pub const MEDIAN_TIME_SPAN: usize = 11;
/// Cuánto adelante del reloj propio admite un nodo la hora de un bloque.
pub const MAX_FUTURE_SECS: u64 = 2 * 60 * 60;

/// La mediana de la hora de los últimos [`MEDIAN_TIME_SPAN`] bloques de la
/// rama que termina en `parent` (incluido). 0 antes del génesis.
pub fn median_time_past(storage: &RocksDbStorage, parent: &Block) -> Result<u64, QuanticBlockError> {
    let mut times = Vec::with_capacity(MEDIAN_TIME_SPAN);
    for ancestor in ancestors(storage, parent).take(MEDIAN_TIME_SPAN) {
        times.push(ancestor?.1.timestamp);
    }
    if times.is_empty() {
        return Ok(0);
    }
    times.sort_unstable();
    Ok(times[times.len() / 2])
}

/// El objetivo de prueba de trabajo del hijo de `parent` (ver `difficulty`):
/// fijo en regtest, el inicial para el génesis y ASERT anclado en el génesis
/// después.
pub fn target_for_child(
    storage: &RocksDbStorage,
    chain_id: &str,
    parent: &Block,
) -> Result<num_bigint::BigUint, QuanticBlockError> {
    let network = difficulty_network(chain_id).ok_or_else(|| QuanticBlockError::NotQuantic(chain_id.to_string()))?;
    if network == Network::Regtest {
        return Ok(difficulty::target_for_bits(crate::economics::QUANTIC_REGTEST_DIFFICULTY_BITS));
    }
    let initial = difficulty::target_for_bits(difficulty::initial_difficulty_bits(network));
    if parent.header.block_height < GENESIS_HEIGHT {
        return Ok(initial);
    }
    // El génesis de la rama: el ancestro de altura 1 (en las redes públicas es
    // uno solo, fijo en el binario).
    let mut genesis = None;
    for ancestor in ancestors(storage, parent) {
        let (_, meta) = ancestor?;
        if meta.height == GENESIS_HEIGHT {
            genesis = Some(meta);
        }
    }
    let genesis = genesis.ok_or_else(|| QuanticBlockError::MissingAncestor("genesis".to_string()))?;
    let time_delta = parent.header.timestamp as i64 - genesis.timestamp as i64;
    let height_delta = (parent.header.block_height - GENESIS_HEIGHT) as i64;
    Ok(difficulty::asert_target(&initial, time_delta, height_delta))
}

/// Lo mismo, en el formato compacto en que el nodo guarda la dificultad.
pub fn compact_target_for_child(storage: &RocksDbStorage, chain_id: &str, parent: &Block) -> Result<u32, QuanticBlockError> {
    target_for_child(storage, chain_id, parent).map(|target| difficulty::to_compact(&target))
}

/// El objetivo del próximo bloque sobre la punta canónica.
pub fn compact_target_for_tip(storage: &RocksDbStorage, chain_id: &str) -> Result<u32, QuanticBlockError> {
    let tip = storage.get_chain_height().map_err(storage_error)?;
    let parent = if tip == 0 {
        crate::state::chain::genesis_parent_for(chain_id)
    } else {
        let hash = storage.get_last_block_hash().map_err(storage_error)?;
        storage
            .get_block(&hash)
            .map_err(storage_error)?
            .ok_or_else(|| QuanticBlockError::MissingAncestor(hash.clone()))?
    };
    compact_target_for_child(storage, chain_id, &parent)
}

fn params_for(chain_id: &str) -> Result<ConsensusParams, QuanticBlockError> {
    network_for_chain(chain_id)
        .map(consensus_params)
        .ok_or_else(|| QuanticBlockError::NotQuantic(chain_id.to_string()))
}

/// Valida todo lo de Quantic de `candidate` encima de `parent` (de la rama que
/// sea): forma, consenso de la biblioteca, anclas dentro de la ventana de su
/// rama, nullifiers declarados, pruebas y firmas, y las raíces de `state_root`.
/// La prueba de trabajo y el enlace con el padre los revisa el validador de
/// siempre; los nullifiers gastados antes, el índice de siempre.
pub fn validate_block(
    storage: &RocksDbStorage,
    chain_id: &str,
    candidate: &Block,
    parent: &Block,
) -> Result<ValidatedBlock, QuanticBlockError> {
    let params = params_for(chain_id)?;
    if candidate.header.block_height == GENESIS_HEIGHT {
        if let Some(genesis) = fixed_genesis(params.network) {
            if candidate.header.hash != genesis.block_hash {
                return Err(QuanticBlockError::NotTheGenesis);
            }
        }
    }
    validate_block_with(storage, &params, candidate, parent)
}

fn validate_block_with(
    storage: &RocksDbStorage,
    params: &ConsensusParams,
    candidate: &Block,
    parent: &Block,
) -> Result<ValidatedBlock, QuanticBlockError> {
    let params = params.clone();
    validate_shape(candidate)?;
    // La hora: después de la mediana de los últimos bloques de la rama, y no
    // más de dos horas adelante del reloj propio.
    let median = median_time_past(storage, parent)?;
    if candidate.header.timestamp <= median {
        return Err(QuanticBlockError::TimeTooOld {
            time: candidate.header.timestamp,
            median,
        });
    }
    if candidate.header.timestamp > now_secs().saturating_add(MAX_FUTURE_SECS) {
        return Err(QuanticBlockError::TimeTooNew {
            time: candidate.header.timestamp,
        });
    }
    let block = decode_block(candidate)?;
    if u64::from(block.height) < GENESIS_HEIGHT {
        return Err(QuanticBlockError::Height(candidate.header.block_height));
    }
    let checks = check_block_shape(&params, &block).map_err(consensus)?;
    if candidate.private_nullifiers != nullifier_list(&checks.nullifiers) {
        return Err(QuanticBlockError::Nullifiers);
    }
    let state = branch_state(storage, &params, &block, parent)?;
    if block
        .transactions
        .iter()
        .any(|tx| !state.recent_roots.contains(&tx.anchor().to_bytes()))
    {
        return Err(consensus(zoka_quantic::state::ConsensusError::UnknownAnchor));
    }
    let roots = roots_of(&block, &state.tree);
    if candidate.header.state_root != roots.to_state_root() {
        return Err(QuanticBlockError::StateRoot);
    }
    if !zoka_quantic::verify::verify_transactions(
        verifying_key(),
        block.all_transactions(),
        zoka_quantic::os_rng(),
    ) {
        return Err(consensus(
            zoka_quantic::state::ConsensusError::InvalidProofOrSignature,
        ));
    }
    Ok(ValidatedBlock {
        roots,
        tree: state.tree,
        fees: checks.fees,
        minted: checks.minted,
        coinbase_commitments: block.coinbase.note_commitments().copied().collect(),
    })
}

/// Guarda lo de Quantic de un bloque validado, sea o no de la rama canónica.
pub fn store_block_state(
    storage: &RocksDbStorage,
    candidate: &Block,
    validated: &ValidatedBlock,
) -> Result<(), QuanticBlockError> {
    let hash = &candidate.header.hash;
    storage
        .put_raw(&format!("{TREE_KEY_PREFIX}{hash}"), encode_tree(&validated.tree))
        .map_err(storage_error)?;
    let meta = QuanticBlockMeta {
        height: candidate.header.block_height,
        prev_hash: candidate.header.prev_hash.clone(),
        timestamp: candidate.header.timestamp,
        tree_root: hex::encode(validated.roots.tree_root),
        coinbase_commitments: validated
            .coinbase_commitments
            .iter()
            .map(|c| hex::encode(c.to_bytes()))
            .collect(),
    };
    storage
        .put_raw(
            &format!("{META_KEY_PREFIX}{hash}"),
            serde_json::to_vec(&meta).map_err(storage_error)?,
        )
        .map_err(storage_error)
}

/// La raíz del árbol de notas después de `block_hash`: el ancla de la próxima
/// transferencia si ese bloque es la punta.
pub fn tree_root_after(
    storage: &RocksDbStorage,
    block_hash: &str,
) -> Result<Option<[u8; 32]>, QuanticBlockError> {
    Ok(read_meta(storage, block_hash)?
        .and_then(|meta| hex::decode(meta.tree_root).ok()?.try_into().ok()))
}

/// Busca el nonce de un bloque armado con [`assemble_block`], con un solo hilo
/// (las pruebas y la red local; el minero del nodo reparte la búsqueda). La
/// entrada compromete el cuerpo, así que el hash del bloque se recalcula.
pub fn solve_pow(chain_id: &str, block: &mut Block, compact_target: u32) -> Result<(), String> {
    let target = difficulty::from_compact(compact_target);
    let engine = crate::mining::pow::engine_for(chain_id, block.header.block_height)
        .map_err(|e| e.to_string())?;
    let mut nonce = 0u64;
    loop {
        let preimage = crate::mining::pow::build_quantic_pow_input(
            chain_id,
            &block.header.prev_hash,
            &block.header.merkle_root,
            &block.header.state_root,
            block.header.block_height,
            block.header.timestamp,
            nonce,
        );
        let hash = engine.calculate_hash(&preimage).map_err(|e| e.to_string())?;
        if difficulty::hash_meets_target(&hash, &target) {
            block.header.nonce = nonce;
            block.header.seal = crate::types::Signature(hash);
            block.header.hash = block.compute_hash();
            return Ok(());
        }
        nonce = nonce.wrapping_add(1);
    }
}

/// Arma un bloque de Quantic encima de `parent`, sin la prueba de trabajo (el
/// nonce y el sello los pone quien mina). Lo valida con las mismas reglas que
/// a uno recibido: nunca sale uno inválido.
pub fn assemble_block(
    storage: &RocksDbStorage,
    chain_id: &str,
    parent: &Block,
    coinbase: &QuanticTransaction,
    transactions: &[QuanticTransaction],
    timestamp: u64,
) -> Result<(Block, ValidatedBlock), QuanticBlockError> {
    let params = params_for(chain_id)?;
    let (block, _) = assemble_block_with(storage, &params, parent, coinbase, transactions, timestamp)?;
    let validated = validate_block(storage, chain_id, &block, parent)?;
    Ok((block, validated))
}

fn assemble_block_with(
    storage: &RocksDbStorage,
    params: &ConsensusParams,
    parent: &Block,
    coinbase: &QuanticTransaction,
    transactions: &[QuanticTransaction],
    timestamp: u64,
) -> Result<(Block, ValidatedBlock), QuanticBlockError> {
    let params = params.clone();
    let height = parent.header.block_height + 1;
    let decoded = QuanticBlock {
        height: u32::try_from(height).map_err(|_| QuanticBlockError::Height(height))?,
        coinbase: coinbase.clone(),
        transactions: transactions.to_vec(),
    };
    let checks = check_block_shape(&params, &decoded).map_err(consensus)?;
    let state = branch_state(storage, &params, &decoded, parent)?;
    let mut block = Block::new(
        parent.header.hash.clone(),
        timestamp,
        0,
        height,
        Vec::new(),
        crate::types::PublicKey(Vec::new()),
        crate::types::Signature(Vec::new()),
    );
    block.private_nullifiers = nullifier_list(&checks.nullifiers);
    block.quantic_transactions = decoded
        .all_transactions()
        .map(QuanticTransaction::to_bytes)
        .collect();
    block.header.state_root = roots_of(&decoded, &state.tree).to_state_root();
    block.header.merkle_root = block.compute_merkle_root();
    block.header.hash = block.compute_hash();
    let validated = validate_block_with(storage, &params, &block, parent)?;
    Ok((block, validated))
}

/// Arma el génesis de una red pública: el bloque 1 con el premine público a
/// `premine_to`, con su prueba de trabajo resuelta. Para `zoka quantic-genesis`:
/// una vez creado, su hash y la dirección se fijan en
/// `economics::QUANTIC_*_GENESIS` y el bloque se embebe en el binario.
pub fn build_genesis(
    chain_id: &str,
    premine_to: &zoka_quantic::QuanticAddress,
    proving_key: &zoka_quantic::orchard::circuit::ProvingKey,
    timestamp: u64,
) -> Result<Block, String> {
    let network = network_for_chain(chain_id).ok_or_else(|| format!("{chain_id} is not a Quantic chain"))?;
    if network == Network::Regtest {
        return Err("regtest has no fixed genesis: any node mines its own".to_string());
    }
    if premine_to.network() != network {
        return Err(format!("the premine address is not a {chain_id} address"));
    }
    let mut params = consensus_params(network);
    params.premine = Some(Premine {
        recipient: premine_to.clone(),
        value: zoka_quantic::state::PREMINE_ATOMS,
    });
    let coinbase = zoka_quantic::builder::build_coinbase(
        network,
        GENESIS_HEIGHT as u32,
        premine_to,
        zoka_quantic::state::PREMINE_ATOMS,
        true,
        proving_key,
        &mut zoka_quantic::os_rng(),
    )
    .map_err(|e| e.to_string())?;
    let dir = tempfile_dir()?;
    let storage = RocksDbStorage::new(dir.to_str().ok_or("temp path")?).map_err(|e| format!("{e:?}"))?;
    let parent = crate::state::chain::genesis_parent_for(chain_id);
    let (mut block, _) = assemble_block_with(&storage, &params, &parent, &coinbase, &[], timestamp).map_err(|e| e.to_string())?;
    let initial = difficulty::to_compact(&difficulty::target_for_bits(difficulty::initial_difficulty_bits(network)));
    solve_pow(chain_id, &mut block, initial)?;
    drop(storage);
    let _ = std::fs::remove_dir_all(&dir);
    Ok(block)
}

fn tempfile_dir() -> Result<std::path::PathBuf, String> {
    let dir = std::env::temp_dir().join(format!("zoka-quantic-genesis-{}-{}", std::process::id(), now_nanos()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn now_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::chain::{apply_block_with_expected_difficulty, genesis_parent_for, ChainError};
    use zoka_quantic::builder::{build_coinbase, Payment};
    use zoka_quantic::orchard::circuit::ProvingKey;
    use zoka_quantic::state::{ATOMS_PER_ZKA, PREMINE_ATOMS};
    use zoka_quantic::wallet::QuanticWallet;
    use zoka_quantic::QuanticSpendingKey;

    const CHAIN: &str = crate::economics::QUANTIC_REGTEST_CHAIN_ID;
    /// El objetivo fijo de regtest, en el formato compacto que guarda el nodo.
    fn bits() -> u32 {
        difficulty::to_compact(&difficulty::target_for_bits(crate::economics::QUANTIC_REGTEST_DIFFICULTY_BITS))
    }
    const ZKA: u64 = ATOMS_PER_ZKA;

    fn proving_key() -> &'static ProvingKey {
        static KEY: OnceLock<ProvingKey> = OnceLock::new();
        KEY.get_or_init(zoka_quantic::verify::proving_key)
    }

    fn storage() -> (RocksDbStorage, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let storage = RocksDbStorage::new(dir.path().to_str().unwrap()).unwrap();
        (storage, dir)
    }

    #[test]
    fn a_saved_scan_resumes_where_it_stopped_and_falls_back_after_a_reorganization() {
        use std::cell::Cell;
        let (storage, _dir) = storage();
        let params = consensus_params(Network::Regtest);
        let team = QuanticSpendingKey::from_root(&REGTEST_TEAM_ROOT, Network::Regtest, 0).unwrap();
        let miner_sk = QuanticSpendingKey::from_root(&[4; 32], Network::Regtest, 0).unwrap();
        let miner = miner_sk.full_viewing_key().address();
        let mut parent = genesis_parent_for(CHAIN);
        let mut chain = Vec::new();
        for height in 1..=4u64 {
            let coinbase = if height == 1 {
                coinbase(1, &team.full_viewing_key().address(), PREMINE_ATOMS, true)
            } else {
                coinbase(height, &miner, params.minted_at(height as u32), false)
            };
            let block = mined(&storage, &parent, &coinbase, &[]);
            apply_block_with_expected_difficulty(&storage, CHAIN, &block, bits()).unwrap();
            chain.push(block.clone());
            parent = block;
        }
        let fetches = Cell::new(0);
        let node = |tip: u64| {
            let storage = &storage;
            let fetches = &fetches;
            move |from: u64, to: u64| {
                fetches.set(fetches.get() + 1);
                Ok::<_, String>((from..=to.min(tip)).map(|h| compact_json(storage, h)).collect())
            }
        };
        let fvk = || miner_sk.full_viewing_key();
        let view = |wallet: &QuanticWallet| (wallet.balance(), wallet.immature_balance(), wallet.anchor().to_bytes());
        let full = wallet_sync::sync_wallet(fvk(), 4, node(4)).unwrap();

        // Primera vez hasta la 2, después solo lo nuevo.
        let first = wallet_sync::resume_wallet(fvk(), vec![], 2, node(2)).unwrap();
        assert_eq!(first.scanned_from, 1);
        assert_eq!(first.points.iter().map(|p| p.height).collect::<Vec<_>>(), vec![2]);
        let second = wallet_sync::resume_wallet(fvk(), first.points, 4, node(4)).unwrap();
        assert_eq!(second.scanned_from, 3, "retoma después del bloque 2");
        assert_eq!(view(&second.wallet), view(&full));
        assert_eq!(second.points.iter().map(|p| p.height).collect::<Vec<_>>(), vec![4, 2]);

        // Al día: una sola consulta, la que comprueba que el punto sigue en la cadena.
        fetches.set(0);
        let idle = wallet_sync::resume_wallet(fvk(), second.points.clone(), 4, node(4)).unwrap();
        assert_eq!((idle.scanned_from, fetches.get()), (5, 1));
        assert_eq!(view(&idle.wallet), view(&full));

        // El punto de la 4 quedó en una rama que el nodo ya no sigue: retoma desde la 2.
        let mut reorged = second.points.clone();
        reorged[0].last_hash = "f".repeat(64);
        let after = wallet_sync::resume_wallet(fvk(), reorged.clone(), 4, node(4)).unwrap();
        assert_eq!(after.scanned_from, 3);
        assert_eq!(view(&after.wallet), view(&full));
        assert_eq!(after.points[0].last_hash, chain[3].header.hash, "el punto nuevo es el de la rama buena");
        // Ninguno sigue: desde el génesis.
        reorged[1].last_hash = "e".repeat(64);
        let again = wallet_sync::resume_wallet(fvk(), reorged, 4, node(4)).unwrap();
        assert_eq!(again.scanned_from, 1);
        assert_eq!(view(&again.wallet), view(&full));

        // Un nodo que todavía está sincronizando (va por la 1): lo último verificado, sin tocar nada.
        let behind = wallet_sync::resume_wallet(fvk(), second.points.clone(), 1, node(1)).unwrap();
        assert!(behind.node_behind);
        assert_eq!(view(&behind.wallet), view(&full));
        assert_eq!(behind.points.len(), 2);

        // Los puntos de otra wallet no se usan.
        let other = QuanticSpendingKey::from_root(&[9; 32], Network::Regtest, 0).unwrap().full_viewing_key();
        let foreign = wallet_sync::resume_wallet(other, second.points, 4, node(4)).unwrap();
        assert_eq!(foreign.scanned_from, 1);
    }

    fn mined(
        storage: &RocksDbStorage,
        parent: &Block,
        coinbase: &QuanticTransaction,
        transactions: &[QuanticTransaction],
    ) -> Block {
        let (mut block, _) =
            assemble_block(storage, CHAIN, parent, coinbase, transactions, now_secs() - 100_000 + parent.header.block_height * 10)
                .expect("assembles");
        solve_pow(CHAIN, &mut block, bits()).unwrap();
        block
    }

    fn coinbase(height: u64, to: &zoka_quantic::QuanticAddress, value: u64, public: bool) -> QuanticTransaction {
        build_coinbase(Network::Regtest, height as u32, to, value, public, proving_key(), &mut zoka_quantic::os_rng())
            .unwrap()
    }

    #[test]
    fn the_chain_ids_are_the_ones_of_the_library() {
        assert_eq!(network_for_chain(crate::economics::QUANTIC_MAINNET_CHAIN_ID), Some(Network::Mainnet));
        assert_eq!(network_for_chain(crate::economics::QUANTIC_TESTNET_CHAIN_ID), Some(Network::Testnet));
        assert_eq!(network_for_chain(CHAIN), Some(Network::Regtest));
        assert_eq!(network_for_chain(crate::economics::MAINNET_CHAIN_ID), None);
        for network in Network::ALL {
            assert!(crate::economics::is_quantic_chain_id(network.chain_id()));
        }
    }

    #[test]
    fn state_roots_round_trip_and_reject_anything_else() {
        let roots = QuanticRoots {
            txid_root: [1; 32],
            auth_root: [2; 32],
            tree_root: [3; 32],
        };
        assert_eq!(QuanticRoots::parse(&roots.to_state_root()), Some(roots));
        for bad in ["0", "quantic-v1:aa", &roots.to_state_root().replace("01", "0G"), &format!("{}:00", roots.to_state_root())] {
            assert_eq!(QuanticRoots::parse(bad), None, "{bad}");
        }
        assert_eq!(QuanticRoots::parse(&roots.to_state_root().to_uppercase()), None);
    }

    #[test]
    fn a_quantic_chain_lives_inside_the_node() {
        let (storage, _dir) = storage();
        let params = consensus_params(Network::Regtest);
        let team_sk = QuanticSpendingKey::from_root(&REGTEST_TEAM_ROOT, Network::Regtest, 0).unwrap();
        let alice_sk = QuanticSpendingKey::from_root(&[3; 32], Network::Regtest, 0).unwrap();
        let miner = QuanticSpendingKey::from_root(&[4; 32], Network::Regtest, 0)
            .unwrap()
            .full_viewing_key()
            .address();
        let mut team = QuanticWallet::new(team_sk.full_viewing_key(), &params);
        let mut alice = QuanticWallet::new(alice_sk.full_viewing_key(), &params);

        // Bloque 1: el génesis, con el premine público.
        let genesis_parent = genesis_parent_for(CHAIN);
        let b1 = mined(&storage, &genesis_parent, &coinbase(1, &team.fvk().address(), PREMINE_ATOMS, true), &[]);
        assert!(b1.transactions.is_empty() && b1.private_coinbase.is_none());
        apply_block_with_expected_difficulty(&storage, CHAIN, &b1, bits()).expect("genesis");
        for wallet in [&mut team, &mut alice] {
            wallet.scan_block(&decode_block(&b1).unwrap());
        }
        assert_eq!(team.balance(), PREMINE_ATOMS);
        assert_eq!(tree_root_after(&storage, &b1.header.hash).unwrap().unwrap(), team.anchor().to_bytes());

        // Bloque 2: el equipo le paga a Alice; el minero cobra subsidio y comisión.
        let tx = team
            .create_transfer(&team_sk, vec![Payment::new(alice.fvk().address(), 10 * ZKA)], 0, proving_key(), &mut zoka_quantic::os_rng())
            .unwrap();
        let fee = tx.fee().unwrap();
        let b2 = mined(&storage, &b1, &coinbase(2, &miner, params.minted_at(2) + fee, false), std::slice::from_ref(&tx));
        let head = apply_block_with_expected_difficulty(&storage, CHAIN, &b2, bits()).expect("block 2");
        assert_eq!(head.height, 2);
        assert_eq!(head.total_supply_emitted, PREMINE_ATOMS + 12 * ZKA, "fees are not emission");
        for wallet in [&mut team, &mut alice] {
            wallet.scan_block(&decode_block(&b2).unwrap());
        }
        assert_eq!(alice.balance(), 10 * ZKA);
        assert_eq!(team.balance(), PREMINE_ATOMS - 10 * ZKA - fee);

        // Repetir el envío: el índice de nullifiers de siempre lo rechaza.
        let replay = mined(&storage, &b2, &coinbase(3, &miner, params.minted_at(3) + fee, false), std::slice::from_ref(&tx));
        assert!(matches!(
            apply_block_with_expected_difficulty(&storage, CHAIN, &replay, bits()),
            Err(ChainError::PrivateNullifierDoubleSpend { .. })
        ));

        // Un state_root que no es el del bloque, aun con su prueba de trabajo.
        let mut forged = mined(&storage, &b2, &coinbase(3, &miner, params.minted_at(3), false), &[]);
        forged.header.state_root = QuanticRoots { txid_root: [9; 32], auth_root: [9; 32], tree_root: [9; 32] }.to_state_root();
        forged.header.merkle_root = forged.compute_merkle_root();
        solve_pow(CHAIN, &mut forged, bits()).unwrap();
        assert!(matches!(
            apply_block_with_expected_difficulty(&storage, CHAIN, &forged, bits()),
            Err(ChainError::InvalidQuanticBlock(_))
        ));

        // La prueba de trabajo cubre el cuerpo: otro cuerpo con el mismo sello no pasa.
        let honest = mined(&storage, &b2, &coinbase(3, &miner, params.minted_at(3), false), &[]);
        let mut swapped = honest.clone();
        swapped.quantic_transactions = vec![coinbase(3, &miner, params.minted_at(3), false).to_bytes()];
        swapped.private_nullifiers = {
            let decoded = decode_block(&swapped).unwrap();
            nullifier_list(&decoded.coinbase.nullifiers().map(|nf| nf.to_bytes()).collect::<Vec<_>>())
        };
        swapped.header.merkle_root = swapped.compute_merkle_root();
        swapped.header.hash = swapped.compute_hash();
        assert!(apply_block_with_expected_difficulty(&storage, CHAIN, &swapped, bits()).is_err());
        apply_block_with_expected_difficulty(&storage, CHAIN, &honest, bits()).expect("the honest block 3");

        // Un bloque con algo de Zenith no es un bloque de Quantic.
        let mut zenith_field = honest.clone();
        zenith_field.private_tx_hashes = vec!["ab".repeat(32)];
        assert!(validate_shape(&zenith_field).is_err());
    }

    #[test]
    fn the_mempool_admits_only_what_the_next_block_would_accept_and_the_miner_uses_it() {
        use std::sync::atomic::AtomicBool;
        use zoka_quantic::builder::{build_transfer, SpendInput};

        let (storage, _dir) = storage();
        let params = consensus_params(Network::Regtest);
        let team_sk = QuanticSpendingKey::from_root(&REGTEST_TEAM_ROOT, Network::Regtest, 0).unwrap();
        let miner = QuanticSpendingKey::from_root(&[4; 32], Network::Regtest, 0)
            .unwrap()
            .full_viewing_key()
            .address();
        let alice = QuanticSpendingKey::from_root(&[3; 32], Network::Regtest, 0)
            .unwrap()
            .full_viewing_key()
            .address();
        let mut team = QuanticWallet::new(team_sk.full_viewing_key(), &params);
        let mut pool = mempool::QuanticMempool::new();

        // Antes del génesis no hay anclas: no entra nada.
        let early = || {
            build_coinbase(Network::Regtest, 9, &alice, 1, false, proving_key(), &mut zoka_quantic::os_rng()).unwrap()
        };
        assert!(pool.insert(&storage, CHAIN, early(), 1).is_err(), "a coinbase is never a transfer");

        // El minero arma el génesis aunque le pidan pagarse a sí mismo: paga el premine.
        let (mut genesis, _) = miner::build_template(&storage, CHAIN, &miner, proving_key(), vec![], now_secs()).unwrap();
        let stop = AtomicBool::new(false);
        assert!(matches!(
            miner::solve_pow_parallel(CHAIN, &mut genesis, bits(), 2, std::time::Duration::from_secs(60), &stop).unwrap(),
            miner::PowOutcome::Solved
        ));
        apply_block_with_expected_difficulty(&storage, CHAIN, &genesis, bits()).expect("genesis from the miner");
        team.scan_block(&decode_block(&genesis).unwrap());
        assert_eq!(team.balance(), PREMINE_ATOMS);

        // Una transferencia válida entra una sola vez.
        let tx = team
            .create_transfer(&team_sk, vec![Payment::new(alice.clone(), 5 * ZKA)], 0, proving_key(), &mut zoka_quantic::os_rng())
            .unwrap();
        let txid = pool.insert(&storage, CHAIN, tx.clone(), 2).expect("admitted");
        assert_eq!(txid, tx.txid());
        assert_eq!(pool.insert(&storage, CHAIN, tx.clone(), 3), Err(mempool::MempoolError::AlreadyKnown));

        // Otra que gasta la misma nota choca; una sin la comisión convencional no se retransmite.
        let twin = team
            .create_transfer(&team_sk, vec![Payment::new(alice.clone(), 7 * ZKA)], 0, proving_key(), &mut zoka_quantic::os_rng())
            .unwrap();
        assert_eq!(pool.insert(&storage, CHAIN, twin, 4), Err(mempool::MempoolError::Conflict));
        let note = team.notes().iter().find(|n| n.is_spendable()).unwrap().clone();
        let path = {
            // El testigo de la única nota del equipo: el árbol tiene una sola hoja.
            let mut tree = NoteCommitmentTree::empty();
            tree.append(MerkleHashOrchard::from_cmx(&note.commitment())).unwrap();
            let witness = zoka_quantic::incrementalmerkletree::witness::IncrementalWitness::from_tree(tree).unwrap();
            zoka_quantic::orchard::tree::MerklePath::from(witness.path().unwrap())
        };
        let cheap = build_transfer(
            &team_sk,
            vec![SpendInput { note: note.note, merkle_path: path }],
            vec![Payment::new(alice.clone(), ZKA)],
            1,
            team.anchor(),
            0,
            proving_key(),
            &mut zoka_quantic::os_rng(),
        )
        .unwrap();
        assert!(matches!(pool.insert(&storage, CHAIN, cheap, 5), Err(mempool::MempoolError::FeeTooLow { .. })));

        // El minero la incluye y el bloque vale.
        let selected = pool.select(512);
        assert_eq!(selected.len(), 1);
        let (mut block, validated) =
            miner::build_template(&storage, CHAIN, &miner, proving_key(), selected, now_secs() + 1).unwrap();
        assert_eq!(validated.fees, tx.fee().unwrap());
        miner::solve_pow_parallel(CHAIN, &mut block, bits(), 2, std::time::Duration::from_secs(60), &stop).unwrap();
        apply_block_with_expected_difficulty(&storage, CHAIN, &block, bits()).expect("block with the mempool tx");

        // Ya en la cadena: sale del mempool y no vuelve a entrar.
        assert_eq!(pool.revalidate(&storage, CHAIN), 1);
        assert!(pool.is_empty());
        assert_eq!(pool.insert(&storage, CHAIN, tx, 6), Err(mempool::MempoolError::Spent));

        // Con una nota del mempool ya gastada en otro bloque, el minero arma el bloque sin ella.
        let template = miner::build_template(&storage, CHAIN, &miner, proving_key(), vec![block_tx(&block)], now_secs() + 2);
        let (empty, _) = template.expect("falls back to a block without transfers");
        assert_eq!(empty.quantic_transactions.len(), 1, "only the coinbase");
    }

    fn block_tx(block: &Block) -> QuanticTransaction {
        decode_block(block).unwrap().transactions.remove(0)
    }

    /// Lo que sirve el nodo (por HTTP y por P2P), que es lo que verifica la wallet.
    fn compact_json(storage: &RocksDbStorage, height: u64) -> wallet_sync::CompactBlockJson {
        light::compact_block_at(storage, height).unwrap().expect("the block exists")
    }

    #[test]
    fn the_phone_engine_follows_a_real_chain_spends_and_resumes() {
        use crate::mobile_quantic::{quantic_build_transfer_json, quantic_sync_json, quantic_wallet_info_json};

        let (storage, _dir) = storage();
        let team_seed = hex::encode(REGTEST_TEAM_ROOT);
        let alice_sk = QuanticSpendingKey::from_root(&[3; 32], Network::Regtest, 0).unwrap();
        let alice = alice_sk.full_viewing_key().address();
        let miner = QuanticSpendingKey::from_root(&[4; 32], Network::Regtest, 0)
            .unwrap()
            .full_viewing_key()
            .address();
        let params = consensus_params(Network::Regtest);
        let call = |f: fn(&str) -> Result<String, String>, request: serde_json::Value| -> Result<serde_json::Value, String> {
            f(&request.to_string()).map(|json| serde_json::from_str(&json).unwrap())
        };

        // La dirección del motor es la de la wallet del equipo de regtest.
        let info = call(quantic_wallet_info_json, serde_json::json!({ "seed_hex": team_seed, "network": "quantic-regtest" })).unwrap();
        let team_address = info["address"].as_str().unwrap().to_string();
        assert_eq!(team_address, premine(Network::Regtest).unwrap().recipient.encode());

        let b1 = mined(&storage, &genesis_parent_for(CHAIN), &coinbase(1, &premine(Network::Regtest).unwrap().recipient, PREMINE_ATOMS, true), &[]);
        apply_block_with_expected_difficulty(&storage, CHAIN, &b1, bits()).unwrap();
        let b2 = mined(&storage, &b1, &coinbase(2, &miner, params.minted_at(2), false), &[]);
        apply_block_with_expected_difficulty(&storage, CHAIN, &b2, bits()).unwrap();

        // Desde el génesis, con un estado seguro: la punta es 2 y el seguro queda antes.
        let blocks = light::compact_blocks(&storage, 0, None, 500).unwrap();
        let synced = call(
            quantic_sync_json,
            serde_json::json!({ "seed_hex": team_seed, "network": "quantic-regtest", "tip_height": 2, "blocks": blocks }),
        )
        .unwrap();
        assert_eq!(synced["balance_atoms"], PREMINE_ATOMS);
        assert_eq!(synced["current"]["next_height"], 3);
        assert_eq!(synced["current"]["last_hash"], b2.header.hash.as_str());
        assert!(synced["safe"].is_null(), "a chain of two blocks has nothing 120 deep");
        let state = synced["current"]["state_hex"].as_str().unwrap().to_string();

        // El envío que arma el motor lo admite el mempool y entra en un bloque.
        let sent = call(
            quantic_build_transfer_json,
            serde_json::json!({
                "seed_hex": team_seed, "network": "quantic-regtest", "state_hex": state,
                "to": alice.encode(), "amount_atoms": 25 * ZKA, "memo_text": "gracias",
            }),
        )
        .unwrap();
        let tx = QuanticTransaction::from_bytes(&hex::decode(sent["tx_hex"].as_str().unwrap()).unwrap()).unwrap();
        assert_eq!(sent["txid"], tx.txid().to_hex().as_str());
        let fee = tx.fee().unwrap();
        assert_eq!(sent["fee_atoms"], fee);
        mempool::QuanticMempool::new().insert(&storage, CHAIN, tx.clone(), now_secs()).expect("the phone's transfer is valid");
        let b3 = mined(&storage, &b2, &coinbase(3, &miner, params.minted_at(3) + fee, false), &[tx]);
        apply_block_with_expected_difficulty(&storage, CHAIN, &b3, bits()).unwrap();

        // Sigue desde el estado guardado con el bloque nuevo solo.
        let resumed = call(
            quantic_sync_json,
            serde_json::json!({
                "seed_hex": team_seed, "network": "quantic-regtest", "state_hex": state,
                "last_hash": b2.header.hash, "tip_height": 3, "blocks": [compact_json(&storage, 3)],
            }),
        )
        .unwrap();
        assert_eq!(resumed["balance_atoms"], PREMINE_ATOMS - 25 * ZKA - fee);
        let history = resumed["history"].as_array().unwrap();
        assert_eq!(history[0]["net_atoms"], -((25 * ZKA + fee) as i64));
        assert_eq!(history[0]["height"], 3);
        // El memo viaja cifrado con la nota: Alice lo lee del bloque entero.
        let mut alice_full = QuanticWallet::new(alice_sk.full_viewing_key(), &params);
        for block in [&b1, &b2, &b3] {
            alice_full.scan_block(&decode_block(block).unwrap());
        }
        let memo = alice_full.notes()[0].memo.expect("full blocks carry the memo");
        assert!(memo.starts_with(b"gracias") && memo[7..].iter().all(|b| *b == 0));
        // Alice, desde el génesis con la misma interfaz, ve sus 25 ZKA.
        let alice_view = call(
            quantic_sync_json,
            serde_json::json!({
                "seed_hex": hex::encode([3u8; 32]), "network": "quantic-regtest",
                "blocks": light::compact_blocks(&storage, 1, None, 500).unwrap(),
            }),
        )
        .unwrap();
        assert_eq!(alice_view["balance_atoms"], 25 * ZKA);

        // Un bloque que no sigue al último escaneado es una reorganización.
        let reorg = call(
            quantic_sync_json,
            serde_json::json!({
                "seed_hex": team_seed, "network": "quantic-regtest", "state_hex": state,
                "last_hash": "00".repeat(32), "blocks": [compact_json(&storage, 3)],
            }),
        )
        .unwrap_err();
        assert!(reorg.starts_with("reorg:"), "{reorg}");
        // El estado de otra wallet no se abre con esta semilla.
        let other = call(
            quantic_sync_json,
            serde_json::json!({
                "seed_hex": hex::encode([3u8; 32]), "network": "quantic-regtest", "state_hex": state,
                "last_hash": b2.header.hash, "blocks": [],
            }),
        )
        .unwrap_err();
        assert!(other.contains("another wallet"), "{other}");
        // Ni para otra red.
        assert!(call(
            quantic_sync_json,
            serde_json::json!({ "seed_hex": team_seed, "network": "quantic-testnet", "state_hex": state, "blocks": [] }),
        )
        .is_err());
    }

    #[test]
    fn a_compact_response_stops_at_its_byte_budget_and_the_wallet_continues() {
        let (storage, _dir) = storage();
        let team = premine(Network::Regtest).unwrap().recipient;
        let params = consensus_params(Network::Regtest);
        let mut parent = genesis_parent_for(CHAIN);
        for height in 1..=3u64 {
            let value = if height == 1 { PREMINE_ATOMS } else { params.minted_at(height as u32) };
            let block = mined(&storage, &parent, &coinbase(height, &team, value, height == 1), &[]);
            apply_block_with_expected_difficulty(&storage, CHAIN, &block, bits()).unwrap();
            parent = block;
        }
        let size = |h: u64| -> usize { compact_json(&storage, h).transactions.iter().map(String::len).sum() };
        let heights = |blocks: Vec<wallet_sync::CompactBlockJson>| blocks.iter().map(|b| b.height).collect::<Vec<_>>();
        // Sin presupuesto que alcance, igual va el primero: la wallet avanza.
        assert_eq!(heights(light::compact_blocks_within(&storage, 1, None, 500, 1).unwrap()), vec![1]);
        let two = size(1) + size(2);
        assert_eq!(heights(light::compact_blocks_within(&storage, 1, None, 500, two).unwrap()), vec![1, 2]);
        assert_eq!(heights(light::compact_blocks_within(&storage, 1, None, 500, two + size(3)).unwrap()), vec![1, 2, 3]);
        assert_eq!(heights(light::compact_blocks(&storage, 1, None, 500).unwrap()), vec![1, 2, 3]);
        // La wallet sigue desde donde quedó la respuesta cortada.
        let team_fvk = QuanticSpendingKey::from_root(&REGTEST_TEAM_ROOT, Network::Regtest, 0).unwrap().full_viewing_key();
        let wallet = wallet_sync::sync_wallet(team_fvk, 3, |from, to| {
            light::compact_blocks_within(&storage, from, Some(to), 500, 1)
        })
        .unwrap();
        assert_eq!(wallet.next_height(), 4);
        assert_eq!(wallet.balance(), PREMINE_ATOMS);
    }

    #[test]
    fn a_snapshot_rebuilds_the_quantic_state_in_a_fresh_node() {
        let (source, _source_dir) = storage();
        let (target, _target_dir) = storage();
        let target = std::sync::Arc::new(target);
        let params = consensus_params(Network::Regtest);
        let team_sk = QuanticSpendingKey::from_root(&REGTEST_TEAM_ROOT, Network::Regtest, 0).unwrap();
        let alice = QuanticSpendingKey::from_root(&[3; 32], Network::Regtest, 0).unwrap().full_viewing_key().address();
        let miner = QuanticSpendingKey::from_root(&[4; 32], Network::Regtest, 0).unwrap().full_viewing_key().address();
        let mut team = QuanticWallet::new(team_sk.full_viewing_key(), &params);
        let b1 = mined(&source, &genesis_parent_for(CHAIN), &coinbase(1, &team.fvk().address(), PREMINE_ATOMS, true), &[]);
        apply_block_with_expected_difficulty(&source, CHAIN, &b1, bits()).unwrap();
        team.scan_block(&decode_block(&b1).unwrap());
        let tx = team
            .create_transfer(&team_sk, vec![Payment::new(alice, 9 * ZKA)], 0, proving_key(), &mut zoka_quantic::os_rng())
            .unwrap();
        let fee = tx.fee().unwrap();
        let b2 = mined(&source, &b1, &coinbase(2, &miner, params.minted_at(2) + fee, false), &[tx.clone()]);
        apply_block_with_expected_difficulty(&source, CHAIN, &b2, bits()).unwrap();
        let b3 = mined(&source, &b2, &coinbase(3, &miner, params.minted_at(3), false), &[]);
        apply_block_with_expected_difficulty(&source, CHAIN, &b3, bits()).unwrap();

        let snapshot = crate::snapshot::export_from_storage(&source, CHAIN, now_secs()).unwrap();
        assert_eq!(crate::snapshot::import_into_storage(&target, CHAIN, &snapshot).unwrap(), 3);
        // La misma punta y el mismo árbol de notas, rearmados bloque por bloque.
        assert_eq!(target.get_last_block_hash().unwrap(), b3.header.hash);
        for block in [&b1, &b2, &b3] {
            assert_eq!(
                tree_root_after(&target, &block.header.hash).unwrap(),
                tree_root_after(&source, &block.header.hash).unwrap()
            );
        }
        // Lo gastado sigue gastado: el mismo envío no vuelve a entrar.
        assert!(mempool::QuanticMempool::new().insert(&target, CHAIN, tx, now_secs()).is_err());
        // Y la cadena importada sigue: un bloque nuevo encima se acepta.
        let b4 = mined(&target, &b3, &coinbase(4, &miner, params.minted_at(4), false), &[]);
        apply_block_with_expected_difficulty(&target, CHAIN, &b4, bits()).unwrap();
        // Importar dos veces no cambia nada.
        assert_eq!(crate::snapshot::import_into_storage(&target, CHAIN, &snapshot).unwrap(), 0);
        assert_eq!(target.get_last_block_hash().unwrap(), b4.header.hash);
    }

    #[test]
    fn the_node_answers_light_wallets_with_its_chain() {
        use crate::network::quantic_light_request_response::{QuanticLightRequest, QuanticLightResponse};

        let (storage, _dir) = storage();
        let params = consensus_params(Network::Regtest);
        let team_sk = QuanticSpendingKey::from_root(&REGTEST_TEAM_ROOT, Network::Regtest, 0).unwrap();
        let alice = QuanticSpendingKey::from_root(&[3; 32], Network::Regtest, 0)
            .unwrap()
            .full_viewing_key()
            .address();
        let mut team = QuanticWallet::new(team_sk.full_viewing_key(), &params);
        let ask = |request: QuanticLightRequest| light::answer(&storage, CHAIN, &request);

        // Antes del génesis: altura 0, sin árbol, sin bloques.
        assert!(matches!(ask(QuanticLightRequest::Info), QuanticLightResponse::Info { height: 0, tree_root: None, .. }));
        assert_eq!(ask(QuanticLightRequest::Compact { from: 0, to: 10 }), QuanticLightResponse::Compact { blocks: vec![] });

        let b1 = mined(&storage, &genesis_parent_for(CHAIN), &coinbase(1, &team.fvk().address(), PREMINE_ATOMS, true), &[]);
        apply_block_with_expected_difficulty(&storage, CHAIN, &b1, bits()).unwrap();
        team.scan_block(&decode_block(&b1).unwrap());
        match ask(QuanticLightRequest::Info) {
            QuanticLightResponse::Info { chain_id, height, tip, tree_root } => {
                assert_eq!((chain_id.as_str(), height, tip.as_str()), (CHAIN, 1, b1.header.hash.as_str()));
                assert_eq!(tree_root, Some(hex::encode(team.anchor().to_bytes())), "the anchor of the next transfer");
            }
            other => panic!("{other:?}"),
        }
        // Desde antes del génesis hasta más allá de la punta: lo que hay.
        assert_eq!(
            ask(QuanticLightRequest::Compact { from: 0, to: 99 }),
            QuanticLightResponse::Compact { blocks: vec![compact_json(&storage, 1)] }
        );

        // Un envío armado en el teléfono entra al mempool; repetido o roto, no.
        let tx = team
            .create_transfer(&team_sk, vec![Payment::new(alice, ZKA)], 0, proving_key(), &mut zoka_quantic::os_rng())
            .unwrap();
        let tx_hex = hex::encode(tx.to_bytes());
        match ask(QuanticLightRequest::SubmitTx { tx_hex: tx_hex.clone() }) {
            QuanticLightResponse::Submitted { accepted: true, txid: Some(txid), reason: None } => {
                assert_eq!(txid, tx.txid().to_hex())
            }
            other => panic!("{other:?}"),
        }
        for bad in [tx_hex, "zz".to_string(), "00".to_string()] {
            assert!(matches!(
                ask(QuanticLightRequest::SubmitTx { tx_hex: bad }),
                QuanticLightResponse::Submitted { accepted: false, txid: None, reason: Some(_) }
            ));
        }
    }

    #[test]
    fn a_wallet_syncs_from_compact_blocks_and_catches_a_node_that_lies() {
        let (storage, _dir) = storage();
        let params = consensus_params(Network::Regtest);
        let team_sk = QuanticSpendingKey::from_root(&REGTEST_TEAM_ROOT, Network::Regtest, 0).unwrap();
        let alice_sk = QuanticSpendingKey::from_root(&[3; 32], Network::Regtest, 0).unwrap();
        let miner = QuanticSpendingKey::from_root(&[4; 32], Network::Regtest, 0)
            .unwrap()
            .full_viewing_key()
            .address();
        let mut team = QuanticWallet::new(team_sk.full_viewing_key(), &params);
        let genesis_parent = genesis_parent_for(CHAIN);
        let b1 = mined(&storage, &genesis_parent, &coinbase(1, &team.fvk().address(), PREMINE_ATOMS, true), &[]);
        apply_block_with_expected_difficulty(&storage, CHAIN, &b1, bits()).unwrap();
        team.scan_block(&decode_block(&b1).unwrap());
        let tx = team
            .create_transfer(&team_sk, vec![Payment::new(alice_sk.full_viewing_key().address(), 3 * ZKA)], 0, proving_key(), &mut zoka_quantic::os_rng())
            .unwrap();
        let fee = tx.fee().unwrap();
        let b2 = mined(&storage, &b1, &coinbase(2, &miner, params.minted_at(2) + fee, false), &[tx]);
        apply_block_with_expected_difficulty(&storage, CHAIN, &b2, bits()).unwrap();

        // Alice, sincronizada solo con bloques compactos, ve sus 3 ZKA y puede gastarlos.
        let honest = |from: u64, to: u64| Ok((from..=to).map(|h| compact_json(&storage, h)).collect());
        let alice = wallet_sync::sync_wallet(alice_sk.full_viewing_key(), 2, honest).unwrap();
        assert_eq!(alice.balance(), 3 * ZKA);
        assert_eq!(alice.anchor().to_bytes(), tree_root_after(&storage, &b2.header.hash).unwrap().unwrap());
        let spend = alice
            .create_transfer(&alice_sk, vec![Payment::new(miner.clone(), ZKA)], 0, proving_key(), &mut zoka_quantic::os_rng())
            .unwrap();
        let mut pool = mempool::QuanticMempool::new();
        pool.insert(&storage, CHAIN, spend, 1).expect("a light wallet's spend is valid");

        // Un nodo que le esconde la transferencia del bloque 2.
        let hiding = |from: u64, to: u64| {
            Ok((from..=to)
                .map(|h| {
                    let mut block = compact_json(&storage, h);
                    if h == 2 {
                        block.transactions.truncate(1);
                    }
                    block
                })
                .collect())
        };
        let error = wallet_sync::sync_wallet(alice_sk.full_viewing_key(), 2, hiding).unwrap_err();
        assert!(error.contains("does not match"), "{error}");
        // Uno que le cambia la raíz del árbol para hacerle armar envíos inválidos.
        let wrong_tree = |from: u64, to: u64| {
            Ok((from..=to)
                .map(|h| {
                    let mut block = compact_json(&storage, h);
                    let mut roots = QuanticRoots::parse(&block.state_root).unwrap();
                    roots.tree_root = [7; 32];
                    block.state_root = roots.to_state_root();
                    block
                })
                .collect())
        };
        assert!(wallet_sync::sync_wallet(alice_sk.full_viewing_key(), 2, wrong_tree).is_err());
    }

    #[test]
    fn a_transfer_of_a_reorganized_block_returns_to_the_mempool() {
        let (storage, _dir) = storage();
        let params = consensus_params(Network::Regtest);
        let team_sk = QuanticSpendingKey::from_root(&REGTEST_TEAM_ROOT, Network::Regtest, 0).unwrap();
        let alice = QuanticSpendingKey::from_root(&[3; 32], Network::Regtest, 0).unwrap().full_viewing_key().address();
        let miner = QuanticSpendingKey::from_root(&[4; 32], Network::Regtest, 0).unwrap().full_viewing_key().address();
        let other_miner = QuanticSpendingKey::from_root(&[5; 32], Network::Regtest, 0).unwrap().full_viewing_key().address();
        let mut team = QuanticWallet::new(team_sk.full_viewing_key(), &params);

        let b1 = mined(&storage, &genesis_parent_for(CHAIN), &coinbase(1, &team.fvk().address(), PREMINE_ATOMS, true), &[]);
        apply_block_with_expected_difficulty(&storage, CHAIN, &b1, bits()).unwrap();
        team.scan_block(&decode_block(&b1).unwrap());
        let tx = team
            .create_transfer(&team_sk, vec![Payment::new(alice, 2 * ZKA)], 0, proving_key(), &mut zoka_quantic::os_rng())
            .unwrap();
        let fee = tx.fee().unwrap();
        let b2 = mined(&storage, &b1, &coinbase(2, &miner, params.minted_at(2) + fee, false), std::slice::from_ref(&tx));
        apply_block_with_expected_difficulty(&storage, CHAIN, &b2, bits()).unwrap();
        assert!(runtime::orphaned_transfers(&storage, &b2.header.hash).is_empty(), "b2 is canonical");

        // Otra rama desde b1, sin la transferencia y con más trabajo.
        let mut parent = b1.clone();
        for round in 0..12u64 {
            let height = parent.header.block_height + 1;
            let cb = coinbase(height, &other_miner, params.minted_at(height as u32), false);
            let (mut side, _) = assemble_block(&storage, CHAIN, &parent, &cb, &[], now_secs() - 50_000 + round * 10).unwrap();
            solve_pow(CHAIN, &mut side, bits()).unwrap();
            let _ = apply_block_with_expected_difficulty(&storage, CHAIN, &side, bits());
            if storage.get_last_block_hash().unwrap() == side.header.hash {
                break;
            }
            parent = side;
        }
        assert_ne!(storage.get_last_block_hash().unwrap(), b2.header.hash, "the other branch won");

        let orphaned = runtime::orphaned_transfers(&storage, &b2.header.hash);
        assert_eq!(orphaned.len(), 1);
        assert_eq!(orphaned[0].txid(), tx.txid());
        let mut pool = mempool::QuanticMempool::new();
        pool.insert(&storage, CHAIN, orphaned[0].clone(), 1)
            .expect("the note is unspent again and its anchor is still in the window");
    }

    #[test]
    fn a_block_needs_a_time_after_the_median_and_not_far_in_the_future() {
        let (storage, _dir) = storage();
        let team = QuanticSpendingKey::from_root(&REGTEST_TEAM_ROOT, Network::Regtest, 0).unwrap();
        let miner = QuanticSpendingKey::from_root(&[4; 32], Network::Regtest, 0).unwrap().full_viewing_key().address();
        let params = consensus_params(Network::Regtest);
        let b1 = mined(&storage, &genesis_parent_for(CHAIN), &coinbase(1, &team.full_viewing_key().address(), PREMINE_ATOMS, true), &[]);
        apply_block_with_expected_difficulty(&storage, CHAIN, &b1, bits()).unwrap();
        let cb = coinbase(2, &miner, params.minted_at(2), false);
        // La misma hora que el génesis (la mediana de un solo bloque): no.
        assert!(matches!(
            assemble_block(&storage, CHAIN, &b1, &cb, &[], b1.header.timestamp),
            Err(QuanticBlockError::TimeTooOld { .. })
        ));
        // Tres horas adelante del reloj: no.
        assert!(matches!(
            assemble_block(&storage, CHAIN, &b1, &cb, &[], now_secs() + 3 * 3600),
            Err(QuanticBlockError::TimeTooNew { .. })
        ));
        // Un segundo después de la mediana: sí.
        assert!(assemble_block(&storage, CHAIN, &b1, &cb, &[], b1.header.timestamp + 1).is_ok());
        assert_eq!(median_time_past(&storage, &b1).unwrap(), b1.header.timestamp);
    }
}
