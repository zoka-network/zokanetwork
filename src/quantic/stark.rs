//! STARK pool wired into the node's block validator, PoW, fork choice and DB.
//! Its chains have their own ids (see [`Profile`]). Public v1 chains cannot
//! accept this format.
use std::collections::BTreeMap;

use crate::{
    economics::{
        chain_uses_randomx, pow_algorithm_for_chain, QUANTIC_PQ_REGTEST_CHAIN_ID,
        QUANTIC_PQ_TESTNET_CHAIN_ID, QUANTIC_PQ_TESTNET_GENESIS,
    },
    state::chain::genesis_parent_for,
    storage::RocksDbStorage,
    types::{Block, PublicKey, Signature},
};
use protocol::{
    ledger::{subsidy, Ledger, MAX_BLOCK_BYTES, MAX_BLOCK_TRANSACTIONS},
    transaction::{coinbase, Transaction},
    wallet::Address,
};
pub use zoka_pq_pool as protocol;

const PREFIX: &str = "quantic-v2:";
const STATE_KEY: &str = "quantic:stark:state:";
pub const MEMPOOL_BYTES: usize = 128 * 1024 * 1024;

/// A STARK v2 chain: its id, the pool's network byte (which also selects the
/// address prefix) and, once launched, the hash of its block 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Profile {
    pub chain_id: &'static str,
    pub network: u8,
    pub name: &'static str,
    pub genesis: Option<&'static str>,
}
impl Profile {
    /// Local chain: CPU test PoW at a fixed difficulty.
    pub const REGTEST: Self = Self {
        chain_id: QUANTIC_PQ_REGTEST_CHAIN_ID,
        network: 3,
        name: "regtest",
        genesis: None,
    };
    /// RandomX (XMRig) and ASERT, like the v1 testnet it replaces.
    pub const TESTNET: Self = Self {
        chain_id: QUANTIC_PQ_TESTNET_CHAIN_ID,
        network: 2,
        name: "testnet",
        genesis: QUANTIC_PQ_TESTNET_GENESIS,
    };
    pub const ALL: [Self; 2] = [Self::REGTEST, Self::TESTNET];

    pub fn for_chain(chain_id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.chain_id == chain_id)
    }
    pub fn named(name: &str) -> Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|p| p.name == name)
            .ok_or_else(|| format!("unknown STARK network {name}; use regtest or testnet"))
    }
    pub fn genesis_parent(&self) -> Block {
        genesis_parent_for(self.chain_id)
    }
    pub fn pow_algorithm(&self) -> &'static str {
        pow_algorithm_for_chain(self.chain_id)
    }
    /// XMRig computes RandomX (`rx/0`): only those chains can use the Stratum.
    pub fn xmrig_compatible(&self) -> bool {
        chain_uses_randomx(self.chain_id)
    }
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn encode_state(state: &Ledger) -> Result<Vec<u8>, String> {
    postcard::to_allocvec(state).map_err(err)
}
fn state_root(bytes: &[u8]) -> String {
    let mut hash = blake3::Hasher::new_derive_key("ZOKA QUANTIC STARK state v2");
    hash.update(bytes);
    let mut digest = [0; 64];
    hash.finalize_xof().fill(&mut digest);
    format!("{PREFIX}{}", hex::encode(digest))
}
fn digest_hex(digest: &protocol::crypto::Digest) -> String {
    hex::encode(
        digest
            .0
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<_>>(),
    )
}
fn nullifiers(txs: &[Transaction]) -> Vec<String> {
    let mut result: Vec<_> = txs
        .iter()
        .flat_map(|tx| tx.statement.nullifiers.iter().map(digest_hex))
        .collect();
    result.sort();
    result
}

pub struct Validated {
    state: Ledger,
    bytes: Vec<u8>,
}
impl Validated {
    pub fn minted(&self) -> u64 {
        subsidy(self.state.height)
    }
}

/// Only locally validated states are cached. The header commits to their full
/// canonical encoding, including anchors, pending rewards and spent nullifiers.
pub fn state_after(
    storage: &RocksDbStorage,
    profile: Profile,
    parent: &Block,
) -> Result<Ledger, String> {
    if parent.header.block_height == 0 {
        return Ledger::new(profile.network).map_err(err);
    }
    let bytes = storage
        .get_raw(&format!("{STATE_KEY}{}", parent.header.hash))
        .map_err(err)?
        .ok_or_else(|| "missing STARK ancestor state".to_string())?;
    if state_root(&bytes) != parent.header.state_root {
        return Err("stored STARK state commitment mismatch".into());
    }
    let state: Ledger = postcard::from_bytes(&bytes).map_err(err)?;
    if state.height as u64 != parent.header.block_height || state.network != profile.network {
        return Err("stored STARK state network/height".into());
    }
    Ok(state)
}

pub fn decode(block: &Block) -> Result<Vec<Transaction>, String> {
    super::validate_shape(block).map_err(err)?;
    if !block.header.state_root.starts_with(PREFIX) {
        return Err("STARK v2 block required".into());
    }
    if block.quantic_transactions.len() > MAX_BLOCK_TRANSACTIONS + 1 {
        return Err("STARK transaction count limit".into());
    }
    let size = block
        .quantic_transactions
        .iter()
        .fold(0usize, |s, v| s.saturating_add(v.len()));
    if size > MAX_BLOCK_BYTES {
        return Err("STARK block byte limit".into());
    }
    block
        .quantic_transactions
        .iter()
        .map(|bytes| Transaction::decode(bytes).map_err(err))
        .collect()
}

pub fn validate(
    storage: &RocksDbStorage,
    profile: Profile,
    block: &Block,
    parent: &Block,
) -> Result<Validated, String> {
    let txs = decode(block)?;
    if txs.is_empty() {
        return Err("STARK block requires a coinbase".into());
    }
    if block.header.block_height != parent.header.block_height + 1
        || block.header.prev_hash != parent.header.hash
    {
        return Err("STARK parent/height".into());
    }
    // A launched network is the one whose premine block is in the binary.
    if block.header.block_height == 1
        && profile
            .genesis
            .is_some_and(|genesis| genesis != block.header.hash)
    {
        return Err("not this network's STARK genesis".into());
    }
    let median = super::median_time_past(storage, parent).map_err(err)?;
    if block.header.timestamp <= median
        || block.header.timestamp > super::now_secs().saturating_add(super::MAX_FUTURE_SECS)
    {
        return Err("STARK block timestamp".into());
    }
    if nullifiers(&txs) != block.private_nullifiers {
        return Err("STARK nullifier commitment".into());
    }
    let mut state = state_after(storage, profile, parent)?;
    state.apply(&txs[0], &txs[1..]).map_err(err)?;
    let bytes = encode_state(&state)?;
    if state_root(&bytes) != block.header.state_root {
        return Err("STARK state commitment".into());
    }
    Ok(Validated { state, bytes })
}

pub fn store(storage: &RocksDbStorage, block: &Block, validated: &Validated) -> Result<(), String> {
    storage
        .put_raw(
            &format!("{STATE_KEY}{}", block.header.hash),
            validated.bytes.clone(),
        )
        .map_err(err)?;
    // The shared difficulty/timestamp walker only needs these common fields.
    let meta = super::QuanticBlockMeta {
        height: block.header.block_height,
        prev_hash: block.header.prev_hash.clone(),
        timestamp: block.header.timestamp,
        tree_root: digest_hex(&validated.state.tree.root()),
        coinbase_commitments: vec![],
    };
    storage
        .put_raw(
            &format!("{}{}", super::META_KEY_PREFIX, block.header.hash),
            serde_json::to_vec(&meta).map_err(err)?,
        )
        .map_err(err)
}

/// Produces a normal node Block. Work is committed to its complete body and
/// state through the existing 76-byte mining blob (nonce at offset 39).
pub fn template(
    storage: &RocksDbStorage,
    profile: Profile,
    parent: &Block,
    to: &Address,
    txs: Vec<Transaction>,
    timestamp: u64,
) -> Result<Block, String> {
    if to.network != profile.network {
        return Err(format!("{} requires a {} v2 address", profile.chain_id, profile.name));
    }
    let mut state = state_after(storage, profile, parent)?;
    let height = state.height.checked_add(1).ok_or("height overflow")?;
    let fees = txs.iter().try_fold(0u64, |v, tx| {
        v.checked_add(tx.statement.fee).ok_or("fee overflow")
    })?;
    let cb = coinbase(
        to,
        subsidy(height).checked_add(fees).ok_or("reward overflow")?,
        height,
    )
    .map_err(err)?;
    state.apply(&cb, &txs).map_err(err)?;
    let mut all = vec![cb];
    all.extend(txs);
    let timestamp = timestamp.max(
        super::median_time_past(storage, parent)
            .map_err(err)?
            .saturating_add(1),
    );
    let mut block = Block::new(
        parent.header.hash.clone(),
        timestamp,
        0,
        height.into(),
        vec![],
        PublicKey(vec![]),
        Signature(vec![]),
    );
    block.private_nullifiers = nullifiers(&all);
    block.quantic_transactions = all
        .iter()
        .map(Transaction::encode)
        .collect::<Result<_, _>>()
        .map_err(err)?;
    block.header.state_root = state_root(&encode_state(&state)?);
    block.header.merkle_root = block.compute_merkle_root();
    block.header.hash = block.compute_hash();
    Ok(block)
}

pub fn mine(
    storage: &RocksDbStorage,
    profile: Profile,
    parent: &Block,
    to: &Address,
    txs: Vec<Transaction>,
    timestamp: u64,
) -> Result<Block, String> {
    let mut block = template(storage, profile, parent, to, txs, timestamp)?;
    let target = super::compact_target_for_child(storage, profile.chain_id, parent).map_err(err)?;
    super::solve_pow(profile.chain_id, &mut block, target)?;
    Ok(block)
}

/// Bounded mempool with proof validation, conflicting-spend rejection and
/// revalidation against the canonical state after a tip change/reorganization.
#[derive(Default)]
pub struct Mempool {
    transactions: BTreeMap<[u8; 32], Transaction>,
    bytes: usize,
}
impl Mempool {
    pub fn len(&self) -> usize {
        self.transactions.len()
    }
    pub fn is_empty(&self) -> bool {
        self.transactions.is_empty()
    }
    pub fn insert(&mut self, state: &Ledger, tx: Transaction) -> Result<(), String> {
        let size = tx.encode().map_err(err)?.len();
        if self.transactions.contains_key(&tx.txid()) {
            return Err("already in mempool".into());
        }
        if self.bytes.saturating_add(size) > MEMPOOL_BYTES {
            return Err("STARK mempool full".into());
        }
        if self.transactions.values().any(|other| {
            other
                .statement
                .nullifiers
                .iter()
                .any(|nf| tx.statement.nullifiers.contains(nf))
        }) {
            return Err("conflicting mempool nullifier".into());
        }
        state.check_transfer(&tx).map_err(err)?;
        self.bytes += size;
        self.transactions.insert(tx.txid(), tx);
        Ok(())
    }
    pub fn revalidate(&mut self, state: &Ledger) {
        self.transactions
            .retain(|_, tx| state.check_transfer(tx).is_ok());
        self.bytes = self
            .transactions
            .values()
            .map(|t| t.encode().map(|v| v.len()).unwrap_or(0))
            .sum();
    }
    pub fn select(&self) -> Vec<Transaction> {
        let mut size = 0usize;
        self.transactions
            .values()
            .filter(|tx| {
                let n = tx.encode().map(|b| b.len()).unwrap_or(MAX_BLOCK_BYTES);
                // Reserve one full transaction allowance for the reward proof.
                if size + n > MAX_BLOCK_BYTES - protocol::transaction::MAX_TRANSACTION_BYTES {
                    return false;
                }
                size += n;
                true
            })
            .take(MAX_BLOCK_TRANSACTIONS)
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::chain::apply_block;
    use protocol::{
        ledger::{ATOMS, PREMINE},
        model::Input,
        transaction::{transfer, Payment},
        wallet::Account,
    };
    const P: Profile = Profile::REGTEST;
    const CHAIN: &str = P.chain_id;

    #[test]
    fn real_node_import_reorg_and_restart_with_private_stark_payments() {
        let dir = tempfile::tempdir().unwrap();
        let db = RocksDbStorage::new(dir.path()).unwrap();
        let alice = Account::from_seed(&[7; 32], 3, 0).unwrap();
        let bob = Account::from_seed(&[8; 32], 3, 0).unwrap();
        let genesis = mine(
            &db,
            P,
            &P.genesis_parent(),
            &alice.address(),
            vec![],
            1_700_000_000,
        )
        .unwrap();
        apply_block(&db, CHAIN, &genesis).unwrap();
        let ledger = state_after(&db, P, &genesis).unwrap();
        let cb = decode(&genesis).unwrap().remove(0);
        let (note, _) = alice
            .receive(cb.statement.commitments[0], &cb.envelopes[0])
            .unwrap();
        let payment = transfer(
            &alice,
            ledger.tree.root(),
            vec![Input {
                key: alice.spend.clone(),
                note,
                path: ledger.tree.path(0).unwrap(),
            }],
            vec![
                Payment::new(bob.address(), ATOMS),
                Payment::new(alice.address(), PREMINE - ATOMS - 200),
            ],
            200,
            10,
        )
        .unwrap();
        let mut pool = Mempool::default();
        pool.insert(&ledger, payment.clone()).unwrap();
        assert!(pool.insert(&ledger, payment.clone()).is_err());
        let paid = mine(&db, P, &genesis, &bob.address(), pool.select(), 1_700_000_120).unwrap();
        apply_block(&db, CHAIN, &paid).unwrap();
        assert_eq!(db.get_last_block_hash().unwrap(), paid.header.hash);
        pool.revalidate(&state_after(&db, P, &paid).unwrap());
        assert!(pool.select().is_empty());
        // A longer competing branch detaches the payment, restoring spendability.
        let side = mine(&db, P, &genesis, &alice.address(), vec![], 1_700_000_121).unwrap();
        // Misma altura que el pago: queda guardado, pero no desplaza la punta.
        let tied = apply_block(&db, CHAIN, &side);
        assert!(matches!(
            tied,
            Err(crate::state::chain::ChainError::SideBranchNotHeavier { .. })
        ));
        let longer = mine(&db, P, &side, &alice.address(), vec![], 1_700_000_240).unwrap();
        apply_block(&db, CHAIN, &longer).unwrap();
        assert_eq!(db.get_last_block_hash().unwrap(), longer.header.hash);
        let restored = state_after(&db, P, &longer).unwrap();
        pool.insert(&restored, payment.clone()).unwrap();
        let paid_again = mine(&db, P, &longer, &bob.address(), pool.select(), 1_700_000_360).unwrap();
        apply_block(&db, CHAIN, &paid_again).unwrap();
        assert!(state_after(&db, P, &paid_again)
            .unwrap()
            .check_transfer(&payment)
            .is_err());
        let mut wrong = mine(&db, P, &paid_again, &alice.address(), vec![], 1_700_000_480).unwrap();
        wrong.header.state_root.push('0');
        let target = super::super::compact_target_for_child(&db, CHAIN, &paid_again).unwrap();
        super::super::solve_pow(CHAIN, &mut wrong, target).unwrap();
        assert!(apply_block(&db, CHAIN, &wrong).is_err());
        drop(db);
        let db = RocksDbStorage::new(dir.path()).unwrap();
        assert_eq!(db.get_chain_height().unwrap(), 4);
        assert_eq!(
            state_after(&db, P, &paid_again).unwrap().issued,
            PREMINE + 36 * ATOMS
        );
        // The published v1 testnet cannot import a block of this separate chain.
        let foreign_dir = tempfile::tempdir().unwrap();
        let foreign = RocksDbStorage::new(foreign_dir.path()).unwrap();
        assert!(apply_block(
            &foreign,
            crate::economics::QUANTIC_TESTNET_CHAIN_ID,
            &genesis
        )
        .is_err());
    }

    #[test]
    fn profiles_keep_testnet_and_regtest_chains_apart() {
        assert_eq!(Profile::for_chain(CHAIN), Some(P));
        assert_eq!(Profile::named("testnet").unwrap(), Profile::TESTNET);
        assert!(Profile::named("mainnet").is_err());
        assert!(!P.xmrig_compatible());
        assert!(Profile::TESTNET.xmrig_compatible());
        assert_eq!(Profile::TESTNET.pow_algorithm(), "zoka-randomx-v1");
        assert_ne!(P.genesis_parent().header.hash, Profile::TESTNET.genesis_parent().header.hash);

        let dir = tempfile::tempdir().unwrap();
        let db = RocksDbStorage::new(dir.path()).unwrap();
        let testnet = Profile::TESTNET;
        let parent = testnet.genesis_parent();
        let local = Account::from_seed(&[5; 32], 3, 0).unwrap().address();
        let err = template(&db, testnet, &parent, &local, vec![], 1_700_000_000)
            .err()
            .unwrap();
        assert!(err.contains("testnet v2 address"), "{err}");
        let miner = Account::from_seed(&[5; 32], 2, 0).unwrap().address();
        assert!(miner.encode().unwrap().starts_with("zqtest2"));
        let block = template(&db, testnet, &parent, &miner, vec![], 1_700_000_000).unwrap();
        // Body, state commitment and reward rules hold under its own profile
        // (PoW is the importer's job), and under no other.
        validate(&db, testnet, &block, &parent).unwrap();
        assert!(validate(&db, P, &block, &parent).is_err());
        let launched = Profile {
            genesis: Some("00"),
            ..testnet
        };
        let err = validate(&db, launched, &block, &parent).err().unwrap();
        assert!(err.contains("genesis"), "{err}");
    }
}
