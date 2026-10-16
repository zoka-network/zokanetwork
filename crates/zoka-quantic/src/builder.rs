//! Building Quantic transactions with the `orchard` builder.
//!
//! The bundle is built through the crate's PCZT roles, which expose each
//! output's note (recipient, value, seed): that is what the X-Wing envelope
//! encrypts. Orchard's own note ciphertexts are produced by the builder but
//! never leave this function. Order of the steps:
//!
//! 1. `Builder::build_for_pczt` (actions shuffled and padded to 2 by the crate);
//! 2. one X-Wing envelope per action (padding outputs go to a throwaway key);
//! 3. txid of the effects = sighash;
//! 4. IO finalizer (signs the padding spends), proof, spend signatures,
//!    binding signature.

use std::fmt;

use orchard::builder::{Builder, BundleType};
use orchard::bundle::Flags;
use orchard::circuit::ProvingKey;
use orchard::keys::{OutgoingViewingKey, Scope, SpendAuthorizingKey};
use orchard::note::Rho;
use orchard::tree::MerklePath;
use orchard::value::NoteValue;
use orchard::{Anchor, Note, NoteVersion};
use rand_core::{CryptoRng, Rng};
use zoka_pq::XWingPublicKey;

use crate::address::QuanticAddress;
use crate::keys::QuanticSpendingKey;
use crate::network::Network;
use crate::note_encryption::{encrypt_dummy_note, encrypt_note, Memo, MEMO_SIZE, PUBLIC_OVK};
use crate::tx::{
    bundle_version, ActionEffects, QuanticTransaction, TxEffects, TxKind, MAX_ACTIONS_PER_TX,
};

/// A note of the wallet and its path to the anchor of the transaction.
pub struct SpendInput {
    pub note: Note,
    pub merkle_path: MerklePath,
}

#[derive(Clone)]
pub struct Payment {
    pub to: QuanticAddress,
    pub value: u64,
    pub memo: Memo,
}

impl Payment {
    pub fn new(to: QuanticAddress, value: u64) -> Self {
        Self {
            to,
            value,
            memo: [0u8; MEMO_SIZE],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildError {
    WrongNetwork,
    NothingToSend,
    InsufficientFunds {
        available: u64,
        needed: u64,
    },
    ValueOverflow,
    TooManyActions(usize),
    /// The `orchard` builder refused the inputs (wrong key, anchor mismatch...).
    Orchard(String),
    Proof(String),
    Signature(String),
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildError::WrongNetwork => f.write_str("recipient is on another network"),
            BuildError::NothingToSend => f.write_str("a transfer needs at least one payment"),
            BuildError::InsufficientFunds { available, needed } => {
                write!(
                    f,
                    "insufficient funds: {available} available, {needed} needed"
                )
            }
            BuildError::ValueOverflow => f.write_str("value overflow"),
            BuildError::TooManyActions(n) => {
                write!(
                    f,
                    "{n} actions; a transaction takes at most {MAX_ACTIONS_PER_TX}"
                )
            }
            BuildError::Orchard(e) => write!(f, "orchard builder: {e}"),
            BuildError::Proof(e) => write!(f, "proof: {e}"),
            BuildError::Signature(e) => write!(f, "signature: {e}"),
        }
    }
}

impl std::error::Error for BuildError {}

fn orchard_error(e: impl fmt::Debug) -> BuildError {
    BuildError::Orchard(format!("{e:?}"))
}

/// How to encrypt each output that was added to the builder, in order.
struct OutputPlan {
    xwing: XWingPublicKey,
    memo: Memo,
    ovk: Option<OutgoingViewingKey>,
}

/// Pays `payments` from `spends`; what is left after the fee goes back to the
/// wallet's change address. `anchor` must be the root the paths lead to.
#[allow(clippy::too_many_arguments)]
pub fn build_transfer<R: Rng + CryptoRng>(
    sk: &QuanticSpendingKey,
    spends: Vec<SpendInput>,
    payments: Vec<Payment>,
    fee: u64,
    anchor: Anchor,
    expiry_height: u32,
    pk: &ProvingKey,
    rng: &mut R,
) -> Result<QuanticTransaction, BuildError> {
    let network = sk.network();
    if payments.is_empty() {
        return Err(BuildError::NothingToSend);
    }
    if payments.iter().any(|p| p.to.network() != network) {
        return Err(BuildError::WrongNetwork);
    }
    let available = spends
        .iter()
        .try_fold(0u64, |sum, s| sum.checked_add(s.note.value().inner()))
        .ok_or(BuildError::ValueOverflow)?;
    let needed = payments
        .iter()
        .try_fold(fee, |sum, p| sum.checked_add(p.value))
        .ok_or(BuildError::ValueOverflow)?;
    if available < needed {
        return Err(BuildError::InsufficientFunds { available, needed });
    }
    let change = available - needed;

    let fvk = sk.full_viewing_key();
    let mut builder = Builder::new(
        BundleType::DEFAULT,
        bundle_version(),
        Flags::ENABLED,
        anchor,
    )
    .map_err(orchard_error)?;
    for spend in spends {
        builder
            .add_spend(fvk.orchard().clone(), spend.note, spend.merkle_path)
            .map_err(orchard_error)?;
    }
    let mut plans = Vec::with_capacity(payments.len() + 1);
    for payment in payments {
        builder
            .add_output(
                None,
                *payment.to.orchard(),
                NoteValue::from_raw(payment.value),
                payment.memo,
            )
            .map_err(orchard_error)?;
        plans.push(OutputPlan {
            xwing: payment.to.xwing().clone(),
            memo: payment.memo,
            ovk: Some(fvk.ovk(Scope::External)),
        });
    }
    if change > 0 {
        let to = fvk.change_address();
        builder
            .add_output(
                None,
                *to.orchard(),
                NoteValue::from_raw(change),
                [0u8; MEMO_SIZE],
            )
            .map_err(orchard_error)?;
        plans.push(OutputPlan {
            xwing: to.xwing().clone(),
            memo: [0u8; MEMO_SIZE],
            ovk: Some(fvk.ovk(Scope::Internal)),
        });
    }
    let (pczt, meta) = builder.build_for_pczt(&mut *rng).map_err(orchard_error)?;
    let ask = sk.spend_authorizing_key();
    finish(
        network,
        TxKind::Transfer,
        expiry_height,
        pczt,
        &meta,
        &plans,
        Some(&ask),
        pk,
        rng,
    )
}

/// The coinbase of block `height`: mints `value` to `to`. With `public`, the
/// output is encrypted for [`PUBLIC_OVK`] too, so anyone can check who
/// received how much (used for the premine).
pub fn build_coinbase<R: Rng + CryptoRng>(
    network: Network,
    height: u32,
    to: &QuanticAddress,
    value: u64,
    public: bool,
    pk: &ProvingKey,
    rng: &mut R,
) -> Result<QuanticTransaction, BuildError> {
    if to.network() != network {
        return Err(BuildError::WrongNetwork);
    }
    let mut builder = Builder::new(
        BundleType::Coinbase,
        bundle_version(),
        Flags::SPENDS_DISABLED,
        Anchor::empty_tree(),
    )
    .map_err(orchard_error)?;
    builder
        .add_output(
            None,
            *to.orchard(),
            NoteValue::from_raw(value),
            [0u8; MEMO_SIZE],
        )
        .map_err(orchard_error)?;
    let plans = [OutputPlan {
        xwing: to.xwing().clone(),
        memo: [0u8; MEMO_SIZE],
        ovk: public.then(|| OutgoingViewingKey::from(PUBLIC_OVK)),
    }];
    let (pczt, meta) = builder.build_for_pczt(&mut *rng).map_err(orchard_error)?;
    finish(
        network,
        TxKind::Coinbase,
        height,
        pczt,
        &meta,
        &plans,
        None,
        pk,
        rng,
    )
}

#[allow(clippy::too_many_arguments)]
fn finish<R: Rng + CryptoRng>(
    network: Network,
    kind: TxKind,
    height: u32,
    mut pczt: orchard::pczt::Bundle,
    meta: &orchard::builder::BundleMetadata,
    plans: &[OutputPlan],
    ask: Option<&SpendAuthorizingKey>,
    pk: &ProvingKey,
    rng: &mut R,
) -> Result<QuanticTransaction, BuildError> {
    let count = pczt.actions().len();
    if count > MAX_ACTIONS_PER_TX {
        return Err(BuildError::TooManyActions(count));
    }
    let mut plan_of_action: Vec<Option<&OutputPlan>> = vec![None; count];
    for (i, plan) in plans.iter().enumerate() {
        let index = meta
            .output_action_index(i)
            .ok_or_else(|| BuildError::Orchard("output missing from the bundle".into()))?;
        plan_of_action[index] = Some(plan);
    }

    let mut actions = Vec::with_capacity(count);
    for (index, action) in pczt.actions().iter().enumerate() {
        let output = action.output();
        let missing = || BuildError::Orchard("PCZT output without note data".into());
        let recipient = output.recipient().ok_or_else(missing)?;
        let value = output.value().ok_or_else(missing)?;
        let rseed = output.rseed().ok_or_else(missing)?;
        let nf = *action.spend().nullifier();
        let rho: Option<Rho> = Rho::from_bytes(&nf.to_bytes()).into();
        let rho = rho.ok_or_else(missing)?;
        let note: Option<Note> =
            Note::from_parts(recipient, value, rho, rseed, NoteVersion::V3).into();
        let note = note.ok_or_else(missing)?;
        let cmx = *output.cmx();
        if orchard::note::ExtractedNoteCommitment::from(note.commitment()).to_bytes()
            != cmx.to_bytes()
        {
            return Err(BuildError::Orchard(
                "note does not match its commitment".into(),
            ));
        }
        let cv_net = action.cv_net().clone();
        let envelope = match plan_of_action[index] {
            Some(plan) => encrypt_note(
                &plan.xwing,
                &note,
                &plan.memo,
                &cv_net,
                &cmx,
                plan.ovk.as_ref(),
                rng,
            ),
            None => encrypt_dummy_note(&note, &cv_net, &cmx, rng),
        };
        actions.push(ActionEffects {
            cv_net,
            nf,
            rk: action.spend().rk().clone(),
            cmx,
            envelope,
        });
    }

    let effects = TxEffects {
        network,
        kind,
        height,
        actions,
        flags: *pczt.flags(),
        value_balance: i64::try_from(*pczt.value_sum()).map_err(|_| BuildError::ValueOverflow)?,
        anchor: *pczt.anchor(),
    };
    let sighash = effects.sighash();

    pczt.finalize_io(sighash, &mut *rng)
        .map_err(|e| BuildError::Signature(format!("{e:?}")))?;
    pczt.create_proof(pk, &mut *rng)
        .map_err(|e| BuildError::Proof(format!("{e:?}")))?;
    for action in pczt.actions_mut() {
        if action.spend().spend_auth_sig().is_none() {
            let ask = ask.ok_or_else(|| BuildError::Signature("missing spending key".into()))?;
            action
                .sign(sighash, ask, &mut *rng)
                .map_err(|e| BuildError::Signature(format!("{e:?}")))?;
        }
    }
    let unbound = pczt
        .extract::<i64>()
        .map_err(|e| BuildError::Signature(format!("{e:?}")))?
        .ok_or_else(|| BuildError::Orchard("empty bundle".into()))?;
    let bundle = unbound
        .apply_binding_signature(sighash, &mut *rng)
        .ok_or_else(|| BuildError::Signature("a spend signature does not verify".into()))?;

    let sigs = bundle
        .actions()
        .iter()
        .zip(&effects.actions)
        .map(|(action, effects)| {
            debug_assert_eq!(action.nullifier().to_bytes(), effects.nf.to_bytes());
            action.authorization().clone()
        })
        .collect();
    Ok(QuanticTransaction::from_parts(
        effects,
        bundle.authorization().proof().clone(),
        sigs,
        bundle.authorization().binding_signature().clone(),
    ))
}
