//! Quantic transactions: wire format, transaction id and authorizing digest.
//!
//! Every transaction is one Ironwood bundle (`orchard` 0.16, `ironwood_v3`)
//! whose note ciphertexts are X-Wing envelopes. There is no transparent pool:
//! a transfer's value balance is its fee, and the coinbase mints with a
//! negative value balance.
//!
//! The id is a tree of digests in the style of ZIP 244, so that a light
//! wallet holding only the compact data of each action (plus two digests) can
//! recompute it and check it against the block:
//!
//! ```text
//! header     = H("ZokaQ_TxHeader__", version ‖ network ‖ kind ‖ height)
//! compact    = H("ZokaQ_ActCompact", per action: nf ‖ cmx ‖ kem ‖ enc[..52])
//! memos      = H("ZokaQ_ActMemos__", per action: enc[52..])
//! noncompact = H("ZokaQ_ActNonComp", per action: cv_net ‖ rk ‖ out)
//! effects    = H("ZokaQ_BundleEff_", compact ‖ memos ‖ noncompact ‖ flags ‖ value_balance ‖ anchor)
//! txid       = H("ZokaQ_TxId______", header ‖ effects)
//! auth       = H("ZokaQ_TxAuth____", proof ‖ spend signatures ‖ binding signature)
//! ```
//!
//! Signatures sign the txid, which does not cover them: the id cannot be
//! changed by re-randomizing a signature. Blocks commit to both the ids and
//! the authorizing digests.

use std::fmt;

use group::{Group, GroupEncoding};
use nonempty::NonEmpty;
use orchard::bundle::{Authorized, BundleVersion, Flags};
use orchard::note::{ExtractedNoteCommitment, Nullifier, TransmittedNoteCiphertext};
use orchard::note_encryption::NoteBytesData;
use orchard::primitives::redpallas::{self, Binding, SpendAuth};
use orchard::value::ValueCommitment;
use orchard::{Action, Anchor, Bundle, Proof};
use pasta_curves::pallas;

use crate::hash::{
    blake2b_256, ACTIONS_COMPACT, ACTIONS_MEMOS, ACTIONS_NONCOMPACT, BUNDLE_EFFECTS, TX_AUTH,
    TX_HEADER, TX_ID,
};
use crate::network::Network;
use crate::note_encryption::{
    NoteEnvelope, COMPACT_NOTE_SIZE, ENC_CIPHERTEXT_SIZE, KEM_CIPHERTEXT_SIZE, OUT_CIPHERTEXT_SIZE,
};

pub const TX_VERSION: u32 = 1;
pub const MAX_ACTIONS_PER_TX: usize = 64;
pub const ACTION_BYTES: usize =
    4 * 32 + KEM_CIPHERTEXT_SIZE + ENC_CIPHERTEXT_SIZE + OUT_CIPHERTEXT_SIZE;
pub const SIGNATURE_BYTES: usize = 64;
const HEADER_BYTES: usize = 4 + 1 + 1 + 4 + 2;

/// The only bundle version of Quantic, from block 0.
pub const fn bundle_version() -> BundleVersion {
    BundleVersion::ironwood_v3()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TxKind {
    /// Spends notes; `height` is the expiry height (0: no expiry).
    Transfer,
    /// First transaction of a block; `height` is the block height.
    Coinbase,
}

impl TxKind {
    fn to_byte(self) -> u8 {
        match self {
            TxKind::Transfer => 0,
            TxKind::Coinbase => 1,
        }
    }

    fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(TxKind::Transfer),
            1 => Some(TxKind::Coinbase),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TxId(pub [u8; 32]);

impl TxId {
    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }
}

impl fmt::Display for TxId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for TxId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TxId({})", self.to_hex())
    }
}

/// One action without its signature.
#[derive(Clone, Debug)]
pub struct ActionEffects {
    pub cv_net: ValueCommitment,
    pub nf: Nullifier,
    pub rk: redpallas::VerificationKey<SpendAuth>,
    pub cmx: ExtractedNoteCommitment,
    pub envelope: NoteEnvelope,
}

/// Everything the txid covers.
#[derive(Clone, Debug)]
pub struct TxEffects {
    pub network: Network,
    pub kind: TxKind,
    pub height: u32,
    pub actions: Vec<ActionEffects>,
    pub flags: Flags,
    pub value_balance: i64,
    pub anchor: Anchor,
}

/// The partial digests that, with the compact actions, give the txid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TxDigests {
    pub header: [u8; 32],
    pub compact: [u8; 32],
    pub memos: [u8; 32],
    pub noncompact: [u8; 32],
}

pub(crate) fn header_digest(network: Network, kind: TxKind, height: u32) -> [u8; 32] {
    blake2b_256(
        TX_HEADER,
        &[
            &TX_VERSION.to_le_bytes(),
            &[network.id(), kind.to_byte()],
            &height.to_le_bytes(),
        ],
    )
}

pub(crate) fn compact_digest<'a>(
    actions: impl Iterator<
        Item = (
            &'a Nullifier,
            &'a ExtractedNoteCommitment,
            &'a [u8],
            &'a [u8],
        ),
    >,
) -> [u8; 32] {
    let mut parts = Vec::new();
    for (nf, cmx, kem, enc_compact) in actions {
        parts.extend_from_slice(&nf.to_bytes());
        parts.extend_from_slice(&cmx.to_bytes());
        parts.extend_from_slice(kem);
        parts.extend_from_slice(enc_compact);
    }
    blake2b_256(ACTIONS_COMPACT, &[&parts])
}

pub(crate) fn txid_from_digests(
    digests: &TxDigests,
    flags_byte: u8,
    value_balance: i64,
    anchor: &Anchor,
) -> TxId {
    let effects = blake2b_256(
        BUNDLE_EFFECTS,
        &[
            &digests.compact,
            &digests.memos,
            &digests.noncompact,
            &[flags_byte],
            &value_balance.to_le_bytes(),
            &anchor.to_bytes(),
        ],
    );
    TxId(blake2b_256(TX_ID, &[&digests.header, &effects]))
}

impl TxEffects {
    pub fn flags_byte(&self) -> u8 {
        self.flags
            .to_byte(bundle_version())
            .expect("Ironwood represents every flag set")
    }

    pub fn digests(&self) -> TxDigests {
        let mut memos =
            Vec::with_capacity(self.actions.len() * (ENC_CIPHERTEXT_SIZE - COMPACT_NOTE_SIZE));
        let mut noncompact = Vec::with_capacity(self.actions.len() * (64 + OUT_CIPHERTEXT_SIZE));
        for action in &self.actions {
            memos.extend_from_slice(&action.envelope.enc_ciphertext()[COMPACT_NOTE_SIZE..]);
            noncompact.extend_from_slice(&action.cv_net.to_bytes());
            noncompact.extend_from_slice(&<[u8; 32]>::from(&action.rk));
            noncompact.extend_from_slice(action.envelope.out_ciphertext());
        }
        TxDigests {
            header: header_digest(self.network, self.kind, self.height),
            compact: compact_digest(self.actions.iter().map(|a| {
                (
                    &a.nf,
                    &a.cmx,
                    &a.envelope.kem_ciphertext()[..],
                    &a.envelope.enc_ciphertext_compact()[..],
                )
            })),
            memos: blake2b_256(ACTIONS_MEMOS, &[&memos]),
            noncompact: blake2b_256(ACTIONS_NONCOMPACT, &[&noncompact]),
        }
    }

    pub fn txid(&self) -> TxId {
        txid_from_digests(
            &self.digests(),
            self.flags_byte(),
            self.value_balance,
            &self.anchor,
        )
    }

    /// What every signature of the transaction signs.
    pub fn sighash(&self) -> [u8; 32] {
        self.txid().0
    }
}

#[derive(Clone, Debug)]
pub struct QuanticTransaction {
    effects: TxEffects,
    proof: Proof,
    spend_auth_sigs: Vec<redpallas::Signature<SpendAuth>>,
    binding_sig: redpallas::Signature<Binding>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    Truncated,
    TrailingBytes,
    UnsupportedVersion(u32),
    UnknownNetwork(u8),
    UnknownKind(u8),
    ActionCount(usize),
    InvalidValueCommitment,
    InvalidNullifier,
    InvalidRk,
    InvalidNoteCommitment,
    InvalidFlags,
    InvalidAnchor,
    ProofSize { expected: usize, actual: usize },
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::Truncated => f.write_str("transaction is truncated"),
            DecodeError::TrailingBytes => f.write_str("transaction has trailing bytes"),
            DecodeError::UnsupportedVersion(v) => write!(f, "unsupported transaction version {v}"),
            DecodeError::UnknownNetwork(n) => write!(f, "unknown network id {n}"),
            DecodeError::UnknownKind(k) => write!(f, "unknown transaction kind {k}"),
            DecodeError::ActionCount(n) => {
                write!(
                    f,
                    "a transaction needs 1 to {MAX_ACTIONS_PER_TX} actions, got {n}"
                )
            }
            DecodeError::InvalidValueCommitment => f.write_str("invalid value commitment"),
            DecodeError::InvalidNullifier => f.write_str("invalid nullifier"),
            DecodeError::InvalidRk => f.write_str("invalid or identity rk"),
            DecodeError::InvalidNoteCommitment => f.write_str("invalid note commitment"),
            DecodeError::InvalidFlags => f.write_str("invalid bundle flags"),
            DecodeError::InvalidAnchor => f.write_str("invalid anchor"),
            DecodeError::ProofSize { expected, actual } => {
                write!(f, "proof must have {expected} bytes, got {actual}")
            }
        }
    }
}

impl std::error::Error for DecodeError {}

struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        if self.bytes.len() < n {
            return Err(DecodeError::Truncated);
        }
        let (head, tail) = self.bytes.split_at(n);
        self.bytes = tail;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        Ok(self.take(N)?.try_into().expect("exact length"))
    }
}

fn placeholder_note_ciphertext() -> TransmittedNoteCiphertext {
    // `orchard::Action` insists on a valid Pallas point here. Quantic never
    // serializes, hashes or reads this field: the X-Wing envelope replaces it.
    TransmittedNoteCiphertext {
        epk_bytes: pallas::Point::generator().to_bytes(),
        enc_ciphertext: NoteBytesData([0u8; ENC_CIPHERTEXT_SIZE]),
        out_ciphertext: [0u8; 80],
    }
}

impl QuanticTransaction {
    pub(crate) fn from_parts(
        effects: TxEffects,
        proof: Proof,
        spend_auth_sigs: Vec<redpallas::Signature<SpendAuth>>,
        binding_sig: redpallas::Signature<Binding>,
    ) -> Self {
        debug_assert_eq!(effects.actions.len(), spend_auth_sigs.len());
        Self {
            effects,
            proof,
            spend_auth_sigs,
            binding_sig,
        }
    }

    pub fn effects(&self) -> &TxEffects {
        &self.effects
    }

    pub fn network(&self) -> Network {
        self.effects.network
    }

    pub fn kind(&self) -> TxKind {
        self.effects.kind
    }

    pub fn height(&self) -> u32 {
        self.effects.height
    }

    pub fn actions(&self) -> &[ActionEffects] {
        &self.effects.actions
    }

    pub fn value_balance(&self) -> i64 {
        self.effects.value_balance
    }

    pub fn anchor(&self) -> &Anchor {
        &self.effects.anchor
    }

    pub fn flags(&self) -> &Flags {
        &self.effects.flags
    }

    pub fn proof(&self) -> &Proof {
        &self.proof
    }

    pub fn txid(&self) -> TxId {
        self.effects.txid()
    }

    pub fn auth_digest(&self) -> [u8; 32] {
        let mut sigs = Vec::with_capacity(self.spend_auth_sigs.len() * SIGNATURE_BYTES);
        for sig in &self.spend_auth_sigs {
            sigs.extend_from_slice(&<[u8; 64]>::from(sig));
        }
        blake2b_256(
            TX_AUTH,
            &[
                self.proof.as_ref(),
                &sigs,
                &<[u8; 64]>::from(&self.binding_sig),
            ],
        )
    }

    /// The fee of a transfer (its value balance); `None` for a coinbase.
    pub fn fee(&self) -> Option<u64> {
        match self.effects.kind {
            TxKind::Transfer => u64::try_from(self.effects.value_balance).ok(),
            TxKind::Coinbase => None,
        }
    }

    pub fn nullifiers(&self) -> impl Iterator<Item = &Nullifier> {
        self.effects.actions.iter().map(|a| &a.nf)
    }

    pub fn note_commitments(&self) -> impl Iterator<Item = &ExtractedNoteCommitment> {
        self.effects.actions.iter().map(|a| &a.cmx)
    }

    pub fn serialized_size(&self) -> usize {
        HEADER_BYTES
            + self.effects.actions.len() * (ACTION_BYTES + SIGNATURE_BYTES)
            + 1
            + 8
            + 32
            + 4
            + self.proof.as_ref().len()
            + SIGNATURE_BYTES
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let e = &self.effects;
        let mut out = Vec::with_capacity(self.serialized_size());
        out.extend_from_slice(&TX_VERSION.to_le_bytes());
        out.push(e.network.id());
        out.push(e.kind.to_byte());
        out.extend_from_slice(&e.height.to_le_bytes());
        out.extend_from_slice(&(e.actions.len() as u16).to_le_bytes());
        for a in &e.actions {
            out.extend_from_slice(&a.cv_net.to_bytes());
            out.extend_from_slice(&a.nf.to_bytes());
            out.extend_from_slice(&<[u8; 32]>::from(&a.rk));
            out.extend_from_slice(&a.cmx.to_bytes());
            out.extend_from_slice(a.envelope.kem_ciphertext());
            out.extend_from_slice(a.envelope.enc_ciphertext());
            out.extend_from_slice(a.envelope.out_ciphertext());
        }
        out.push(e.flags_byte());
        out.extend_from_slice(&e.value_balance.to_le_bytes());
        out.extend_from_slice(&e.anchor.to_bytes());
        out.extend_from_slice(&(self.proof.as_ref().len() as u32).to_le_bytes());
        out.extend_from_slice(self.proof.as_ref());
        for sig in &self.spend_auth_sigs {
            out.extend_from_slice(&<[u8; 64]>::from(sig));
        }
        out.extend_from_slice(&<[u8; 64]>::from(&self.binding_sig));
        out
    }

    /// Strict decoding: exact sizes, canonical field and point encodings, the
    /// canonical proof length and no trailing bytes. It does not verify proofs,
    /// signatures or any rule that needs the chain.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader { bytes };
        let version = u32::from_le_bytes(r.array()?);
        if version != TX_VERSION {
            return Err(DecodeError::UnsupportedVersion(version));
        }
        let [network_id] = r.array()?;
        let network =
            Network::from_id(network_id).ok_or(DecodeError::UnknownNetwork(network_id))?;
        let [kind_byte] = r.array()?;
        let kind = TxKind::from_byte(kind_byte).ok_or(DecodeError::UnknownKind(kind_byte))?;
        let height = u32::from_le_bytes(r.array()?);
        let count = u16::from_le_bytes(r.array()?) as usize;
        if !(1..=MAX_ACTIONS_PER_TX).contains(&count) {
            return Err(DecodeError::ActionCount(count));
        }
        let mut actions = Vec::with_capacity(count);
        for _ in 0..count {
            let cv_net: Option<ValueCommitment> = ValueCommitment::from_bytes(&r.array()?).into();
            let nf: Option<Nullifier> = Nullifier::from_bytes(&r.array()?).into();
            let rk = redpallas::VerificationKey::<SpendAuth>::try_from(r.array::<32>()?)
                .map_err(|_| DecodeError::InvalidRk)?;
            if rk.is_identity() {
                return Err(DecodeError::InvalidRk);
            }
            let cmx: Option<ExtractedNoteCommitment> =
                ExtractedNoteCommitment::from_bytes(&r.array()?).into();
            let envelope = NoteEnvelope::from_parts(r.array()?, r.array()?, r.array()?);
            actions.push(ActionEffects {
                cv_net: cv_net.ok_or(DecodeError::InvalidValueCommitment)?,
                nf: nf.ok_or(DecodeError::InvalidNullifier)?,
                rk,
                cmx: cmx.ok_or(DecodeError::InvalidNoteCommitment)?,
                envelope,
            });
        }
        let [flags_byte] = r.array()?;
        let flags =
            Flags::from_byte(flags_byte, bundle_version()).ok_or(DecodeError::InvalidFlags)?;
        let value_balance = i64::from_le_bytes(r.array()?);
        let anchor: Option<Anchor> = Anchor::from_bytes(r.array()?).into();
        let anchor = anchor.ok_or(DecodeError::InvalidAnchor)?;
        let proof_len = u32::from_le_bytes(r.array()?) as usize;
        let expected = Proof::expected_proof_size(count);
        if proof_len != expected {
            return Err(DecodeError::ProofSize {
                expected,
                actual: proof_len,
            });
        }
        let proof = Proof::new(r.take(proof_len)?.to_vec());
        let mut spend_auth_sigs = Vec::with_capacity(count);
        for _ in 0..count {
            spend_auth_sigs.push(redpallas::Signature::<SpendAuth>::from(r.array::<64>()?));
        }
        let binding_sig = redpallas::Signature::<Binding>::from(r.array::<64>()?);
        if !r.bytes.is_empty() {
            return Err(DecodeError::TrailingBytes);
        }
        Ok(Self {
            effects: TxEffects {
                network,
                kind,
                height,
                actions,
                flags,
                value_balance,
                anchor,
            },
            proof,
            spend_auth_sigs,
            binding_sig,
        })
    }

    /// The same transaction as an `orchard` bundle, to verify its proof and
    /// signatures with the crate. The note ciphertexts are placeholders.
    pub(crate) fn to_orchard_bundle(&self) -> Bundle<Authorized, i64> {
        let actions: Vec<Action<redpallas::Signature<SpendAuth>>> = self
            .effects
            .actions
            .iter()
            .zip(&self.spend_auth_sigs)
            .map(|(a, sig)| {
                Action::from_parts(
                    a.nf,
                    a.rk.clone(),
                    a.cmx,
                    placeholder_note_ciphertext(),
                    a.cv_net.clone(),
                    sig.clone(),
                )
                .expect(
                    "rk was checked when decoding or building, and the placeholder epk is valid",
                )
            })
            .collect();
        Bundle::try_from_parts(
            NonEmpty::from_vec(actions).expect("a transaction has at least one action"),
            self.effects.flags,
            self.effects.value_balance,
            self.effects.anchor,
            Authorized::from_parts(self.proof.clone(), self.binding_sig.clone()),
            bundle_version(),
        )
        .expect("the proof size and flags were checked when decoding or building")
    }
}
