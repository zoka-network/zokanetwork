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
        if self.count >= 1u64 << DEPTH {
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

#[derive(Clone, Serialize, Deserialize)]
pub struct Ledger {
    pub network: u8,
    pub height: u32,
    pub issued: u64,
    pub tree: NoteTree,
    pub spent: BTreeSet<Digest>,
    anchors: VecDeque<Digest>,
    pending: BTreeMap<u32, Vec<Digest>>,
    maturity: u32,
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
            maturity: if network == 3 { 2 } else { 60 },
        })
    }
    pub fn contains_anchor(&self, root: &Digest) -> bool {
        self.anchors.contains(root)
    }
    pub fn check_transfer(&self, tx: &Transaction) -> Result<(), Error> {
        let s = &tx.statement;
        if s.minted != 0 || s.nullifiers.is_empty() {
            return Err(Error::Ledger("expected a transfer"));
        }
        if !self.contains_anchor(&s.anchor) {
            return Err(Error::Ledger("unknown or expired anchor"));
        }
        if s.nullifiers.iter().any(|nf| self.spent.contains(nf)) {
            return Err(Error::Ledger("spent nullifier"));
        }
        if s.fee < conventional_fee(s.nullifiers.len(), s.commitments.len()) {
            return Err(Error::Ledger("fee"));
        }
        tx.verify(
            self.network,
            self.height
                .checked_add(1)
                .ok_or(Error::Ledger("height overflow"))?,
        )
    }

    /// Validation precedes all mutations. Invalid blocks leave state unchanged.
    pub fn apply(
        &mut self,
        coinbase: &Transaction,
        transfers: &[Transaction],
    ) -> Result<(), Error> {
        let height = self
            .height
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
            self.check_transfer(tx)?;
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
        coinbase.verify(self.network, height)?;
        if height == 1 && !transfers.is_empty() {
            return Err(Error::Ledger("genesis transfers"));
        }
        let issued = self
            .issued
            .checked_add(subsidy(height))
            .filter(|v| *v <= MAX_SUPPLY)
            .ok_or(Error::Ledger("supply"))?;
        let maturity_height = height
            .checked_add(self.maturity)
            .ok_or(Error::Ledger("maturity overflow"))?;
        let matured = self.pending.get(&height).map_or(0, Vec::len);
        let inserts = matured
            + transfers
                .iter()
                .map(|t| t.statement.commitments.len())
                .sum::<usize>()
            + if height == 1 {
                coinbase.statement.commitments.len()
            } else {
                0
            };
        if self.tree.len() + inserts as u64 > 1u64 << DEPTH {
            return Err(Error::Ledger("note tree full"));
        }
        // Every digest was checked by proof validation above. Matured digests
        // came from earlier verified coinbases; these appends cannot fail.
        if let Some(notes) = self.pending.remove(&height) {
            for note in notes {
                self.tree.append(note)?;
            }
        }
        if height == 1 {
            for note in &coinbase.statement.commitments {
                self.tree.append(*note)?;
            }
        } else {
            self.pending
                .insert(maturity_height, coinbase.statement.commitments.clone());
        }
        for tx in transfers {
            for note in &tx.statement.commitments {
                self.tree.append(*note)?;
            }
        }
        self.spent.extend(nullifiers);
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
