//! Reference consensus of Quantic: the rules every block must follow and the
//! state a node keeps to check them.
//!
//! - **Emission:** premine at height 0, then 12 ZKA per block for 1,051,200
//!   blocks and 6 ZKA for 897,600 more: 23,000,000 ZKA in total.
//! - **Coinbase:** first transaction of each block, spends disabled, anchor of
//!   the empty tree, mints exactly subsidy + fees. Its notes enter the
//!   commitment tree [`COINBASE_MATURITY`] blocks later, so they cannot be
//!   spent before maturing (no anchor contains them). The premine enters at
//!   once and must be a public output that matches the announcement.
//! - **Transfers:** spends and outputs enabled, value balance ≥ 0 (it is the
//!   fee; nothing else can leave the pool), anchor = the tree root at the end
//!   of one of the [`ANCHOR_WINDOW`] previous blocks, fresh nullifiers,
//!   unexpired.
//! - **Genesis height:** the premine block. The node numbers its blocks from
//!   1, so its Quantic genesis is height 1; heights are otherwise absolute.
//! - **Proofs and signatures** of the whole block are checked in one batch.
//!
//! Reorganizations are the node's business (it rebuilds this state); this
//! module only moves forward.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::fmt;

use incrementalmerkletree::frontier::CommitmentTree;
use orchard::bundle::Flags;
use orchard::circuit::VerifyingKey;
use orchard::note::ExtractedNoteCommitment;
use orchard::tree::MerkleHashOrchard;
use orchard::value::NoteValue;
use orchard::{Anchor, NOTE_COMMITMENT_TREE_DEPTH};
use rand_core::{CryptoRng, Rng};

use crate::address::QuanticAddress;
use crate::hash::{blake2b_256, MERKLE_NODE};
use crate::network::Network;
use crate::note_encryption::verify_public_output;
use crate::tx::{QuanticTransaction, TxId, TxKind, MAX_ACTIONS_PER_TX};
use crate::verify::verify_transactions;

pub const ATOMS_PER_ZKA: u64 = 1_000_000;
pub const MAX_SUPPLY_ATOMS: u64 = 23_000_000 * ATOMS_PER_ZKA;
pub const PREMINE_ATOMS: u64 = 5_000_000 * ATOMS_PER_ZKA;
pub const FIRST_ERA_BLOCKS: u32 = 1_051_200;
pub const FIRST_ERA_SUBSIDY: u64 = 12 * ATOMS_PER_ZKA;
pub const SECOND_ERA_BLOCKS: u32 = 897_600;
pub const SECOND_ERA_SUBSIDY: u64 = 6 * ATOMS_PER_ZKA;
/// Same as Zenith.
pub const COINBASE_MATURITY: u32 = 60;
/// Actions of all transfers of a block (the coinbase is not counted).
pub const MAX_BLOCK_ACTIONS: usize = 512;
/// Conventional fee: 0.0001 ZKA per action, with a minimum of 2 actions
/// (ZIP 317 style). Every wallet pays exactly this, so the fee says nothing
/// about who built the transaction.
pub const MARGINAL_FEE: u64 = ATOMS_PER_ZKA / 10_000;
pub const GRACE_ACTIONS: usize = 2;
/// A transfer anchors in the final tree root of one of this many previous
/// blocks (about 3.3 hours at 120 s). Zcash accepts any past root; a window
/// keeps reorganizations simple and costs wallets nothing (they use the last).
pub const ANCHOR_WINDOW: usize = 100;

/// Depth of the note commitment tree, as the `u8` that `incrementalmerkletree` wants.
pub const TREE_DEPTH: u8 = 32;
const _: () = assert!(TREE_DEPTH as usize == NOTE_COMMITMENT_TREE_DEPTH);

pub type NoteCommitmentTree = CommitmentTree<MerkleHashOrchard, TREE_DEPTH>;

/// Subsidy of the block `blocks_after_genesis` blocks after the genesis
/// (the genesis itself carries the premine instead).
pub fn block_subsidy(blocks_after_genesis: u32) -> u64 {
    let height = blocks_after_genesis;
    if height == 0 {
        0
    } else if height <= FIRST_ERA_BLOCKS {
        FIRST_ERA_SUBSIDY
    } else if height <= FIRST_ERA_BLOCKS + SECOND_ERA_BLOCKS {
        SECOND_ERA_SUBSIDY
    } else {
        0
    }
}

pub fn conventional_fee(actions: usize) -> u64 {
    MARGINAL_FEE * actions.max(GRACE_ACTIONS) as u64
}

/// The premine, announced in the chain parameters: anyone can check that the
/// genesis coinbase paid exactly this, to exactly this address.
#[derive(Clone, Debug)]
pub struct Premine {
    pub recipient: QuanticAddress,
    pub value: u64,
}

#[derive(Clone, Debug)]
pub struct ConsensusParams {
    pub network: Network,
    /// Height of the genesis block (the premine).
    pub genesis_height: u32,
    pub coinbase_maturity: u32,
    pub anchor_window: usize,
    pub premine: Option<Premine>,
}

impl ConsensusParams {
    pub fn new(network: Network, premine: Option<Premine>) -> Self {
        Self {
            network,
            genesis_height: 0,
            coinbase_maturity: COINBASE_MATURITY,
            anchor_window: ANCHOR_WINDOW,
            premine,
        }
    }

    /// What the coinbase of block `height` mints, besides the fees: the
    /// premine at the genesis, the subsidy afterwards.
    pub fn minted_at(&self, height: u32) -> u64 {
        if height == self.genesis_height {
            self.premine.as_ref().map_or(0, |p| p.value)
        } else {
            block_subsidy(height.saturating_sub(self.genesis_height))
        }
    }

    pub fn schedule(&self) -> CommitmentSchedule {
        CommitmentSchedule::new(self.genesis_height, self.coinbase_maturity)
    }
}

/// When the notes of each coinbase enter the tree. Nodes and wallets must
/// follow the same schedule, so it lives in one place.
#[derive(Clone, Debug, Default)]
pub struct CommitmentSchedule {
    genesis_height: u32,
    maturity: u32,
    pending: BTreeMap<u32, Vec<ExtractedNoteCommitment>>,
}

impl CommitmentSchedule {
    pub fn new(genesis_height: u32, maturity: u32) -> Self {
        Self {
            genesis_height,
            maturity,
            pending: BTreeMap::new(),
        }
    }

    pub fn insertion_height(&self, coinbase_height: u32) -> u32 {
        if coinbase_height == self.genesis_height {
            coinbase_height
        } else {
            coinbase_height + self.maturity
        }
    }

    /// The commitments block `height` appends, in order: coinbases that
    /// mature now, then the outputs of its transfers. Queues its own coinbase.
    pub fn commitments_for_block(
        &mut self,
        height: u32,
        coinbase: impl IntoIterator<Item = ExtractedNoteCommitment>,
        transfers: impl IntoIterator<Item = ExtractedNoteCommitment>,
    ) -> Vec<ExtractedNoteCommitment> {
        let mut out = self.pending.remove(&height).unwrap_or_default();
        let at = self.insertion_height(height);
        if at == height {
            out.extend(coinbase);
        } else {
            self.pending.entry(at).or_default().extend(coinbase);
        }
        out.extend(transfers);
        out
    }
}

#[derive(Clone, Debug)]
pub struct QuanticBlock {
    pub height: u32,
    pub coinbase: QuanticTransaction,
    pub transactions: Vec<QuanticTransaction>,
}

impl QuanticBlock {
    pub fn all_transactions(&self) -> impl Iterator<Item = &QuanticTransaction> {
        std::iter::once(&self.coinbase).chain(&self.transactions)
    }

    pub fn txids(&self) -> Vec<TxId> {
        self.all_transactions()
            .map(QuanticTransaction::txid)
            .collect()
    }

    /// Goes in the header: commits to every txid, in order.
    pub fn txid_root(&self) -> [u8; 32] {
        merkle_root(&self.txids().iter().map(|id| id.0).collect::<Vec<_>>())
    }

    /// Goes in the header too: commits to proofs and signatures.
    pub fn auth_root(&self) -> [u8; 32] {
        merkle_root(
            &self
                .all_transactions()
                .map(QuanticTransaction::auth_digest)
                .collect::<Vec<_>>(),
        )
    }
}

/// Binary tree of 32-byte leaves; an odd node moves up unchanged and the
/// number of leaves is hashed into the root, so two different lists never
/// share a root (no Bitcoin-style duplicate-leaf ambiguity).
pub fn merkle_root(leaves: &[[u8; 32]]) -> [u8; 32] {
    let count = (leaves.len() as u64).to_le_bytes();
    if leaves.is_empty() {
        return blake2b_256(MERKLE_NODE, &[&count]);
    }
    let mut level = leaves.to_vec();
    while level.len() > 1 {
        level = level
            .chunks(2)
            .map(|pair| match pair {
                [left, right] => blake2b_256(MERKLE_NODE, &[left, right]),
                [single] => *single,
                _ => unreachable!("chunks of two"),
            })
            .collect();
    }
    blake2b_256(MERKLE_NODE, &[&count, &level[0]])
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConsensusError {
    WrongHeight { expected: u32, found: u32 },
    WrongNetwork,
    NotCoinbase,
    CoinbaseHeight,
    CoinbaseFlags,
    CoinbaseAnchor,
    CoinbaseValue { expected: u64, found: i64 },
    PremineMismatch,
    NotTransfer,
    TransferFlags,
    NegativeValueBalance,
    Expired { expiry: u32, height: u32 },
    UnknownAnchor,
    DuplicateNullifier,
    SpentNullifier,
    TooManyActions,
    ValueOverflow,
    SupplyExceeded,
    InvalidProofOrSignature,
}

impl fmt::Display for ConsensusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConsensusError::WrongHeight { expected, found } => {
                write!(f, "block height {found}, expected {expected}")
            }
            ConsensusError::WrongNetwork => f.write_str("transaction of another network"),
            ConsensusError::NotCoinbase => {
                f.write_str("the first transaction must be the coinbase")
            }
            ConsensusError::CoinbaseHeight => {
                f.write_str("coinbase height does not match the block")
            }
            ConsensusError::CoinbaseFlags => f.write_str("coinbase must disable spends"),
            ConsensusError::CoinbaseAnchor => f.write_str("coinbase anchor must be the empty tree"),
            ConsensusError::CoinbaseValue { expected, found } => {
                write!(
                    f,
                    "coinbase mints {}, expected {expected}",
                    found.unsigned_abs()
                )
            }
            ConsensusError::PremineMismatch => {
                f.write_str("genesis does not pay the announced premine")
            }
            ConsensusError::NotTransfer => {
                f.write_str("only the first transaction may be a coinbase")
            }
            ConsensusError::TransferFlags => {
                f.write_str("transfers must enable spends and outputs")
            }
            ConsensusError::NegativeValueBalance => f.write_str("a transfer cannot mint"),
            ConsensusError::Expired { expiry, height } => {
                write!(f, "transaction expired at {expiry}, block {height}")
            }
            ConsensusError::UnknownAnchor => f.write_str("anchor is not a past tree root"),
            ConsensusError::DuplicateNullifier => f.write_str("nullifier repeated in the block"),
            ConsensusError::SpentNullifier => f.write_str("note already spent"),
            ConsensusError::TooManyActions => f.write_str("too many actions"),
            ConsensusError::ValueOverflow => f.write_str("value overflow"),
            ConsensusError::SupplyExceeded => f.write_str("block exceeds the maximum supply"),
            ConsensusError::InvalidProofOrSignature => f.write_str("invalid proof or signature"),
        }
    }
}

impl std::error::Error for ConsensusError {}

/// What a valid block changed.
#[derive(Clone, Debug)]
pub struct BlockEffects {
    pub height: u32,
    /// Tree root after the block: goes in the header and becomes a valid anchor.
    pub final_root: Anchor,
    pub fees: u64,
    pub minted: u64,
}

#[derive(Clone, Debug)]
pub struct ChainState {
    params: ConsensusParams,
    next_height: u32,
    tree: NoteCommitmentTree,
    schedule: CommitmentSchedule,
    /// Final roots of the last `anchor_window` blocks, oldest first.
    recent_roots: VecDeque<[u8; 32]>,
    nullifiers: HashSet<[u8; 32]>,
    issued: u64,
}

impl ChainState {
    pub fn new(params: ConsensusParams) -> Self {
        let schedule = params.schedule();
        Self {
            next_height: params.genesis_height,
            params,
            tree: CommitmentTree::empty(),
            schedule,
            recent_roots: VecDeque::new(),
            nullifiers: HashSet::new(),
            issued: 0,
        }
    }

    pub fn params(&self) -> &ConsensusParams {
        &self.params
    }

    pub fn next_height(&self) -> u32 {
        self.next_height
    }

    pub fn root(&self) -> Anchor {
        Anchor::from(self.tree.root())
    }

    /// Whether a transfer of the next block may anchor in `anchor`.
    pub fn is_valid_anchor(&self, anchor: &Anchor) -> bool {
        self.recent_roots.contains(&anchor.to_bytes())
    }

    pub fn is_spent(&self, nf: &orchard::note::Nullifier) -> bool {
        self.nullifiers.contains(&nf.to_bytes())
    }

    /// Premine plus subsidies so far.
    pub fn issued(&self) -> u64 {
        self.issued
    }

    /// Rules of a transfer that do not need its proof: the mempool runs this
    /// against the next block height.
    pub fn check_transfer(
        &self,
        tx: &QuanticTransaction,
        height: u32,
    ) -> Result<(), ConsensusError> {
        if tx.network() != self.params.network {
            return Err(ConsensusError::WrongNetwork);
        }
        if tx.kind() != TxKind::Transfer {
            return Err(ConsensusError::NotTransfer);
        }
        if *tx.flags() != Flags::ENABLED {
            return Err(ConsensusError::TransferFlags);
        }
        if tx.value_balance() < 0 {
            return Err(ConsensusError::NegativeValueBalance);
        }
        if tx.height() != 0 && height > tx.height() {
            return Err(ConsensusError::Expired {
                expiry: tx.height(),
                height,
            });
        }
        if tx.actions().len() > MAX_ACTIONS_PER_TX {
            return Err(ConsensusError::TooManyActions);
        }
        if !self.is_valid_anchor(tx.anchor()) {
            return Err(ConsensusError::UnknownAnchor);
        }
        let mut seen = HashSet::new();
        for nf in tx.nullifiers() {
            if !seen.insert(nf.to_bytes()) {
                return Err(ConsensusError::DuplicateNullifier);
            }
            if self.is_spent(nf) {
                return Err(ConsensusError::SpentNullifier);
            }
        }
        Ok(())
    }

    fn check_coinbase(&self, block: &QuanticBlock, fees: u64) -> Result<u64, ConsensusError> {
        let cb = &block.coinbase;
        if cb.network() != self.params.network {
            return Err(ConsensusError::WrongNetwork);
        }
        if cb.kind() != TxKind::Coinbase {
            return Err(ConsensusError::NotCoinbase);
        }
        if cb.height() != block.height {
            return Err(ConsensusError::CoinbaseHeight);
        }
        if *cb.flags() != Flags::SPENDS_DISABLED {
            return Err(ConsensusError::CoinbaseFlags);
        }
        if cb.anchor().to_bytes() != Anchor::empty_tree().to_bytes() {
            return Err(ConsensusError::CoinbaseAnchor);
        }
        let minted = self.params.minted_at(block.height);
        let expected = minted
            .checked_add(fees)
            .ok_or(ConsensusError::ValueOverflow)?;
        if cb
            .value_balance()
            .checked_neg()
            .and_then(|v| u64::try_from(v).ok())
            != Some(expected)
        {
            return Err(ConsensusError::CoinbaseValue {
                expected,
                found: cb.value_balance(),
            });
        }
        if block.height == self.params.genesis_height {
            if let Some(premine) = &self.params.premine {
                let [action] = cb.actions() else {
                    return Err(ConsensusError::PremineMismatch);
                };
                if !verify_public_output(
                    &premine.recipient,
                    NoteValue::from_raw(premine.value),
                    &action.nf,
                    &action.cmx,
                    &action.cv_net,
                    &action.envelope,
                ) {
                    return Err(ConsensusError::PremineMismatch);
                }
            }
        }
        Ok(minted)
    }

    /// Checks `block` entirely and, only if it is valid, applies it.
    pub fn apply_block<R: Rng + CryptoRng>(
        &mut self,
        block: &QuanticBlock,
        vk: &VerifyingKey,
        rng: R,
    ) -> Result<BlockEffects, ConsensusError> {
        if block.height != self.next_height {
            return Err(ConsensusError::WrongHeight {
                expected: self.next_height,
                found: block.height,
            });
        }
        let mut block_nullifiers = HashSet::new();
        let mut fees = 0u64;
        let mut actions = 0usize;
        for tx in &block.transactions {
            self.check_transfer(tx, block.height)?;
            for nf in tx.nullifiers() {
                if !block_nullifiers.insert(nf.to_bytes()) {
                    return Err(ConsensusError::DuplicateNullifier);
                }
            }
            fees = fees
                .checked_add(tx.fee().ok_or(ConsensusError::NegativeValueBalance)?)
                .ok_or(ConsensusError::ValueOverflow)?;
            actions += tx.actions().len();
        }
        if actions > MAX_BLOCK_ACTIONS || block.coinbase.actions().len() > MAX_ACTIONS_PER_TX {
            return Err(ConsensusError::TooManyActions);
        }
        let minted = self.check_coinbase(block, fees)?;
        // Coinbase nullifiers are random, but must be unique like any other.
        for nf in block.coinbase.nullifiers() {
            if self.is_spent(nf) || !block_nullifiers.insert(nf.to_bytes()) {
                return Err(ConsensusError::DuplicateNullifier);
            }
        }
        let issued = self
            .issued
            .checked_add(minted)
            .ok_or(ConsensusError::ValueOverflow)?;
        if issued > MAX_SUPPLY_ATOMS {
            return Err(ConsensusError::SupplyExceeded);
        }
        if !verify_transactions(vk, block.all_transactions(), rng) {
            return Err(ConsensusError::InvalidProofOrSignature);
        }

        // Valid: apply.
        let commitments = self.schedule.commitments_for_block(
            block.height,
            block.coinbase.note_commitments().copied(),
            block
                .transactions
                .iter()
                .flat_map(|tx| tx.note_commitments().copied()),
        );
        for cmx in &commitments {
            self.tree
                .append(MerkleHashOrchard::from_cmx(cmx))
                .expect("the tree has room for 2^32 notes");
        }
        self.nullifiers.extend(block_nullifiers);
        let final_root = self.root();
        self.recent_roots.push_back(final_root.to_bytes());
        if self.recent_roots.len() > self.params.anchor_window {
            self.recent_roots.pop_front();
        }
        self.issued = issued;
        self.next_height += 1;
        Ok(BlockEffects {
            height: block.height,
            final_root,
            fees,
            minted,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_emission_adds_up_to_exactly_23_million() {
        let mined = FIRST_ERA_SUBSIDY * u64::from(FIRST_ERA_BLOCKS)
            + SECOND_ERA_SUBSIDY * u64::from(SECOND_ERA_BLOCKS);
        assert_eq!(mined, 18_000_000 * ATOMS_PER_ZKA);
        assert_eq!(PREMINE_ATOMS + mined, MAX_SUPPLY_ATOMS);
        assert_eq!(block_subsidy(0), 0);
        assert_eq!(block_subsidy(1), 12 * ATOMS_PER_ZKA);
        assert_eq!(block_subsidy(FIRST_ERA_BLOCKS), 12 * ATOMS_PER_ZKA);
        assert_eq!(block_subsidy(FIRST_ERA_BLOCKS + 1), 6 * ATOMS_PER_ZKA);
        assert_eq!(
            block_subsidy(FIRST_ERA_BLOCKS + SECOND_ERA_BLOCKS),
            6 * ATOMS_PER_ZKA
        );
        assert_eq!(block_subsidy(FIRST_ERA_BLOCKS + SECOND_ERA_BLOCKS + 1), 0);
        // At 120 s per block, mining ends after about 7.4 years.
        let years = f64::from(FIRST_ERA_BLOCKS + SECOND_ERA_BLOCKS) * 120.0 / (365.25 * 86_400.0);
        assert!((7.3..7.5).contains(&years), "{years}");
    }

    #[test]
    fn the_genesis_can_be_at_any_height() {
        let mut params = ConsensusParams::new(Network::Regtest, None);
        params.genesis_height = 1;
        params.coinbase_maturity = 2;
        assert_eq!(params.minted_at(1), 0, "no premine announced");
        assert_eq!(params.minted_at(2), 12 * ATOMS_PER_ZKA);
        assert_eq!(params.minted_at(1 + FIRST_ERA_BLOCKS + 1), 6 * ATOMS_PER_ZKA);
        let schedule = params.schedule();
        assert_eq!(schedule.insertion_height(1), 1, "the genesis enters at once");
        assert_eq!(schedule.insertion_height(2), 4);
        let chain = ChainState::new(params);
        assert_eq!(chain.next_height(), 1);
    }

    #[test]
    fn the_conventional_fee_depends_only_on_the_action_count() {
        assert_eq!(conventional_fee(0), 200);
        assert_eq!(conventional_fee(2), 200);
        assert_eq!(conventional_fee(3), 300);
    }

    #[test]
    fn coinbase_notes_wait_for_maturity_and_the_premine_does_not() {
        let mut schedule = CommitmentSchedule::new(0, 2);
        let cmx = |b: u8| -> ExtractedNoteCommitment {
            Option::from(ExtractedNoteCommitment::from_bytes(&[b; 32])).unwrap()
        };
        let order =
            |v: Vec<ExtractedNoteCommitment>| v.iter().map(|c| c.to_bytes()[0]).collect::<Vec<_>>();
        assert_eq!(
            order(schedule.commitments_for_block(0, [cmx(1)], [])),
            vec![1]
        );
        assert_eq!(
            order(schedule.commitments_for_block(1, [cmx(2)], [cmx(3)])),
            vec![3]
        );
        assert_eq!(
            order(schedule.commitments_for_block(2, [cmx(4)], [])),
            Vec::<u8>::new()
        );
        assert_eq!(
            order(schedule.commitments_for_block(3, [cmx(5)], [cmx(6)])),
            vec![2, 6]
        );
        assert_eq!(order(schedule.commitments_for_block(4, [], [])), vec![4]);
    }

    #[test]
    fn the_merkle_root_depends_on_order_and_count() {
        let a = [1u8; 32];
        let b = [2u8; 32];
        let c = [3u8; 32];
        assert_ne!(merkle_root(&[a, b]), merkle_root(&[b, a]));
        // Bitcoin's duplicate-last-leaf ambiguity cannot happen.
        assert_ne!(merkle_root(&[a, b, c]), merkle_root(&[a, b, c, c]));
        assert_ne!(merkle_root(&[a]), merkle_root(&[]));
        assert_eq!(merkle_root(&[a, b, c]), merkle_root(&[a, b, c]));
    }
}
