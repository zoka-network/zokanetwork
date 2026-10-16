//! A reference wallet: scans blocks, keeps its notes and their witnesses, and
//! builds transfers. The node and the phone engine follow the same steps.

use std::collections::HashMap;

use incrementalmerkletree::frontier::CommitmentTree;
use incrementalmerkletree::witness::IncrementalWitness;
use orchard::circuit::ProvingKey;
use orchard::keys::Scope;
use orchard::note::{ExtractedNoteCommitment, Nullifier};
use orchard::tree::{MerkleHashOrchard, MerklePath};
use orchard::{Anchor, Note};
use rand_core::{CryptoRng, Rng};

use crate::builder::{build_transfer, BuildError, Payment, SpendInput};
use crate::compact::CompactBlock;
use crate::keys::{QuanticFullViewingKey, QuanticSpendingKey};
use crate::note_encryption::{try_note_decryption, Memo};
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

impl QuanticWallet {
    pub fn new(fvk: QuanticFullViewingKey, params: &ConsensusParams) -> Self {
        Self {
            fvk,
            next_height: 0,
            tree: CommitmentTree::empty(),
            schedule: CommitmentSchedule::new(params.coinbase_maturity),
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
}
