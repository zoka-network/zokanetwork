//! A reference wallet: scans blocks, keeps its notes and their witnesses, and
//! builds transfers. The node and the phone engine follow the same steps.
//!
//! [`QuanticWallet::to_state_bytes`] saves everything it knows, so a phone
//! resumes scanning where it stopped instead of from the genesis.

use std::collections::{BTreeMap, HashMap};
use std::fmt;

use incrementalmerkletree::frontier::CommitmentTree;
use incrementalmerkletree::witness::IncrementalWitness;
use orchard::circuit::ProvingKey;
use orchard::keys::Scope;
use orchard::note::{ExtractedNoteCommitment, Nullifier, RandomSeed, Rho};
use orchard::tree::{MerkleHashOrchard, MerklePath};
use orchard::value::NoteValue;
use orchard::{Address, Anchor, Note, NoteVersion};
use rand_core::{CryptoRng, Rng};
use zeroize::Zeroizing;

use crate::builder::{build_transfer, BuildError, Payment, SpendInput};
use crate::compact::CompactBlock;
use crate::keys::{QuanticFullViewingKey, QuanticSpendingKey, FULL_VIEWING_KEY_BYTES};
use crate::note_encryption::{try_note_decryption, Memo, MEMO_SIZE};
use crate::state::{
    conventional_fee, CommitmentSchedule, ConsensusParams, NoteCommitmentTree, QuanticBlock,
    TREE_DEPTH,
};
use crate::tx::{QuanticTransaction, TxId, TxKind, MAX_ACTIONS_PER_TX};

#[derive(Clone, Debug)]
pub struct WalletNote {
    pub note: Note,
    pub scope: Scope,
    /// `None` when the note was found in a compact block.
    pub memo: Option<Memo>,
    pub nullifier: Nullifier,
    pub height: u32,
    pub txid: TxId,
    pub coinbase: bool,
    pub spent_in: Option<TxId>,
    /// Altura del bloque que la gastó.
    pub spent_height: Option<u32>,
    cmx: ExtractedNoteCommitment,
    /// `None` until the note is in the tree (a coinbase waits for maturity).
    witness: Option<IncrementalWitness<MerkleHashOrchard, TREE_DEPTH>>,
}

impl WalletNote {
    pub fn value(&self) -> u64 {
        self.note.value().inner()
    }

    pub fn commitment(&self) -> ExtractedNoteCommitment {
        self.cmx
    }

    pub fn is_spendable(&self) -> bool {
        self.spent_in.is_none() && self.witness.is_some()
    }
}

/// One transaction as the wallet saw it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryEntry {
    pub txid: TxId,
    pub height: u32,
    pub coinbase: bool,
    /// Value of the wallet's notes this transaction created (payments and change).
    pub received: u64,
    /// Value of the wallet's notes this transaction spent.
    pub spent: u64,
}

impl HistoryEntry {
    /// Positive when the wallet gained; for a send, minus (amount + fee).
    pub fn net(&self) -> i128 {
        i128::from(self.received) - i128::from(self.spent)
    }
}

/// What one block meant for the wallet.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScanSummary {
    pub received: u64,
    pub spent: u64,
    pub notes_found: usize,
}

pub struct QuanticWallet {
    fvk: QuanticFullViewingKey,
    next_height: u32,
    tree: NoteCommitmentTree,
    schedule: CommitmentSchedule,
    notes: Vec<WalletNote>,
    /// Notes found whose commitment is not in the tree yet, by commitment.
    awaiting_tree: HashMap<[u8; 32], usize>,
}

impl std::fmt::Debug for QuanticWallet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "QuanticWallet(next height {}, {} notes)",
            self.next_height,
            self.notes.len()
        )
    }
}

impl QuanticWallet {
    pub fn new(fvk: QuanticFullViewingKey, params: &ConsensusParams) -> Self {
        Self {
            fvk,
            next_height: params.genesis_height,
            tree: CommitmentTree::empty(),
            schedule: params.schedule(),
            notes: Vec::new(),
            awaiting_tree: HashMap::new(),
        }
    }

    pub fn fvk(&self) -> &QuanticFullViewingKey {
        &self.fvk
    }

    pub fn next_height(&self) -> u32 {
        self.next_height
    }

    pub fn notes(&self) -> &[WalletNote] {
        &self.notes
    }

    /// The wallet's transactions, newest first.
    pub fn history(&self) -> Vec<HistoryEntry> {
        let mut entries: HashMap<[u8; 32], HistoryEntry> = HashMap::new();
        for note in &self.notes {
            let created = entries.entry(note.txid.0).or_insert(HistoryEntry {
                txid: note.txid,
                height: note.height,
                coinbase: note.coinbase,
                received: 0,
                spent: 0,
            });
            created.received += note.value();
            if let (Some(txid), Some(height)) = (note.spent_in, note.spent_height) {
                let spending = entries.entry(txid.0).or_insert(HistoryEntry {
                    txid,
                    height,
                    coinbase: false,
                    received: 0,
                    spent: 0,
                });
                spending.spent += note.value();
            }
        }
        let mut list: Vec<HistoryEntry> = entries.into_values().collect();
        list.sort_by(|a, b| b.height.cmp(&a.height).then(a.txid.cmp(&b.txid)));
        list
    }

    /// Value that can be spent now.
    pub fn balance(&self) -> u64 {
        self.notes
            .iter()
            .filter(|n| n.is_spendable())
            .map(WalletNote::value)
            .sum()
    }

    /// Received but not yet spendable (immature coinbase).
    pub fn immature_balance(&self) -> u64 {
        self.notes
            .iter()
            .filter(|n| n.spent_in.is_none() && n.witness.is_none())
            .map(WalletNote::value)
            .sum()
    }

    /// The root the witnesses lead to: the anchor of the next transfer.
    pub fn anchor(&self) -> Anchor {
        Anchor::from(self.tree.root())
    }

    /// Processes the next block, which must already be valid.
    pub fn scan_block(&mut self, block: &QuanticBlock) -> ScanSummary {
        let mut found = Vec::new();
        for tx in block.all_transactions() {
            let actions = tx
                .actions()
                .iter()
                .map(|a| {
                    let note = try_note_decryption(&self.fvk, &a.nf, &a.cmx, &a.envelope)
                        .map(|d| (d.note, d.scope, Some(d.memo)));
                    (a.nf, a.cmx, note)
                })
                .collect();
            found.push((tx.txid(), tx.kind(), actions));
        }
        self.apply_scanned_block(block.height, found)
    }

    /// The same for a compact block, as a light wallet receives it. The caller
    /// checks it first with [`CompactBlock::matches_header`]. Memos are not
    /// included.
    pub fn scan_compact_block(&mut self, block: &CompactBlock) -> ScanSummary {
        let mut found = Vec::new();
        for tx in &block.transactions {
            let mut owned: HashMap<usize, (Note, Scope)> = tx
                .owned_notes(&self.fvk)
                .into_iter()
                .map(|(i, note, scope)| (i, (note, scope)))
                .collect();
            let actions = tx
                .actions
                .iter()
                .enumerate()
                .map(|(i, a)| {
                    let note = owned.remove(&i).map(|(note, scope)| (note, scope, None));
                    (a.nf, a.cmx, note)
                })
                .collect();
            found.push((tx.txid(), tx.kind, actions));
        }
        self.apply_scanned_block(block.height, found)
    }

    #[allow(clippy::type_complexity)]
    fn apply_scanned_block(
        &mut self,
        height: u32,
        transactions: Vec<(
            TxId,
            TxKind,
            Vec<(
                Nullifier,
                ExtractedNoteCommitment,
                Option<(Note, Scope, Option<Memo>)>,
            )>,
        )>,
    ) -> ScanSummary {
        assert_eq!(height, self.next_height, "blocks must be scanned in order");
        let mut summary = ScanSummary::default();
        let mut coinbase_commitments = Vec::new();
        let mut transfer_commitments = Vec::new();
        for (txid, kind, actions) in transactions {
            for (nf, cmx, found) in actions {
                match kind {
                    TxKind::Coinbase => coinbase_commitments.push(cmx),
                    TxKind::Transfer => transfer_commitments.push(cmx),
                }
                if let Some(note) = self
                    .notes
                    .iter_mut()
                    .find(|n| n.spent_in.is_none() && n.nullifier.to_bytes() == nf.to_bytes())
                {
                    note.spent_in = Some(txid);
                    note.spent_height = Some(height);
                    summary.spent += note.value();
                }
                if let Some((note, scope, memo)) = found {
                    summary.received += note.value().inner();
                    summary.notes_found += 1;
                    self.awaiting_tree.insert(cmx.to_bytes(), self.notes.len());
                    self.notes.push(WalletNote {
                        nullifier: note.nullifier(self.fvk.orchard()),
                        note,
                        scope,
                        memo,
                        height,
                        txid,
                        coinbase: kind == TxKind::Coinbase,
                        spent_in: None,
                        spent_height: None,
                        cmx,
                        witness: None,
                    });
                }
            }
        }
        let commitments =
            self.schedule
                .commitments_for_block(height, coinbase_commitments, transfer_commitments);
        for cmx in commitments {
            let node = MerkleHashOrchard::from_cmx(&cmx);
            self.tree
                .append(node)
                .expect("the tree has room for 2^32 notes");
            for note in &mut self.notes {
                if let Some(witness) = note.witness.as_mut() {
                    witness.append(node).expect("witness tree has room");
                }
            }
            if let Some(index) = self.awaiting_tree.remove(&cmx.to_bytes()) {
                self.notes[index].witness = IncrementalWitness::from_tree(self.tree.clone());
            }
        }
        self.next_height += 1;
        summary
    }

    /// Picks notes (largest first) and builds a transfer paying the
    /// conventional fee. The notes stay unspent until a block spends them.
    pub fn create_transfer<R: Rng + CryptoRng>(
        &self,
        sk: &QuanticSpendingKey,
        payments: Vec<Payment>,
        expiry_height: u32,
        pk: &ProvingKey,
        rng: &mut R,
    ) -> Result<QuanticTransaction, BuildError> {
        let target: u64 = payments.iter().map(|p| p.value).sum();
        let mut candidates: Vec<&WalletNote> =
            self.notes.iter().filter(|n| n.is_spendable()).collect();
        candidates.sort_by_key(|n| std::cmp::Reverse(n.value()));
        let mut chosen = Vec::new();
        let mut total = 0u64;
        // Outputs: the payments plus, almost always, one change note.
        let outputs = payments.len() + 1;
        for note in candidates {
            if chosen.len() == MAX_ACTIONS_PER_TX {
                break;
            }
            chosen.push(note);
            total += note.value();
            if total >= target + conventional_fee(chosen.len().max(outputs)) {
                break;
            }
        }
        let fee = conventional_fee(chosen.len().max(outputs));
        if total < target + fee {
            return Err(BuildError::InsufficientFunds {
                available: self.balance(),
                needed: target + fee,
            });
        }
        let spends = chosen
            .into_iter()
            .map(|n| SpendInput {
                note: n.note,
                merkle_path: MerklePath::from(
                    n.witness
                        .as_ref()
                        .and_then(IncrementalWitness::path)
                        .expect("spendable notes have a witness"),
                ),
            })
            .collect();
        build_transfer(
            sk,
            spends,
            payments,
            fee,
            self.anchor(),
            expiry_height,
            pk,
            rng,
        )
    }

    /// Everything the wallet knows, to resume scanning where it stopped.
    ///
    /// It contains the full viewing key, which shows the whole history of
    /// the wallet: store it encrypted.
    pub fn to_state_bytes(&self) -> Zeroizing<Vec<u8>> {
        let mut w = Writer::default();
        w.bytes(WALLET_STATE_MAGIC);
        w.u8(WALLET_STATE_VERSION);
        w.bytes(self.fvk.to_bytes().as_ref());
        w.u32(self.next_height);
        w.u32(self.schedule.genesis_height());
        w.u32(self.schedule.maturity());
        write_tree(&mut w, &self.tree);
        w.u32(self.schedule.pending().len() as u32);
        for (height, commitments) in self.schedule.pending() {
            w.u32(*height);
            w.u32(commitments.len() as u32);
            for cmx in commitments {
                w.bytes(&cmx.to_bytes());
            }
        }
        w.u32(self.notes.len() as u32);
        for note in &self.notes {
            write_note(&mut w, note);
        }
        w.0
    }

    /// A wallet saved with [`Self::to_state_bytes`], for the chain of
    /// `params`. What can be derived (nullifiers, commitments, the notes
    /// waiting to enter the tree) is recomputed, not read; every note has to
    /// belong to the saved key and every witness has to lead to the saved
    /// tree.
    pub fn from_state_bytes(bytes: &[u8], params: &ConsensusParams) -> Result<Self, WalletStateError> {
        let mut r = Reader { bytes };
        if r.take(WALLET_STATE_MAGIC.len())? != WALLET_STATE_MAGIC {
            return Err(WalletStateError::NotAWalletState);
        }
        let version = r.u8()?;
        if version != WALLET_STATE_VERSION {
            return Err(WalletStateError::UnknownVersion(version));
        }
        let fvk = QuanticFullViewingKey::from_bytes(r.take(FULL_VIEWING_KEY_BYTES)?)
            .map_err(|_| WalletStateError::Invalid("viewing key"))?;
        let next_height = r.u32()?;
        let genesis_height = r.u32()?;
        let maturity = r.u32()?;
        if fvk.network() != params.network
            || genesis_height != params.genesis_height
            || maturity != params.coinbase_maturity
        {
            return Err(WalletStateError::OtherChain);
        }
        if next_height < genesis_height {
            return Err(WalletStateError::Invalid("height"));
        }
        let tree = read_tree(&mut r)?;
        let mut pending = BTreeMap::new();
        for _ in 0..r.u32()? {
            let height = r.u32()?;
            let commitments = (0..r.u32()?)
                .map(|_| read_cmx(&mut r))
                .collect::<Result<Vec<_>, _>>()?;
            if height < next_height || pending.insert(height, commitments).is_some() {
                return Err(WalletStateError::Invalid("pending coinbase"));
            }
        }
        let mut notes = Vec::new();
        for _ in 0..r.u32()? {
            notes.push(read_note(&mut r, &fvk)?);
        }
        r.finish()?;
        let root = tree.root();
        for note in &notes {
            if let Some(witness) = &note.witness {
                if witness.root() != root
                    || witness.tree().leaf() != Some(&MerkleHashOrchard::from_cmx(&note.cmx))
                {
                    return Err(WalletStateError::Invalid("witness"));
                }
            }
        }
        let awaiting_tree = notes
            .iter()
            .enumerate()
            .filter(|(_, note)| note.witness.is_none())
            .map(|(index, note)| (note.cmx.to_bytes(), index))
            .collect();
        Ok(Self {
            fvk,
            next_height,
            tree,
            schedule: params.schedule().with_pending(pending),
            notes,
            awaiting_tree,
        })
    }
}

// ---------------------------------------------------------------------------
// Saved state

/// Version of the format of [`QuanticWallet::to_state_bytes`].
pub const WALLET_STATE_VERSION: u8 = 1;
const WALLET_STATE_MAGIC: &[u8; 4] = b"ZQWS";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WalletStateError {
    NotAWalletState,
    UnknownVersion(u8),
    /// Saved for another network, genesis or maturity.
    OtherChain,
    Truncated,
    Invalid(&'static str),
}

impl fmt::Display for WalletStateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAWalletState => f.write_str("not a saved Quantic wallet"),
            Self::UnknownVersion(v) => write!(f, "unknown wallet state version {v}"),
            Self::OtherChain => f.write_str("the wallet was saved for another chain"),
            Self::Truncated => f.write_str("the saved wallet is truncated"),
            Self::Invalid(what) => write!(f, "the saved wallet has an invalid {what}"),
        }
    }
}

impl std::error::Error for WalletStateError {}

#[derive(Default)]
struct Writer(Zeroizing<Vec<u8>>);

impl Writer {
    fn bytes(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }

    fn u8(&mut self, value: u8) {
        self.0.push(value);
    }

    fn u32(&mut self, value: u32) {
        self.bytes(&value.to_le_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.bytes(&value.to_le_bytes());
    }

    fn flag(&mut self, present: bool) {
        self.u8(u8::from(present));
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], WalletStateError> {
        if self.bytes.len() < n {
            return Err(WalletStateError::Truncated);
        }
        let (head, rest) = self.bytes.split_at(n);
        self.bytes = rest;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], WalletStateError> {
        Ok(self.take(N)?.try_into().expect("length checked"))
    }

    fn u8(&mut self) -> Result<u8, WalletStateError> {
        Ok(self.array::<1>()?[0])
    }

    fn u32(&mut self) -> Result<u32, WalletStateError> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    fn u64(&mut self) -> Result<u64, WalletStateError> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    fn flag(&mut self) -> Result<bool, WalletStateError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(WalletStateError::Invalid("flag")),
        }
    }

    fn finish(&self) -> Result<(), WalletStateError> {
        if self.bytes.is_empty() {
            Ok(())
        } else {
            Err(WalletStateError::Invalid("length"))
        }
    }
}

fn write_node(w: &mut Writer, node: Option<&MerkleHashOrchard>) {
    w.flag(node.is_some());
    if let Some(node) = node {
        w.bytes(&node.to_bytes());
    }
}

fn read_node(r: &mut Reader<'_>) -> Result<MerkleHashOrchard, WalletStateError> {
    Option::from(MerkleHashOrchard::from_bytes(&r.array()?)).ok_or(WalletStateError::Invalid("tree node"))
}

fn read_optional_node(r: &mut Reader<'_>) -> Result<Option<MerkleHashOrchard>, WalletStateError> {
    if r.flag()? {
        read_node(r).map(Some)
    } else {
        Ok(None)
    }
}

fn read_cmx(r: &mut Reader<'_>) -> Result<ExtractedNoteCommitment, WalletStateError> {
    Option::from(ExtractedNoteCommitment::from_bytes(&r.array()?))
        .ok_or(WalletStateError::Invalid("note commitment"))
}

/// The tree as it is (left, right and parents), not its frontier: a
/// witness keeps the tree of the moment it started.
fn write_tree(w: &mut Writer, tree: &NoteCommitmentTree) {
    write_node(w, tree.left().as_ref());
    write_node(w, tree.right().as_ref());
    w.u8(tree.parents().len() as u8);
    for parent in tree.parents() {
        write_node(w, parent.as_ref());
    }
}

fn read_tree(r: &mut Reader<'_>) -> Result<NoteCommitmentTree, WalletStateError> {
    let left = read_optional_node(r)?;
    let right = read_optional_node(r)?;
    if left.is_none() && right.is_some() {
        return Err(WalletStateError::Invalid("tree"));
    }
    let parents = (0..r.u8()?)
        .map(|_| read_optional_node(r))
        .collect::<Result<Vec<_>, _>>()?;
    CommitmentTree::from_parts(left, right, parents).map_err(|_| WalletStateError::Invalid("tree"))
}

fn write_witness(w: &mut Writer, witness: &IncrementalWitness<MerkleHashOrchard, TREE_DEPTH>) {
    write_tree(w, witness.tree());
    w.u8(witness.filled().len() as u8);
    for node in witness.filled() {
        w.bytes(&node.to_bytes());
    }
    w.flag(witness.cursor().is_some());
    if let Some(cursor) = witness.cursor() {
        write_tree(w, cursor);
    }
}

fn read_witness(r: &mut Reader<'_>) -> Result<IncrementalWitness<MerkleHashOrchard, TREE_DEPTH>, WalletStateError> {
    let tree = read_tree(r)?;
    let filled_count = r.u8()?;
    if filled_count > TREE_DEPTH {
        return Err(WalletStateError::Invalid("witness"));
    }
    let filled = (0..filled_count)
        .map(|_| read_node(r))
        .collect::<Result<Vec<_>, _>>()?;
    let cursor = if r.flag()? { Some(read_tree(r)?) } else { None };
    IncrementalWitness::from_parts(tree, filled, cursor).ok_or(WalletStateError::Invalid("witness"))
}

fn write_note(w: &mut Writer, note: &WalletNote) {
    w.bytes(&note.note.recipient().to_raw_address_bytes());
    w.u64(note.value());
    w.bytes(&note.note.rho().to_bytes());
    w.bytes(note.note.rseed().as_bytes());
    w.u8(match note.note.version() {
        NoteVersion::V2 => 2,
        NoteVersion::V3 => 3,
    });
    w.u8(match note.scope {
        Scope::External => 0,
        Scope::Internal => 1,
    });
    w.flag(note.memo.is_some());
    if let Some(memo) = &note.memo {
        w.bytes(memo);
    }
    w.u32(note.height);
    w.bytes(&note.txid.0);
    w.flag(note.coinbase);
    w.flag(note.spent_in.is_some());
    if let Some(txid) = note.spent_in {
        w.bytes(&txid.0);
    }
    w.flag(note.spent_height.is_some());
    if let Some(height) = note.spent_height {
        w.u32(height);
    }
    w.flag(note.witness.is_some());
    if let Some(witness) = &note.witness {
        write_witness(w, witness);
    }
}

fn read_note(r: &mut Reader<'_>, fvk: &QuanticFullViewingKey) -> Result<WalletNote, WalletStateError> {
    let invalid = || WalletStateError::Invalid("note");
    let recipient: Address = Option::from(Address::from_raw_address_bytes(&r.array()?)).ok_or_else(invalid)?;
    let value = NoteValue::from_raw(r.u64()?);
    let rho: Rho = Option::from(Rho::from_bytes(&r.array()?)).ok_or_else(invalid)?;
    let rseed: RandomSeed = Option::from(RandomSeed::from_bytes(r.array()?, &rho)).ok_or_else(invalid)?;
    let version = match r.u8()? {
        2 => NoteVersion::V2,
        3 => NoteVersion::V3,
        _ => return Err(invalid()),
    };
    let note: Note = Option::from(Note::from_parts(recipient, value, rho, rseed, version)).ok_or_else(invalid)?;
    let scope = match r.u8()? {
        0 => Scope::External,
        1 => Scope::Internal,
        _ => return Err(invalid()),
    };
    // A note of another wallet cannot be spent with this key.
    if fvk.orchard().scope_for_address(&recipient) != Some(scope) {
        return Err(WalletStateError::Invalid("note owner"));
    }
    let memo = if r.flag()? { Some(r.array::<MEMO_SIZE>()?) } else { None };
    let height = r.u32()?;
    let txid = TxId(r.array()?);
    let coinbase = r.flag()?;
    let spent_in = if r.flag()? { Some(TxId(r.array()?)) } else { None };
    let spent_height = if r.flag()? { Some(r.u32()?) } else { None };
    if spent_in.is_some() != spent_height.is_some() {
        return Err(invalid());
    }
    let witness = if r.flag()? { Some(read_witness(r)?) } else { None };
    Ok(WalletNote {
        nullifier: note.nullifier(fvk.orchard()),
        cmx: ExtractedNoteCommitment::from(note.commitment()),
        note,
        scope,
        memo,
        height,
        txid,
        coinbase,
        spent_in,
        spent_height,
        witness,
    })
}
