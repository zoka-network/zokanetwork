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
pub mod mempool;
pub mod miner;
pub mod runtime;
pub mod wallet_file;
pub mod wallet_sync;

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
    Network::ALL.into_iter().find(|n| n.chain_id() == chain_id)
}

/// Raíz de la wallet del equipo en regtest. **Pública a propósito**: regtest
/// es una red local de pruebas sin valor, y así una prueba o una red local
/// pueden gastar su premine. Nunca se usa en la testnet ni en mainnet.
pub const REGTEST_TEAM_ROOT: [u8; 32] = *b"zoka-quantic-regtest-team-root!!";
/// En regtest el coinbase madura en 2 bloques (60 en las redes públicas).
pub const REGTEST_COINBASE_MATURITY: u32 = 2;

/// El premine anunciado de cada red. La testnet todavía no tiene génesis: se
/// fija cuando se cree (paso 7 del diseño).
fn premine(network: Network) -> Option<Premine> {
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
        let maturing_done = maturing_height.map_or(true, |h| meta.height <= h);
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
    validate_shape(candidate)?;
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
pub fn solve_pow(chain_id: &str, block: &mut Block, difficulty_bits: u32) -> Result<(), String> {
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
        if crate::mining::pow::hash_meets_difficulty(&hash, difficulty_bits) {
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
    let validated = validate_block(storage, chain_id, &block, parent)?;
    Ok((block, validated))
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
    /// Dificultad mínima: la prueba de trabajo de regtest es la liviana.
    const BITS: u32 = 1;
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

    fn mined(
        storage: &RocksDbStorage,
        parent: &Block,
        coinbase: &QuanticTransaction,
        transactions: &[QuanticTransaction],
    ) -> Block {
        let (mut block, _) =
            assemble_block(storage, CHAIN, parent, coinbase, transactions, 1_800_000_000 + parent.header.block_height)
                .expect("assembles");
        solve_pow(CHAIN, &mut block, BITS).unwrap();
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
        apply_block_with_expected_difficulty(&storage, CHAIN, &b1, BITS).expect("genesis");
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
        let head = apply_block_with_expected_difficulty(&storage, CHAIN, &b2, BITS).expect("block 2");
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
            apply_block_with_expected_difficulty(&storage, CHAIN, &replay, BITS),
            Err(ChainError::PrivateNullifierDoubleSpend { .. })
        ));

        // Un state_root que no es el del bloque, aun con su prueba de trabajo.
        let mut forged = mined(&storage, &b2, &coinbase(3, &miner, params.minted_at(3), false), &[]);
        forged.header.state_root = QuanticRoots { txid_root: [9; 32], auth_root: [9; 32], tree_root: [9; 32] }.to_state_root();
        forged.header.merkle_root = forged.compute_merkle_root();
        solve_pow(CHAIN, &mut forged, BITS).unwrap();
        assert!(matches!(
            apply_block_with_expected_difficulty(&storage, CHAIN, &forged, BITS),
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
        assert!(apply_block_with_expected_difficulty(&storage, CHAIN, &swapped, BITS).is_err());
        apply_block_with_expected_difficulty(&storage, CHAIN, &honest, BITS).expect("the honest block 3");

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
        let (mut genesis, _) = miner::build_template(&storage, CHAIN, &miner, proving_key(), vec![], 1_800_000_001).unwrap();
        let stop = AtomicBool::new(false);
        assert!(matches!(
            miner::solve_pow_parallel(CHAIN, &mut genesis, BITS, 2, std::time::Duration::from_secs(60), &stop).unwrap(),
            miner::PowOutcome::Solved
        ));
        apply_block_with_expected_difficulty(&storage, CHAIN, &genesis, BITS).expect("genesis from the miner");
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
            miner::build_template(&storage, CHAIN, &miner, proving_key(), selected, 1_800_000_002).unwrap();
        assert_eq!(validated.fees, tx.fee().unwrap());
        miner::solve_pow_parallel(CHAIN, &mut block, BITS, 2, std::time::Duration::from_secs(60), &stop).unwrap();
        apply_block_with_expected_difficulty(&storage, CHAIN, &block, BITS).expect("block with the mempool tx");

        // Ya en la cadena: sale del mempool y no vuelve a entrar.
        assert_eq!(pool.revalidate(&storage, CHAIN), 1);
        assert!(pool.is_empty());
        assert_eq!(pool.insert(&storage, CHAIN, tx, 6), Err(mempool::MempoolError::Spent));

        // Con una nota del mempool ya gastada en otro bloque, el minero arma el bloque sin ella.
        let template = miner::build_template(&storage, CHAIN, &miner, proving_key(), vec![block_tx(&block)], 1_800_000_003);
        let (empty, _) = template.expect("falls back to a block without transfers");
        assert_eq!(empty.quantic_transactions.len(), 1, "only the coinbase");
    }

    fn block_tx(block: &Block) -> QuanticTransaction {
        decode_block(block).unwrap().transactions.remove(0)
    }

    fn compact_json(storage: &RocksDbStorage, height: u64) -> wallet_sync::CompactBlockJson {
        let block = storage.get_canonical_block_by_height(height).unwrap().unwrap();
        let decoded = decode_block(&block).unwrap();
        let compact = zoka_quantic::compact::CompactBlock::from_block(&decoded);
        wallet_sync::CompactBlockJson {
            height,
            hash: block.header.hash.clone(),
            prev_hash: block.header.prev_hash.clone(),
            state_root: block.header.state_root.clone(),
            transactions: compact.transactions.iter().map(|tx| hex::encode(tx.to_bytes())).collect(),
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
        apply_block_with_expected_difficulty(&storage, CHAIN, &b1, BITS).unwrap();
        team.scan_block(&decode_block(&b1).unwrap());
        let tx = team
            .create_transfer(&team_sk, vec![Payment::new(alice_sk.full_viewing_key().address(), 3 * ZKA)], 0, proving_key(), &mut zoka_quantic::os_rng())
            .unwrap();
        let fee = tx.fee().unwrap();
        let b2 = mined(&storage, &b1, &coinbase(2, &miner, params.minted_at(2) + fee, false), &[tx]);
        apply_block_with_expected_difficulty(&storage, CHAIN, &b2, BITS).unwrap();

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
}
