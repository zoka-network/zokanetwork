//! What a light wallet downloads, and how it checks it (A5 of the light
//! wallet plan).
//!
//! Per action only the nullifier, the note commitment, the X-Wing ciphertext
//! and the first 52 bytes of the note ciphertext travel (1,236 bytes): enough
//! to recognize payments, detect spends and keep the commitment tree. Per
//! transaction, the two digests the wallet cannot compute (memos and the
//! non-compact part) let it rebuild the txid; the txids of a block rebuild its
//! `txid_root`, which the header commits to. So a node can neither remove nor
//! alter an output without the wallet noticing.

use std::fmt;

use orchard::bundle::Flags;
use orchard::note::{ExtractedNoteCommitment, Nullifier};
use orchard::Anchor;

use crate::keys::QuanticFullViewingKey;
use crate::network::Network;
use crate::note_encryption::{try_compact_note_decryption, COMPACT_NOTE_SIZE, KEM_CIPHERTEXT_SIZE};
use crate::state::{merkle_root, QuanticBlock};
use crate::tx::{
    bundle_version, compact_digest, header_digest, txid_from_digests, QuanticTransaction,
    TxDigests, TxId, TxKind, MAX_ACTIONS_PER_TX,
};

pub const COMPACT_ACTION_BYTES: usize = 32 + 32 + KEM_CIPHERTEXT_SIZE + COMPACT_NOTE_SIZE;

#[derive(Clone, Debug)]
pub struct CompactAction {
    pub nf: Nullifier,
    pub cmx: ExtractedNoteCommitment,
    pub kem: Box<[u8; KEM_CIPHERTEXT_SIZE]>,
    pub enc_compact: [u8; COMPACT_NOTE_SIZE],
}

#[derive(Clone, Debug)]
pub struct CompactTx {
    pub network: Network,
    pub kind: TxKind,
    pub height: u32,
    pub memos_digest: [u8; 32],
    pub noncompact_digest: [u8; 32],
    pub flags: Flags,
    pub value_balance: i64,
    pub anchor: Anchor,
    pub actions: Vec<CompactAction>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompactError {
    Truncated,
    TrailingBytes,
    Invalid,
    ActionCount(usize),
}

impl fmt::Display for CompactError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CompactError::Truncated => f.write_str("compact data is truncated"),
            CompactError::TrailingBytes => f.write_str("compact data has trailing bytes"),
            CompactError::Invalid => f.write_str("invalid compact data"),
            CompactError::ActionCount(n) => write!(f, "invalid action count {n}"),
        }
    }
}

impl std::error::Error for CompactError {}

impl CompactTx {
    pub fn from_tx(tx: &QuanticTransaction) -> Self {
        let effects = tx.effects();
        let digests = effects.digests();
        Self {
            network: effects.network,
            kind: effects.kind,
            height: effects.height,
            memos_digest: digests.memos,
            noncompact_digest: digests.noncompact,
            flags: effects.flags,
            value_balance: effects.value_balance,
            anchor: effects.anchor,
            actions: effects
                .actions
                .iter()
                .map(|a| CompactAction {
                    nf: a.nf,
                    cmx: a.cmx,
                    kem: Box::new(*a.envelope.kem_ciphertext()),
                    enc_compact: *a.envelope.enc_ciphertext_compact(),
                })
                .collect(),
        }
    }

    /// The txid, recomputed from the compact data alone.
    pub fn txid(&self) -> TxId {
        let digests = TxDigests {
            header: header_digest(self.network, self.kind, self.height),
            compact: compact_digest(
                self.actions
                    .iter()
                    .map(|a| (&a.nf, &a.cmx, &a.kem[..], &a.enc_compact[..])),
            ),
            memos: self.memos_digest,
            noncompact: self.noncompact_digest,
        };
        let flags = self
            .flags
            .to_byte(bundle_version())
            .expect("Ironwood represents every flag set");
        txid_from_digests(&digests, flags, self.value_balance, &self.anchor)
    }

    /// The fee, for a transfer.
    pub fn fee(&self) -> Option<u64> {
        match self.kind {
            TxKind::Transfer => u64::try_from(self.value_balance).ok(),
            TxKind::Coinbase => None,
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(
            4 + 2 + 64 + 1 + 8 + 32 + 2 + self.actions.len() * COMPACT_ACTION_BYTES,
        );
        out.push(self.network.id());
        out.push(match self.kind {
            TxKind::Transfer => 0,
            TxKind::Coinbase => 1,
        });
        out.extend_from_slice(&self.height.to_le_bytes());
        out.extend_from_slice(&self.memos_digest);
        out.extend_from_slice(&self.noncompact_digest);
        out.push(
            self.flags
                .to_byte(bundle_version())
                .expect("Ironwood flags"),
        );
        out.extend_from_slice(&self.value_balance.to_le_bytes());
        out.extend_from_slice(&self.anchor.to_bytes());
        out.extend_from_slice(&(self.actions.len() as u16).to_le_bytes());
        for a in &self.actions {
            out.extend_from_slice(&a.nf.to_bytes());
            out.extend_from_slice(&a.cmx.to_bytes());
            out.extend_from_slice(&a.kem[..]);
            out.extend_from_slice(&a.enc_compact);
        }
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CompactError> {
        let mut rest = bytes;
        let mut take = |n: usize| -> Result<&[u8], CompactError> {
            if rest.len() < n {
                return Err(CompactError::Truncated);
            }
            let (head, tail) = rest.split_at(n);
            rest = tail;
            Ok(head)
        };
        let network = Network::from_id(take(1)?[0]).ok_or(CompactError::Invalid)?;
        let kind = match take(1)?[0] {
            0 => TxKind::Transfer,
            1 => TxKind::Coinbase,
            _ => return Err(CompactError::Invalid),
        };
        let height = u32::from_le_bytes(take(4)?.try_into().expect("4 bytes"));
        let memos_digest: [u8; 32] = take(32)?.try_into().expect("32 bytes");
        let noncompact_digest: [u8; 32] = take(32)?.try_into().expect("32 bytes");
        let flags = Flags::from_byte(take(1)?[0], bundle_version()).ok_or(CompactError::Invalid)?;
        let value_balance = i64::from_le_bytes(take(8)?.try_into().expect("8 bytes"));
        let anchor: Option<Anchor> =
            Anchor::from_bytes(take(32)?.try_into().expect("32 bytes")).into();
        let anchor = anchor.ok_or(CompactError::Invalid)?;
        let count = u16::from_le_bytes(take(2)?.try_into().expect("2 bytes")) as usize;
        if !(1..=MAX_ACTIONS_PER_TX).contains(&count) {
            return Err(CompactError::ActionCount(count));
        }
        let mut actions = Vec::with_capacity(count);
        for _ in 0..count {
            let nf: Option<Nullifier> =
                Nullifier::from_bytes(take(32)?.try_into().expect("32 bytes")).into();
            let cmx: Option<ExtractedNoteCommitment> =
                ExtractedNoteCommitment::from_bytes(take(32)?.try_into().expect("32 bytes")).into();
            let kem: [u8; KEM_CIPHERTEXT_SIZE] =
                take(KEM_CIPHERTEXT_SIZE)?.try_into().expect("exact");
            let enc_compact: [u8; COMPACT_NOTE_SIZE] =
                take(COMPACT_NOTE_SIZE)?.try_into().expect("exact");
            actions.push(CompactAction {
                nf: nf.ok_or(CompactError::Invalid)?,
                cmx: cmx.ok_or(CompactError::Invalid)?,
                kem: Box::new(kem),
                enc_compact,
            });
        }
        if !rest.is_empty() {
            return Err(CompactError::TrailingBytes);
        }
        Ok(Self {
            network,
            kind,
            height,
            memos_digest,
            noncompact_digest,
            flags,
            value_balance,
            anchor,
            actions,
        })
    }

    /// The actions of this transaction that pay `fvk`, by index. No memo: a
    /// wallet that wants it downloads the whole transaction and checks its txid.
    pub fn owned_notes(
        &self,
        fvk: &QuanticFullViewingKey,
    ) -> Vec<(usize, orchard::Note, orchard::keys::Scope)> {
        self.actions
            .iter()
            .enumerate()
            .filter_map(|(i, a)| {
                try_compact_note_decryption(fvk, &a.nf, &a.cmx, &a.kem, &a.enc_compact)
                    .map(|(note, scope)| (i, note, scope))
            })
            .collect()
    }
}

/// A block as a light wallet sees it.
#[derive(Clone, Debug)]
pub struct CompactBlock {
    pub height: u32,
    /// The coinbase first, as in the block.
    pub transactions: Vec<CompactTx>,
}

impl CompactBlock {
    pub fn from_block(block: &QuanticBlock) -> Self {
        Self {
            height: block.height,
            transactions: block.all_transactions().map(CompactTx::from_tx).collect(),
        }
    }

    pub fn txid_root(&self) -> [u8; 32] {
        merkle_root(
            &self
                .transactions
                .iter()
                .map(|tx| tx.txid().0)
                .collect::<Vec<_>>(),
        )
    }

    /// Checks the compact block against the `txid_root` of a header the wallet
    /// already trusts (proof of work and authority checkpoints). Also requires
    /// the coinbase in first place and nowhere else.
    pub fn matches_header(&self, txid_root: &[u8; 32]) -> bool {
        let shape_ok = self.transactions.first().map(|tx| tx.kind) == Some(TxKind::Coinbase)
            && self
                .transactions
                .iter()
                .skip(1)
                .all(|tx| tx.kind == TxKind::Transfer)
            && self.transactions.first().map(|tx| tx.height) == Some(self.height);
        shape_ok && self.txid_root() == *txid_root
    }

    pub fn serialized_size(&self) -> usize {
        self.transactions.iter().map(|tx| tx.to_bytes().len()).sum()
    }
}
