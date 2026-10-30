//! A wallet that spends without a node of its own: a phone, say.
//!
//! It reads the chain as pages of outputs and nullifiers ([`ScanPage2`]) and
//! replays them with the ledger's own [`replay_effects`], so its note tree is
//! the one every node has. Its anchors and Merkle paths come from that copy:
//! nobody learns which note it spends. The pages carry no proofs, so the
//! wallet trusts whoever serves them about what is canonical, as a view key
//! does; a payment built on a wrong tree is refused by the network, never
//! accepted.
use crate::{
    crypto::Digest,
    ledger::{conventional_fee, replay_effects, Ledger, LedgerState, MAX_BLOCK_TRANSACTIONS},
    model::{Input, MerklePath, Note, MAX_INPUTS},
    scan::{memo_text, ScanBlock2, ScanPage2, ScanTransaction2},
    transaction::{self, Payment, Transaction},
    wallet::{Account, Address, MEMO_BYTES},
    Error,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Blocks this far under the tip are taken as final: the wallet keeps a copy
/// of its state there to go back to if the chain moves to another branch.
pub const SETTLE_DEPTH: u32 = 100;
/// Blocks between two such copies.
const SETTLE_STEP: u32 = 100;
/// A payment can be mined for this many blocks after the one it was built on.
pub const PAYMENT_LIFETIME: u32 = 20;

/// A note the wallet received, spent or not.
#[derive(Clone, Serialize, Deserialize)]
pub struct OwnedNote {
    pub note: Note,
    pub commitment: Digest,
    pub nullifier: Digest,
    /// Its place in the tree; `None` while a coinbase matures.
    pub index: Option<u32>,
    pub height: u32,
    pub coinbase: bool,
}

/// One transaction that touched the wallet.
#[derive(Clone, Serialize, Deserialize)]
pub struct Movement {
    pub txid: [u8; 32],
    pub height: u32,
    pub timestamp: u64,
    pub coinbase: bool,
    /// What it brought to the wallet, change included.
    pub received: u64,
    /// What the wallet's notes it consumed were worth.
    pub spent: u64,
    /// Its fee when the wallet paid it.
    pub fee: u64,
    /// The memo of what it brought, or of what the wallet paid.
    pub memo: String,
}
impl Movement {
    /// What it paid to others: zero unless the wallet sent it.
    pub fn paid(&self) -> u64 {
        self.spent
            .saturating_sub(self.received)
            .saturating_sub(self.fee)
    }
}

/// A payment built and handed to the network: its inputs stay out of the
/// balance until it is mined or expires.
#[derive(Clone, Serialize, Deserialize)]
pub struct Pending {
    pub txid: [u8; 32],
    pub nullifiers: Vec<Digest>,
    pub expiry: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct LightBalance {
    /// The last block read, and the tip the last page reported.
    pub height: u32,
    pub tip: u32,
    pub spendable_atoms: u64,
    pub immature_atoms: u64,
    pub reserved_atoms: u64,
    /// The most one payment can send now (eight inputs, less its fee).
    pub max_sendable_atoms: u64,
    pub notes: usize,
    pub pending: bool,
}

/// What a block left: the part a reorganisation takes back.
#[derive(Clone, Serialize, Deserialize)]
struct State {
    ledger: Ledger,
    /// The hash of the last block read.
    hash: String,
    notes: Vec<OwnedNote>,
    history: Vec<Movement>,
}
impl State {
    fn new(network: u8) -> Result<Self, Error> {
        Ok(Self {
            ledger: Ledger::new(network)?,
            hash: String::new(),
            notes: Vec::new(),
            history: Vec::new(),
        })
    }
    fn unspent(&self) -> impl Iterator<Item = &OwnedNote> {
        self.notes
            .iter()
            .filter(|owned| !self.ledger.spent.contains(&owned.nullifier))
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct LightWallet {
    network: u8,
    /// Outputs of earlier blocks are not opened: a fresh wallet's own
    /// height. The tree is still built from block 1.
    birth: u32,
    tip: u32,
    current: State,
    settled: Option<State>,
    /// A copy taken near the tip, waiting to become the settled one.
    recent: Option<State>,
    pending: Option<Pending>,
}

impl LightWallet {
    pub fn new(network: u8, birth: u32) -> Result<Self, Error> {
        Ok(Self {
            network,
            birth,
            tip: 0,
            current: State::new(network)?,
            settled: None,
            recent: None,
            pending: None,
        })
    }
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        postcard::to_allocvec(self).map_err(|_| Error::Encoding)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let (wallet, rest): (Self, _) =
            postcard::take_from_bytes(bytes).map_err(|_| Error::Encoding)?;
        if !rest.is_empty() {
            return Err(Error::Encoding);
        }
        Ok(wallet)
    }
    pub fn network(&self) -> u8 {
        self.network
    }
    /// The last block read.
    pub fn height(&self) -> u32 {
        self.current.ledger.height
    }
    /// The first block of the next page to read.
    pub fn next(&self) -> u32 {
        self.height() + 1
    }
    /// The chain's tip as the last page reported it.
    pub fn tip(&self) -> u32 {
        self.tip
    }
    pub fn in_sync(&self) -> bool {
        self.height() >= self.tip
    }
    /// The root a payment proves membership against now.
    pub fn anchor(&self) -> Digest {
        self.current.ledger.tree.root()
    }
    /// Oldest first.
    pub fn history(&self) -> &[Movement] {
        &self.current.history
    }
    pub fn pending(&self) -> Option<&Pending> {
        self.pending.as_ref()
    }

    /// Read a page that starts at [`Self::next`]. If the chain moved to
    /// another branch, the wallet goes back to its last settled copy and
    /// the rest of the page is dropped: read again from [`Self::next`].
    pub fn add_page(&mut self, account: &Account, page: &ScanPage2) -> Result<(), Error> {
        if page.network != self.network || account.network != self.network {
            return Err(Error::Network);
        }
        if page.from != self.next() {
            return Err(Error::Shape);
        }
        self.tip = page.tip;
        for block in &page.blocks {
            if self.height() > 0 && block.prev_hash != self.current.hash {
                self.rewind()?;
                return Ok(());
            }
            check_shape(block, self.next())?;
            if let Err(e) = self.add_block(account, block) {
                // A block that fails halfway leaves no trusted state.
                self.rewind()?;
                return Err(e);
            }
            self.keep_copies(block.height);
        }
        Ok(())
    }

    /// A copy of the state at least [`SETTLE_DEPTH`] under the tip, at most
    /// about twice that: one is taken where the line already is while
    /// catching up, and at the tip every [`SETTLE_STEP`] blocks, to settle
    /// once the chain has grown past it.
    fn keep_copies(&mut self, height: u32) {
        if self
            .recent
            .as_ref()
            .is_some_and(|r| r.ledger.height.saturating_add(SETTLE_DEPTH) <= self.tip)
        {
            self.settled = self.recent.take();
        }
        let settled = self.settled.as_ref().map_or(0, |s| s.ledger.height);
        let due = settled.saturating_add(SETTLE_STEP) <= height;
        let line = height.saturating_add(SETTLE_DEPTH);
        if line == self.tip || (line < self.tip && due) {
            self.settled = Some(self.current.clone());
        } else if line > self.tip && due && self.recent.is_none() {
            self.recent = Some(self.current.clone());
        }
    }

    /// Back to the last settled copy, or to nothing.
    fn rewind(&mut self) -> Result<(), Error> {
        self.recent = None;
        self.current = match &self.settled {
            Some(settled) => settled.clone(),
            None => State::new(self.network)?,
        };
        Ok(())
    }

    /// `block` passed [`check_shape`].
    fn add_block(&mut self, account: &Account, block: &ScanBlock2) -> Result<(), Error> {
        let height = self.next();
        let (coinbase, transfers) = block.transactions.split_first().ok_or(Error::Shape)?;
        let network = self.network;
        let state = &mut self.current;
        if height >= self.birth {
            let mut known: BTreeMap<Digest, u64> = state
                .notes
                .iter()
                .map(|owned| (owned.nullifier, owned.note.value))
                .collect();
            for tx in &block.transactions {
                let mut received = 0u64;
                let mut memo = None;
                let mut theirs = Vec::new();
                for output in &tx.outputs {
                    match account.receive(output.commitment, &output.envelope) {
                        Ok((note, text)) => {
                            let nullifier = note.nullifier(&account.spend, network)?;
                            if known.contains_key(&nullifier) {
                                continue;
                            }
                            known.insert(nullifier, note.value);
                            received = received.checked_add(note.value).ok_or(Error::Shape)?;
                            if memo.is_none() && text.iter().any(|&b| b != 0) {
                                memo = Some(text);
                            }
                            state.notes.push(OwnedNote {
                                note,
                                commitment: output.commitment,
                                nullifier,
                                index: None,
                                height,
                                coinbase: tx.coinbase(),
                            });
                        }
                        Err(_) => theirs.push(output),
                    }
                }
                let spent = tx
                    .nullifiers
                    .iter()
                    .filter_map(|nf| known.get(nf))
                    .try_fold(0u64, |sum, value| sum.checked_add(*value))
                    .ok_or(Error::Shape)?;
                if spent > 0 {
                    // What the wallet paid: the memo it wrote.
                    memo = theirs
                        .iter()
                        .find_map(|output| {
                            account
                                .recover_sent(output.commitment, &output.envelope)
                                .ok()
                        })
                        .map(|(_, text)| text);
                }
                if received > 0 || spent > 0 {
                    state.history.push(Movement {
                        txid: tx.txid,
                        height,
                        timestamp: block.timestamp,
                        coinbase: tx.coinbase(),
                        received,
                        spent,
                        fee: if spent > 0 { tx.fee } else { 0 },
                        memo: memo
                            .map(|text: [u8; MEMO_BYTES]| memo_text(&text))
                            .unwrap_or_default(),
                    });
                }
            }
        }
        let waiting = state
            .notes
            .iter()
            .enumerate()
            .filter(|(_, owned)| owned.index.is_none())
            .map(|(at, owned)| (owned.commitment, at))
            .collect();
        let mut tracking = Tracking {
            ledger: &mut state.ledger,
            waiting,
            found: Vec::new(),
        };
        let commitments =
            |tx: &ScanTransaction2| tx.outputs.iter().map(|o| o.commitment).collect::<Vec<_>>();
        let outputs: Vec<(Vec<Digest>, &[Digest])> = transfers
            .iter()
            .map(|tx| (commitments(tx), tx.nullifiers.as_slice()))
            .collect();
        replay_effects(
            &mut tracking,
            &commitments(coinbase),
            outputs.iter().map(|(c, n)| (c.as_slice(), *n)),
        )?;
        for (at, index) in tracking.found {
            state.notes[at].index = Some(index);
        }
        state.hash = block.hash.clone();
        let ledger = &state.ledger;
        if self.pending.as_ref().is_some_and(|p| {
            p.expiry <= ledger.height || p.nullifiers.iter().any(|nf| ledger.spent.contains(nf))
        }) {
            self.pending = None;
        }
        Ok(())
    }

    fn reserved(&self) -> BTreeSet<Digest> {
        self.pending
            .iter()
            .flat_map(|p| p.nullifiers.iter().copied())
            .collect()
    }

    /// Mature notes that no pending payment holds.
    fn spendable(&self) -> Vec<&OwnedNote> {
        let reserved = self.reserved();
        self.current
            .unspent()
            .filter(|owned| owned.index.is_some() && !reserved.contains(&owned.nullifier))
            .collect()
    }

    pub fn balance(&self) -> LightBalance {
        let reserved = self.reserved();
        let mut balance = LightBalance {
            height: self.height(),
            tip: self.tip,
            pending: self.pending.is_some(),
            ..LightBalance::default()
        };
        for owned in self.current.unspent() {
            balance.notes += 1;
            let total = if owned.index.is_none() {
                &mut balance.immature_atoms
            } else if reserved.contains(&owned.nullifier) {
                &mut balance.reserved_atoms
            } else {
                &mut balance.spendable_atoms
            };
            *total = total.saturating_add(owned.note.value);
        }
        if self.pending.is_none() {
            let mut values: Vec<u64> = self.spendable().iter().map(|o| o.note.value).collect();
            values.sort_unstable_by(|a, b| b.cmp(a));
            values.truncate(MAX_INPUTS);
            let total = values.iter().fold(0u64, |sum, v| sum.saturating_add(*v));
            balance.max_sendable_atoms =
                total.saturating_sub(conventional_fee(values.len().max(1), 2));
        }
        balance
    }

    /// The fee a payment of `amount` costs now, and how many notes it spends.
    pub fn quote(&self, amount: u64) -> Result<(u64, usize), Error> {
        let (inputs, _, fee) = self.select(amount)?;
        Ok((fee, inputs.len()))
    }

    /// The largest mature notes first, at most [`MAX_INPUTS`], until they pay
    /// `amount` and the fee of a two-output payment.
    fn select(&self, amount: u64) -> Result<(Vec<&OwnedNote>, u64, u64), Error> {
        if amount == 0 {
            return Err(Error::Ledger("the amount must be positive"));
        }
        if self.pending.is_some() {
            return Err(Error::Ledger(
                "a payment is still pending: wait until it is mined or expires",
            ));
        }
        let mut notes = self.spendable();
        notes.sort_by_key(|owned| std::cmp::Reverse(owned.note.value));
        let mut selected = Vec::new();
        let mut total = 0u64;
        for owned in notes.into_iter().take(MAX_INPUTS) {
            total = total.checked_add(owned.note.value).ok_or(Error::Shape)?;
            selected.push(owned);
            let needed = amount
                .checked_add(conventional_fee(selected.len(), 2))
                .ok_or(Error::Shape)?;
            if total >= needed {
                break;
            }
        }
        let fee = conventional_fee(selected.len().max(1), 2);
        if total < amount.checked_add(fee).ok_or(Error::Shape)? {
            return Err(Error::Ledger(
                "not enough mature funds within the eight-input limit",
            ));
        }
        Ok((selected, total, fee))
    }

    /// Everything a payment of `amount` to `to` needs but its proof: build it
    /// with [`Plan::prove`], then [`Self::reserve`] it before handing it to
    /// the network. The change goes back to `account`.
    pub fn plan(
        &self,
        account: &Account,
        to: Address,
        amount: u64,
        memo: &[u8],
    ) -> Result<Plan, Error> {
        if account.network != self.network || to.network != self.network {
            return Err(Error::Network);
        }
        if !self.in_sync() {
            return Err(Error::Ledger("the wallet is behind the chain: sync first"));
        }
        if memo.len() > MEMO_BYTES {
            return Err(Error::Ledger("the memo is longer than 512 bytes"));
        }
        let (selected, total, fee) = self.select(amount)?;
        let tree = &self.current.ledger.tree;
        let inputs = selected
            .into_iter()
            .map(|owned| {
                let index = owned.index.ok_or(Error::Ledger("immature input"))?;
                Ok((owned.note.clone(), tree.path(index)?))
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let mut payment = Payment::new(to, amount);
        payment.memo[..memo.len()].copy_from_slice(memo);
        let change = total - amount - fee;
        Ok(Plan {
            anchor: tree.root(),
            inputs,
            payments: vec![payment, Payment::new(account.address(), change)],
            fee,
            expiry: self
                .height()
                .checked_add(PAYMENT_LIFETIME)
                .ok_or(Error::Shape)?,
        })
    }

    /// Check a built payment against the wallet's copy of the ledger (its
    /// proof included) and hold its inputs until it is mined or expires.
    pub fn reserve(&mut self, tx: &Transaction) -> Result<(), Error> {
        self.current.ledger.check_transfer(tx)?;
        self.pending = Some(Pending {
            txid: tx.txid(),
            nullifiers: tx.statement.nullifiers.clone(),
            expiry: tx.height,
        });
        Ok(())
    }

    /// Free the inputs of a payment the network refused.
    pub fn release(&mut self) {
        self.pending = None;
    }
}

/// Block `height`: a coinbase first, then at most a block's worth of
/// transfers, each spending something.
fn check_shape(block: &ScanBlock2, height: u32) -> Result<(), Error> {
    let (coinbase, transfers) = block.transactions.split_first().ok_or(Error::Shape)?;
    if block.height != height
        || !coinbase.coinbase()
        || transfers.len() > MAX_BLOCK_TRANSACTIONS
        || transfers.iter().any(ScanTransaction2::coinbase)
    {
        return Err(Error::Shape);
    }
    Ok(())
}

/// A payment ready to be proved.
pub struct Plan {
    pub anchor: Digest,
    pub inputs: Vec<(Note, MerklePath)>,
    pub payments: Vec<Payment>,
    pub fee: u64,
    pub expiry: u32,
}
impl Plan {
    pub fn amount(&self) -> u64 {
        self.payments[0].value
    }
    pub fn change(&self) -> u64 {
        self.payments[1].value
    }
    /// The slow part: the STARK proof, where the keys are.
    pub fn prove(self, account: &Account) -> Result<Transaction, Error> {
        let inputs = self
            .inputs
            .into_iter()
            .map(|(note, path)| Input {
                key: account.spend.clone(),
                note,
                path,
            })
            .collect();
        transaction::transfer(
            account,
            self.anchor,
            inputs,
            self.payments,
            self.fee,
            self.expiry,
        )
    }
}

/// The ledger, telling which of the wallet's notes land where in the tree.
struct Tracking<'a> {
    ledger: &'a mut Ledger,
    waiting: BTreeMap<Digest, usize>,
    found: Vec<(usize, u32)>,
}
impl LedgerState for Tracking<'_> {
    fn network(&self) -> u8 {
        self.ledger.network
    }
    fn height(&self) -> u32 {
        self.ledger.height
    }
    fn issued(&self) -> u64 {
        self.ledger.issued
    }
    fn notes(&self) -> u64 {
        self.ledger.tree.len()
    }
    fn contains_anchor(&self, root: &Digest) -> bool {
        self.ledger.contains_anchor(root)
    }
    fn is_spent(&self, nullifier: &Digest) -> Result<bool, Error> {
        Ok(self.ledger.spent.contains(nullifier))
    }
    fn matured_count(&self, height: u32) -> usize {
        LedgerState::matured_count(&*self.ledger, height)
    }
    fn take_matured(&mut self, height: u32) -> Vec<Digest> {
        LedgerState::take_matured(&mut *self.ledger, height)
    }
    fn append(&mut self, note: Digest) -> Result<(), Error> {
        let index = self.ledger.tree.append(note)?;
        if let Some(at) = self.waiting.remove(&note) {
            self.found.push((at, index));
        }
        Ok(())
    }
    fn defer(&mut self, matures_at: u32, notes: Vec<Digest>) {
        LedgerState::defer(&mut *self.ledger, matures_at, notes)
    }
    fn spend(&mut self, nullifiers: BTreeSet<Digest>) {
        LedgerState::spend(&mut *self.ledger, nullifiers)
    }
    fn close(&mut self, height: u32, issued: u64) -> Result<(), Error> {
        LedgerState::close(&mut *self.ledger, height, issued)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ledger::{subsidy, ATOMS, PREMINE},
        scan::MAX_SCAN_BLOCKS,
    };

    const NET: u8 = 3;

    /// A chain kept by a full ledger, served as pages.
    struct Chain {
        ledger: Ledger,
        blocks: Vec<ScanBlock2>,
    }
    impl Chain {
        fn new() -> Self {
            Self {
                ledger: Ledger::new(NET).unwrap(),
                blocks: Vec::new(),
            }
        }
        fn mine(&mut self, miner: &Account, transfers: Vec<Transaction>, branch: u8) {
            let height = self.ledger.height + 1;
            let fees: u64 = transfers.iter().map(|tx| tx.statement.fee).sum();
            let cb =
                transaction::coinbase(&miner.address(), subsidy(height) + fees, height).unwrap();
            self.ledger.apply(&cb, &transfers).unwrap();
            let prev_hash = self
                .blocks
                .last()
                .map(|b| b.hash.clone())
                .unwrap_or_default();
            self.blocks.push(ScanBlock2 {
                height,
                hash: format!("{branch:02x}{height:062x}"),
                prev_hash,
                timestamp: 1_000 + u64::from(height) * 120,
                transactions: std::iter::once(&cb)
                    .chain(&transfers)
                    .map(ScanTransaction2::of)
                    .collect(),
            });
        }
        /// At most a page's worth of blocks, as a node serves them.
        fn page(&self, from: u32) -> ScanPage2 {
            let first = from as usize - 1;
            let last = (first + MAX_SCAN_BLOCKS as usize).min(self.blocks.len());
            let page = ScanPage2 {
                network: NET,
                tip: self.ledger.height,
                from,
                blocks: self.blocks[first..last].to_vec(),
            };
            ScanPage2::decode(&page.encode().unwrap()).unwrap()
        }
        /// Page after page until the wallet is at the tip.
        fn sync(&self, wallet: &mut LightWallet, account: &Account) {
            for _ in 0..10 {
                wallet.add_page(account, &self.page(wallet.next())).unwrap();
                if wallet.in_sync() {
                    return;
                }
            }
            panic!("the wallet does not catch up");
        }
    }

    fn fee() -> u64 {
        conventional_fee(1, 2)
    }

    #[test]
    fn a_light_wallet_rebuilds_the_tree_and_its_payment_is_accepted() {
        let alice = Account::from_seed(&[11; 32], NET, 0).unwrap();
        let bob = Account::from_seed(&[12; 32], NET, 0).unwrap();
        let mut chain = Chain::new();
        chain.mine(&alice, vec![], 0);
        chain.mine(&bob, vec![], 0);
        chain.mine(&bob, vec![], 0);

        let mut light = LightWallet::new(NET, 1).unwrap();
        light.add_page(&alice, &chain.page(1)).unwrap();
        assert_eq!((light.height(), light.next(), light.tip()), (3, 4, 3));
        assert_eq!(light.anchor(), chain.ledger.tree.root());
        let seen = light.balance();
        assert_eq!(seen.spendable_atoms, PREMINE);
        assert_eq!(seen.max_sendable_atoms, PREMINE - fee());

        let mut bobs = LightWallet::new(NET, 1).unwrap();
        bobs.add_page(&bob, &chain.page(1)).unwrap();
        // Regtest coinbases wait two blocks: block 2's joins the tree at 4.
        assert_eq!(bobs.balance().immature_atoms, 2 * 12 * ATOMS);
        assert!(bobs.plan(&bob, alice.address(), 1, b"").is_err());

        // Alice pays bob from her light wallet.
        let plan = light
            .plan(&alice, bob.address(), 42 * ATOMS, b"for the phone")
            .unwrap();
        assert_eq!(plan.anchor, chain.ledger.tree.root());
        assert_eq!(plan.fee, fee());
        assert_eq!(plan.change(), PREMINE - 42 * ATOMS - fee());
        assert_eq!(plan.expiry, 3 + PAYMENT_LIFETIME);
        let tx = plan.prove(&alice).unwrap();
        light.reserve(&tx).unwrap();
        let held = light.balance();
        assert!(held.pending);
        assert_eq!(held.spendable_atoms, 0);
        assert_eq!(held.reserved_atoms, PREMINE);
        assert!(light.plan(&alice, bob.address(), 1, b"").is_err());
        let mut light = LightWallet::decode(&light.encode().unwrap()).unwrap();

        // The full ledger takes it, and both light wallets follow.
        chain.ledger.check_transfer(&tx).unwrap();
        chain.mine(&bob, vec![tx.clone()], 0);
        light.add_page(&alice, &chain.page(4)).unwrap();
        bobs.add_page(&bob, &chain.page(4)).unwrap();
        assert_eq!(light.anchor(), chain.ledger.tree.root());
        assert_eq!(bobs.anchor(), chain.ledger.tree.root());

        let seen = light.balance();
        assert!(!seen.pending, "a mined payment frees the wallet");
        assert_eq!(seen.spendable_atoms, PREMINE - 42 * ATOMS - fee());
        let sent = light.history().last().unwrap();
        assert_eq!(sent.txid, tx.txid());
        assert_eq!(sent.paid(), 42 * ATOMS);
        assert_eq!(sent.fee, fee());
        assert_eq!(sent.memo, "for the phone");

        // 42 from alice, and block 2's coinbase is mature now; 3's and 4's
        // (with the fee) are not.
        let seen = bobs.balance();
        assert_eq!(seen.spendable_atoms, 42 * ATOMS + 12 * ATOMS);
        assert_eq!(seen.immature_atoms, 12 * ATOMS + 12 * ATOMS + fee());
        // The block's coinbase comes first, then its transfers.
        let reward = &bobs.history()[bobs.history().len() - 2];
        assert_eq!(
            (reward.coinbase, reward.received, reward.memo.as_str()),
            (true, 12 * ATOMS + fee(), "")
        );
        let got = bobs.history().last().unwrap();
        assert_eq!(
            (got.received, got.spent, got.memo.as_str()),
            (42 * ATOMS, 0, "for the phone")
        );
        // Every mature note's path leads to the ledger's root.
        for owned in bobs.current.unspent().filter(|o| o.index.is_some()) {
            let path = chain.ledger.tree.path(owned.index.unwrap()).unwrap();
            assert_eq!(
                path.root(NET, owned.commitment).unwrap(),
                chain.ledger.tree.root()
            );
        }
        let plan = bobs.plan(&bob, alice.address(), 50 * ATOMS, b"").unwrap();
        assert_eq!(plan.inputs.len(), 2);
        for (note, path) in &plan.inputs {
            let cm = note.commitment(NET).unwrap();
            assert_eq!(path.root(NET, cm).unwrap(), plan.anchor);
        }
    }

    #[test]
    fn another_branch_takes_the_wallet_back_to_its_settled_copy() {
        let miner = Account::from_seed(&[13; 32], NET, 0).unwrap();
        let mut chain = Chain::new();
        for _ in 0..(SETTLE_DEPTH + 29) {
            chain.mine(&miner, vec![], 0);
        }
        // Two branches share the first 129 blocks.
        let mut fork = Chain {
            ledger: chain.ledger.clone(),
            blocks: chain.blocks.clone(),
        };
        chain.mine(&miner, vec![], 0);
        let mut light = LightWallet::new(NET, 1).unwrap();
        chain.sync(&mut light, &miner);
        assert_eq!(light.height(), SETTLE_DEPTH + 30);
        let settled = light.settled.as_ref().unwrap().ledger.height;
        assert!(settled + SETTLE_DEPTH <= light.tip());

        // The chain replaces its last block: the next one names another
        // parent.
        fork.mine(&miner, vec![], 1);
        fork.mine(&miner, vec![], 1);
        light.add_page(&miner, &fork.page(light.next())).unwrap();
        assert_eq!(light.height(), settled, "back to the settled copy");
        assert_eq!(settled, 30, "the line under the tip when it caught up");
        fork.sync(&mut light, &miner);
        assert_eq!(light.height(), SETTLE_DEPTH + 31);
        assert_eq!(light.anchor(), fork.ledger.tree.root());
        assert_eq!(light.history().len(), (SETTLE_DEPTH + 31) as usize);

        // A page that does not start where the wallet is, or of another
        // network, changes nothing.
        let mut wrong = fork.page(1);
        assert!(light.add_page(&miner, &wrong).is_err());
        wrong.from = light.next();
        wrong.network = 2;
        assert!(light.add_page(&miner, &wrong).is_err());
        assert_eq!(light.height(), SETTLE_DEPTH + 31);

        // At the tip, a copy waits until the chain grows a settle depth past
        // it.
        let recent = light.recent.as_ref().unwrap().ledger.height;
        for _ in 0..SETTLE_DEPTH {
            fork.mine(&miner, vec![], 1);
            fork.sync(&mut light, &miner);
        }
        assert_eq!(light.settled.as_ref().unwrap().ledger.height, recent);
    }

    #[test]
    fn a_wallet_born_late_skips_what_came_before_but_keeps_the_tree() {
        let miner = Account::from_seed(&[14; 32], NET, 0).unwrap();
        let mut chain = Chain::new();
        for _ in 0..6 {
            chain.mine(&miner, vec![], 0);
        }
        let mut late = LightWallet::new(NET, 5).unwrap();
        late.add_page(&miner, &chain.page(1)).unwrap();
        assert_eq!(late.anchor(), chain.ledger.tree.root());
        assert_eq!(late.history().len(), 2);
        assert_eq!(late.balance().immature_atoms, 2 * 12 * ATOMS);
    }
}
