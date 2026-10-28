//! STARK pool wired into the node's block validator, PoW, fork choice and DB.
//! Its chains have their own ids (see [`Profile`]). Public v1 chains cannot
//! accept this format.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

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
    crypto::Digest,
    ledger::{
        self as rules, maturity, subsidy, Frontier, Ledger, LedgerState, ANCHOR_WINDOW,
        MAX_BLOCK_BYTES, MAX_BLOCK_TRANSACTIONS,
    },
    transaction::{coinbase, Transaction},
    wallet::Address,
};
pub use zoka_pq_pool as protocol;

const PREFIX: &str = "quantic-v2:";
/// One [`Record`] per validated block, on any branch.
const RECORD_KEY: &str = "quantic:stark:record:";
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
fn digest_hex(digest: &Digest) -> String {
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

/// The consensus state after a block, which `state_root` commits to: height,
/// issuance, the note tree (size and root) and an accumulator of every spent
/// nullifier. Constant size, whatever the length of the chain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Committed {
    pub network: u8,
    pub height: u32,
    pub issued: u64,
    pub notes: u64,
    pub root: Digest,
    pub nullifiers: [u8; 32],
}
fn state_root(state: &Committed) -> Result<String, String> {
    let bytes = postcard::to_allocvec(state).map_err(err)?;
    let mut hash = blake3::Hasher::new_derive_key("ZOKA QUANTIC STARK state v3");
    hash.update(&bytes);
    let mut digest = [0; 64];
    hash.finalize_xof().fill(&mut digest);
    Ok(format!("{PREFIX}{}", hex::encode(digest)))
}
/// `H(previous ‖ this block's nullifiers, sorted)`: binds the whole spent set.
fn accumulate(previous: &[u8; 32], spent: &BTreeSet<Digest>) -> [u8; 32] {
    let mut hash = blake3::Hasher::new_derive_key("ZOKA QUANTIC STARK nullifiers v1");
    hash.update(previous);
    for nullifier in spent {
        for word in nullifier.0 {
            hash.update(&word.to_le_bytes());
        }
    }
    *hash.finalize().as_bytes()
}

/// What a node keeps per validated block, on any branch: the committed state,
/// the tree frontier to append to, and what later blocks look back for (the
/// coinbase notes that mature later and the nullifiers this block spent).
#[derive(Clone, Serialize, Deserialize)]
struct Record {
    state: Committed,
    frontier: Frontier,
    prev: String,
    coinbase: Vec<Digest>,
    spent: Vec<Digest>,
}
fn read_record(storage: &RocksDbStorage, hash: &str) -> Result<Record, String> {
    let bytes = storage
        .get_raw(&format!("{RECORD_KEY}{hash}"))
        .map_err(err)?
        .ok_or_else(|| "missing STARK ancestor state".to_string())?;
    postcard::from_bytes(&bytes).map_err(err)
}

/// The ledger on top of one block of any branch, without the whole ledger.
/// Anchors and maturing coinbases come from the branch's last records; a
/// nullifier is spent if a block of the branch above the canonical chain spent
/// it, or the canonical index has it at or below the fork point.
pub struct Branch<'a> {
    storage: &'a RocksDbStorage,
    record: Record,
    anchors: BTreeSet<Digest>,
    matured: Vec<Digest>,
    side: BTreeSet<Digest>,
    fork_height: u64,
    spent: BTreeSet<Digest>,
}
impl Branch<'_> {
    pub fn state(&self) -> &Committed {
        &self.record.state
    }
    fn into_record(mut self) -> Record {
        self.record.spent = self.spent.into_iter().collect();
        self.record
    }
}
impl LedgerState for Branch<'_> {
    fn network(&self) -> u8 {
        self.record.state.network
    }
    fn height(&self) -> u32 {
        self.record.state.height
    }
    fn issued(&self) -> u64 {
        self.record.state.issued
    }
    fn notes(&self) -> u64 {
        self.record.frontier.len()
    }
    fn contains_anchor(&self, root: &Digest) -> bool {
        self.anchors.contains(root)
    }
    fn is_spent(&self, nullifier: &Digest) -> Result<bool, protocol::Error> {
        if self.side.contains(nullifier) {
            return Ok(true);
        }
        let canonical = self
            .storage
            .spent_nullifier_height(&digest_hex(nullifier))
            .map_err(|_| protocol::Error::Ledger("nullifier index unavailable"))?;
        Ok(canonical.is_some_and(|height| height <= self.fork_height))
    }
    fn matured_count(&self, height: u32) -> usize {
        if height == self.record.state.height + 1 {
            self.matured.len()
        } else {
            0
        }
    }
    fn take_matured(&mut self, height: u32) -> Vec<Digest> {
        if height == self.record.state.height + 1 {
            std::mem::take(&mut self.matured)
        } else {
            vec![]
        }
    }
    fn append(&mut self, note: Digest) -> Result<(), protocol::Error> {
        let network = self.record.state.network;
        self.record.frontier.append(network, note)
    }
    fn defer(&mut self, _matures_at: u32, notes: Vec<Digest>) {
        // It matures `maturity` blocks later: the branch walk finds it there.
        self.record.coinbase = notes;
    }
    fn spend(&mut self, nullifiers: BTreeSet<Digest>) {
        self.spent = nullifiers;
    }
    fn close(&mut self, height: u32, issued: u64) -> Result<(), protocol::Error> {
        let state = &mut self.record.state;
        state.height = height;
        state.issued = issued;
        state.notes = self.record.frontier.len();
        state.root = self.record.frontier.root(state.network)?;
        state.nullifiers = accumulate(&state.nullifiers, &self.spent);
        Ok(())
    }
}

/// The ledger on top of `parent`, a validated block of any branch (or the
/// genesis parent). Reads at most `ANCHOR_WINDOW` small records.
pub fn branch<'a>(
    storage: &'a RocksDbStorage,
    profile: Profile,
    parent: &Block,
) -> Result<Branch<'a>, String> {
    let network = profile.network;
    let fresh = |record| Branch {
        storage,
        record,
        anchors: BTreeSet::new(),
        matured: vec![],
        side: BTreeSet::new(),
        fork_height: 0,
        spent: BTreeSet::new(),
    };
    if parent.header.block_height == 0 {
        let frontier = Frontier::default();
        return Ok(fresh(Record {
            state: Committed {
                network,
                height: 0,
                issued: 0,
                notes: 0,
                root: frontier.root(network).map_err(err)?,
                nullifiers: [0; 32],
            },
            frontier,
            prev: parent.header.hash.clone(),
            coinbase: vec![],
            spent: vec![],
        }));
    }
    let top = read_record(storage, &parent.header.hash)?;
    if state_root(&top.state)? != parent.header.state_root {
        return Err("stored STARK state commitment mismatch".into());
    }
    if u64::from(top.state.height) != parent.header.block_height || top.state.network != network {
        return Err("stored STARK state network/height".into());
    }
    let next = top.state.height + 1;
    let due = next.checked_sub(maturity(network)).filter(|h| *h >= 2);
    let mut walk = fresh(Record {
        prev: parent.header.hash.clone(),
        coinbase: vec![],
        spent: vec![],
        ..top.clone()
    });
    let mut fork_height = None;
    let (mut hash, mut record, mut depth) = (parent.header.hash.clone(), top, 0usize);
    loop {
        let height = record.state.height;
        if depth < ANCHOR_WINDOW {
            walk.anchors.insert(record.state.root);
        }
        if Some(height) == due {
            walk.matured = record.coinbase.clone();
        }
        if fork_height.is_none() {
            let canonical = storage
                .get_canonical_block_hash_at_height(u64::from(height))
                .map_err(err)?;
            if canonical.as_deref() == Some(hash.as_str()) {
                fork_height = Some(u64::from(height));
            } else {
                walk.side.extend(record.spent.iter().copied());
            }
        }
        depth += 1;
        let done = depth >= ANCHOR_WINDOW && due.is_none_or(|due| height <= due);
        if height == 1 || (done && fork_height.is_some()) {
            break;
        }
        hash = record.prev.clone();
        record = read_record(storage, &hash)?;
    }
    // A branch with another block 1 shares nothing with the canonical index.
    walk.fork_height = fork_height.unwrap_or(0);
    Ok(walk)
}

/// The complete ledger of the canonical chain up to `tip`, rebuilt by
/// replaying its (already validated) blocks: what a wallet needs for note
/// positions and Merkle paths. Linear in the chain; consensus never uses it.
pub fn full_ledger(
    storage: &RocksDbStorage,
    profile: Profile,
    tip: &Block,
) -> Result<Ledger, String> {
    let mut ledger = Ledger::new(profile.network).map_err(err)?;
    let height = tip.header.block_height;
    if height == 0 {
        return Ok(ledger);
    }
    if storage
        .get_canonical_block_hash_at_height(height)
        .map_err(err)?
        .as_deref()
        != Some(tip.header.hash.as_str())
    {
        return Err("a full ledger needs a canonical tip".into());
    }
    for at in 1..=height {
        let block = storage
            .get_canonical_block_by_height(at)
            .map_err(err)?
            .ok_or("a full ledger needs every canonical block")?;
        let txs = decode(&block)?;
        let (coinbase, transfers) = txs.split_first().ok_or("block without a coinbase")?;
        ledger.replay(coinbase, transfers).map_err(err)?;
    }
    if ledger.tree.root() != read_record(storage, &tip.header.hash)?.state.root {
        return Err("replayed notes do not match the stored state".into());
    }
    Ok(ledger)
}

pub struct Validated {
    record: Record,
}
impl Validated {
    pub fn minted(&self) -> u64 {
        subsidy(self.record.state.height)
    }
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
    let mut state = branch(storage, profile, parent)?;
    rules::apply(&mut state, &txs[0], &txs[1..]).map_err(err)?;
    let record = state.into_record();
    if state_root(&record.state)? != block.header.state_root {
        return Err("STARK state commitment".into());
    }
    Ok(Validated { record })
}

pub fn store(storage: &RocksDbStorage, block: &Block, validated: &Validated) -> Result<(), String> {
    storage
        .put_raw(
            &format!("{RECORD_KEY}{}", block.header.hash),
            postcard::to_allocvec(&validated.record).map_err(err)?,
        )
        .map_err(err)?;
    // The shared difficulty/timestamp walker only needs these common fields.
    let meta = super::QuanticBlockMeta {
        height: block.header.block_height,
        prev_hash: block.header.prev_hash.clone(),
        timestamp: block.header.timestamp,
        tree_root: digest_hex(&validated.record.state.root),
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
    let mut state = branch(storage, profile, parent)?;
    let height = state.height().checked_add(1).ok_or("height overflow")?;
    let fees = txs.iter().try_fold(0u64, |v, tx| {
        v.checked_add(tx.statement.fee).ok_or("fee overflow")
    })?;
    let cb = coinbase(
        to,
        subsidy(height).checked_add(fees).ok_or("reward overflow")?,
        height,
    )
    .map_err(err)?;
    rules::apply(&mut state, &cb, &txs).map_err(err)?;
    let record = state.into_record();
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
    block.header.state_root = state_root(&record.state)?;
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
    pub fn insert(&mut self, state: &impl LedgerState, tx: Transaction) -> Result<(), String> {
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
        rules::check_transfer(state, &tx).map_err(err)?;
        self.bytes += size;
        self.transactions.insert(tx.txid(), tx);
        Ok(())
    }
    pub fn revalidate(&mut self, state: &impl LedgerState) {
        self.transactions
            .retain(|_, tx| rules::check_transfer(state, tx).is_ok());
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
                // Leave room for the coinbase (small: it carries no proof).
                if size + n > MAX_BLOCK_BYTES - 64 * 1024 {
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
        let ledger = full_ledger(&db, P, &genesis).unwrap();
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
        let tip = branch(&db, P, &genesis).unwrap();
        pool.insert(&tip, payment.clone()).unwrap();
        assert!(pool.insert(&tip, payment.clone()).is_err());
        let paid = mine(&db, P, &genesis, &bob.address(), pool.select(), 1_700_000_120).unwrap();
        apply_block(&db, CHAIN, &paid).unwrap();
        assert_eq!(db.get_last_block_hash().unwrap(), paid.header.hash);
        pool.revalidate(&branch(&db, P, &paid).unwrap());
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
        let restored = branch(&db, P, &longer).unwrap();
        pool.insert(&restored, payment.clone()).unwrap();
        let paid_again = mine(&db, P, &longer, &bob.address(), pool.select(), 1_700_000_360).unwrap();
        apply_block(&db, CHAIN, &paid_again).unwrap();
        assert!(rules::check_transfer(&branch(&db, P, &paid_again).unwrap(), &payment).is_err());
        let mut wrong = mine(&db, P, &paid_again, &alice.address(), vec![], 1_700_000_480).unwrap();
        wrong.header.state_root.push('0');
        let target = super::super::compact_target_for_child(&db, CHAIN, &paid_again).unwrap();
        super::super::solve_pow(CHAIN, &mut wrong, target).unwrap();
        assert!(apply_block(&db, CHAIN, &wrong).is_err());
        drop(db);
        let db = RocksDbStorage::new(dir.path()).unwrap();
        assert_eq!(db.get_chain_height().unwrap(), 4);
        assert_eq!(
            branch(&db, P, &paid_again).unwrap().state().issued,
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

    // The compact state answers "spent on this branch?" without a full
    // ledger: a nullifier spent on the canonical chain after the fork point
    // is still spendable on the side branch, never twice on the same branch.
    #[test]
    fn side_branches_see_their_own_spends_and_records_stay_small() {
        let dir = tempfile::tempdir().unwrap();
        let db = RocksDbStorage::new(dir.path()).unwrap();
        let alice = Account::from_seed(&[17; 32], 3, 0).unwrap();
        let bob = Account::from_seed(&[18; 32], 3, 0).unwrap();
        let genesis = mine(&db, P, &P.genesis_parent(), &alice.address(), vec![], 1_700_000_000).unwrap();
        apply_block(&db, CHAIN, &genesis).unwrap();
        let ledger = full_ledger(&db, P, &genesis).unwrap();
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
        let canonical = mine(&db, P, &genesis, &bob.address(), vec![payment.clone()], 1_700_000_120).unwrap();
        apply_block(&db, CHAIN, &canonical).unwrap();
        assert!(mine(&db, P, &canonical, &bob.address(), vec![payment.clone()], 1_700_000_240).is_err());

        // Side branch: an empty block, then the same payment, then a reorg.
        let side = mine(&db, P, &genesis, &alice.address(), vec![], 1_700_000_121).unwrap();
        assert!(apply_block(&db, CHAIN, &side).is_err(), "tied: stored, not canonical");
        let side_paid = mine(&db, P, &side, &bob.address(), vec![payment.clone()], 1_700_000_241).unwrap();
        apply_block(&db, CHAIN, &side_paid).unwrap();
        assert_eq!(db.get_last_block_hash().unwrap(), side_paid.header.hash);
        assert!(mine(&db, P, &side_paid, &bob.address(), vec![payment.clone()], 1_700_000_361).is_err());
        // The old canonical block is now a side branch that spent it too.
        assert!(mine(&db, P, &canonical, &bob.address(), vec![payment.clone()], 1_700_000_362).is_err());
        let state = branch(&db, P, &side_paid).unwrap();
        assert_eq!(state.state().height, 3);
        assert_eq!(state.state().issued, PREMINE + 2 * 12 * ATOMS);
        let full = full_ledger(&db, P, &side_paid).unwrap();
        assert_eq!(full.tree.root(), state.state().root);
        assert_eq!(full.tree.len(), state.state().notes);

        // A record is a few hundred bytes, not a copy of the ledger.
        let record = db
            .get_raw(&format!("{RECORD_KEY}{}", side_paid.header.hash))
            .unwrap()
            .unwrap();
        assert!(record.len() < 4096, "{} bytes", record.len());
        let empty_block = serde_json::to_vec(&side).unwrap().len();
        assert!(empty_block < 64 * 1024, "{empty_block} bytes");
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
