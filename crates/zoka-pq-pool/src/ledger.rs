//! Deterministic private ledger with membership, nullifier uniqueness, exact
//! emission, coinbase maturity and bounded anchor history. PoW is checked by
//! the node before this state transition; this layer never chooses a fork.
use crate::{
    crypto::{self, Digest},
    model::{check_network, MerklePath, DEPTH},
    transaction::Transaction,
    Error,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
pub const ATOMS: u64 = 1_000_000;
pub const PREMINE: u64 = 5_000_000 * ATOMS;
pub const MAX_SUPPLY: u64 = 23_000_000 * ATOMS;
pub const ANCHOR_WINDOW: usize = 100;
pub const MAX_BLOCK_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_BLOCK_TRANSACTIONS: usize = 16;
/// One short of a full depth-32 tree, so a frontier always has its root.
pub const MAX_NOTES: u64 = (1 << DEPTH) - 1;
pub fn subsidy(height: u32) -> u64 {
    match height {
        1 => PREMINE,
        2..=1_051_201 => 12 * ATOMS,
        1_051_202..=1_948_801 => 6 * ATOMS,
        _ => 0,
    }
}
pub fn conventional_fee(inputs: usize, outputs: usize) -> u64 {
    100 * (inputs.max(outputs).max(2) as u64)
}

#[derive(Clone, Serialize, Deserialize)]
pub struct NoteTree {
    network: u8,
    count: u64,
    levels: Vec<BTreeMap<u32, Digest>>,
    empty: Vec<Digest>,
}
impl NoteTree {
    pub fn new(network: u8) -> Result<Self, Error> {
        check_network(network)?;
        let mut empty = vec![Digest::ZERO];
        for depth in 0..DEPTH {
            empty.push(crypto::parent(network, depth, empty[depth], empty[depth])?);
        }
        Ok(Self {
            network,
            count: 0,
            levels: vec![BTreeMap::new(); DEPTH + 1],
            empty,
        })
    }
    pub fn len(&self) -> u64 {
        self.count
    }
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
    pub fn root(&self) -> Digest {
        self.levels[DEPTH]
            .get(&0)
            .copied()
            .unwrap_or(self.empty[DEPTH])
    }
    pub fn append(&mut self, note: Digest) -> Result<u32, Error> {
        let _ = note.fields()?;
        if self.count >= MAX_NOTES {
            return Err(Error::Ledger("note tree full"));
        }
        let index = self.count as u32;
        self.count += 1;
        let mut position = index;
        self.levels[0].insert(position, note);
        for level in 0..DEPTH {
            let left = self.levels[level]
                .get(&(position & !1))
                .copied()
                .unwrap_or(self.empty[level]);
            let right = self.levels[level]
                .get(&(position | 1))
                .copied()
                .unwrap_or(self.empty[level]);
            position >>= 1;
            self.levels[level + 1]
                .insert(position, crypto::parent(self.network, level, left, right)?);
        }
        Ok(index)
    }
    pub fn path(&self, index: u32) -> Result<MerklePath, Error> {
        if u64::from(index) >= self.count {
            return Err(Error::Ledger("unknown note"));
        }
        Ok(MerklePath {
            index,
            siblings: std::array::from_fn(|depth| {
                self.levels[depth]
                    .get(&((index >> depth) ^ 1))
                    .copied()
                    .unwrap_or(self.empty[depth])
            }),
        })
    }
    pub fn commitment(&self, index: u32) -> Option<Digest> {
        self.levels[0].get(&index).copied()
    }
}

/// Coinbase rewards join the note tree this many blocks after their block.
pub fn maturity(network: u8) -> u32 {
    if network == 3 {
        2
    } else {
        60
    }
}

/// An incremental note tree: the left nodes that still wait for a right
/// sibling, one per set bit of `count`. Enough to append and compute the root
/// of the full tree, so a node keeps this per block instead of every note.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frontier {
    count: u64,
    ommers: Vec<Option<Digest>>,
}
impl Default for Frontier {
    fn default() -> Self {
        Self {
            count: 0,
            ommers: vec![None; DEPTH],
        }
    }
}
impl Frontier {
    pub fn len(&self) -> u64 {
        self.count
    }
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
    pub fn append(&mut self, network: u8, leaf: Digest) -> Result<(), Error> {
        let _ = leaf.fields()?;
        if self.count >= MAX_NOTES || self.ommers.len() != DEPTH {
            return Err(Error::Ledger("note tree full"));
        }
        let mut node = leaf;
        for level in 0..DEPTH {
            if (self.count >> level) & 1 == 0 {
                self.ommers[level] = Some(node);
                break;
            }
            let left = self.ommers[level]
                .take()
                .ok_or(Error::Ledger("corrupt frontier"))?;
            node = crypto::parent(network, level, left, node)?;
        }
        self.count += 1;
        Ok(())
    }
    /// The same root as a [`NoteTree`] holding the same notes.
    pub fn root(&self, network: u8) -> Result<Digest, Error> {
        let mut empty = Digest::ZERO;
        let mut acc: Option<Digest> = None;
        for (level, ommer) in self.ommers.iter().enumerate() {
            acc = match (ommer, acc) {
                (Some(left), right) => Some(crypto::parent(
                    network,
                    level,
                    *left,
                    right.unwrap_or(empty),
                )?),
                (None, Some(left)) => Some(crypto::parent(network, level, left, empty)?),
                (None, None) => None,
            };
            empty = crypto::parent(network, level, empty, empty)?;
        }
        Ok(acc.unwrap_or(empty))
    }
}

/// What the ledger rules read and change. [`Ledger`] keeps everything in
/// memory (wallets need every note for their paths); a node keeps a compact
/// state per block and looks the rest up. Both run the same [`apply`].
pub trait LedgerState {
    fn network(&self) -> u8;
    fn height(&self) -> u32;
    fn issued(&self) -> u64;
    fn notes(&self) -> u64;
    fn contains_anchor(&self, root: &Digest) -> bool;
    fn is_spent(&self, nullifier: &Digest) -> Result<bool, Error>;
    /// How many coinbase notes join the tree at `height`.
    fn matured_count(&self, height: u32) -> usize;
    fn take_matured(&mut self, height: u32) -> Vec<Digest>;
    /// Called only once every rule of the block holds.
    fn append(&mut self, note: Digest) -> Result<(), Error>;
    fn defer(&mut self, matures_at: u32, notes: Vec<Digest>);
    fn spend(&mut self, nullifiers: BTreeSet<Digest>);
    /// Closes the block; its final root becomes an anchor.
    fn close(&mut self, height: u32, issued: u64) -> Result<(), Error>;
}

pub fn check_transfer<S: LedgerState + ?Sized>(state: &S, tx: &Transaction) -> Result<(), Error> {
    let s = &tx.statement;
    if s.minted != 0 || s.nullifiers.is_empty() {
        return Err(Error::Ledger("expected a transfer"));
    }
    if !state.contains_anchor(&s.anchor) {
        return Err(Error::Ledger("unknown or expired anchor"));
    }
    for nf in &s.nullifiers {
        if state.is_spent(nf)? {
            return Err(Error::Ledger("spent nullifier"));
        }
    }
    if s.fee < conventional_fee(s.nullifiers.len(), s.commitments.len()) {
        return Err(Error::Ledger("fee"));
    }
    tx.verify(
        state.network(),
        state
            .height()
            .checked_add(1)
            .ok_or(Error::Ledger("height overflow"))?,
    )
}

/// Validation precedes all mutations. Invalid blocks leave state unchanged.
pub fn apply<S: LedgerState + ?Sized>(
    state: &mut S,
    coinbase: &Transaction,
    transfers: &[Transaction],
) -> Result<(), Error> {
    let height = state
        .height()
        .checked_add(1)
        .ok_or(Error::Ledger("height overflow"))?;
    if transfers.len() > MAX_BLOCK_TRANSACTIONS {
        return Err(Error::Ledger("too many transactions"));
    }
    let bytes = transfers
        .iter()
        .chain(std::iter::once(coinbase))
        .try_fold(0usize, |sum, tx| {
            Ok::<_, Error>(sum.saturating_add(tx.encode()?.len()))
        })?;
    if bytes > MAX_BLOCK_BYTES {
        return Err(Error::Ledger("block byte limit"));
    }
    let mut nullifiers = BTreeSet::new();
    let mut fees = 0u64;
    for tx in transfers {
        check_transfer(state, tx)?;
        fees = fees
            .checked_add(tx.statement.fee)
            .ok_or(Error::Ledger("fee overflow"))?;
        for nf in &tx.statement.nullifiers {
            if !nullifiers.insert(*nf) {
                return Err(Error::DuplicateNullifier);
            }
        }
    }
    let reward = subsidy(height)
        .checked_add(fees)
        .ok_or(Error::Ledger("reward overflow"))?;
    if coinbase.statement.minted != reward || !coinbase.statement.nullifiers.is_empty() {
        return Err(Error::Ledger("coinbase reward"));
    }
    coinbase.verify(state.network(), height)?;
    if height == 1 && !transfers.is_empty() {
        return Err(Error::Ledger("genesis transfers"));
    }
    let issued = state
        .issued()
        .checked_add(subsidy(height))
        .filter(|v| *v <= MAX_SUPPLY)
        .ok_or(Error::Ledger("supply"))?;
    height
        .checked_add(maturity(state.network()))
        .ok_or(Error::Ledger("maturity overflow"))?;
    let inserts = state.matured_count(height)
        + transfers
            .iter()
            .map(|t| t.statement.commitments.len())
            .sum::<usize>()
        + if height == 1 {
            coinbase.statement.commitments.len()
        } else {
            0
        };
    if state.notes() + inserts as u64 > MAX_NOTES {
        return Err(Error::Ledger("note tree full"));
    }
    commit(
        state,
        height,
        issued,
        &coinbase.statement.commitments,
        transfers.iter().flat_map(|tx| &tx.statement.commitments),
        nullifiers,
    )
}

/// Re-applies an already validated block (a node's own canonical chain)
/// without verifying its proofs again: how a wallet rebuilds the full tree.
pub fn replay<S: LedgerState + ?Sized>(
    state: &mut S,
    coinbase: &Transaction,
    transfers: &[Transaction],
) -> Result<(), Error> {
    let height = state
        .height()
        .checked_add(1)
        .ok_or(Error::Ledger("height overflow"))?;
    let issued = state
        .issued()
        .checked_add(subsidy(height))
        .ok_or(Error::Ledger("supply"))?;
    let nullifiers = transfers
        .iter()
        .flat_map(|tx| tx.statement.nullifiers.iter().copied())
        .collect();
    commit(
        state,
        height,
        issued,
        &coinbase.statement.commitments,
        transfers.iter().flat_map(|tx| &tx.statement.commitments),
        nullifiers,
    )
}

/// [`replay`] from a block's effects alone, without its proofs: how a light
/// wallet that reads pages of outputs (see [`crate::scan::ScanPage2`])
/// rebuilds the very tree a node has, to spend from it.
pub fn replay_effects<'a, S: LedgerState + ?Sized>(
    state: &mut S,
    coinbase: &[Digest],
    transfers: impl IntoIterator<Item = (&'a [Digest], &'a [Digest])>,
) -> Result<(), Error> {
    let height = state
        .height()
        .checked_add(1)
        .ok_or(Error::Ledger("height overflow"))?;
    let issued = state
        .issued()
        .checked_add(subsidy(height))
        .ok_or(Error::Ledger("supply"))?;
    let mut outputs = Vec::new();
    let mut nullifiers = BTreeSet::new();
    for (commitments, spent) in transfers {
        outputs.extend_from_slice(commitments);
        nullifiers.extend(spent.iter().copied());
    }
    commit(state, height, issued, coinbase, outputs.iter(), nullifiers)
}

// Every digest was checked by validation. Matured digests came from earlier
// verified coinbases; capacity was checked, so these appends cannot fail.
fn commit<'a, S: LedgerState + ?Sized>(
    state: &mut S,
    height: u32,
    issued: u64,
    coinbase: &[Digest],
    outputs: impl Iterator<Item = &'a Digest>,
    nullifiers: BTreeSet<Digest>,
) -> Result<(), Error> {
    for note in state.take_matured(height) {
        state.append(note)?;
    }
    if height == 1 {
        for note in coinbase {
            state.append(*note)?;
        }
    } else {
        let at = height
            .checked_add(maturity(state.network()))
            .ok_or(Error::Ledger("maturity overflow"))?;
        state.defer(at, coinbase.to_vec());
    }
    for note in outputs {
        state.append(*note)?;
    }
    state.spend(nullifiers);
    state.close(height, issued)
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Ledger {
    pub network: u8,
    pub height: u32,
    pub issued: u64,
    pub tree: NoteTree,
    pub spent: BTreeSet<Digest>,
    anchors: VecDeque<Digest>,
    pending: BTreeMap<u32, Vec<Digest>>,
}
impl Ledger {
    pub fn new(network: u8) -> Result<Self, Error> {
        let tree = NoteTree::new(network)?;
        Ok(Self {
            network,
            height: 0,
            issued: 0,
            tree,
            spent: BTreeSet::new(),
            anchors: VecDeque::new(),
            pending: BTreeMap::new(),
        })
    }
    pub fn contains_anchor(&self, root: &Digest) -> bool {
        self.anchors.contains(root)
    }
    pub fn check_transfer(&self, tx: &Transaction) -> Result<(), Error> {
        check_transfer(self, tx)
    }
    pub fn apply(&mut self, coinbase: &Transaction, transfers: &[Transaction]) -> Result<(), Error> {
        apply(self, coinbase, transfers)
    }
    pub fn replay(&mut self, coinbase: &Transaction, transfers: &[Transaction]) -> Result<(), Error> {
        replay(self, coinbase, transfers)
    }
}
impl LedgerState for Ledger {
    fn network(&self) -> u8 {
        self.network
    }
    fn height(&self) -> u32 {
        self.height
    }
    fn issued(&self) -> u64 {
        self.issued
    }
    fn notes(&self) -> u64 {
        self.tree.len()
    }
    fn contains_anchor(&self, root: &Digest) -> bool {
        self.anchors.contains(root)
    }
    fn is_spent(&self, nullifier: &Digest) -> Result<bool, Error> {
        Ok(self.spent.contains(nullifier))
    }
    fn matured_count(&self, height: u32) -> usize {
        self.pending.get(&height).map_or(0, Vec::len)
    }
    fn take_matured(&mut self, height: u32) -> Vec<Digest> {
        self.pending.remove(&height).unwrap_or_default()
    }
    fn append(&mut self, note: Digest) -> Result<(), Error> {
        self.tree.append(note).map(|_| ())
    }
    fn defer(&mut self, matures_at: u32, notes: Vec<Digest>) {
        self.pending.insert(matures_at, notes);
    }
    fn spend(&mut self, nullifiers: BTreeSet<Digest>) {
        self.spent.extend(nullifiers);
    }
    fn close(&mut self, height: u32, issued: u64) -> Result<(), Error> {
        self.issued = issued;
        self.height = height;
        self.anchors.push_back(self.tree.root());
        while self.anchors.len() > ANCHOR_WINDOW {
            self.anchors.pop_front();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn merkle_paths_follow_append_and_detect_another_position() {
        let mut tree = NoteTree::new(3).unwrap();
        for i in 0..17 {
            tree.append(Digest([i + 1; 16])).unwrap();
        }
        for i in 0..17 {
            let leaf = tree.commitment(i).unwrap();
            let path = tree.path(i).unwrap();
            assert_eq!(path.root(3, leaf).unwrap(), tree.root());
        }
        let mut path = tree.path(3).unwrap();
        path.index = 4;
        assert_ne!(
            path.root(3, tree.commitment(3).unwrap()).unwrap(),
            tree.root()
        );
    }
    #[test]
    fn a_frontier_has_the_root_of_the_full_tree() {
        let mut tree = NoteTree::new(3).unwrap();
        let mut frontier = Frontier::default();
        assert_eq!(frontier.root(3).unwrap(), tree.root());
        for i in 0..70u32 {
            let leaf = Digest([i + 7; 16]);
            tree.append(leaf).unwrap();
            frontier.append(3, leaf).unwrap();
            assert_eq!(frontier.root(3).unwrap(), tree.root(), "{} notes", i + 1);
        }
        assert_eq!(frontier.len(), 70);
        assert_ne!(
            Frontier::default().root(2).unwrap(),
            Frontier::default().root(3).unwrap()
        );
    }
    #[test]
    fn schedule_stops_at_the_supply_cap() {
        assert_eq!(
            PREMINE + 1_051_200 * 12 * ATOMS + 897_600 * 6 * ATOMS,
            MAX_SUPPLY
        );
        assert_eq!(subsidy(1), PREMINE);
        assert_eq!(subsidy(1_051_201), 12 * ATOMS);
        assert_eq!(subsidy(1_051_202), 6 * ATOMS);
        assert_eq!(subsidy(1_948_801), 6 * ATOMS);
        assert_eq!(subsidy(1_948_802), 0);
    }

    #[test]
    fn encrypted_payment_maturity_replay_and_atomic_rejection() {
        use crate::{
            model::Input,
            transaction::{self, Payment},
            wallet::Account,
        };
        let alice = Account::from_seed(&[1; 32], 3, 0).unwrap();
        let bob = Account::from_seed(&[2; 32], 3, 0).unwrap();
        let mut ledger = Ledger::new(3).unwrap();
        let genesis = transaction::coinbase(&alice.address(), PREMINE, 1).unwrap();
        ledger.apply(&genesis, &[]).unwrap();
        let cm = genesis.statement.commitments[0];
        let (note, _) = alice.receive(cm, &genesis.envelopes[0]).unwrap();
        let fee = conventional_fee(1, 2);
        let payment = transaction::transfer(
            &alice,
            ledger.tree.root(),
            vec![Input {
                key: alice.spend.clone(),
                note,
                path: ledger.tree.path(0).unwrap(),
            }],
            vec![
                Payment::new(bob.address(), 42 * ATOMS),
                Payment::new(alice.address(), PREMINE - 42 * ATOMS - fee),
            ],
            fee,
            4,
        )
        .unwrap();
        let payment = Transaction::decode(&payment.encode().unwrap()).unwrap();
        let mut extra = payment.encode().unwrap();
        extra.push(0);
        assert!(Transaction::decode(&extra).is_err());
        let mut overlong = payment.encode().unwrap();
        assert_eq!(overlong[0], 2);
        overlong[0] = 0x82;
        overlong.insert(1, 0);
        assert!(Transaction::decode(&overlong).is_err());
        ledger.check_transfer(&payment).unwrap();
        let reward = transaction::coinbase(&bob.address(), subsidy(2) + fee, 2).unwrap();
        let before = postcard::to_allocvec(&ledger).unwrap();
        assert!(ledger
            .apply(&reward, &[payment.clone(), payment.clone()])
            .is_err());
        assert_eq!(postcard::to_allocvec(&ledger).unwrap(), before);
        let mut inflated = reward.clone();
        inflated.statement.minted += 1;
        assert!(ledger.apply(&inflated, &[payment.clone()]).is_err());
        assert_eq!(postcard::to_allocvec(&ledger).unwrap(), before);
        ledger.apply(&reward, &[payment.clone()]).unwrap();
        assert!(ledger.check_transfer(&payment).is_err());
        assert_eq!(ledger.tree.len(), 3, "coinbase must still be immature");
        let (received, _) = bob
            .receive(payment.statement.commitments[0], &payment.envelopes[0])
            .unwrap();
        assert_eq!(received.value, 42 * ATOMS);
        assert!(alice
            .receive(payment.statement.commitments[0], &payment.envelopes[0])
            .is_err());
        let (sent, _) = alice
            .recover_sent(payment.statement.commitments[0], &payment.envelopes[0])
            .unwrap();
        assert_eq!(sent.value, received.value);
        for height in 3..=4 {
            let cb = transaction::coinbase(&alice.address(), subsidy(height), height).unwrap();
            ledger.apply(&cb, &[]).unwrap();
        }
        assert_eq!(ledger.tree.len(), 4);
        assert_eq!(
            ledger.tree.commitment(3),
            Some(reward.statement.commitments[0])
        );
        assert_eq!(
            ledger.issued,
            PREMINE + 3 * 12 * ATOMS,
            "fees are not issuance"
        );
        let path = ledger.tree.path(3).unwrap();
        assert_eq!(
            path.root(3, reward.statement.commitments[0]).unwrap(),
            ledger.tree.root()
        );
    }
}
