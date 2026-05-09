use crate::crypto::{
    private_tree_commitment_words, private_tree_scalar_from_hex, private_tree_scalar_to_words,
};
/// Private Transaction Management for Users
/// Provides confidential transaction support with commitment/nullifier/ZK proofs
use crate::state::{
    build_private_commitment_inclusion_proofs, PrivateCommitmentInclusionProof, StateManager,
};
use crate::transaction::commitment::Commitment;
use crate::transaction::nullifier::{Nullifier, NullifierSet};
use crate::types::BlockError;
use crate::zk::groth16_backend::{
    PRIVATE_TX_MAX_INPUTS, PRIVATE_TX_MERKLE_MAX_DEPTH, PRIVATE_TX_MERKLE_PROVEN_INPUTS,
    PRIVATE_TX_ROOT_WORDS,
};
use crate::zk::prover::Prover;
use crate::zk::range_proofs::{
    amount_commitment_point as bulletproof_amount_commitment_point, pedersen_gens,
    prove_output_range, verify_output_range,
};
use crate::zk::verifier::Verifier;
use curve25519_dalek::constants::RISTRETTO_BASEPOINT_POINT;
use curve25519_dalek::ristretto::{CompressedRistretto, RistrettoPoint};
use curve25519_dalek::scalar::Scalar;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use zeroize::Zeroize;
mod clsag;

/// Helper function to convert bytes to hex string
fn bytes_to_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<String>()
}

fn first_u64_from_bytes(bytes: &[u8]) -> u64 {
    let mut arr = [0u8; 8];
    let copy_len = bytes.len().min(8);
    arr[..copy_len].copy_from_slice(&bytes[..copy_len]);
    u64::from_le_bytes(arr)
}

fn first_u64_from_hex(hex_str: &str) -> u64 {
    hex::decode(hex_str)
        .map(|bytes| first_u64_from_bytes(&bytes))
        .unwrap_or(0)
}

fn asset_tag_to_u64(asset_tag: &str) -> u64 {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(b"zoka-asset-tag");
    hasher.update([0x1f]);
    hasher.update(asset_tag.as_bytes());
    let digest = hasher.finalize();
    let mut word = [0u8; 8];
    word.copy_from_slice(&digest[..8]);
    let value = u64::from_le_bytes(word);
    if value == 0 {
        1
    } else {
        value
    }
}

fn encode_u64_words(values: &[u64]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 8);
    for v in values {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

fn fixed_u64_words_from_hex(hex_str: &str, width: usize) -> Vec<u64> {
    let mut words = Vec::with_capacity(width);
    let bytes = hex::decode(hex_str).unwrap_or_default();
    for idx in 0..width {
        let start = idx * 8;
        let end = (start + 8).min(bytes.len());
        let mut chunk = [0u8; 8];
        if start < bytes.len() {
            chunk[..(end - start)].copy_from_slice(&bytes[start..end]);
        }
        words.push(u64::from_le_bytes(chunk));
    }
    words
}

fn pad_u64s(values: &[u64], width: usize) -> Result<Vec<u64>, BlockError> {
    if values.len() > width {
        return Err(BlockError::TransactionValidationFailed(format!(
            "private transaction exceeds Groth16 max input width ({})",
            width
        )));
    }

    let mut padded = values.to_vec();
    padded.resize(width, 0);
    Ok(padded)
}

fn proof_public_inputs_for_tx(tx: &PrivateTransaction) -> Result<Vec<Vec<u8>>, BlockError> {
    if tx.proof_commitment == 0 && tx.proof_nullifier == 0 {
        return Ok(vec![]);
    }

    let padded_key_images = pad_u64s(
        &tx.input_key_images
            .iter()
            .map(|image| first_u64_from_hex(image))
            .collect::<Vec<_>>(),
        PRIVATE_TX_MAX_INPUTS,
    )?;

    let mut words = vec![tx.proof_commitment, tx.proof_nullifier];
    words.extend_from_slice(&padded_key_images);
    let active_proven_inputs = tx
        .input_commitments
        .len()
        .min(PRIVATE_TX_MERKLE_PROVEN_INPUTS);
    let mut proven_flags = vec![0u64; PRIVATE_TX_MERKLE_PROVEN_INPUTS];
    let mut proven_commitment_words =
        Vec::with_capacity(PRIVATE_TX_MERKLE_PROVEN_INPUTS * PRIVATE_TX_ROOT_WORDS);
    for proven_idx in 0..PRIVATE_TX_MERKLE_PROVEN_INPUTS {
        if proven_idx < active_proven_inputs {
            proven_flags[proven_idx] = 1;
            proven_commitment_words.extend_from_slice(&private_tree_commitment_words(
                &tx.input_commitments[proven_idx],
            ));
        } else {
            proven_commitment_words.extend_from_slice(&[0u64; PRIVATE_TX_ROOT_WORDS]);
        }
    }
    words.extend_from_slice(&proven_flags);
    words.extend_from_slice(&proven_commitment_words);
    let mut root_words = if tx.proof_private_root_words.is_empty() {
        fixed_u64_words_from_hex(&tx.private_commitment_root, PRIVATE_TX_ROOT_WORDS)
    } else {
        tx.proof_private_root_words.clone()
    };
    root_words.resize(PRIVATE_TX_ROOT_WORDS, 0);
    words.extend_from_slice(&root_words);
    Ok(words
        .into_iter()
        .map(|word| word.to_le_bytes().to_vec())
        .collect())
}

fn constant_time_eq_bytes(a: &[u8], b: &[u8]) -> bool {
    let mut diff = a.len() ^ b.len();
    let max_len = a.len().max(b.len());
    for i in 0..max_len {
        let lhs = *a.get(i).unwrap_or(&0);
        let rhs = *b.get(i).unwrap_or(&0);
        diff |= usize::from(lhs ^ rhs);
    }
    diff == 0
}

fn constant_time_eq_str(a: &str, b: &str) -> bool {
    constant_time_eq_bytes(a.as_bytes(), b.as_bytes())
}

fn scalar_from_secret_bytes(bytes: &[u8]) -> Scalar {
    Scalar::from_bytes_mod_order(signing_seed_from_bytes(bytes))
}

fn signing_seed_from_bytes(bytes: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};

    let digest = Sha256::digest(bytes);
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&digest[..32]);
    seed
}

fn hash_to_scalar(parts: &[&[u8]]) -> Scalar {
    use sha2::{Digest, Sha512};

    let mut hasher = Sha512::new();
    for part in parts {
        hasher.update(part);
        hasher.update([0x1f]);
    }
    let digest = hasher.finalize();
    let mut wide = [0u8; 64];
    wide.copy_from_slice(&digest);
    Scalar::from_bytes_mod_order_wide(&wide)
}

fn hash_to_point(parts: &[&[u8]]) -> RistrettoPoint {
    use sha2::{Digest, Sha512};

    let mut hasher = Sha512::new();
    for part in parts {
        hasher.update(part);
        hasher.update([0x1f]);
    }
    let digest = hasher.finalize();
    let mut wide = [0u8; 64];
    wide.copy_from_slice(&digest);
    RistrettoPoint::from_uniform_bytes(&wide)
}

fn point_to_hex(point: &RistrettoPoint) -> String {
    hex::encode(point.compress().to_bytes())
}

fn point_from_hex(hex_str: &str) -> Option<RistrettoPoint> {
    let bytes = hex::decode(hex_str).ok()?;
    if bytes.len() != 32 {
        return None;
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    CompressedRistretto(arr).decompress()
}

fn public_key_hex(secret_key: &[u8]) -> String {
    point_to_hex(&(scalar_from_secret_bytes(secret_key) * RISTRETTO_BASEPOINT_POINT))
}

fn anonymity_public_key_hex(spending_key: &[u8]) -> String {
    public_key_hex(spending_key)
}

fn scan_public_key_hex(scan_key: &[u8]) -> String {
    public_key_hex(scan_key)
}

fn payment_address_from_keys(scan_key: &[u8], spending_key: &[u8]) -> String {
    format!(
        "zpriv:{}:{}",
        scan_public_key_hex(scan_key),
        anonymity_public_key_hex(spending_key)
    )
}

pub fn derive_spending_key_from_recovery_seed(seed_entropy: &[u8]) -> Vec<u8> {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(b"zoka-private-wallet-spend-v1");
    hasher.update([0x1f]);
    hasher.update(seed_entropy);
    hasher.finalize().to_vec()
}

fn parse_payment_address(payment_address: &str) -> Option<(RistrettoPoint, RistrettoPoint)> {
    let parts = payment_address.split(':').collect::<Vec<_>>();
    if parts.len() != 3 || parts[0] != "zpriv" {
        return None;
    }
    let scan_public = point_from_hex(parts[1])?;
    let spend_public = point_from_hex(parts[2])?;
    Some((scan_public, spend_public))
}

fn deterministic_decoy_public_key(label: &[u8]) -> String {
    point_to_hex(&hash_to_point(&[b"zoka-decoy-public", label]))
}

fn ring_message(
    tx_hash: &str,
    commitment: &str,
    nullifier: &str,
    recipient_commitment: &str,
    ring_members: &[String],
    input_public_keys: &[String],
    input_key_images: &[String],
) -> Vec<u8> {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(tx_hash.as_bytes());
    hasher.update([0x1f]);
    hasher.update(commitment.as_bytes());
    hasher.update([0x1f]);
    hasher.update(nullifier.as_bytes());
    hasher.update([0x1f]);
    hasher.update(recipient_commitment.as_bytes());
    hasher.update([0x1f]);
    for member in ring_members {
        hasher.update(member.as_bytes());
        hasher.update([0x1e]);
    }
    for public_key in input_public_keys {
        hasher.update(public_key.as_bytes());
        hasher.update([0x1d]);
    }
    for key_image in input_key_images {
        hasher.update(key_image.as_bytes());
        hasher.update([0x1c]);
    }
    hasher.finalize().to_vec()
}

fn ring_id_from_members(ring_members: &[String]) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    for member in ring_members {
        hasher.update(member.as_bytes());
        hasher.update([0x1e]);
    }
    hex::encode(hasher.finalize())
}

fn amount_commitment_point(amount: u64, blind: Scalar) -> RistrettoPoint {
    bulletproof_amount_commitment_point(amount, blind)
}

fn amount_commitment_hex(amount: u64, blind: Scalar) -> String {
    point_to_hex(&amount_commitment_point(amount, blind))
}

fn amount_balance_commitment_hex(blind: Scalar) -> String {
    point_to_hex(&(blind * pedersen_gens().B_blinding))
}

fn fee_amount_commitment_point(fee: u64) -> RistrettoPoint {
    pedersen_gens().commit(Scalar::from(fee), Scalar::ZERO)
}

fn derive_decoy_ring_member(
    base_member: &str,
    tx_hash: &str,
    recipient_commitment: &str,
    ordinal: usize,
) -> RistrettoPoint {
    hash_to_point(&[
        b"zoka-ring-member",
        base_member.as_bytes(),
        tx_hash.as_bytes(),
        recipient_commitment.as_bytes(),
        &ordinal.to_le_bytes(),
    ])
}

fn derive_matrix_ring_member(
    base_member: &str,
    tx_hash: &str,
    recipient_commitment: &str,
    input_index: usize,
) -> RistrettoPoint {
    hash_to_point(&[
        b"zoka-ring-member-matrix",
        base_member.as_bytes(),
        tx_hash.as_bytes(),
        recipient_commitment.as_bytes(),
        &input_index.to_le_bytes(),
    ])
}

fn build_ring_matrix(
    ring_members: &[String],
    signer_index: usize,
    input_public_keys: &[String],
    tx_hash: &str,
    recipient_commitment: &str,
) -> Option<Vec<Vec<RistrettoPoint>>> {
    let base_points = ring_members
        .iter()
        .map(|member| point_from_hex(member))
        .collect::<Option<Vec<_>>>()?;
    let input_points = input_public_keys
        .iter()
        .map(|key| point_from_hex(key))
        .collect::<Option<Vec<_>>>()?;

    let mut matrix = vec![vec![RistrettoPoint::default(); input_points.len()]; base_points.len()];
    for (row_idx, base_point) in base_points.iter().enumerate() {
        for (input_idx, input_point) in input_points.iter().enumerate() {
            matrix[row_idx][input_idx] = if row_idx == signer_index {
                *input_point
            } else if input_idx == 0 {
                *base_point
            } else {
                derive_matrix_ring_member(
                    &ring_members[row_idx],
                    tx_hash,
                    recipient_commitment,
                    input_idx,
                )
            };
        }
    }
    Some(matrix)
}

fn sign_clsag_v1_bundle(
    ring_secrets: &[Scalar],
    signer_index: usize,
    ring_members: &[String],
    input_public_keys: &[String],
    input_key_images: &[String],
    tx_hash: &str,
    recipient_commitment: &str,
    message: &[u8],
) -> Vec<u8> {
    let ring_matrix = build_ring_matrix(
        ring_members,
        signer_index,
        input_public_keys,
        tx_hash,
        recipient_commitment,
    )
    .expect("ring matrix must be derivable for clsag-v1");
    let key_images = input_key_images
        .iter()
        .map(|key_image| point_from_hex(key_image))
        .collect::<Option<Vec<_>>>()
        .expect("input key images must be valid");

    let mut bundled = Vec::new();
    for input_idx in 0..ring_secrets.len() {
        let ring_points = ring_matrix
            .iter()
            .map(|row| row[input_idx])
            .collect::<Vec<_>>();
        let signature = clsag::sign_single_input(
            &ring_points,
            signer_index,
            ring_secrets[input_idx],
            key_images[input_idx],
            message,
        )
        .expect("clsag-v1 signing should produce signature");
        bundled.extend_from_slice(&signature);
    }
    bundled
}

fn verify_clsag_v1_bundle(
    ring_members: &[String],
    input_public_keys: &[String],
    input_key_images: &[String],
    tx_hash: &str,
    recipient_commitment: &str,
    message: &[u8],
    signature: &[u8],
) -> bool {
    if ring_members.len() < 2 || input_public_keys.is_empty() {
        return false;
    }
    if input_key_images.len() != input_public_keys.len() {
        return false;
    }
    let per_input_signature_len =
        match 64usize.checked_add(ring_members.len().checked_mul(32).unwrap_or(0)) {
            Some(v) => v,
            None => return false,
        };
    if signature.len()
        != input_public_keys
            .len()
            .saturating_mul(per_input_signature_len)
    {
        return false;
    }

    let signer_index = ring_members.iter().position(|member| {
        input_public_keys
            .first()
            .map(|primary| constant_time_eq_str(member, primary))
            .unwrap_or(false)
    });
    let Some(signer_index) = signer_index else {
        return false;
    };

    let Some(ring_matrix) = build_ring_matrix(
        ring_members,
        signer_index,
        input_public_keys,
        tx_hash,
        recipient_commitment,
    ) else {
        return false;
    };

    let Some(decoded_input_key_images) = input_key_images
        .iter()
        .map(|key_image| point_from_hex(key_image))
        .collect::<Option<Vec<_>>>()
    else {
        return false;
    };

    for input_idx in 0..input_public_keys.len() {
        let offset = input_idx * per_input_signature_len;
        let ring_points = ring_matrix
            .iter()
            .map(|row| row[input_idx])
            .collect::<Vec<_>>();
        if !clsag::verify_single_input(
            &ring_points,
            decoded_input_key_images[input_idx],
            message,
            &signature[offset..offset + per_input_signature_len],
        ) {
            return false;
        }
    }
    true
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum RingSignatureBackend {
    LegacyLinkable,
    #[serde(alias = "ClsagPlanned")]
    ClsagV1,
}

fn ring_version_for_backend(backend: RingSignatureBackend) -> u8 {
    let _ = backend;
    1
}

fn backend_for_ring_version(version: u8) -> Option<RingSignatureBackend> {
    match version {
        1 => Some(RingSignatureBackend::ClsagV1),
        _ => None,
    }
}

impl Default for RingSignatureBackend {
    fn default() -> Self {
        Self::ClsagV1
    }
}

fn sign_ring_signature_with_backend(
    backend: RingSignatureBackend,
    ring_secrets: &[Scalar],
    signer_index: usize,
    ring_members: &[String],
    input_public_keys: &[String],
    input_key_images: &[String],
    tx_hash: &str,
    recipient_commitment: &str,
    message: &[u8],
) -> Vec<u8> {
    match backend {
        RingSignatureBackend::LegacyLinkable => sign_clsag_v1_bundle(
            ring_secrets,
            signer_index,
            ring_members,
            input_public_keys,
            input_key_images,
            tx_hash,
            recipient_commitment,
            message,
        ),
        RingSignatureBackend::ClsagV1 => sign_clsag_v1_bundle(
            ring_secrets,
            signer_index,
            ring_members,
            input_public_keys,
            input_key_images,
            tx_hash,
            recipient_commitment,
            message,
        ),
    }
}

fn verify_ring_signature_with_backend(
    backend: RingSignatureBackend,
    ring_members: &[String],
    input_public_keys: &[String],
    input_key_images: &[String],
    tx_hash: &str,
    recipient_commitment: &str,
    message: &[u8],
    signature: &[u8],
) -> bool {
    match backend {
        RingSignatureBackend::LegacyLinkable => verify_clsag_v1_bundle(
            ring_members,
            input_public_keys,
            input_key_images,
            tx_hash,
            recipient_commitment,
            message,
            signature,
        ),
        RingSignatureBackend::ClsagV1 => verify_clsag_v1_bundle(
            ring_members,
            input_public_keys,
            input_key_images,
            tx_hash,
            recipient_commitment,
            message,
            signature,
        ),
    }
}

/// Recompute the one-time public key for an output, given the
/// recipient's payment address and the sender's ephemeral pubkey.
///
/// Previous versions accepted any `recipient_commitment` and silently
/// fell back to a `hash_to_point`-derived one-time key whenever the
/// string wasn't a `zpriv:<scan>:<spend>` address. That fallback
/// produced a non-recoverable stealth output (the recipient could
/// never derive the matching secret), so malformed addresses turned
/// into quiet burns. Now we reject the output at construction time.
fn derive_output_public_key(
    recipient_commitment: &str,
    commitment: &str,
    ephemeral_pubkey: &[u8],
) -> Result<String, BlockError> {
    let (scan_public, spend_public) =
        parse_payment_address(recipient_commitment).ok_or_else(|| {
            BlockError::TransactionValidationFailed(format!(
                "recipient payment address is not a valid zpriv stealth address: {}",
                recipient_commitment
            ))
        })?;
    let shared_point = hash_to_point(&[
        b"zoka-stealth-shared",
        ephemeral_pubkey,
        scan_public.compress().as_bytes(),
    ]);
    let derived_scalar = hash_to_scalar(&[
        b"zoka-output-key",
        shared_point.compress().as_bytes(),
        commitment.as_bytes(),
    ]);
    Ok(point_to_hex(
        &(spend_public + derived_scalar * RISTRETTO_BASEPOINT_POINT),
    ))
}

/// Recover the spending scalar for an output this wallet owns.
///
/// Requires a valid Ristretto `ephemeral_pubkey` on the output. The
/// previous hash-based fallback is gone: if the ephemeral pubkey is
/// malformed the output was not produced by a compliant sender and
/// we refuse to derive a scalar rather than silently burning funds.
fn derive_output_secret_scalar(
    spending_key: &[u8],
    scan_key: &[u8],
    output: &PrivateOutput,
) -> Result<Scalar, BlockError> {
    let spend_secret = scalar_from_secret_bytes(spending_key);
    let ephemeral_bytes: [u8; 32] =
        output.ephemeral_pubkey.as_slice().try_into().map_err(|_| {
            BlockError::TransactionValidationFailed(format!(
                "output {}: ephemeral pubkey must be 32 bytes, got {}",
                output.output_id,
                output.ephemeral_pubkey.len()
            ))
        })?;
    let ephemeral_public = CompressedRistretto(ephemeral_bytes)
        .decompress()
        .ok_or_else(|| {
            BlockError::TransactionValidationFailed(format!(
                "output {}: ephemeral pubkey is not a valid Ristretto point",
                output.output_id
            ))
        })?;
    let shared_point = scalar_from_secret_bytes(scan_key) * ephemeral_public;
    let derived_scalar = hash_to_scalar(&[
        b"zoka-output-key",
        shared_point.compress().as_bytes(),
        output.commitment.as_bytes(),
    ]);
    Ok(spend_secret + derived_scalar)
}

fn derive_key_image_hex(public_key: &RistrettoPoint, secret_scalar: Scalar) -> String {
    let hash_point = hash_to_point(&[b"zoka-key-image", public_key.compress().as_bytes()]);
    point_to_hex(&(secret_scalar * hash_point))
}

struct OutputStealthMaterial {
    stealth_address: String,
    ephemeral_pubkey: Vec<u8>,
    one_time_public_key: String,
    amount_blind: Scalar,
}

fn amount_blind_from_shared(shared_bytes: &[u8], commitment: &str) -> Scalar {
    hash_to_scalar(&[b"zoka-amount-blind", shared_bytes, commitment.as_bytes()])
}

fn derive_output_amount_blind_from_note(scan_key: &[u8], output: &PrivateOutput) -> Option<Scalar> {
    let ephemeral_public = output
        .ephemeral_pubkey
        .as_slice()
        .try_into()
        .ok()
        .and_then(|bytes: [u8; 32]| CompressedRistretto(bytes).decompress())?;
    let shared_point = scalar_from_secret_bytes(scan_key) * ephemeral_public;
    Some(amount_blind_from_shared(
        shared_point.compress().as_bytes(),
        &output.commitment,
    ))
}

/// Produce the ephemeral keypair, one-time address and amount-blind for a
/// stealth output to `recipient_commitment`.
///
/// Fails fast when the recipient is not a valid `zpriv:<scan>:<spend>`
/// address. The previous code fell back to a deterministic hash-derived
/// one-time key for malformed recipients — nobody could ever spend those
/// outputs, so they were effectively a burn path triggered by a typo.
fn derive_stealth_payload(
    recipient_commitment: &str,
    commitment: &str,
) -> Result<OutputStealthMaterial, BlockError> {
    use rand::RngCore;
    use sha2::{Digest, Sha256};

    let (scan_public, spend_public) =
        parse_payment_address(recipient_commitment).ok_or_else(|| {
            BlockError::TransactionValidationFailed(format!(
                "recipient payment address is not a valid zpriv stealth address: {}",
                recipient_commitment
            ))
        })?;

    let mut ephemeral_secret = [0u8; 64];
    rand::rngs::OsRng.fill_bytes(&mut ephemeral_secret);
    let ephemeral_scalar = Scalar::from_bytes_mod_order_wide(&ephemeral_secret);
    let ephemeral_pubkey = (ephemeral_scalar * RISTRETTO_BASEPOINT_POINT)
        .compress()
        .to_bytes()
        .to_vec();

    let shared_point = ephemeral_scalar * scan_public;
    let derived_scalar = hash_to_scalar(&[
        b"zoka-output-key",
        shared_point.compress().as_bytes(),
        commitment.as_bytes(),
    ]);
    let one_time_public_key =
        point_to_hex(&(spend_public + derived_scalar * RISTRETTO_BASEPOINT_POINT));
    let amount_blind = amount_blind_from_shared(shared_point.compress().as_bytes(), commitment);

    let mut stealth_hasher = Sha256::new();
    stealth_hasher.update(&ephemeral_pubkey);
    stealth_hasher.update(recipient_commitment.as_bytes());
    stealth_hasher.update(commitment.as_bytes());
    stealth_hasher.update(one_time_public_key.as_bytes());
    let stealth_digest = stealth_hasher.finalize();
    let stealth_address = format!("stlth_{}", hex::encode(&stealth_digest[..20]));
    ephemeral_secret.zeroize();

    Ok(OutputStealthMaterial {
        stealth_address,
        ephemeral_pubkey,
        one_time_public_key,
        amount_blind,
    })
}

fn private_tx_hash(
    input_nullifiers: &[String],
    input_commitments: &[String],
    input_asset_tags: &[String],
    private_commitment_root: &str,
    input_public_keys: &[String],
    input_key_images: &[String],
    input_amount_commitments: &[String],
    amount_balance_commitment: &str,
    fee_asset_tag: &str,
    asset_balance_commitments: &[AssetBalanceCommitment],
    fee_commitment: &str,
    outputs: &[PrivateOutput],
) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    for nullifier in input_nullifiers {
        hasher.update(nullifier.as_bytes());
        hasher.update([0x1f]);
    }
    for commitment in input_commitments {
        hasher.update(commitment.as_bytes());
        hasher.update([0x1f]);
    }
    for asset_tag in input_asset_tags {
        hasher.update(asset_tag.as_bytes());
        hasher.update([0x1f]);
    }
    hasher.update(private_commitment_root.as_bytes());
    hasher.update([0x1f]);
    for public_key in input_public_keys {
        hasher.update(public_key.as_bytes());
        hasher.update([0x1f]);
    }
    for key_image in input_key_images {
        hasher.update(key_image.as_bytes());
        hasher.update([0x1f]);
    }
    for amount_commitment in input_amount_commitments {
        hasher.update(amount_commitment.as_bytes());
        hasher.update([0x1f]);
    }
    hasher.update(amount_balance_commitment.as_bytes());
    hasher.update([0x1f]);
    hasher.update(fee_asset_tag.as_bytes());
    hasher.update([0x1f]);
    for commitment in asset_balance_commitments {
        hasher.update(commitment.asset_tag.as_bytes());
        hasher.update([0x1f]);
        hasher.update(commitment.balance_commitment.as_bytes());
        hasher.update([0x1f]);
    }
    hasher.update(fee_commitment.as_bytes());
    hasher.update([0x1f]);
    for output in outputs {
        hasher.update(output.asset_tag.as_bytes());
        hasher.update([0x1f]);
        hasher.update(output.asset_tag_commitment.as_bytes());
        hasher.update([0x1f]);
        hasher.update(output.recipient_commitment.as_bytes());
        hasher.update([0x1f]);
        hasher.update(output.commitment.as_bytes());
        hasher.update([0x1f]);
        hasher.update(output.amount_commitment.as_bytes());
        hasher.update([0x1f]);
        hasher.update(&output.range_proof);
        hasher.update([0x1f]);
        hasher.update(&output.ephemeral_pubkey);
        hasher.update([0x1f]);
        hasher.update(output.one_time_public_key.as_bytes());
        hasher.update([0x1f]);
        hasher.update(&output.encrypted_amount);
        hasher.update([u8::from(output.is_change)]);
    }
    hex::encode(&hasher.finalize()[..16])
}

fn derive_output_id(
    tx_hash: &str,
    ordinal: usize,
    commitment: &str,
    recipient_commitment: &str,
) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(b"zoka-private-output");
    hasher.update(tx_hash.as_bytes());
    hasher.update([0x1f]);
    hasher.update(&ordinal.to_le_bytes());
    hasher.update([0x1f]);
    hasher.update(commitment.as_bytes());
    hasher.update([0x1f]);
    hasher.update(recipient_commitment.as_bytes());
    hex::encode(hasher.finalize())
}

fn derive_note_nullifier(spending_key: &[u8], note_id: &str, commitment: &str) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(b"zoka-note-nullifier");
    hasher.update(spending_key);
    hasher.update([0x1f]);
    hasher.update(note_id.as_bytes());
    hasher.update([0x1f]);
    hasher.update(commitment.as_bytes());
    hex::encode(hasher.finalize())
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PrivateOutput {
    #[serde(default)]
    pub output_id: String,
    #[serde(default = "default_asset_tag")]
    pub asset_tag: String,
    #[serde(default)]
    pub asset_tag_commitment: String,
    #[serde(default)]
    pub recipient_commitment: String,
    #[serde(default)]
    pub stealth_address: String,
    #[serde(default)]
    pub ephemeral_pubkey: Vec<u8>,
    #[serde(default)]
    pub commitment: String,
    #[serde(default)]
    pub amount_commitment: String,
    #[serde(default)]
    pub range_proof: Vec<u8>,
    #[serde(default)]
    pub one_time_public_key: String,
    #[serde(default)]
    pub encrypted_amount: Vec<u8>,
    #[serde(default)]
    pub commitment_blind: Vec<u8>,
    #[serde(default)]
    pub is_change: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AssetBalanceCommitment {
    #[serde(default = "default_asset_tag")]
    pub asset_tag: String,
    #[serde(default)]
    pub balance_commitment: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PrivateNote {
    pub note_id: String,
    #[serde(default = "default_asset_tag")]
    pub asset_tag: String,
    pub source_tx_hash: String,
    pub recipient_commitment: String,
    pub stealth_address: String,
    pub ephemeral_pubkey: Vec<u8>,
    pub commitment: String,
    #[serde(default)]
    pub amount_commitment: String,
    #[serde(default)]
    pub range_proof: Vec<u8>,
    pub one_time_public_key: String,
    pub commitment_blind: Vec<u8>,
    pub amount: u64,
    #[serde(default)]
    pub is_change: bool,
    #[serde(default)]
    pub spent: bool,
    pub created_at: u64,
    #[serde(default)]
    pub spent_at: Option<u64>,
}

#[derive(Clone, Debug)]
struct PlannedPrivateOutput {
    asset_tag: String,
    recipient_commitment: String,
    amount: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PrivateFeeOpening {
    pub fee: u64,
    /// Scalar bytes for `r_fee` in `fee_commitment = fee·G + r_fee·H`.
    pub fee_blind: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PrivateFeeClaimEnvelope {
    pub tx_hash: String,
    pub ephemeral_pubkey: Vec<u8>,
    pub ciphertext: Vec<u8>,
}

impl From<PrivateFeeClaimEnvelope> for crate::types::PrivateFeeClaim {
    fn from(envelope: PrivateFeeClaimEnvelope) -> Self {
        crate::types::PrivateFeeClaim {
            tx_hash: envelope.tx_hash,
            ephemeral_pubkey: envelope.ephemeral_pubkey,
            ciphertext: envelope.ciphertext,
        }
    }
}

impl From<&crate::types::PrivateFeeClaim> for PrivateFeeClaimEnvelope {
    fn from(claim: &crate::types::PrivateFeeClaim) -> Self {
        PrivateFeeClaimEnvelope {
            tx_hash: claim.tx_hash.clone(),
            ephemeral_pubkey: claim.ephemeral_pubkey.clone(),
            ciphertext: claim.ciphertext.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PrivateFeeClaimOutcome {
    pub wallet: String,
    pub total_fee_claimed: u64,
    pub claimed_tx_hashes: Vec<String>,
    pub skipped_tx_hashes: Vec<String>,
}

fn legacy_output_from_tx(tx: &PrivateTransaction) -> Option<PrivateOutput> {
    if tx.commitment.is_empty() {
        return None;
    }

    // Legacy single-output tx: skip if the recipient isn't a valid zpriv
    // address. The rest of the pipeline can cope with zero legacy outputs;
    // it would have rejected a non-stealth output later anyway.
    let one_time_public_key = derive_output_public_key(
        &tx.recipient_commitment,
        &tx.commitment,
        &tx.ephemeral_pubkey,
    )
    .ok()?;

    Some(PrivateOutput {
        output_id: derive_output_id(&tx.tx_hash, 0, &tx.commitment, &tx.recipient_commitment),
        asset_tag: default_asset_tag(),
        asset_tag_commitment: asset_tag_commitment_hex(&default_asset_tag()),
        recipient_commitment: tx.recipient_commitment.clone(),
        stealth_address: tx.stealth_address.clone(),
        ephemeral_pubkey: tx.ephemeral_pubkey.clone(),
        commitment: tx.commitment.clone(),
        amount_commitment: String::new(),
        range_proof: Vec::new(),
        one_time_public_key,
        encrypted_amount: tx.encrypted_amount.clone(),
        commitment_blind: tx.commitment_blind.clone(),
        is_change: false,
    })
}

fn tx_outputs(tx: &PrivateTransaction) -> Vec<PrivateOutput> {
    if tx.outputs.is_empty() {
        legacy_output_from_tx(tx).into_iter().collect()
    } else {
        tx.outputs.clone()
    }
}

fn tx_input_nullifiers(tx: &PrivateTransaction) -> Vec<String> {
    if tx.input_nullifiers.is_empty() {
        if tx.nullifier.is_empty() {
            vec![]
        } else {
            vec![tx.nullifier.clone()]
        }
    } else {
        tx.input_nullifiers.clone()
    }
}

fn tx_ring_domain(tx: &PrivateTransaction) -> String {
    if !tx.recipient_commitment.is_empty() {
        return tx.recipient_commitment.clone();
    }
    tx.outputs
        .first()
        .map(|output| output.one_time_public_key.clone())
        .filter(|key| !key.is_empty())
        .unwrap_or_else(|| tx.stealth_address.clone())
}

fn default_ring_version() -> u8 {
    1
}

fn default_asset_tag() -> String {
    "ZOKA".to_string()
}

fn asset_tag_commitment_hex(asset_tag: &str) -> String {
    point_to_hex(&hash_to_point(&[
        b"zoka-asset-tag-commitment",
        asset_tag.as_bytes(),
    ]))
}

/// Private transaction for confidential transfers
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PrivateTransaction {
    pub tx_hash: String,
    pub commitment: String, // Pedersen commitment of amount
    #[serde(default)]
    pub recipient_commitment: String, // Receiver commitment identifier
    #[serde(default)]
    pub stealth_address: String, // One-time destination tag
    #[serde(default)]
    pub ephemeral_pubkey: Vec<u8>, // Sender ephemeral pubkey for stealth addressing
    #[serde(default)]
    pub encrypted_amount: Vec<u8>, // Obfuscated amount payload for receiver
    #[serde(default)]
    pub proof_commitment: u64, // Groth16 public input #1
    #[serde(default)]
    pub proof_nullifier: u64, // Groth16 public input #2
    #[serde(default)]
    pub proof_input_sum: u64, // Legacy public anchor retained for compatibility; new proofs keep this at 0
    #[serde(default)]
    pub proof_output_sum: u64, // Legacy public anchor retained for compatibility; new proofs keep this at 0
    #[serde(default)]
    pub proof_fee: u64, // Legacy public anchor retained for compatibility; new proofs keep this at 0
    #[serde(default)]
    pub proof_key_image: u64, // Legacy public anchor retained for compatibility; new proofs keep this at 0
    #[serde(default)]
    pub proof_private_root_words: Vec<u64>, // Groth16 public inputs for private root anchor
    #[serde(default)]
    pub fee: u64,
    #[serde(default)]
    pub ring_id: String, // Digest of the sender anonymity set
    #[serde(default = "default_ring_version")]
    pub ring_version: u8, // 1=ClsagV1
    #[serde(default)]
    pub ring_members: Vec<String>, // Anonymity set public keys
    #[serde(default)]
    pub ring_signature: Vec<u8>, // Linkable ring authorization signature
    #[serde(default)]
    pub input_nullifiers: Vec<String>, // Spend tags for each consumed note
    #[serde(default)]
    pub input_commitments: Vec<String>, // Commitments of consumed notes
    #[serde(default)]
    pub input_asset_tags: Vec<String>,
    #[serde(default)]
    pub input_public_keys: Vec<String>, // One-time public keys of consumed notes
    #[serde(default)]
    pub input_key_images: Vec<String>, // Linkable key images derived from spent notes
    #[serde(default)]
    pub input_amount_commitments: Vec<String>, // Algebraic amount commitments for confidential balance checks
    #[serde(default)]
    pub amount_balance_commitment: String, // Blinding excess commitment for public amount-balance verification
    #[serde(default = "default_asset_tag")]
    pub fee_asset_tag: String,
    #[serde(default)]
    pub asset_balance_commitments: Vec<AssetBalanceCommitment>,
    #[serde(default)]
    pub fee_commitment: String, // Pedersen commitment `fee·G + fee_blind·H` hiding the fee amount.
    // Empty string = legacy path where `fee` is visible (used only for
    // backwards compatibility with pre-private-fees txs and zero-fee
    // test fixtures). Non-empty enforces a Bulletproof range check and
    // participates in the on-chain balance equation.
    #[serde(default)]
    pub fee_range_proof: Vec<u8>, // Bulletproof over `fee_commitment`, 32-bit bound. Required when
    // `fee_commitment` is non-empty.
    #[serde(default)]
    pub private_commitment_root: String, // State root used for spent note inclusion
    #[serde(default)]
    pub input_inclusion_proofs: Vec<PrivateCommitmentInclusionProof>, // Inclusion proofs for spent inputs
    #[serde(default)]
    pub outputs: Vec<PrivateOutput>, // Explicit one-time outputs carried by the tx
    pub nullifier: String,         // Proof of spend without revealing which
    pub zk_proof: Option<Vec<u8>>, // Optional ZK-SNARK proof
    pub timestamp: u64,
    pub status: PrivacyStatus,
    #[serde(default)]
    pub commitment_blind: Vec<u8>, // Blindness factor for commitment (needed by receiver)
    #[serde(default)]
    pub nullifier_nonce: Vec<u8>, // Random nonce used in nullifier derivation
    /// Sender-local opening `(fee, fee_blind)` used to build encrypted miner
    /// claim envelopes after finality. Never serialized to network/storage.
    #[serde(skip)]
    pub fee_opening: Option<PrivateFeeOpening>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum PrivacyStatus {
    Pending,   // Waiting for inclusion
    Verified,  // ZK proof verified
    Confirmed, // Included in block
    Failed,    // Validation failed
}

impl std::fmt::Display for PrivacyStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PrivacyStatus::Pending => write!(f, "pending (private)"),
            PrivacyStatus::Verified => write!(f, "verified"),
            PrivacyStatus::Confirmed => write!(f, "confirmed (private)"),
            PrivacyStatus::Failed => write!(f, "failed"),
        }
    }
}

/// Private transaction with zero-knowledge proof generation
pub struct PrivateTransactionBuilder {
    amount: u64,
    fee: u64,
    ring_backend: RingSignatureBackend,
    recipient_commitment: Option<String>,
    change_output: Option<PlannedPrivateOutput>,
    private_commitment_root: String,
    spending_key: Vec<u8>,
    ring_members: Vec<String>,
    input_commitments: Vec<String>,
    input_amounts: Vec<u64>,
    input_asset_tags: Vec<u64>,
    input_nullifiers: Vec<String>,
    input_public_keys: Vec<String>,
    input_key_images: Vec<String>,
    input_amount_commitments: Vec<String>,
    input_inclusion_proofs: Vec<PrivateCommitmentInclusionProof>,
    input_secret_keys: Vec<Scalar>,
    input_amount_blinds: Vec<Scalar>,
    prover: Prover,
    nullifier_set: Arc<Mutex<NullifierSet>>,
}

const PRIVATE_NOTE_PAYLOAD_V2_LEN: usize = 40;

/// Legacy amount obfuscation retained only so already-created local txs can
/// still be imported. New transactions use `encrypt_private_note_payload_v2`
/// below because this legacy stream was keyed by public tx metadata.
fn xor_amount_with_stream(
    amount: u64,
    commitment_blind: &[u8],
    recipient_commitment: &str,
    nullifier: &str,
) -> Vec<u8> {
    let mut amount_bytes = amount.to_le_bytes();
    let mut stream = [0u8; 8];
    let mut hasher = blake3::Hasher::new();
    hasher.update(commitment_blind);
    hasher.update(b":");
    hasher.update(recipient_commitment.as_bytes());
    hasher.update(b":");
    hasher.update(nullifier.as_bytes());
    let digest = hasher.finalize();
    stream.copy_from_slice(&digest.as_bytes()[..8]);

    for i in 0..8 {
        amount_bytes[i] ^= stream[i];
    }
    amount_bytes.to_vec()
}

fn decode_amount_from_payload(
    payload: &[u8],
    commitment_blind: &[u8],
    recipient_commitment: &str,
    nullifier: &str,
) -> Result<u64, BlockError> {
    if payload.len() != 8 {
        return Err(BlockError::TransactionValidationFailed(
            "Invalid encrypted amount payload".to_string(),
        ));
    }
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(payload);

    let mut stream = [0u8; 8];
    let mut hasher = blake3::Hasher::new();
    hasher.update(commitment_blind);
    hasher.update(b":");
    hasher.update(recipient_commitment.as_bytes());
    hasher.update(b":");
    hasher.update(nullifier.as_bytes());
    let digest = hasher.finalize();
    stream.copy_from_slice(&digest.as_bytes()[..8]);

    for i in 0..8 {
        bytes[i] ^= stream[i];
    }

    Ok(u64::from_le_bytes(bytes))
}

fn private_note_payload_key(
    amount_blind: Scalar,
    commitment: &str,
    nullifier_domain: &str,
) -> Vec<u8> {
    let mut key = amount_blind.to_bytes().to_vec();
    key.extend_from_slice(commitment.as_bytes());
    key.extend_from_slice(nullifier_domain.as_bytes());
    key
}

fn encrypt_private_note_payload_v2(
    amount: u64,
    commitment_blind: &[u8],
    amount_blind: Scalar,
    commitment: &str,
    nullifier_domain: &str,
) -> Result<Vec<u8>, BlockError> {
    if commitment_blind.len() != 32 {
        return Err(BlockError::TransactionValidationFailed(
            "private note commitment blind must be 32 bytes".to_string(),
        ));
    }
    let mut payload = Vec::with_capacity(PRIVATE_NOTE_PAYLOAD_V2_LEN);
    payload.extend_from_slice(&amount.to_le_bytes());
    payload.extend_from_slice(commitment_blind);
    let key = private_note_payload_key(amount_blind, commitment, nullifier_domain);
    Ok(xor_with_blake3_stream(
        &payload,
        &key,
        b"zoka-private-note-payload-v2",
    ))
}

fn decrypt_private_note_payload_v2(
    payload: &[u8],
    amount_blind: Scalar,
    commitment: &str,
    nullifier_domain: &str,
) -> Result<(u64, Vec<u8>), BlockError> {
    if payload.len() != PRIVATE_NOTE_PAYLOAD_V2_LEN {
        return Err(BlockError::TransactionValidationFailed(
            "Invalid private note payload length".to_string(),
        ));
    }
    let key = private_note_payload_key(amount_blind, commitment, nullifier_domain);
    let plaintext = xor_with_blake3_stream(payload, &key, b"zoka-private-note-payload-v2");
    let mut amount_bytes = [0u8; 8];
    amount_bytes.copy_from_slice(&plaintext[..8]);
    let commitment_blind = plaintext[8..].to_vec();
    Ok((u64::from_le_bytes(amount_bytes), commitment_blind))
}

fn coinbase_amount_domain(block_height: u64, recipient_address: &str) -> String {
    format!("coinbase-v1:{block_height}:{recipient_address}")
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PrivateFeeClaimPayload {
    fee: u64,
    fee_blind: Vec<u8>,
}

fn xor_with_blake3_stream(data: &[u8], key_material: &[u8], domain: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; data.len()];
    let mut offset = 0usize;
    let mut counter = 0u64;

    while offset < data.len() {
        let mut hasher = blake3::Hasher::new();
        hasher.update(domain);
        hasher.update(key_material);
        hasher.update(&counter.to_le_bytes());
        let block = hasher.finalize();
        let block_bytes = block.as_bytes();
        let take = (data.len() - offset).min(block_bytes.len());
        for i in 0..take {
            out[offset + i] = data[offset + i] ^ block_bytes[i];
        }
        offset += take;
        counter = counter.saturating_add(1);
    }

    out
}

fn fee_claim_shared_secret_from_sender(
    miner_commitment_key: &str,
) -> Result<(Scalar, Vec<u8>), BlockError> {
    use rand::RngCore;

    let (miner_scan_public, _) = parse_payment_address(miner_commitment_key).ok_or_else(|| {
        BlockError::TransactionValidationFailed(format!(
            "miner commitment key is not a valid zpriv stealth address: {}",
            miner_commitment_key
        ))
    })?;

    let mut ephemeral_secret = [0u8; 64];
    rand::rngs::OsRng.fill_bytes(&mut ephemeral_secret);
    let ephemeral_scalar = Scalar::from_bytes_mod_order_wide(&ephemeral_secret);
    ephemeral_secret.zeroize();
    let ephemeral_pubkey = (ephemeral_scalar * RISTRETTO_BASEPOINT_POINT)
        .compress()
        .to_bytes()
        .to_vec();
    let shared = ephemeral_scalar * miner_scan_public;
    Ok((
        scalar_from_secret_bytes(&shared.compress().to_bytes()),
        ephemeral_pubkey,
    ))
}

fn fee_claim_shared_secret_from_receiver(
    miner_scan_key: &[u8],
    ephemeral_pubkey: &[u8],
) -> Result<Scalar, BlockError> {
    let ephemeral_bytes: [u8; 32] = ephemeral_pubkey.try_into().map_err(|_| {
        BlockError::TransactionValidationFailed(
            "fee claim ephemeral pubkey has invalid length".to_string(),
        )
    })?;
    let ephemeral_point = CompressedRistretto(ephemeral_bytes)
        .decompress()
        .ok_or_else(|| {
            BlockError::TransactionValidationFailed(
                "fee claim ephemeral pubkey is not a valid Ristretto point".to_string(),
            )
        })?;
    let shared = scalar_from_secret_bytes(miner_scan_key) * ephemeral_point;
    Ok(scalar_from_secret_bytes(&shared.compress().to_bytes()))
}

impl PrivateTransactionBuilder {
    /// Create new private transaction builder
    pub fn new(
        amount: u64,
        spending_key: Vec<u8>,
        nullifier_set: Arc<Mutex<NullifierSet>>,
    ) -> Self {
        PrivateTransactionBuilder {
            amount,
            fee: 0,
            ring_backend: RingSignatureBackend::default(),
            recipient_commitment: None,
            change_output: None,
            private_commitment_root: String::new(),
            spending_key,
            ring_members: Vec::new(),
            input_commitments: Vec::new(),
            input_amounts: Vec::new(),
            input_asset_tags: Vec::new(),
            input_nullifiers: Vec::new(),
            input_public_keys: Vec::new(),
            input_key_images: Vec::new(),
            input_amount_commitments: Vec::new(),
            input_inclusion_proofs: Vec::new(),
            input_secret_keys: Vec::new(),
            input_amount_blinds: Vec::new(),
            prover: Prover::new("private_tx".to_string()),
            nullifier_set,
        }
    }

    /// Set recipient's commitment (public key in privacy context)
    pub fn with_recipient_commitment(mut self, commitment: String) -> Self {
        self.recipient_commitment = Some(commitment);
        self
    }

    pub fn with_ring_members(mut self, ring_members: Vec<String>) -> Self {
        self.ring_members = ring_members;
        self
    }

    pub fn with_change_output(mut self, recipient_commitment: String, amount: u64) -> Self {
        self.change_output = Some(PlannedPrivateOutput {
            asset_tag: default_asset_tag(),
            recipient_commitment,
            amount,
        });
        self
    }

    pub fn with_input_nullifiers(mut self, input_nullifiers: Vec<String>) -> Self {
        self.input_nullifiers = input_nullifiers;
        self
    }

    pub fn with_input_commitments(mut self, input_commitments: Vec<String>) -> Self {
        self.input_commitments = input_commitments;
        self
    }

    pub fn with_input_amounts(mut self, input_amounts: Vec<u64>) -> Self {
        self.input_amounts = input_amounts;
        self
    }

    pub fn with_input_asset_tags(mut self, input_asset_tags: Vec<u64>) -> Self {
        self.input_asset_tags = input_asset_tags;
        self
    }

    pub fn with_fee(mut self, fee: u64) -> Self {
        self.fee = fee;
        self
    }

    pub fn with_ring_backend(mut self, ring_backend: RingSignatureBackend) -> Self {
        self.ring_backend = ring_backend;
        self
    }

    pub fn with_private_commitment_root(mut self, private_commitment_root: String) -> Self {
        self.private_commitment_root = private_commitment_root;
        self
    }

    pub fn with_input_public_keys(mut self, input_public_keys: Vec<String>) -> Self {
        self.input_public_keys = input_public_keys;
        self
    }

    pub fn with_input_key_images(mut self, input_key_images: Vec<String>) -> Self {
        self.input_key_images = input_key_images;
        self
    }

    pub fn with_input_amount_commitments(mut self, input_amount_commitments: Vec<String>) -> Self {
        self.input_amount_commitments = input_amount_commitments;
        self
    }

    pub fn with_input_inclusion_proofs(
        mut self,
        input_inclusion_proofs: Vec<PrivateCommitmentInclusionProof>,
    ) -> Self {
        self.input_inclusion_proofs = input_inclusion_proofs;
        self
    }

    pub fn with_input_secret_keys(mut self, input_secret_keys: Vec<Scalar>) -> Self {
        self.input_secret_keys = input_secret_keys;
        self
    }

    pub fn with_input_amount_blinds(mut self, input_amount_blinds: Vec<Scalar>) -> Self {
        self.input_amount_blinds = input_amount_blinds;
        self
    }

    /// Build and generate ZK proof for private transaction
    pub fn build(self) -> Result<PrivateTransaction, BlockError> {
        use rand::RngCore;

        let recipient_commitment = self.recipient_commitment.ok_or_else(|| {
            BlockError::TransactionValidationFailed("Recipient commitment required".to_string())
        })?;

        if recipient_commitment.is_empty() {
            return Err(BlockError::TransactionValidationFailed(
                "Recipient commitment required".to_string(),
            ));
        }

        let mut nullifier_nonce = vec![0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut nullifier_nonce);

        let mut input_nullifiers = if self.input_nullifiers.is_empty() {
            let nullifier_data = [
                self.spending_key.clone(),
                self.amount.to_le_bytes().to_vec(),
                nullifier_nonce.clone(),
            ]
            .concat();
            vec![bytes_to_hex(Nullifier::new(nullifier_data).as_bytes())]
        } else {
            self.input_nullifiers
        };

        input_nullifiers.retain(|tag| !tag.is_empty());
        input_nullifiers.sort();
        input_nullifiers.dedup();
        if input_nullifiers.is_empty() {
            return Err(BlockError::TransactionValidationFailed(
                "private transaction requires at least one spend tag".to_string(),
            ));
        }
        let primary_nullifier = input_nullifiers[0].clone();

        let mut input_public_keys = self.input_public_keys;
        input_public_keys.retain(|key| !key.is_empty());
        if input_public_keys.is_empty() {
            input_public_keys.push(point_to_hex(
                &(scalar_from_secret_bytes(&self.spending_key) * RISTRETTO_BASEPOINT_POINT),
            ));
        }

        let mut input_key_images = self.input_key_images;
        input_key_images.retain(|image| !image.is_empty());
        let mut input_amount_commitments = self.input_amount_commitments;
        input_amount_commitments.retain(|commitment| !commitment.is_empty());
        let mut input_commitments = self.input_commitments;
        input_commitments.retain(|commitment| !commitment.is_empty());
        let mut input_inclusion_proofs = self.input_inclusion_proofs;
        let input_secret_keys = self.input_secret_keys;
        let input_amount_blinds = self.input_amount_blinds;

        let mut input_amounts = if self.input_amounts.is_empty() {
            vec![self.amount.saturating_add(self.fee)]
        } else {
            self.input_amounts
        };
        let mut input_asset_tags = if self.input_asset_tags.is_empty() {
            vec![asset_tag_to_u64(&default_asset_tag()); input_amounts.len()]
        } else {
            self.input_asset_tags
        };
        input_amounts.retain(|amount| *amount > 0);
        if input_amounts.is_empty() {
            return Err(BlockError::TransactionValidationFailed(
                "private transaction requires at least one input note".to_string(),
            ));
        }
        input_asset_tags.truncate(input_amounts.len());
        if input_amounts.len() != input_nullifiers.len() {
            return Err(BlockError::TransactionValidationFailed(
                "private input amounts/nullifiers cardinality mismatch".to_string(),
            ));
        }
        if input_asset_tags.len() != input_amounts.len() {
            return Err(BlockError::TransactionValidationFailed(
                "private input asset tags/amounts cardinality mismatch".to_string(),
            ));
        }
        if !input_commitments.is_empty() && input_commitments.len() != input_amounts.len() {
            return Err(BlockError::TransactionValidationFailed(
                "private input commitments/amounts cardinality mismatch".to_string(),
            ));
        }
        if input_key_images.len() != input_amounts.len() {
            return Err(BlockError::TransactionValidationFailed(
                "private input key images/amounts cardinality mismatch".to_string(),
            ));
        }
        if input_public_keys.len() != input_amounts.len() {
            return Err(BlockError::TransactionValidationFailed(
                "private input public keys/amounts cardinality mismatch".to_string(),
            ));
        }
        if !input_amount_commitments.is_empty()
            && input_amount_commitments.len() != input_amounts.len()
        {
            return Err(BlockError::TransactionValidationFailed(
                "private input amount commitments/amounts cardinality mismatch".to_string(),
            ));
        }
        if !input_secret_keys.is_empty() && input_secret_keys.len() != input_amounts.len() {
            return Err(BlockError::TransactionValidationFailed(
                "private input secret keys/amounts cardinality mismatch".to_string(),
            ));
        }
        if !input_amount_blinds.is_empty() && input_amount_blinds.len() != input_amounts.len() {
            return Err(BlockError::TransactionValidationFailed(
                "private input amount blinds/amounts cardinality mismatch".to_string(),
            ));
        }
        if input_amounts.len() > PRIVATE_TX_MAX_INPUTS {
            return Err(BlockError::TransactionValidationFailed(format!(
                "private transaction exceeds Groth16 input bound ({})",
                PRIVATE_TX_MAX_INPUTS
            )));
        }
        if input_asset_tags.iter().any(|tag| *tag == 0) {
            return Err(BlockError::TransactionValidationFailed(
                "private input asset tags must be non-zero".to_string(),
            ));
        }
        if input_asset_tags
            .first()
            .map(|anchor| input_asset_tags.iter().any(|tag| tag != anchor))
            .unwrap_or(false)
        {
            return Err(BlockError::TransactionValidationFailed(
                "multi-asset inputs are not enabled yet in transaction builder".to_string(),
            ));
        }
        for amount in &input_amounts {
            if *amount > u32::MAX as u64 {
                return Err(BlockError::TransactionValidationFailed(
                    "private input note exceeds current 32-bit range proof bound".to_string(),
                ));
            }
        }

        if input_commitments.is_empty() {
            return Err(BlockError::TransactionValidationFailed(
                "private transaction requires committed input notes".to_string(),
            ));
        }
        if input_amount_commitments.is_empty() {
            return Err(BlockError::TransactionValidationFailed(
                "private transaction requires amount commitments for spent notes".to_string(),
            ));
        }
        if input_secret_keys.is_empty() {
            return Err(BlockError::TransactionValidationFailed(
                "private transaction requires secret keys for spent notes".to_string(),
            ));
        }
        if input_amount_blinds.is_empty() {
            return Err(BlockError::TransactionValidationFailed(
                "private transaction requires amount blinds for spent notes".to_string(),
            ));
        }

        let private_commitment_root = if self.private_commitment_root.is_empty() {
            let (derived_root, derived_proofs) =
                build_private_commitment_inclusion_proofs(&input_commitments, &input_commitments)
                    .ok_or_else(|| {
                    BlockError::TransactionValidationFailed(
                        "failed to derive local private commitment root".to_string(),
                    )
                })?;
            if input_inclusion_proofs.is_empty() {
                input_inclusion_proofs = derived_proofs;
            }
            derived_root
        } else {
            self.private_commitment_root.clone()
        };

        if !input_inclusion_proofs.is_empty() {
            if input_inclusion_proofs.len() != input_commitments.len() {
                return Err(BlockError::TransactionValidationFailed(
                    "private input commitments/inclusion proofs cardinality mismatch".to_string(),
                ));
            }
            for (commitment, proof) in input_commitments.iter().zip(input_inclusion_proofs.iter()) {
                if !constant_time_eq_str(commitment, &proof.commitment) {
                    return Err(BlockError::TransactionValidationFailed(
                        "private input inclusion proof commitment mismatch".to_string(),
                    ));
                }
                if proof.steps.len() > PRIVATE_TX_MERKLE_MAX_DEPTH {
                    return Err(BlockError::TransactionValidationFailed(format!(
                        "private input inclusion proof depth exceeds circuit bound ({})",
                        PRIVATE_TX_MERKLE_MAX_DEPTH
                    )));
                }
                if !StateManager::verify_private_commitment_inclusion_proof(
                    &private_commitment_root,
                    proof,
                ) {
                    return Err(BlockError::TransactionValidationFailed(
                        "private input inclusion proof is invalid".to_string(),
                    ));
                }
            }
        }

        let mut set = self.nullifier_set.lock().unwrap();
        for nullifier_hex in &input_nullifiers {
            let nullifier_bytes = hex::decode(nullifier_hex).map_err(|_| {
                BlockError::TransactionValidationFailed(
                    "private spend tag is not valid hex".to_string(),
                )
            })?;
            let nullifier = Nullifier::new(nullifier_bytes);
            set.spend(nullifier).map_err(|e| {
                BlockError::TransactionValidationFailed(format!("Double spend detected: {}", e))
            })?;
        }
        drop(set);

        if self.amount > u32::MAX as u64 {
            return Err(BlockError::TransactionValidationFailed(
                "private amount exceeds current 32-bit range proof bound".to_string(),
            ));
        }
        if self.fee > u32::MAX as u64 {
            return Err(BlockError::TransactionValidationFailed(
                "private fee exceeds current 32-bit range proof bound".to_string(),
            ));
        }

        let (primary_commitment, primary_commitment_blind) =
            Commitment::new_random(&self.amount.to_le_bytes());

        // Build Groth16 witness/public inputs with guaranteed non-overflow arithmetic.
        let blind_cap = u64::MAX.saturating_sub(self.amount);
        let blind_seed = first_u64_from_bytes(&primary_commitment_blind);
        let blind_witness = if blind_cap == 0 {
            0
        } else {
            blind_seed % blind_cap
        };
        let proof_commitment = self.amount + blind_witness;

        let index_witness = 1u64;
        let secret_cap = u64::MAX.saturating_sub(blind_witness);
        let secret_seed = first_u64_from_bytes(&self.spending_key);
        let secret_witness = if secret_cap == 0 {
            0
        } else {
            secret_seed % secret_cap
        };
        let proof_nullifier = secret_witness + blind_witness;

        let output_sum = self
            .amount
            .checked_add(self.change_output.as_ref().map(|o| o.amount).unwrap_or(0))
            .ok_or_else(|| {
                BlockError::TransactionValidationFailed("private output sum overflow".to_string())
            })?;
        let input_sum = output_sum.checked_add(self.fee).ok_or_else(|| {
            BlockError::TransactionValidationFailed("private input sum overflow".to_string())
        })?;
        let witness_input_sum = input_amounts.iter().try_fold(0u64, |acc, amount| {
            acc.checked_add(*amount).ok_or_else(|| {
                BlockError::TransactionValidationFailed(
                    "private witness input sum overflow".to_string(),
                )
            })
        })?;
        if witness_input_sum != input_sum {
            return Err(BlockError::TransactionValidationFailed(
                "private witness inputs do not preserve value".to_string(),
            ));
        }
        if input_sum > u32::MAX as u64 || output_sum > u32::MAX as u64 {
            return Err(BlockError::TransactionValidationFailed(
                "private sum exceeds current 32-bit range proof bound".to_string(),
            ));
        }

        let proof_key_image = input_key_images
            .first()
            .map(|image| first_u64_from_hex(image))
            .unwrap_or(0);
        let proven_input_count = input_commitments.len().min(PRIVATE_TX_MERKLE_PROVEN_INPUTS);
        let proof_private_root_words =
            fixed_u64_words_from_hex(&private_commitment_root, PRIVATE_TX_ROOT_WORDS);
        let mut proven_input_flags = vec![0u64; PRIVATE_TX_MERKLE_PROVEN_INPUTS];
        let mut proven_commitment_words =
            Vec::with_capacity(PRIVATE_TX_MERKLE_PROVEN_INPUTS * PRIVATE_TX_ROOT_WORDS);
        let padded_input_amounts = pad_u64s(&input_amounts, PRIVATE_TX_MAX_INPUTS)?;
        let padded_input_key_images = pad_u64s(
            &input_key_images
                .iter()
                .map(|image| first_u64_from_hex(image))
                .collect::<Vec<_>>(),
            PRIVATE_TX_MAX_INPUTS,
        )?;
        let padded_input_asset_tags = pad_u64s(&input_asset_tags, PRIVATE_TX_MAX_INPUTS)?;
        let input_asset_tag = *input_asset_tags.first().ok_or_else(|| {
            BlockError::TransactionValidationFailed(
                "private transaction requires at least one input asset tag".to_string(),
            )
        })?;
        let output_asset_tag = input_asset_tag;
        let mut proven_path_sibling_words = Vec::with_capacity(
            PRIVATE_TX_MERKLE_PROVEN_INPUTS * PRIVATE_TX_MERKLE_MAX_DEPTH * PRIVATE_TX_ROOT_WORDS,
        );
        let mut proven_path_directions =
            Vec::with_capacity(PRIVATE_TX_MERKLE_PROVEN_INPUTS * PRIVATE_TX_MERKLE_MAX_DEPTH);
        for proven_idx in 0..PRIVATE_TX_MERKLE_PROVEN_INPUTS {
            if proven_idx < proven_input_count {
                let proof = input_inclusion_proofs.get(proven_idx).ok_or_else(|| {
                    BlockError::TransactionValidationFailed(format!(
                        "private transaction requires inclusion proof {} for the Groth16 witness",
                        proven_idx
                    ))
                })?;
                proven_input_flags[proven_idx] = 1;
                proven_commitment_words.extend_from_slice(&private_tree_commitment_words(
                    &input_commitments[proven_idx],
                ));
                if proof.steps.len() != PRIVATE_TX_MERKLE_MAX_DEPTH {
                    return Err(BlockError::TransactionValidationFailed(format!(
                        "input inclusion proof {} depth {} does not match circuit depth {}",
                        proven_idx,
                        proof.steps.len(),
                        PRIVATE_TX_MERKLE_MAX_DEPTH
                    )));
                }
                for step in &proof.steps {
                    let sibling_scalar = private_tree_scalar_from_hex(&step.sibling_hash)
                        .ok_or_else(|| {
                            BlockError::TransactionValidationFailed(
                                "private inclusion proof sibling is not canonical".to_string(),
                            )
                        })?;
                    proven_path_sibling_words
                        .extend_from_slice(&private_tree_scalar_to_words(&sibling_scalar));
                    proven_path_directions.push(if step.sibling_on_left { 1u64 } else { 0u64 });
                }
            } else {
                proven_commitment_words.extend_from_slice(&[0u64; PRIVATE_TX_ROOT_WORDS]);
                for _ in 0..PRIVATE_TX_MERKLE_MAX_DEPTH {
                    proven_path_sibling_words.extend_from_slice(&[0u64; PRIVATE_TX_ROOT_WORDS]);
                    proven_path_directions.push(0u64);
                }
            }
        }

        let mut private_input_words = vec![
            self.amount,
            blind_witness,
            secret_witness,
            index_witness,
            input_sum,
            output_sum,
            self.fee,
            proof_key_image,
            input_asset_tag,
            output_asset_tag,
        ];
        private_input_words.extend_from_slice(&padded_input_amounts);
        private_input_words.extend_from_slice(&padded_input_key_images);
        private_input_words.extend_from_slice(&padded_input_asset_tags);
        private_input_words.extend_from_slice(&proven_path_sibling_words);
        private_input_words.extend_from_slice(&proven_path_directions);

        let mut public_input_words = vec![proof_commitment, proof_nullifier];
        public_input_words.extend_from_slice(&padded_input_key_images);
        public_input_words.extend_from_slice(&proven_input_flags);
        public_input_words.extend_from_slice(&proven_commitment_words);
        public_input_words.extend_from_slice(&proof_private_root_words);

        let private_inputs_bytes = encode_u64_words(&private_input_words);
        let public_inputs_bytes = encode_u64_words(&public_input_words);

        let zk_proof = self
            .prover
            .prove(&private_inputs_bytes, &public_inputs_bytes)?;

        if input_amount_commitments
            .iter()
            .any(|commitment| point_from_hex(commitment).is_none())
        {
            return Err(BlockError::TransactionValidationFailed(
                "private input amount commitment is malformed".to_string(),
            ));
        }
        let input_blind_sum = input_amount_blinds
            .iter()
            .fold(Scalar::ZERO, |acc, blind| acc + *blind);

        let mut outputs = Vec::new();
        let tx_asset_tag = default_asset_tag();
        let mut planned_outputs = vec![PlannedPrivateOutput {
            asset_tag: tx_asset_tag.clone(),
            recipient_commitment: recipient_commitment.clone(),
            amount: self.amount,
        }];
        if let Some(change_output) = self.change_output {
            if change_output.amount > 0 {
                planned_outputs.push(change_output);
            }
        }

        let mut output_blind_sum = Scalar::ZERO;
        for (ordinal, planned_output) in planned_outputs.into_iter().enumerate() {
            let (commitment_hex, commitment_blind) = if ordinal == 0 {
                (
                    bytes_to_hex(primary_commitment.as_bytes()),
                    primary_commitment_blind.clone(),
                )
            } else {
                let (commitment, commitment_blind) =
                    Commitment::new_random(&planned_output.amount.to_le_bytes());
                (bytes_to_hex(commitment.as_bytes()), commitment_blind)
            };
            let stealth_material =
                derive_stealth_payload(&planned_output.recipient_commitment, &commitment_hex)?;
            let amount_blind = stealth_material.amount_blind;
            let encrypted_amount = encrypt_private_note_payload_v2(
                planned_output.amount,
                &commitment_blind,
                amount_blind,
                &commitment_hex,
                &primary_nullifier,
            )?;
            output_blind_sum += amount_blind;
            outputs.push(PrivateOutput {
                output_id: String::new(),
                asset_tag: planned_output.asset_tag.clone(),
                asset_tag_commitment: asset_tag_commitment_hex(&planned_output.asset_tag),
                recipient_commitment: String::new(),
                stealth_address: stealth_material.stealth_address,
                ephemeral_pubkey: stealth_material.ephemeral_pubkey,
                commitment: commitment_hex,
                amount_commitment: amount_commitment_hex(planned_output.amount, amount_blind),
                range_proof: prove_output_range(planned_output.amount, amount_blind)?,
                one_time_public_key: stealth_material.one_time_public_key,
                encrypted_amount,
                commitment_blind: Vec::new(),
                is_change: false,
            });
        }
        // Generate a private fee commitment when fee > 0. The commitment is a
        // Pedersen point `fee·G + fee_blind·H`; it hides the fee amount from
        // on-chain observers. `amount_balance_commitment` absorbs `fee_blind`
        // so the on-chain equation
        //   sum(input_commits) = sum(output_commits) + fee_commit + amount_balance_commit
        // still closes. When fee == 0, we stay on the legacy visible-fee path
        // for backwards compatibility (empty fee_commitment means "use
        // `fee·G + 0·H` reconstructed from the cleartext `fee` field").
        let (fee_commitment, fee_range_proof, fee_blind) = if self.fee > 0 {
            let mut fee_blind_bytes = [0u8; 64];
            use rand::RngCore;
            rand::rngs::OsRng.fill_bytes(&mut fee_blind_bytes);
            let fee_blind = Scalar::from_bytes_mod_order_wide(&fee_blind_bytes);
            let commitment_hex = amount_commitment_hex(self.fee, fee_blind);
            let range_proof = prove_output_range(self.fee, fee_blind)?;
            fee_blind_bytes.zeroize();
            (commitment_hex, range_proof, fee_blind)
        } else {
            (String::new(), Vec::new(), Scalar::ZERO)
        };

        let amount_balance_commitment =
            amount_balance_commitment_hex(input_blind_sum - output_blind_sum - fee_blind);
        let input_asset_tag_labels = vec![tx_asset_tag.clone(); input_nullifiers.len()];
        let fee_asset_tag = tx_asset_tag.clone();
        let asset_balance_commitments = vec![AssetBalanceCommitment {
            asset_tag: tx_asset_tag.clone(),
            balance_commitment: amount_balance_commitment.clone(),
        }];

        let tx_hash = private_tx_hash(
            &input_nullifiers,
            &input_commitments,
            &input_asset_tag_labels,
            &private_commitment_root,
            &input_public_keys,
            &input_key_images,
            &input_amount_commitments,
            &amount_balance_commitment,
            &fee_asset_tag,
            &asset_balance_commitments,
            &fee_commitment,
            &outputs,
        );
        for (ordinal, output) in outputs.iter_mut().enumerate() {
            output.output_id = derive_output_id(
                &tx_hash,
                ordinal,
                &output.commitment,
                &output.one_time_public_key,
            );
        }

        let primary_output = outputs
            .iter()
            .find(|output| !output.is_change)
            .cloned()
            .ok_or_else(|| {
                BlockError::TransactionValidationFailed(
                    "private transaction requires a recipient output".to_string(),
                )
            })?;
        let commitment_hex = primary_output.commitment.clone();
        let commitment_blind = Vec::new();
        let encrypted_amount = primary_output.encrypted_amount.clone();
        let stealth_address = primary_output.stealth_address.clone();
        let ephemeral_pubkey = primary_output.ephemeral_pubkey.clone();

        let input_secret_keys = input_secret_keys;
        let signer_public = input_public_keys
            .first()
            .and_then(|key| point_from_hex(key))
            .unwrap_or(input_secret_keys[0] * RISTRETTO_BASEPOINT_POINT);

        if input_key_images.is_empty() {
            input_key_images = input_secret_keys
                .iter()
                .zip(input_public_keys.iter())
                .filter_map(|(secret_scalar, public_key)| {
                    point_from_hex(public_key)
                        .map(|public_key| derive_key_image_hex(&public_key, *secret_scalar))
                })
                .collect::<Vec<_>>();
        }
        let mut ring_points = self
            .ring_members
            .iter()
            .enumerate()
            .map(|(idx, member)| {
                derive_decoy_ring_member(member, &tx_hash, &primary_output.one_time_public_key, idx)
            })
            .collect::<Vec<_>>();
        ring_points.push(signer_public);

        ring_points.sort_by(|lhs, rhs| lhs.compress().as_bytes().cmp(rhs.compress().as_bytes()));
        ring_points.dedup_by(|lhs, rhs| lhs.compress().as_bytes() == rhs.compress().as_bytes());

        if ring_points.len() < 3 {
            return Err(BlockError::TransactionValidationFailed(
                "private transaction ring must contain at least 3 members".to_string(),
            ));
        }

        let ring_members = ring_points.iter().map(point_to_hex).collect::<Vec<_>>();
        let ring_id = ring_id_from_members(&ring_members);
        let ring_message = ring_message(
            &tx_hash,
            &commitment_hex,
            &primary_nullifier,
            &primary_output.one_time_public_key,
            &ring_members,
            &input_public_keys,
            &input_key_images,
        );
        let signer_index = ring_members
            .iter()
            .position(|member| constant_time_eq_str(member, &point_to_hex(&signer_public)))
            .ok_or_else(|| {
                BlockError::TransactionValidationFailed(
                    "failed to locate signer in transaction ring".to_string(),
                )
            })?;
        let signing_backend = match self.ring_backend {
            RingSignatureBackend::LegacyLinkable => RingSignatureBackend::ClsagV1,
            RingSignatureBackend::ClsagV1 => RingSignatureBackend::ClsagV1,
        };
        let ring_signature = sign_ring_signature_with_backend(
            signing_backend,
            &input_secret_keys,
            signer_index,
            &ring_members,
            &input_public_keys,
            &input_key_images,
            &tx_hash,
            &primary_output.one_time_public_key,
            &ring_message,
        );

        Ok(PrivateTransaction {
            tx_hash,
            commitment: commitment_hex,
            recipient_commitment: String::new(),
            stealth_address,
            ephemeral_pubkey,
            encrypted_amount,
            proof_commitment,
            proof_nullifier,
            proof_input_sum: 0,
            proof_output_sum: 0,
            proof_fee: 0,
            proof_key_image: 0,
            proof_private_root_words,
            fee: if fee_commitment.is_empty() {
                self.fee
            } else {
                0
            },
            ring_id,
            ring_version: ring_version_for_backend(signing_backend),
            ring_members,
            ring_signature,
            input_nullifiers,
            input_commitments,
            input_asset_tags: input_asset_tag_labels,
            input_public_keys,
            input_key_images,
            input_amount_commitments,
            amount_balance_commitment,
            fee_asset_tag,
            asset_balance_commitments,
            fee_commitment,
            fee_range_proof,
            private_commitment_root,
            input_inclusion_proofs,
            outputs,
            nullifier: primary_nullifier,
            zk_proof: Some(zk_proof.proof_bytes),
            timestamp: current_timestamp(),
            status: PrivacyStatus::Pending,
            commitment_blind,
            nullifier_nonce,
            fee_opening: if self.fee > 0 {
                Some(PrivateFeeOpening {
                    fee: self.fee,
                    fee_blind: fee_blind.to_bytes().to_vec(),
                })
            } else {
                None
            },
        })
    }
}

/// Private transaction pool (maintains privacy)
pub struct PrivateTransactionPool {
    pending: HashMap<String, PrivateTransaction>,
    verified: HashMap<String, PrivateTransaction>,
    confirmed: HashMap<String, PrivateTransaction>,
    failed: HashMap<String, PrivateTransaction>,
    nullifier_set: Arc<Mutex<NullifierSet>>,
}

impl PrivateTransactionPool {
    fn update_transaction<F>(&mut self, tx_hash: &str, mut updater: F) -> bool
    where
        F: FnMut(&mut PrivateTransaction) -> bool,
    {
        if let Some(tx) = self.pending.get_mut(tx_hash) {
            return updater(tx);
        }
        if let Some(tx) = self.verified.get_mut(tx_hash) {
            return updater(tx);
        }
        if let Some(tx) = self.confirmed.get_mut(tx_hash) {
            return updater(tx);
        }
        if let Some(tx) = self.failed.get_mut(tx_hash) {
            return updater(tx);
        }
        false
    }

    /// Create new private transaction pool
    pub fn new() -> Self {
        PrivateTransactionPool {
            pending: HashMap::new(),
            verified: HashMap::new(),
            confirmed: HashMap::new(),
            failed: HashMap::new(),
            nullifier_set: Arc::new(Mutex::new(NullifierSet::new())),
        }
    }

    /// Get nullifier set for building transactions
    pub fn nullifier_set(&self) -> Arc<Mutex<NullifierSet>> {
        Arc::clone(&self.nullifier_set)
    }

    /// Submit private transaction
    pub fn submit_private_transaction(
        &mut self,
        tx: PrivateTransaction,
    ) -> Result<String, BlockError> {
        let tx_hash = tx.tx_hash.clone();

        // Verify ZK proof if present
        if let Some(proof_bytes) = &tx.zk_proof {
            let verifier = Verifier::new();
            let public_inputs = proof_public_inputs_for_tx(&tx)?;
            let temp_proof = crate::zk::prover::ZKProof::new(
                proof_bytes.clone(),
                "private_tx".to_string(),
                public_inputs,
                current_timestamp(),
            );

            match verifier.verify(&temp_proof) {
                Ok(result) if result.is_valid => {
                    // Move to verified
                    let mut verified_tx = tx;
                    verified_tx.status = PrivacyStatus::Verified;
                    self.verified.insert(tx_hash.clone(), verified_tx);
                }
                _ => {
                    // Move to failed
                    let mut failed_tx = tx;
                    failed_tx.status = PrivacyStatus::Failed;
                    self.failed.insert(tx_hash.clone(), failed_tx);
                    return Err(BlockError::TransactionValidationFailed(
                        "ZK proof verification failed".to_string(),
                    ));
                }
            }
        } else {
            // No proof, keep pending
            self.pending.insert(tx_hash.clone(), tx);
        }

        Ok(tx_hash)
    }

    /// Confirm private transaction
    pub fn confirm_private_transaction(&mut self, tx_hash: &str) -> Result<(), BlockError> {
        // Move from verified or pending to confirmed
        if let Some(tx) = self.verified.remove(tx_hash) {
            let mut confirmed_tx = tx;
            confirmed_tx.status = PrivacyStatus::Confirmed;
            self.confirmed.insert(tx_hash.to_string(), confirmed_tx);
            Ok(())
        } else if let Some(tx) = self.pending.remove(tx_hash) {
            let mut confirmed_tx = tx;
            confirmed_tx.status = PrivacyStatus::Confirmed;
            self.confirmed.insert(tx_hash.to_string(), confirmed_tx);
            Ok(())
        } else {
            Err(BlockError::TransactionValidationFailed(
                "Transaction not found in pending pool".to_string(),
            ))
        }
    }

    /// Get transaction by hash (returns only confirmed/verified, not pending)
    pub fn get_private_transaction(&self, tx_hash: &str) -> Option<PrivateTransaction> {
        self.confirmed
            .get(tx_hash)
            .or_else(|| self.verified.get(tx_hash))
            .cloned()
    }

    /// Get all pending private transactions
    pub fn get_all_pending(&self) -> Vec<PrivateTransaction> {
        self.pending.values().cloned().collect()
    }

    /// Get pool statistics
    pub fn get_private_pool_stats(&self) -> (usize, usize, usize, usize) {
        (
            self.pending.len(),
            self.verified.len(),
            self.confirmed.len(),
            self.failed.len(),
        )
    }

    /// Clear pending private transactions
    pub fn clear_pending(&mut self) {
        self.pending.clear();
    }
}

/// Privacy-aware wallet for confidential transactions
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PrivateWallet {
    pub name: String,
    pub spending_key: Vec<u8>,
    #[serde(default)]
    pub scan_key: Vec<u8>,
    #[serde(default)]
    pub view_only: bool,
    #[serde(default)]
    pub restore_height: u64,
    #[serde(default)]
    pub historical_spending_keys: Vec<Vec<u8>>,
    #[serde(default)]
    pub historical_scan_keys: Vec<Vec<u8>>,
    pub commitment_key: String, // Public identifier
    #[serde(default)]
    pub anonymity_public_key: String, // Public key used in ring membership
    #[serde(default)]
    pub private_notes: Vec<PrivateNote>,
    pub private_balance: u64,
    pub private_transactions: Vec<PrivateTransaction>,
    #[serde(default)]
    pub private_fee_openings: HashMap<String, PrivateFeeOpening>,
    pub created_at: u64,
}

impl PrivateWallet {
    /// Create new private wallet
    pub fn new(
        name: String,
        spending_key: Vec<u8>,
        scan_key: Vec<u8>,
        commitment_key: String,
    ) -> Self {
        let anonymity_public_key = anonymity_public_key_hex(&spending_key);
        PrivateWallet {
            name,
            spending_key,
            scan_key,
            view_only: false,
            restore_height: 0,
            historical_spending_keys: vec![],
            historical_scan_keys: vec![],
            commitment_key,
            anonymity_public_key,
            private_notes: vec![],
            private_balance: 0,
            private_transactions: vec![],
            private_fee_openings: HashMap::new(),
            created_at: current_timestamp(),
        }
    }

    fn sync_balance_cache(&mut self) {
        self.private_balance = self
            .private_notes
            .iter()
            .filter(|note| !note.spent)
            .map(|note| note.amount)
            .sum();
    }

    fn ensure_key_material(&mut self) {
        if self.scan_key.is_empty() {
            self.scan_key = self.spending_key.clone();
        }
        if self.commitment_key.is_empty() || !self.commitment_key.starts_with("zpriv:") {
            self.commitment_key = payment_address_from_keys(&self.scan_key, &self.spending_key);
        }
        if self.anonymity_public_key.is_empty() {
            self.anonymity_public_key = anonymity_public_key_hex(&self.spending_key);
        }
    }

    pub fn normalize_key_material(&mut self) {
        self.ensure_key_material();
        self.sync_balance_cache();
    }

    fn materialize_legacy_balance_note(&mut self) {
        self.ensure_key_material();
        if self.private_notes.is_empty() && self.private_balance > 0 {
            let note = self.create_local_note(self.private_balance);
            self.private_notes.push(note);
        }
        self.sync_balance_cache();
    }

    fn receive_note(&mut self, note: PrivateNote) {
        if self
            .private_notes
            .iter()
            .any(|existing| existing.note_id == note.note_id)
        {
            return;
        }
        self.private_notes.push(note);
        self.sync_balance_cache();
    }

    fn create_local_note(&self, amount: u64) -> PrivateNote {
        let now = current_timestamp();
        let (commitment, commitment_blind) = Commitment::new_random(&amount.to_le_bytes());
        let commitment_hex = bytes_to_hex(commitment.as_bytes());
        // `self.commitment_key` is normalized to a `zpriv:` address by
        // `ensure_key_material` before any mint runs, so this unwrap only
        // trips on programmer error (missing normalization), not on user
        // input.
        let stealth_material = derive_stealth_payload(&self.commitment_key, &commitment_hex)
            .expect("wallet commitment_key must be a valid zpriv stealth address");
        let note_seed = format!(
            "mint:{}:{}:{}",
            self.name,
            now,
            self.private_transactions.len()
        );
        PrivateNote {
            note_id: derive_output_id(
                "mint",
                self.private_notes.len(),
                &commitment_hex,
                &self.commitment_key,
            ),
            asset_tag: default_asset_tag(),
            source_tx_hash: format!(
                "mint:{}",
                hex::encode(blake3::hash(note_seed.as_bytes()).as_bytes())
            ),
            recipient_commitment: self.commitment_key.clone(),
            stealth_address: stealth_material.stealth_address,
            ephemeral_pubkey: stealth_material.ephemeral_pubkey,
            commitment: commitment_hex,
            amount_commitment: amount_commitment_hex(amount, stealth_material.amount_blind),
            range_proof: prove_output_range(amount, stealth_material.amount_blind)
                .expect("local private note amount should satisfy Bulletproof range bound"),
            one_time_public_key: stealth_material.one_time_public_key,
            commitment_blind,
            amount,
            is_change: false,
            spent: false,
            created_at: now,
            spent_at: None,
        }
    }

    fn create_private_coinbase_output(
        &self,
        amount: u64,
        block_height: u64,
        recipient_address: &str,
        unlock_height: u64,
    ) -> Result<crate::types::PrivateCoinbaseOutput, BlockError> {
        let (commitment, commitment_blind) = Commitment::new_random(&amount.to_le_bytes());
        let commitment_hex = bytes_to_hex(commitment.as_bytes());
        let stealth_material = derive_stealth_payload(&self.commitment_key, &commitment_hex)?;
        let amount_domain = coinbase_amount_domain(block_height, recipient_address);
        let encrypted_amount = xor_amount_with_stream(
            amount,
            &commitment_blind,
            &self.commitment_key,
            &amount_domain,
        );

        Ok(crate::types::PrivateCoinbaseOutput {
            block_height,
            recipient_address: recipient_address.to_string(),
            unlock_height,
            output_id: derive_output_id(
                &format!("coinbase:{block_height}"),
                0,
                &commitment_hex,
                &self.commitment_key,
            ),
            asset_tag: default_asset_tag(),
            asset_tag_commitment: asset_tag_commitment_hex(&default_asset_tag()),
            recipient_commitment: self.commitment_key.clone(),
            stealth_address: stealth_material.stealth_address,
            ephemeral_pubkey: stealth_material.ephemeral_pubkey,
            commitment: commitment_hex,
            amount_commitment: amount_commitment_hex(amount, stealth_material.amount_blind),
            range_proof: prove_output_range(amount, stealth_material.amount_blind)?,
            one_time_public_key: stealth_material.one_time_public_key,
            encrypted_amount,
            commitment_blind,
        })
    }

    fn receive_private_amount(&mut self, amount: u64) {
        self.ensure_key_material();
        let note = self.create_local_note(amount);
        self.receive_note(note);
    }

    fn import_output_as_note(&mut self, source_tx_hash: &str, output: &PrivateOutput, amount: u64) {
        self.receive_note(PrivateNote {
            note_id: output.output_id.clone(),
            asset_tag: output.asset_tag.clone(),
            source_tx_hash: source_tx_hash.to_string(),
            recipient_commitment: output.recipient_commitment.clone(),
            stealth_address: output.stealth_address.clone(),
            ephemeral_pubkey: output.ephemeral_pubkey.clone(),
            commitment: output.commitment.clone(),
            amount_commitment: output.amount_commitment.clone(),
            range_proof: output.range_proof.clone(),
            one_time_public_key: output.one_time_public_key.clone(),
            commitment_blind: output.commitment_blind.clone(),
            amount,
            is_change: output.is_change,
            spent: false,
            created_at: current_timestamp(),
            spent_at: None,
        });
    }

    fn select_spendable_note_indices(&self, amount: u64) -> Result<Vec<usize>, BlockError> {
        let mut candidates = self
            .private_notes
            .iter()
            .enumerate()
            .filter(|(_, note)| !note.spent)
            .map(|(idx, note)| (idx, note.amount))
            .collect::<Vec<_>>();
        candidates.sort_by(|lhs, rhs| rhs.1.cmp(&lhs.1).then_with(|| lhs.0.cmp(&rhs.0)));

        let mut selected = Vec::new();
        let mut total = 0u64;
        for (idx, note_amount) in candidates {
            selected.push(idx);
            total = total.saturating_add(note_amount);
            if total >= amount {
                return Ok(selected);
            }
        }

        Err(BlockError::TransactionValidationFailed(
            "Insufficient private balance".to_string(),
        ))
    }

    fn prepare_selected_notes(
        &mut self,
        total_required: u64,
    ) -> Result<(Vec<usize>, Vec<PrivateNote>), BlockError> {
        self.materialize_legacy_balance_note();
        let selected_note_indices = self.select_spendable_note_indices(total_required)?;
        let selected_notes = selected_note_indices
            .iter()
            .map(|idx| self.private_notes[*idx].clone())
            .collect::<Vec<_>>();
        Ok((selected_note_indices, selected_notes))
    }

    pub fn preview_input_commitments(&mut self, amount: u64) -> Result<Vec<String>, BlockError> {
        let (_, selected_notes) = self.prepare_selected_notes(amount)?;
        Ok(selected_notes
            .into_iter()
            .map(|note| note.commitment)
            .collect::<Vec<_>>())
    }

    pub fn preview_input_commitments_with_fee(
        &mut self,
        amount: u64,
        fee: u64,
    ) -> Result<Vec<String>, BlockError> {
        let total_required = amount.checked_add(fee).ok_or_else(|| {
            BlockError::TransactionValidationFailed("private amount + fee overflow".to_string())
        })?;
        let (_, selected_notes) = self.prepare_selected_notes(total_required)?;
        Ok(selected_notes
            .into_iter()
            .map(|note| note.commitment)
            .collect::<Vec<_>>())
    }

    /// Create private transaction
    pub fn create_private_transaction(
        &mut self,
        amount: u64,
        fee: u64,
        recipient_commitment: String,
        nullifier_set: Arc<Mutex<NullifierSet>>,
        ring_members: Vec<String>,
        private_commitment_root: String,
        input_inclusion_proofs: Vec<PrivateCommitmentInclusionProof>,
        ring_backend: RingSignatureBackend,
    ) -> Result<PrivateTransaction, BlockError> {
        let total_required = amount.checked_add(fee).ok_or_else(|| {
            BlockError::TransactionValidationFailed("private amount + fee overflow".to_string())
        })?;
        let (selected_note_indices, selected_notes) =
            self.prepare_selected_notes(total_required)?;
        if selected_notes.len() > PRIVATE_TX_MAX_INPUTS {
            return Err(BlockError::TransactionValidationFailed(format!(
                "private transaction selected {} inputs, exceeding Groth16 bound {}",
                selected_notes.len(),
                PRIVATE_TX_MAX_INPUTS
            )));
        }
        let total_input = selected_notes.iter().map(|note| note.amount).sum::<u64>();
        if total_input < amount.saturating_add(fee) {
            return Err(BlockError::TransactionValidationFailed(
                "Insufficient private balance after fee".to_string(),
            ));
        }
        let change_amount = total_input.saturating_sub(amount).saturating_sub(fee);
        let input_nullifiers = selected_notes
            .iter()
            .map(|note| derive_note_nullifier(&self.spending_key, &note.note_id, &note.commitment))
            .collect::<Vec<_>>();
        let input_commitments = selected_notes
            .iter()
            .map(|note| note.commitment.clone())
            .collect::<Vec<_>>();
        let input_public_keys = selected_notes
            .iter()
            .map(|note| note.one_time_public_key.clone())
            .collect::<Vec<_>>();
        let input_amounts = selected_notes
            .iter()
            .map(|note| note.amount)
            .collect::<Vec<_>>();
        let input_asset_tags = selected_notes
            .iter()
            .map(|note| asset_tag_to_u64(&note.asset_tag))
            .collect::<Vec<_>>();
        let input_amount_commitments = selected_notes
            .iter()
            .map(|note| note.amount_commitment.clone())
            .collect::<Vec<_>>();
        let input_secret_scalars = selected_notes
            .iter()
            .map(|note| {
                derive_output_secret_scalar(
                    &self.spending_key,
                    &self.scan_key,
                    &PrivateOutput {
                        output_id: note.note_id.clone(),
                        asset_tag: note.asset_tag.clone(),
                        asset_tag_commitment: asset_tag_commitment_hex(&note.asset_tag),
                        recipient_commitment: note.recipient_commitment.clone(),
                        stealth_address: note.stealth_address.clone(),
                        ephemeral_pubkey: note.ephemeral_pubkey.clone(),
                        commitment: note.commitment.clone(),
                        amount_commitment: note.amount_commitment.clone(),
                        range_proof: note.range_proof.clone(),
                        one_time_public_key: note.one_time_public_key.clone(),
                        encrypted_amount: vec![],
                        commitment_blind: note.commitment_blind.clone(),
                        is_change: note.is_change,
                    },
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let input_amount_blinds = selected_notes
            .iter()
            .map(|note| {
                derive_output_amount_blind_from_note(
                    if self.scan_key.is_empty() {
                        &self.spending_key
                    } else {
                        &self.scan_key
                    },
                    &PrivateOutput {
                        output_id: note.note_id.clone(),
                        asset_tag: note.asset_tag.clone(),
                        asset_tag_commitment: asset_tag_commitment_hex(&note.asset_tag),
                        recipient_commitment: note.recipient_commitment.clone(),
                        stealth_address: note.stealth_address.clone(),
                        ephemeral_pubkey: note.ephemeral_pubkey.clone(),
                        commitment: note.commitment.clone(),
                        amount_commitment: note.amount_commitment.clone(),
                        range_proof: note.range_proof.clone(),
                        one_time_public_key: note.one_time_public_key.clone(),
                        encrypted_amount: vec![],
                        commitment_blind: note.commitment_blind.clone(),
                        is_change: note.is_change,
                    },
                )
                .ok_or_else(|| {
                    BlockError::TransactionValidationFailed(
                        "failed to derive private input amount blind".to_string(),
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let input_key_images = selected_notes
            .iter()
            .zip(input_secret_scalars.iter())
            .filter_map(|(note, secret_scalar)| {
                point_from_hex(&note.one_time_public_key)
                    .map(|public_key| derive_key_image_hex(&public_key, *secret_scalar))
            })
            .collect::<Vec<_>>();

        let mut builder =
            PrivateTransactionBuilder::new(amount, self.spending_key.clone(), nullifier_set)
                .with_recipient_commitment(recipient_commitment)
                .with_ring_members(ring_members)
                .with_private_commitment_root(private_commitment_root)
                .with_input_nullifiers(input_nullifiers)
                .with_input_commitments(input_commitments)
                .with_input_amounts(input_amounts)
                .with_input_asset_tags(input_asset_tags)
                .with_input_public_keys(input_public_keys)
                .with_input_key_images(input_key_images)
                .with_input_amount_commitments(input_amount_commitments)
                .with_input_inclusion_proofs(input_inclusion_proofs)
                .with_input_secret_keys(input_secret_scalars)
                .with_input_amount_blinds(input_amount_blinds)
                .with_ring_backend(ring_backend)
                .with_fee(fee);
        if change_amount > 0 {
            builder = builder.with_change_output(self.commitment_key.clone(), change_amount);
        }

        let tx = builder.build()?;
        let spend_timestamp = current_timestamp();
        for idx in selected_note_indices {
            if let Some(note) = self.private_notes.get_mut(idx) {
                note.spent = true;
                note.spent_at = Some(spend_timestamp);
            }
        }
        for output in tx_outputs(&tx) {
            if let Some((local_output, amount)) =
                decrypt_private_output_for_wallet(self, &tx, &output)?
            {
                self.import_output_as_note(&tx.tx_hash, &local_output, amount);
            }
        }
        if let Some(opening) = tx.fee_opening.clone() {
            self.private_fee_openings
                .insert(tx.tx_hash.clone(), opening);
        }
        self.private_transactions.push(tx.clone());
        self.sync_balance_cache();

        Ok(tx)
    }

    /// Get private transaction history
    pub fn get_private_history(&self) -> Vec<PrivateTransaction> {
        self.private_transactions.clone()
    }

    /// Get transaction count
    pub fn private_transaction_count(&self) -> usize {
        self.private_transactions.len()
    }

    /// Rotate spending key and commitment key while preserving legacy keys for recovery.
    pub fn rotate_spending_key(&mut self) -> String {
        use rand::RngCore;

        self.historical_spending_keys
            .push(self.spending_key.clone());
        self.historical_scan_keys.push(self.scan_key.clone());
        let mut next_key = vec![0u8; 32];
        let mut next_scan_key = vec![0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut next_key);
        rand::rngs::OsRng.fill_bytes(&mut next_scan_key);
        self.spending_key = next_key;
        self.scan_key = next_scan_key;

        self.commitment_key = payment_address_from_keys(&self.scan_key, &self.spending_key);
        self.anonymity_public_key = anonymity_public_key_hex(&self.spending_key);
        self.sync_balance_cache();
        self.commitment_key.clone()
    }
}

impl Drop for PrivateWallet {
    fn drop(&mut self) {
        self.spending_key.zeroize();
        self.scan_key.zeroize();
        for key in &mut self.historical_spending_keys {
            key.zeroize();
        }
        for key in &mut self.historical_scan_keys {
            key.zeroize();
        }
    }
}

fn decrypt_private_output_for_wallet(
    wallet: &PrivateWallet,
    tx: &PrivateTransaction,
    output: &PrivateOutput,
) -> Result<Option<(PrivateOutput, u64)>, BlockError> {
    let scan_key = if wallet.scan_key.is_empty() {
        &wallet.spending_key
    } else {
        &wallet.scan_key
    };

    let derived_secret = match derive_output_secret_scalar(&wallet.spending_key, scan_key, output) {
        Ok(secret) => secret,
        Err(_) => return Ok(None),
    };
    let derived_public = point_to_hex(&(derived_secret * RISTRETTO_BASEPOINT_POINT));
    if !constant_time_eq_str(&derived_public, &output.one_time_public_key) {
        return Ok(None);
    }

    let amount_blind = derive_output_amount_blind_from_note(scan_key, output).ok_or_else(|| {
        BlockError::TransactionValidationFailed("Failed to derive output amount blind".to_string())
    })?;

    let (amount, commitment_blind) = if output.encrypted_amount.len() == PRIVATE_NOTE_PAYLOAD_V2_LEN
    {
        decrypt_private_note_payload_v2(
            &output.encrypted_amount,
            amount_blind,
            &output.commitment,
            &tx.nullifier,
        )?
    } else {
        let recipient_commitment = if output.recipient_commitment.is_empty() {
            wallet.commitment_key.as_str()
        } else {
            output.recipient_commitment.as_str()
        };
        if output.commitment_blind.is_empty() {
            return Err(BlockError::TransactionValidationFailed(
                "Legacy private output is missing commitment blind".to_string(),
            ));
        }
        (
            decode_amount_from_payload(
                &output.encrypted_amount,
                &output.commitment_blind,
                recipient_commitment,
                &tx.nullifier,
            )?,
            output.commitment_blind.clone(),
        )
    };

    let expected_commitment = Commitment::new_blinded(&amount.to_le_bytes(), &commitment_blind);
    if !constant_time_eq_str(&expected_commitment.to_hex(), &output.commitment) {
        return Err(BlockError::TransactionValidationFailed(
            "Commitment mismatch while decrypting private amount".to_string(),
        ));
    }

    let expected_amount_commitment = amount_commitment_hex(amount, amount_blind);
    if !output.amount_commitment.is_empty()
        && !constant_time_eq_str(&expected_amount_commitment, &output.amount_commitment)
    {
        return Err(BlockError::TransactionValidationFailed(
            "Amount commitment mismatch while decrypting private amount".to_string(),
        ));
    }
    if !output.range_proof.is_empty()
        && !verify_output_range(&output.amount_commitment, &output.range_proof)?
    {
        return Err(BlockError::TransactionValidationFailed(
            "Range proof mismatch while decrypting private amount".to_string(),
        ));
    }

    let mut local_output = output.clone();
    local_output.recipient_commitment = wallet.commitment_key.clone();
    local_output.commitment_blind = commitment_blind;
    Ok(Some((local_output, amount)))
}

/// Privacy-enabled wallet manager
pub struct PrivateWalletManager {
    private_wallets: HashMap<String, PrivateWallet>,
    private_pool: PrivateTransactionPool,
    received_nullifiers: HashSet<String>,
    claimed_fee_tx_hashes: HashSet<String>,
    ring_backend: RingSignatureBackend,
}

impl PrivateWalletManager {
    /// Create new private wallet manager
    pub fn new() -> Self {
        PrivateWalletManager {
            private_wallets: HashMap::new(),
            private_pool: PrivateTransactionPool::new(),
            received_nullifiers: HashSet::new(),
            claimed_fee_tx_hashes: HashSet::new(),
            ring_backend: RingSignatureBackend::default(),
        }
    }

    pub fn set_ring_signature_backend(&mut self, ring_backend: RingSignatureBackend) {
        self.ring_backend = ring_backend;
    }

    pub fn ring_signature_backend(&self) -> RingSignatureBackend {
        self.ring_backend
    }

    /// Create new private wallet
    pub fn create_private_wallet(&mut self, name: &str) -> Result<String, BlockError> {
        use rand::RngCore;

        if self.private_wallets.contains_key(name) {
            return Err(BlockError::TransactionValidationFailed(
                "Private wallet already exists".to_string(),
            ));
        }

        // The scan key is the BIP39 entropy. The spend key is derived from it so
        // the printed recovery phrase can restore the full wallet.
        let mut scan_key = vec![0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut scan_key);
        let spending_key = derive_spending_key_from_recovery_seed(&scan_key);

        // Use the spend public key as the private receive address so one-time
        // outputs can be derived from public data and later spent privately.
        let commitment_key = payment_address_from_keys(&scan_key, &spending_key);

        let wallet = PrivateWallet::new(
            name.to_string(),
            spending_key,
            scan_key,
            commitment_key.clone(),
        );
        self.private_wallets.insert(name.to_string(), wallet);

        Ok(commitment_key)
    }

    fn build_sender_ring_members(
        &self,
        sender_wallet: &str,
        recipient_commitment: &str,
        amount: u64,
    ) -> Vec<String> {
        use rand::seq::SliceRandom;

        let mut ring_members = self
            .private_wallets
            .iter()
            .filter(|(name, _)| name.as_str() != sender_wallet)
            .map(|(_, wallet)| wallet.anonymity_public_key.clone())
            .collect::<Vec<_>>();

        let mut rng = rand::rngs::OsRng;
        ring_members.shuffle(&mut rng);
        ring_members.truncate(10);

        let mut counter = 0u64;
        while ring_members.len() < 2 {
            let label = format!(
                "decoy:{}:{}:{}:{}",
                sender_wallet, recipient_commitment, amount, counter
            );
            ring_members.push(deterministic_decoy_public_key(label.as_bytes()));
            counter += 1;
        }

        ring_members
    }

    /// Send private transaction
    pub fn send_private_transaction(
        &mut self,
        from_wallet: &str,
        to_commitment: String,
        amount: u64,
    ) -> Result<String, BlockError> {
        self.send_private_transaction_with_fee_and_membership(
            from_wallet,
            to_commitment,
            amount,
            0,
            String::new(),
            Vec::new(),
        )
    }

    pub fn send_private_transaction_with_fee(
        &mut self,
        from_wallet: &str,
        to_commitment: String,
        amount: u64,
        fee: u64,
    ) -> Result<String, BlockError> {
        self.send_private_transaction_with_fee_and_membership(
            from_wallet,
            to_commitment,
            amount,
            fee,
            String::new(),
            Vec::new(),
        )
    }

    pub fn send_private_transaction_with_state_root(
        &mut self,
        from_wallet: &str,
        to_commitment: String,
        amount: u64,
        private_commitment_root: String,
    ) -> Result<String, BlockError> {
        self.send_private_transaction_with_fee_and_membership(
            from_wallet,
            to_commitment,
            amount,
            0,
            private_commitment_root,
            Vec::new(),
        )
    }

    pub fn preview_private_transaction_input_commitments(
        &mut self,
        from_wallet: &str,
        amount: u64,
    ) -> Result<Vec<String>, BlockError> {
        let wallet = self.private_wallets.get_mut(from_wallet).ok_or_else(|| {
            BlockError::TransactionValidationFailed("Wallet not found".to_string())
        })?;
        wallet.preview_input_commitments(amount)
    }

    pub fn preview_private_transaction_input_commitments_with_fee(
        &mut self,
        from_wallet: &str,
        amount: u64,
        fee: u64,
    ) -> Result<Vec<String>, BlockError> {
        let wallet = self.private_wallets.get_mut(from_wallet).ok_or_else(|| {
            BlockError::TransactionValidationFailed("Wallet not found".to_string())
        })?;
        wallet.preview_input_commitments_with_fee(amount, fee)
    }

    pub fn send_private_transaction_with_membership(
        &mut self,
        from_wallet: &str,
        to_commitment: String,
        amount: u64,
        private_commitment_root: String,
        input_inclusion_proofs: Vec<PrivateCommitmentInclusionProof>,
    ) -> Result<String, BlockError> {
        self.send_private_transaction_with_fee_and_membership(
            from_wallet,
            to_commitment,
            amount,
            0,
            private_commitment_root,
            input_inclusion_proofs,
        )
    }

    pub fn send_private_transaction_with_fee_and_membership(
        &mut self,
        from_wallet: &str,
        to_commitment: String,
        amount: u64,
        fee: u64,
        private_commitment_root: String,
        input_inclusion_proofs: Vec<PrivateCommitmentInclusionProof>,
    ) -> Result<String, BlockError> {
        let ring_members = self.build_sender_ring_members(from_wallet, &to_commitment, amount);
        let wallet = self.private_wallets.get_mut(from_wallet).ok_or_else(|| {
            BlockError::TransactionValidationFailed("Wallet not found".to_string())
        })?;

        let nullifier_set = self.private_pool.nullifier_set();
        let tx = wallet.create_private_transaction(
            amount,
            fee,
            to_commitment,
            nullifier_set,
            ring_members,
            private_commitment_root,
            input_inclusion_proofs,
            self.ring_backend,
        )?;

        let tx_hash = tx.tx_hash.clone();
        self.private_pool.submit_private_transaction(tx)?;

        Ok(tx_hash)
    }

    pub fn attach_inclusion_proofs(
        &mut self,
        tx_hash: &str,
        private_commitment_root: String,
        input_inclusion_proofs: Vec<PrivateCommitmentInclusionProof>,
    ) -> Result<(), BlockError> {
        let expected_root_words =
            fixed_u64_words_from_hex(&private_commitment_root, PRIVATE_TX_ROOT_WORDS);
        let mut updated = self.private_pool.update_transaction(tx_hash, |tx| {
            if tx.proof_private_root_words != expected_root_words {
                return false;
            }
            tx.private_commitment_root = private_commitment_root.clone();
            tx.input_inclusion_proofs = input_inclusion_proofs.clone();
            true
        });

        for wallet in self.private_wallets.values_mut() {
            if let Some(tx) = wallet
                .private_transactions
                .iter_mut()
                .find(|candidate| candidate.tx_hash == tx_hash)
            {
                if tx.proof_private_root_words != expected_root_words {
                    continue;
                }
                tx.private_commitment_root = private_commitment_root.clone();
                tx.input_inclusion_proofs = input_inclusion_proofs.clone();
                updated = true;
            }
        }

        if updated {
            Ok(())
        } else {
            Err(BlockError::TransactionValidationFailed(
                "Private transaction not found for inclusion proof attachment or root anchor mismatch"
                    .to_string(),
            ))
        }
    }

    /// Get private wallet
    pub fn get_private_wallet(&self, name: &str) -> Option<&PrivateWallet> {
        self.private_wallets.get(name)
    }

    /// Get private wallet (mutable)
    pub fn get_private_wallet_mut(&mut self, name: &str) -> Option<&mut PrivateWallet> {
        self.private_wallets.get_mut(name)
    }

    /// Deposit to private wallet
    pub fn deposit_to_private_wallet(&mut self, name: &str, amount: u64) -> Result<(), BlockError> {
        let wallet = self.private_wallets.get_mut(name).ok_or_else(|| {
            BlockError::TransactionValidationFailed("Wallet not found".to_string())
        })?;

        wallet.receive_private_amount(amount);
        Ok(())
    }

    /// Build the spendable private output for a block subsidy. The public
    /// coinbase transaction remains the auditable emission anchor; this is
    /// the wallet-scannable reward output committed into the block body.
    pub fn create_private_coinbase_output(
        &self,
        wallet_name: &str,
        amount: u64,
        block_height: u64,
        recipient_address: &str,
        unlock_height: u64,
    ) -> Result<crate::types::PrivateCoinbaseOutput, BlockError> {
        let wallet = self.private_wallets.get(wallet_name).ok_or_else(|| {
            BlockError::TransactionValidationFailed("Wallet not found".to_string())
        })?;
        wallet.create_private_coinbase_output(
            amount,
            block_height,
            recipient_address,
            unlock_height,
        )
    }

    /// Import a matured private coinbase output as a spendable note.
    pub fn claim_private_coinbase_output(
        &mut self,
        wallet_name: &str,
        source_block_hash: &str,
        output: &crate::types::PrivateCoinbaseOutput,
        expected_amount: u64,
        current_height: u64,
    ) -> Result<u64, BlockError> {
        if current_height < output.unlock_height {
            return Err(BlockError::TransactionValidationFailed(format!(
                "Coinbase output is locked until height {}",
                output.unlock_height
            )));
        }

        let wallet = self.private_wallets.get(wallet_name).ok_or_else(|| {
            BlockError::TransactionValidationFailed("Wallet not found".to_string())
        })?;
        if !constant_time_eq_str(&wallet.commitment_key, &output.recipient_commitment) {
            return Err(BlockError::TransactionValidationFailed(
                "Coinbase output does not belong to wallet".to_string(),
            ));
        }

        let note_output = PrivateOutput {
            output_id: output.output_id.clone(),
            asset_tag: output.asset_tag.clone(),
            asset_tag_commitment: output.asset_tag_commitment.clone(),
            recipient_commitment: output.recipient_commitment.clone(),
            stealth_address: output.stealth_address.clone(),
            ephemeral_pubkey: output.ephemeral_pubkey.clone(),
            commitment: output.commitment.clone(),
            amount_commitment: output.amount_commitment.clone(),
            range_proof: output.range_proof.clone(),
            one_time_public_key: output.one_time_public_key.clone(),
            encrypted_amount: output.encrypted_amount.clone(),
            commitment_blind: output.commitment_blind.clone(),
            is_change: false,
        };
        let amount_domain = coinbase_amount_domain(output.block_height, &output.recipient_address);
        let amount = decode_amount_from_payload(
            &note_output.encrypted_amount,
            &note_output.commitment_blind,
            &note_output.recipient_commitment,
            &amount_domain,
        )?;
        if amount != expected_amount {
            return Err(BlockError::TransactionValidationFailed(format!(
                "Coinbase amount mismatch: expected {expected_amount}, got {amount}"
            )));
        }
        let scan_key = if wallet.scan_key.is_empty() {
            &wallet.spending_key
        } else {
            &wallet.scan_key
        };
        let amount_blind = derive_output_amount_blind_from_note(scan_key, &note_output)
            .ok_or_else(|| {
                BlockError::TransactionValidationFailed(
                    "Failed to derive coinbase amount blind".to_string(),
                )
            })?;
        if !constant_time_eq_str(
            &note_output.amount_commitment,
            &amount_commitment_hex(amount, amount_blind),
        ) {
            return Err(BlockError::TransactionValidationFailed(
                "Coinbase amount commitment mismatch".to_string(),
            ));
        }
        if !verify_output_range(&note_output.amount_commitment, &note_output.range_proof)? {
            return Err(BlockError::TransactionValidationFailed(
                "Coinbase range proof mismatch".to_string(),
            ));
        }

        let wallet = self.private_wallets.get_mut(wallet_name).ok_or_else(|| {
            BlockError::TransactionValidationFailed("Wallet not found".to_string())
        })?;
        let source = format!("coinbase:{}:{}", output.block_height, source_block_hash);
        wallet.import_output_as_note(&source, &note_output, amount);
        Ok(amount)
    }

    /// Get private pool stats
    pub fn get_private_pool_stats(&self) -> (usize, usize, usize, usize) {
        self.private_pool.get_private_pool_stats()
    }

    /// Return a private transaction by hash from any internal bucket or wallet history.
    pub fn get_private_transaction(&self, tx_hash: &str) -> Option<PrivateTransaction> {
        if let Some(tx) = self.private_pool.get_private_transaction(tx_hash) {
            return Some(tx);
        }
        self.private_wallets
            .values()
            .flat_map(|wallet| wallet.private_transactions.iter())
            .find(|tx| tx.tx_hash == tx_hash)
            .cloned()
    }

    /// Build an encrypted fee-opening envelope `(fee, fee_blind)` for the
    /// block-winning miner. This is the sender side of pending item `#1b`.
    pub fn create_miner_fee_claim_envelope(
        &self,
        from_wallet: &str,
        tx_hash: &str,
        miner_commitment_key: &str,
    ) -> Result<PrivateFeeClaimEnvelope, BlockError> {
        self.create_miner_fee_claim_envelope_with_statuses(
            from_wallet,
            tx_hash,
            miner_commitment_key,
            &[PrivacyStatus::Confirmed],
        )
    }

    /// Build the fee-opening envelope while the transaction is being
    /// committed into a block. The claim itself is only redeemable after the
    /// block confirms the referenced private tx hash.
    pub fn create_miner_fee_claim_envelope_for_block(
        &self,
        from_wallet: &str,
        tx_hash: &str,
        miner_commitment_key: &str,
    ) -> Result<PrivateFeeClaimEnvelope, BlockError> {
        self.create_miner_fee_claim_envelope_with_statuses(
            from_wallet,
            tx_hash,
            miner_commitment_key,
            &[
                PrivacyStatus::Pending,
                PrivacyStatus::Verified,
                PrivacyStatus::Confirmed,
            ],
        )
    }

    fn create_miner_fee_claim_envelope_with_statuses(
        &self,
        from_wallet: &str,
        tx_hash: &str,
        miner_commitment_key: &str,
        allowed_statuses: &[PrivacyStatus],
    ) -> Result<PrivateFeeClaimEnvelope, BlockError> {
        let wallet = self.private_wallets.get(from_wallet).ok_or_else(|| {
            BlockError::TransactionValidationFailed("Wallet not found".to_string())
        })?;
        if !wallet
            .private_transactions
            .iter()
            .any(|candidate| candidate.tx_hash == tx_hash)
        {
            return Err(BlockError::TransactionValidationFailed(
                "Sender does not own the referenced private transaction".to_string(),
            ));
        }
        let tx = self.get_private_transaction(tx_hash).ok_or_else(|| {
            BlockError::TransactionValidationFailed(
                "Private transaction not found in local pool/history".to_string(),
            )
        })?;
        if !allowed_statuses.iter().any(|status| status == &tx.status) {
            return Err(BlockError::TransactionValidationFailed(
                "Miner fee claim is not allowed for this private transaction status".to_string(),
            ));
        }
        let opening = tx
            .fee_opening
            .clone()
            .or_else(|| wallet.private_fee_openings.get(tx_hash).cloned())
            .ok_or_else(|| {
                BlockError::TransactionValidationFailed(
                    "Missing local fee opening for this private transaction".to_string(),
                )
            })?;
        if opening.fee == 0 || tx.fee_commitment.is_empty() {
            return Err(BlockError::TransactionValidationFailed(
                "Private transaction has no claimable private fee".to_string(),
            ));
        }

        let (shared_secret, ephemeral_pubkey) =
            fee_claim_shared_secret_from_sender(miner_commitment_key)?;
        let payload = serde_json::to_vec(&PrivateFeeClaimPayload {
            fee: opening.fee,
            fee_blind: opening.fee_blind,
        })
        .map_err(|e| BlockError::SerializationError(e.to_string()))?;
        let mut key_material = shared_secret.to_bytes().to_vec();
        key_material.extend_from_slice(tx_hash.as_bytes());
        let ciphertext = xor_with_blake3_stream(&payload, &key_material, b"zoka-fee-claim-v1");

        Ok(PrivateFeeClaimEnvelope {
            tx_hash: tx_hash.to_string(),
            ephemeral_pubkey,
            ciphertext,
        })
    }

    /// Create encrypted fee claims for locally owned private txs that are
    /// about to be committed by this miner. Transactions without a local fee
    /// opening are skipped; the network still accepts them, but there is no
    /// private-safe way for this node to claim their fee.
    pub fn create_local_private_fee_claims_for_block(
        &self,
        tx_hashes: &[String],
        miner_commitment_key: &str,
    ) -> Result<Vec<crate::types::PrivateFeeClaim>, BlockError> {
        let mut claims = Vec::new();
        for tx_hash in tx_hashes {
            let Some(from_wallet) = self.local_fee_opening_wallet_for_tx(tx_hash) else {
                continue;
            };
            let envelope = self.create_miner_fee_claim_envelope_for_block(
                &from_wallet,
                tx_hash,
                miner_commitment_key,
            )?;
            claims.push(envelope.into());
        }
        Ok(claims)
    }

    fn local_fee_opening_wallet_for_tx(&self, tx_hash: &str) -> Option<String> {
        for (name, wallet) in &self.private_wallets {
            let Some(tx) = wallet
                .private_transactions
                .iter()
                .find(|candidate| candidate.tx_hash == tx_hash)
            else {
                continue;
            };
            let has_opening =
                tx.fee_opening.is_some() || wallet.private_fee_openings.contains_key(tx_hash);
            if has_opening && !tx.fee_commitment.is_empty() {
                return Some(name.clone());
            }
        }
        None
    }

    /// Decrypt and claim a batch of sender-provided fee openings.
    /// Successful claims credit `miner_wallet` with an aggregated spendable
    /// private note amount (`coinbase-style` payout in local wallet state).
    pub fn claim_private_fees_for_wallet(
        &mut self,
        miner_wallet: &str,
        envelopes: &[PrivateFeeClaimEnvelope],
    ) -> Result<PrivateFeeClaimOutcome, BlockError> {
        let scan_key = self
            .private_wallets
            .get(miner_wallet)
            .ok_or_else(|| BlockError::TransactionValidationFailed("Wallet not found".to_string()))?
            .scan_key
            .clone();
        let scan_key = if scan_key.is_empty() {
            self.private_wallets
                .get(miner_wallet)
                .map(|wallet| wallet.spending_key.clone())
                .unwrap_or_default()
        } else {
            scan_key
        };

        let mut request_seen = HashSet::new();
        let mut claimed = Vec::new();
        let mut skipped = Vec::new();
        let mut total_fee = 0u64;

        for envelope in envelopes {
            if self.claimed_fee_tx_hashes.contains(&envelope.tx_hash)
                || !request_seen.insert(envelope.tx_hash.clone())
            {
                skipped.push(envelope.tx_hash.clone());
                continue;
            }

            let shared_secret = match fee_claim_shared_secret_from_receiver(
                &scan_key,
                &envelope.ephemeral_pubkey,
            ) {
                Ok(secret) => secret,
                Err(_) => {
                    skipped.push(envelope.tx_hash.clone());
                    continue;
                }
            };
            let mut key_material = shared_secret.to_bytes().to_vec();
            key_material.extend_from_slice(envelope.tx_hash.as_bytes());
            let plaintext =
                xor_with_blake3_stream(&envelope.ciphertext, &key_material, b"zoka-fee-claim-v1");
            let payload: PrivateFeeClaimPayload = match serde_json::from_slice(&plaintext) {
                Ok(payload) => payload,
                Err(_) => {
                    skipped.push(envelope.tx_hash.clone());
                    continue;
                }
            };
            if payload.fee_blind.len() != 32 {
                skipped.push(envelope.tx_hash.clone());
                continue;
            }
            let mut fee_blind_bytes = [0u8; 32];
            fee_blind_bytes.copy_from_slice(&payload.fee_blind);
            let fee_blind =
                match Option::<Scalar>::from(Scalar::from_canonical_bytes(fee_blind_bytes)) {
                    Some(value) => value,
                    None => {
                        skipped.push(envelope.tx_hash.clone());
                        continue;
                    }
                };

            let tx = match self.get_private_transaction(&envelope.tx_hash) {
                Some(tx) => tx,
                None => {
                    skipped.push(envelope.tx_hash.clone());
                    continue;
                }
            };
            if tx.status != PrivacyStatus::Confirmed || tx.fee_commitment.is_empty() {
                skipped.push(envelope.tx_hash.clone());
                continue;
            }
            if tx.fee_commitment.is_empty() && tx.fee != payload.fee {
                skipped.push(envelope.tx_hash.clone());
                continue;
            }
            if !constant_time_eq_str(
                &tx.fee_commitment,
                &amount_commitment_hex(payload.fee, fee_blind),
            ) {
                skipped.push(envelope.tx_hash.clone());
                continue;
            }

            total_fee = total_fee.checked_add(payload.fee).ok_or_else(|| {
                BlockError::TransactionValidationFailed(
                    "Fee claim total overflowed u64 range".to_string(),
                )
            })?;
            claimed.push(envelope.tx_hash.clone());
        }

        if total_fee == 0 {
            return Err(BlockError::TransactionValidationFailed(
                "No valid private fee claims found".to_string(),
            ));
        }

        self.deposit_to_private_wallet(miner_wallet, total_fee)?;
        for tx_hash in &claimed {
            self.claimed_fee_tx_hashes.insert(tx_hash.clone());
        }

        Ok(PrivateFeeClaimOutcome {
            wallet: miner_wallet.to_string(),
            total_fee_claimed: total_fee,
            claimed_tx_hashes: claimed,
            skipped_tx_hashes: skipped,
        })
    }

    /// Claim fee envelopes committed inside a block body. This is the
    /// miner-side bridge from consensus-visible block metadata to local
    /// wallet crediting.
    pub fn claim_private_fees_from_block_claims(
        &mut self,
        miner_wallet: &str,
        claims: &[crate::types::PrivateFeeClaim],
    ) -> Result<PrivateFeeClaimOutcome, BlockError> {
        let envelopes = claims
            .iter()
            .map(PrivateFeeClaimEnvelope::from)
            .collect::<Vec<_>>();
        self.claim_private_fees_for_wallet(miner_wallet, &envelopes)
    }

    /// List all private wallets
    pub fn list_private_wallets(&self) -> Vec<(String, String, u64, usize)> {
        self.private_wallets
            .iter()
            .map(|(name, wallet)| {
                (
                    name.clone(),
                    wallet.commitment_key.clone(),
                    wallet.private_balance,
                    wallet.private_transaction_count(),
                )
            })
            .collect()
    }

    fn decrypt_matching_outputs_for_wallet(
        &self,
        wallet_name: &str,
        tx: &PrivateTransaction,
    ) -> Result<Vec<(PrivateOutput, u64)>, BlockError> {
        let wallet = self.private_wallets.get(wallet_name).ok_or_else(|| {
            BlockError::TransactionValidationFailed("Wallet not found".to_string())
        })?;

        let mut decrypted = Vec::new();
        for output in tx_outputs(tx) {
            if let Some(match_output) = decrypt_private_output_for_wallet(wallet, tx, &output)? {
                decrypted.push(match_output);
            }
        }

        Ok(decrypted)
    }

    /// Check if we can decrypt a private transaction (if we're the recipient)
    pub fn can_wallet_decrypt_private_tx(
        &self,
        wallet_name: &str,
        tx: &PrivateTransaction,
    ) -> Result<bool, BlockError> {
        Ok(!self
            .decrypt_matching_outputs_for_wallet(wallet_name, tx)?
            .is_empty())
    }

    /// Check if any local private wallet can decrypt this transaction.
    pub fn can_decrypt_private_tx(&self, tx: &PrivateTransaction) -> Result<bool, BlockError> {
        for name in self.private_wallets.keys() {
            if self.can_wallet_decrypt_private_tx(name, tx)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Decrypt amount for a specific recipient wallet.
    pub fn decrypt_private_amount_for_wallet(
        &self,
        wallet_name: &str,
        tx: &PrivateTransaction,
    ) -> Result<u64, BlockError> {
        let decrypted_outputs = self.decrypt_matching_outputs_for_wallet(wallet_name, tx)?;
        if let Some((_, amount)) = decrypted_outputs
            .iter()
            .find(|(output, _)| !output.is_change)
            .or_else(|| decrypted_outputs.first())
        {
            Ok(*amount)
        } else {
            Err(BlockError::TransactionValidationFailed(
                "Transaction recipient does not match wallet commitment".to_string(),
            ))
        }
    }

    /// Decrypt private transaction amount with first matching wallet.
    pub fn decrypt_private_amount(&self, tx: &PrivateTransaction) -> Result<u64, BlockError> {
        for name in self.private_wallets.keys() {
            if let Ok(amount) = self.decrypt_private_amount_for_wallet(name, tx) {
                return Ok(amount);
            }
        }
        Err(BlockError::TransactionValidationFailed(
            "No local wallet can decrypt this private transaction".to_string(),
        ))
    }

    /// Get private wallet names as vector of strings (simplified version)
    pub fn get_private_wallet_names(&self) -> Vec<String> {
        self.private_wallets.keys().cloned().collect()
    }

    /// Get private balance for wallet
    pub fn get_private_balance(&self, name: &str) -> Option<u64> {
        self.private_wallets.get(name).map(|w| w.private_balance)
    }

    pub fn latest_private_note_commitment(&self, name: &str) -> Option<String> {
        self.private_wallets
            .get(name)
            .and_then(|wallet| wallet.private_notes.last())
            .map(|note| note.commitment.clone())
    }

    pub fn receive_private_transaction_for_wallet(
        &mut self,
        wallet_name: &str,
        tx: &PrivateTransaction,
    ) -> Result<u64, BlockError> {
        let decrypted_outputs = self.decrypt_matching_outputs_for_wallet(wallet_name, tx)?;
        let wallet = self.private_wallets.get_mut(wallet_name).ok_or_else(|| {
            BlockError::TransactionValidationFailed("Wallet not found".to_string())
        })?;

        let mut credited = 0u64;
        for (output, amount) in decrypted_outputs {
            wallet.import_output_as_note(&tx.tx_hash, &output, amount);
            credited = credited.saturating_add(amount);
        }
        wallet.sync_balance_cache();
        Ok(credited)
    }

    /// Apply a private transaction to one local wallet when scanning persisted
    /// chain data. This marks locally-owned inputs as spent and imports any
    /// decryptable outputs, including sender change outputs.
    pub fn apply_private_transaction_for_wallet(
        &mut self,
        wallet_name: &str,
        tx: &PrivateTransaction,
    ) -> Result<u64, BlockError> {
        let decrypted_outputs = self.decrypt_matching_outputs_for_wallet(wallet_name, tx)?;
        let input_nullifiers = tx_input_nullifiers(tx);
        let input_nullifiers = input_nullifiers.into_iter().collect::<HashSet<_>>();
        let wallet = self.private_wallets.get_mut(wallet_name).ok_or_else(|| {
            BlockError::TransactionValidationFailed("Wallet not found".to_string())
        })?;

        let spent_at = current_timestamp();
        for note in &mut wallet.private_notes {
            if note.spent {
                continue;
            }
            let nullifier =
                derive_note_nullifier(&wallet.spending_key, &note.note_id, &note.commitment);
            if input_nullifiers.contains(&nullifier) {
                note.spent = true;
                note.spent_at = Some(spent_at);
            }
        }

        let mut credited = 0u64;
        for (output, amount) in decrypted_outputs {
            if wallet
                .private_notes
                .iter()
                .any(|existing| existing.note_id == output.output_id)
            {
                continue;
            }
            wallet.import_output_as_note(&tx.tx_hash, &output, amount);
            credited = credited.saturating_add(amount);
        }
        if !wallet
            .private_transactions
            .iter()
            .any(|existing| existing.tx_hash == tx.tx_hash)
        {
            wallet.private_transactions.push(tx.clone());
        }
        wallet.sync_balance_cache();
        Ok(credited)
    }

    /// Confirm a private transaction (for miners/validators)
    pub fn confirm_private_transaction(&mut self, tx_hash: &str) -> Result<(), BlockError> {
        let pool_result = self.private_pool.confirm_private_transaction(tx_hash);
        let wallet_updated =
            self.update_wallet_private_transaction_status(tx_hash, PrivacyStatus::Confirmed);
        if pool_result.is_ok() || wallet_updated {
            Ok(())
        } else {
            pool_result
        }
    }

    /// Mark locally stored wallet copies as confirmed after a mined block
    /// commits the corresponding private tx hashes.
    pub fn mark_private_transactions_confirmed_locally(&mut self, tx_hashes: &[String]) -> usize {
        tx_hashes
            .iter()
            .filter(|tx_hash| {
                self.update_wallet_private_transaction_status(tx_hash, PrivacyStatus::Confirmed)
            })
            .count()
    }

    fn update_wallet_private_transaction_status(
        &mut self,
        tx_hash: &str,
        status: PrivacyStatus,
    ) -> bool {
        let mut updated = false;
        for wallet in self.private_wallets.values_mut() {
            for tx in &mut wallet.private_transactions {
                if tx.tx_hash == tx_hash && tx.status != status {
                    tx.status = status.clone();
                    updated = true;
                }
            }
        }
        updated
    }

    /// Mark inbound private transaction nullifier as seen and validate optional proof.
    pub fn validate_inbound_private_transaction(
        &mut self,
        tx: &PrivateTransaction,
    ) -> Result<(), BlockError> {
        let input_nullifiers = tx_input_nullifiers(tx);
        if input_nullifiers.is_empty() {
            return Err(BlockError::TransactionValidationFailed(
                "Missing nullifier in private transaction".to_string(),
            ));
        }

        let mut unique_nullifiers = input_nullifiers.clone();
        unique_nullifiers.sort();
        unique_nullifiers.dedup();
        if unique_nullifiers.len() != input_nullifiers.len() {
            return Err(BlockError::TransactionValidationFailed(
                "Duplicate input nullifiers in private transaction".to_string(),
            ));
        }
        if input_nullifiers
            .iter()
            .any(|nullifier| self.received_nullifiers.contains(nullifier))
        {
            return Err(BlockError::TransactionValidationFailed(
                "Duplicate private transaction nullifier".to_string(),
            ));
        }

        let outputs = tx_outputs(tx);
        if outputs.is_empty() {
            return Err(BlockError::TransactionValidationFailed(
                "Private transaction must contain at least one output".to_string(),
            ));
        }
        if outputs.iter().any(|output| output.range_proof.is_empty()) {
            return Err(BlockError::TransactionValidationFailed(
                "Missing Bulletproof range proof in private transaction output".to_string(),
            ));
        }
        if tx.zk_proof.is_some()
            && outputs.iter().any(|output| {
                output.stealth_address.is_empty() || output.ephemeral_pubkey.is_empty()
            })
        {
            return Err(BlockError::TransactionValidationFailed(
                "Missing stealth address metadata in private transaction".to_string(),
            ));
        }

        let mut unique_output_ids = outputs
            .iter()
            .map(|output| output.output_id.clone())
            .collect::<Vec<_>>();
        unique_output_ids.sort();
        unique_output_ids.dedup();
        if unique_output_ids.len() != outputs.len() {
            return Err(BlockError::TransactionValidationFailed(
                "Duplicate output identifiers in private transaction".to_string(),
            ));
        }

        let mut unique_output_pubkeys = outputs
            .iter()
            .map(|output| output.one_time_public_key.clone())
            .collect::<Vec<_>>();
        unique_output_pubkeys.sort();
        unique_output_pubkeys.dedup();
        if unique_output_pubkeys.len() != outputs.len() {
            return Err(BlockError::TransactionValidationFailed(
                "Duplicate one-time output public keys in private transaction".to_string(),
            ));
        }

        if !tx.input_public_keys.is_empty() && tx.input_public_keys.len() != input_nullifiers.len()
        {
            return Err(BlockError::TransactionValidationFailed(
                "Input public key count does not match spent note count".to_string(),
            ));
        }
        if !tx.input_commitments.is_empty() && tx.input_commitments.len() != input_nullifiers.len()
        {
            return Err(BlockError::TransactionValidationFailed(
                "Input commitment count does not match spent note count".to_string(),
            ));
        }
        if tx.zk_proof.is_some()
            && !tx.input_commitments.is_empty()
            && tx.input_inclusion_proofs.len() != tx.input_commitments.len()
        {
            return Err(BlockError::TransactionValidationFailed(
                "Private transaction proof requires inclusion proofs for every spent input"
                    .to_string(),
            ));
        }
        if !tx.input_key_images.is_empty() && tx.input_key_images.len() != input_nullifiers.len() {
            return Err(BlockError::TransactionValidationFailed(
                "Input key image count does not match spent note count".to_string(),
            ));
        }
        if !tx.input_amount_commitments.is_empty()
            && tx.input_amount_commitments.len() != input_nullifiers.len()
        {
            return Err(BlockError::TransactionValidationFailed(
                "Input amount commitment count does not match spent note count".to_string(),
            ));
        }
        if tx.input_key_images.len() > PRIVATE_TX_MAX_INPUTS {
            return Err(BlockError::TransactionValidationFailed(format!(
                "Private transaction exceeds Groth16 input bound ({})",
                PRIVATE_TX_MAX_INPUTS
            )));
        }

        let mut unique_input_pubkeys = tx.input_public_keys.clone();
        unique_input_pubkeys.sort();
        unique_input_pubkeys.dedup();
        if unique_input_pubkeys.len() != tx.input_public_keys.len() {
            return Err(BlockError::TransactionValidationFailed(
                "Duplicate input public keys in private transaction".to_string(),
            ));
        }

        let mut unique_input_key_images = tx.input_key_images.clone();
        unique_input_key_images.sort();
        unique_input_key_images.dedup();
        if unique_input_key_images.len() != tx.input_key_images.len() {
            return Err(BlockError::TransactionValidationFailed(
                "Duplicate input key images in private transaction".to_string(),
            ));
        }

        let mut unique_input_commitments = tx.input_commitments.clone();
        unique_input_commitments.sort();
        unique_input_commitments.dedup();
        if unique_input_commitments.len() != tx.input_commitments.len() {
            return Err(BlockError::TransactionValidationFailed(
                "Duplicate input commitments in private transaction".to_string(),
            ));
        }
        if outputs
            .iter()
            .any(|output| output.amount_commitment.is_empty())
        {
            return Err(BlockError::TransactionValidationFailed(
                "Missing amount commitment in private transaction output".to_string(),
            ));
        }
        if outputs
            .iter()
            .any(|output| output.asset_tag.trim().is_empty())
        {
            return Err(BlockError::TransactionValidationFailed(
                "Missing asset tag in private transaction output".to_string(),
            ));
        }
        if outputs
            .iter()
            .any(|output| output.asset_tag_commitment.is_empty())
        {
            return Err(BlockError::TransactionValidationFailed(
                "Missing asset tag commitment in private transaction output".to_string(),
            ));
        }
        if outputs.iter().any(|output| {
            !constant_time_eq_str(
                &output.asset_tag_commitment,
                &asset_tag_commitment_hex(&output.asset_tag),
            )
        }) {
            return Err(BlockError::TransactionValidationFailed(
                "Asset tag commitment mismatch in private transaction output".to_string(),
            ));
        }
        let first_output_asset_tag = outputs[0].asset_tag.clone();
        let input_asset_tags = if tx.input_asset_tags.is_empty() {
            vec![first_output_asset_tag.clone(); input_nullifiers.len()]
        } else {
            tx.input_asset_tags.clone()
        };
        if input_asset_tags.len() != input_nullifiers.len() {
            return Err(BlockError::TransactionValidationFailed(
                "Input asset tag count does not match spent note count".to_string(),
            ));
        }
        if tx.input_amount_commitments.is_empty() {
            return Err(BlockError::TransactionValidationFailed(
                "Missing input amount commitments in private transaction".to_string(),
            ));
        }
        let mut input_amount_commitment_sums: HashMap<String, RistrettoPoint> = HashMap::new();
        for (asset_tag, commitment_hex) in input_asset_tags
            .iter()
            .zip(tx.input_amount_commitments.iter())
        {
            let point = point_from_hex(commitment_hex).ok_or_else(|| {
                BlockError::TransactionValidationFailed(
                    "Malformed input amount commitment in private transaction".to_string(),
                )
            })?;
            *input_amount_commitment_sums
                .entry(asset_tag.clone())
                .or_insert_with(RistrettoPoint::default) += point;
        }
        let mut output_amount_commitment_sums: HashMap<String, RistrettoPoint> = HashMap::new();
        for output in &outputs {
            let point = point_from_hex(&output.amount_commitment).ok_or_else(|| {
                BlockError::TransactionValidationFailed(
                    "Malformed output amount commitment in private transaction".to_string(),
                )
            })?;
            *output_amount_commitment_sums
                .entry(output.asset_tag.clone())
                .or_insert_with(RistrettoPoint::default) += point;
        }
        let mut amount_balance_commitment_sums: HashMap<String, RistrettoPoint> = HashMap::new();
        if tx.asset_balance_commitments.is_empty() {
            let amount_balance_commitment = point_from_hex(&tx.amount_balance_commitment)
                .ok_or_else(|| {
                    BlockError::TransactionValidationFailed(
                        "Malformed amount balance commitment in private transaction".to_string(),
                    )
                })?;
            amount_balance_commitment_sums
                .insert(first_output_asset_tag.clone(), amount_balance_commitment);
        } else {
            for entry in &tx.asset_balance_commitments {
                if entry.asset_tag.trim().is_empty() || entry.balance_commitment.trim().is_empty() {
                    return Err(BlockError::TransactionValidationFailed(
                        "Malformed asset balance commitment entry".to_string(),
                    ));
                }
                let point = point_from_hex(&entry.balance_commitment).ok_or_else(|| {
                    BlockError::TransactionValidationFailed(
                        "Malformed asset balance commitment in private transaction".to_string(),
                    )
                })?;
                if amount_balance_commitment_sums
                    .insert(entry.asset_tag.clone(), point)
                    .is_some()
                {
                    return Err(BlockError::TransactionValidationFailed(
                        "Duplicate asset tag in asset balance commitments".to_string(),
                    ));
                }
            }
        }
        // Private-fee path: the tx carries a Pedersen commitment hiding `fee`.
        // Legacy path: `fee_commitment` is empty, fall back to the visible
        // `fee·G + 0·H` reconstruction from `tx.fee`. Mixing the two
        // (non-empty commitment but inconsistent range proof) is rejected.
        let fee_component_point = if tx.fee_commitment.is_empty() {
            fee_amount_commitment_point(tx.fee)
        } else {
            if tx.fee_range_proof.is_empty() {
                return Err(BlockError::TransactionValidationFailed(
                    "Private transaction fee commitment requires a range proof".to_string(),
                ));
            }
            if !verify_output_range(&tx.fee_commitment, &tx.fee_range_proof)? {
                return Err(BlockError::TransactionValidationFailed(
                    "Private transaction fee range proof is invalid".to_string(),
                ));
            }
            point_from_hex(&tx.fee_commitment).ok_or_else(|| {
                BlockError::TransactionValidationFailed(
                    "Malformed fee commitment in private transaction".to_string(),
                )
            })?
        };
        let fee_asset_tag = if tx.fee_asset_tag.trim().is_empty() {
            first_output_asset_tag.clone()
        } else {
            tx.fee_asset_tag.clone()
        };
        let mut all_asset_tags = std::collections::BTreeSet::new();
        all_asset_tags.extend(input_amount_commitment_sums.keys().cloned());
        all_asset_tags.extend(output_amount_commitment_sums.keys().cloned());
        all_asset_tags.extend(amount_balance_commitment_sums.keys().cloned());
        for asset_tag in all_asset_tags {
            let input_sum = input_amount_commitment_sums
                .get(&asset_tag)
                .copied()
                .unwrap_or_default();
            let output_sum = output_amount_commitment_sums
                .get(&asset_tag)
                .copied()
                .unwrap_or_default();
            let balance_sum = amount_balance_commitment_sums
                .get(&asset_tag)
                .copied()
                .unwrap_or_default();
            let fee_sum = if constant_time_eq_str(&asset_tag, &fee_asset_tag) {
                fee_component_point
            } else {
                RistrettoPoint::default()
            };
            if input_sum != output_sum + fee_sum + balance_sum {
                return Err(BlockError::TransactionValidationFailed(format!(
                    "Private transaction amount commitments do not preserve value for asset {}",
                    asset_tag
                )));
            }
        }
        for output in &outputs {
            if !verify_output_range(&output.amount_commitment, &output.range_proof)? {
                return Err(BlockError::TransactionValidationFailed(
                    "Private transaction output range proof is invalid".to_string(),
                ));
            }
        }

        if !tx.input_inclusion_proofs.is_empty() {
            if tx.private_commitment_root.is_empty() {
                return Err(BlockError::TransactionValidationFailed(
                    "Private transaction inclusion proofs require a commitment root".to_string(),
                ));
            }
            if tx.input_inclusion_proofs.len() != tx.input_commitments.len() {
                return Err(BlockError::TransactionValidationFailed(
                    "Input inclusion proof count does not match input commitments".to_string(),
                ));
            }
            for (commitment, proof) in tx
                .input_commitments
                .iter()
                .zip(tx.input_inclusion_proofs.iter())
            {
                if !constant_time_eq_str(commitment, &proof.commitment) {
                    return Err(BlockError::TransactionValidationFailed(
                        "Private transaction inclusion proof commitment mismatch".to_string(),
                    ));
                }
                if !StateManager::verify_private_commitment_inclusion_proof(
                    &tx.private_commitment_root,
                    proof,
                ) {
                    return Err(BlockError::TransactionValidationFailed(
                        "Private transaction input inclusion proof is invalid".to_string(),
                    ));
                }
            }
        }

        if tx.ring_members.len() < 3 {
            return Err(BlockError::TransactionValidationFailed(
                "Private transaction ring must contain at least 3 members".to_string(),
            ));
        }

        let mut unique_ring_members = tx.ring_members.clone();
        unique_ring_members.sort();
        unique_ring_members.dedup();
        if unique_ring_members.len() != tx.ring_members.len() {
            return Err(BlockError::TransactionValidationFailed(
                "Duplicate ring members in private transaction".to_string(),
            ));
        }

        let expected_ring_id = ring_id_from_members(&tx.ring_members);
        if !constant_time_eq_str(&tx.ring_id, &expected_ring_id) {
            return Err(BlockError::TransactionValidationFailed(
                "Invalid private transaction ring identifier".to_string(),
            ));
        }
        let signature_backend = backend_for_ring_version(tx.ring_version).ok_or_else(|| {
            BlockError::TransactionValidationFailed(format!(
                "Unsupported private transaction ring version: {}",
                tx.ring_version
            ))
        })?;

        let ring_domain = tx_ring_domain(tx);
        let message = ring_message(
            &tx.tx_hash,
            &tx.commitment,
            &tx.nullifier,
            &ring_domain,
            &tx.ring_members,
            &tx.input_public_keys,
            &tx.input_key_images,
        );
        if !verify_ring_signature_with_backend(
            signature_backend,
            &tx.ring_members,
            &tx.input_public_keys,
            &tx.input_key_images,
            &tx.tx_hash,
            &ring_domain,
            &message,
            &tx.ring_signature,
        ) {
            return Err(BlockError::TransactionValidationFailed(
                "Invalid private transaction ring signature".to_string(),
            ));
        }

        if let Some(primary_input_public_key) = tx.input_public_keys.first() {
            if !tx
                .ring_members
                .iter()
                .any(|member| constant_time_eq_str(member, primary_input_public_key))
            {
                return Err(BlockError::TransactionValidationFailed(
                    "Primary input public key missing from ring".to_string(),
                ));
            }
        }

        if !tx.input_key_images.is_empty() {
            let per_input_signature_len = 64usize
                .checked_add(tx.ring_members.len().checked_mul(32).ok_or_else(|| {
                    BlockError::TransactionValidationFailed(
                        "Invalid private transaction ring size".to_string(),
                    )
                })?)
                .ok_or_else(|| {
                    BlockError::TransactionValidationFailed(
                        "Invalid private transaction ring signature length".to_string(),
                    )
                })?;
            if tx.ring_signature.len()
                != tx
                    .input_key_images
                    .len()
                    .checked_mul(per_input_signature_len)
                    .ok_or_else(|| {
                        BlockError::TransactionValidationFailed(
                            "Invalid private transaction ring signature bundle".to_string(),
                        )
                    })?
            {
                return Err(BlockError::TransactionValidationFailed(
                    "Private transaction ring signature bundle size mismatch".to_string(),
                ));
            }
        }

        if tx.proof_input_sum != 0
            || tx.proof_output_sum != 0
            || tx.proof_fee != 0
            || tx.proof_key_image != 0
        {
            if tx
                .proof_output_sum
                .checked_add(tx.proof_fee)
                .ok_or_else(|| {
                    BlockError::TransactionValidationFailed(
                        "Private transaction proof balance overflow".to_string(),
                    )
                })?
                != tx.proof_input_sum
            {
                return Err(BlockError::TransactionValidationFailed(
                    "Private transaction proof does not preserve value".to_string(),
                ));
            }
            if tx.fee != tx.proof_fee {
                return Err(BlockError::TransactionValidationFailed(
                    "Private transaction fee does not match proof fee".to_string(),
                ));
            }
            if let Some(primary_key_image) = tx.input_key_images.first() {
                if tx.proof_key_image != first_u64_from_hex(primary_key_image) {
                    return Err(BlockError::TransactionValidationFailed(
                        "Private transaction proof key image does not match primary input"
                            .to_string(),
                    ));
                }
            }
        }
        if !tx.private_commitment_root.is_empty() {
            let expected_root_words =
                fixed_u64_words_from_hex(&tx.private_commitment_root, PRIVATE_TX_ROOT_WORDS);
            if tx.proof_private_root_words != expected_root_words {
                return Err(BlockError::TransactionValidationFailed(
                    "Private transaction proof root anchor does not match commitment root"
                        .to_string(),
                ));
            }
        }

        if let Some(proof_bytes) = &tx.zk_proof {
            let verifier = Verifier::new();
            let public_inputs = proof_public_inputs_for_tx(tx)?;
            let proof = crate::zk::prover::ZKProof::new(
                proof_bytes.clone(),
                "private_tx".to_string(),
                public_inputs,
                current_timestamp(),
            );
            let result = verifier.verify(&proof)?;
            if !result.is_valid {
                return Err(BlockError::TransactionValidationFailed(
                    "Invalid private transaction proof".to_string(),
                ));
            }
        }

        for nullifier in input_nullifiers {
            self.received_nullifiers.insert(nullifier);
        }
        Ok(())
    }

    /// Rotate wallet spending key and return new commitment key.
    pub fn rotate_private_wallet_key(&mut self, wallet_name: &str) -> Result<String, BlockError> {
        let wallet = self.private_wallets.get_mut(wallet_name).ok_or_else(|| {
            BlockError::TransactionValidationFailed("Wallet not found".to_string())
        })?;
        Ok(wallet.rotate_spending_key())
    }
}

/// Helper function to get current timestamp
fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::HDWallet;
    use proptest::prelude::*;

    #[test]
    fn test_private_wallet_creation() {
        let mut manager = PrivateWalletManager::new();
        let result = manager.create_private_wallet("alice");
        assert!(result.is_ok());
        assert!(manager.get_private_wallet("alice").is_some());
        assert!(result.unwrap().starts_with("zpriv:"));
    }

    #[test]
    fn test_private_wallet_mnemonic_restores_same_address() {
        let mut manager = PrivateWalletManager::new();
        let original_address = manager.create_private_wallet("alice").unwrap();
        let wallet = manager.get_private_wallet("alice").unwrap();
        let mnemonic = HDWallet::new(wallet.scan_key.clone())
            .unwrap()
            .seed_to_mnemonic()
            .unwrap();
        let recovered_scan_key = HDWallet::mnemonic_to_seed(&mnemonic).unwrap();
        let recovered_spending_key = derive_spending_key_from_recovery_seed(&recovered_scan_key);
        let recovered_address =
            payment_address_from_keys(&recovered_scan_key, &recovered_spending_key);

        assert_eq!(wallet.scan_key, recovered_scan_key);
        assert_eq!(wallet.spending_key, recovered_spending_key);
        assert_eq!(original_address, recovered_address);
    }

    #[test]
    fn test_private_wallet_duplicate() {
        let mut manager = PrivateWalletManager::new();
        let _ = manager.create_private_wallet("alice");
        let result = manager.create_private_wallet("alice");
        assert!(result.is_err());
    }

    #[test]
    fn test_deposit_to_private_wallet() {
        let mut manager = PrivateWalletManager::new();
        manager.create_private_wallet("alice").unwrap();
        manager.deposit_to_private_wallet("alice", 100).unwrap();

        let wallet = manager.get_private_wallet("alice").unwrap();
        assert_eq!(wallet.private_balance, 100);
    }

    #[test]
    fn test_private_transaction_insufficient_balance() {
        let mut manager = PrivateWalletManager::new();
        manager.create_private_wallet("alice").unwrap();
        manager.deposit_to_private_wallet("alice", 50).unwrap();

        let result = manager.send_private_transaction("alice", "commitment_key".to_string(), 100);

        assert!(result.is_err());
    }

    #[test]
    fn test_private_transaction_success() {
        let mut manager = PrivateWalletManager::new();
        manager.create_private_wallet("alice").unwrap();
        manager.deposit_to_private_wallet("alice", 100).unwrap();
        let bob_address = manager.create_private_wallet("bob").unwrap();

        let result = manager.send_private_transaction("alice", bob_address, 50);

        assert!(result.is_ok());
        let wallet = manager.get_private_wallet("alice").unwrap();
        assert_eq!(wallet.private_balance, 50);
    }

    #[test]
    fn test_private_pool_stats() {
        let mut manager = PrivateWalletManager::new();
        manager.create_private_wallet("alice").unwrap();
        manager.deposit_to_private_wallet("alice", 100).unwrap();
        let bob_address = manager.create_private_wallet("bob").unwrap();

        manager
            .send_private_transaction("alice", bob_address, 30)
            .unwrap();

        let (pending, verified, _confirmed, _failed) = manager.get_private_pool_stats();
        assert!(pending > 0 || verified > 0);
    }

    #[test]
    fn test_list_private_wallets() {
        let mut manager = PrivateWalletManager::new();
        manager.create_private_wallet("alice").unwrap();
        manager.create_private_wallet("bob").unwrap();
        manager.deposit_to_private_wallet("alice", 100).unwrap();
        manager.deposit_to_private_wallet("bob", 50).unwrap();

        let wallets = manager.list_private_wallets();
        assert_eq!(wallets.len(), 2);
        let balances_by_name: std::collections::HashMap<String, u64> = wallets
            .into_iter()
            .map(|(name, _commitment, balance, _tx_count)| (name, balance))
            .collect();
        assert_eq!(balances_by_name.get("alice"), Some(&100));
        assert_eq!(balances_by_name.get("bob"), Some(&50));
    }

    #[test]
    fn test_private_tx_decrypt_for_matching_wallet() {
        let mut manager = PrivateWalletManager::new();
        manager.create_private_wallet("alice").unwrap();
        let bob_commitment = manager.create_private_wallet("bob").unwrap();
        manager.deposit_to_private_wallet("alice", 100).unwrap();

        let tx_hash = manager
            .send_private_transaction("alice", bob_commitment, 40)
            .unwrap();
        let tx = manager
            .get_private_transaction(&tx_hash)
            .expect("tx should exist");

        assert!(manager
            .can_wallet_decrypt_private_tx("bob", &tx)
            .expect("bob should be valid"));
        assert!(manager
            .can_wallet_decrypt_private_tx("alice", &tx)
            .expect("alice change output check should be valid"));

        let amount = manager
            .decrypt_private_amount_for_wallet("bob", &tx)
            .expect("bob should decrypt");
        assert_eq!(amount, 40);
        let alice_change = manager
            .decrypt_private_amount_for_wallet("alice", &tx)
            .expect("alice should decrypt change");
        assert_eq!(alice_change, 60);
    }

    #[test]
    fn test_private_transaction_hides_public_recipient_fee_and_change_metadata() {
        let mut manager = PrivateWalletManager::new();
        manager.create_private_wallet("alice").unwrap();
        let bob_commitment = manager.create_private_wallet("bob").unwrap();
        manager.deposit_to_private_wallet("alice", 50).unwrap();

        let tx_hash = manager
            .send_private_transaction_with_fee("alice", bob_commitment, 20, 3)
            .unwrap();
        let tx = manager.get_private_transaction(&tx_hash).unwrap();

        assert!(tx.recipient_commitment.is_empty());
        assert_eq!(tx.fee, 0);
        assert!(!tx.fee_commitment.is_empty());
        assert!(tx.outputs.iter().all(|output| {
            output.recipient_commitment.is_empty()
                && output.commitment_blind.is_empty()
                && !output.is_change
                && output.encrypted_amount.len() == PRIVATE_NOTE_PAYLOAD_V2_LEN
        }));
        assert_eq!(
            manager
                .decrypt_private_amount_for_wallet("bob", &tx)
                .expect("bob can still scan one-time output"),
            20
        );
    }

    #[test]
    fn test_rotate_private_wallet_key_changes_commitment() {
        let mut manager = PrivateWalletManager::new();
        let original = manager.create_private_wallet("alice").unwrap();
        let rotated = manager.rotate_private_wallet_key("alice").unwrap();
        assert_ne!(original, rotated);
    }

    #[test]
    fn test_validate_inbound_private_transaction_duplicate_nullifier() {
        let mut manager = PrivateWalletManager::new();
        manager.create_private_wallet("alice").unwrap();
        let bob_commitment = manager.create_private_wallet("bob").unwrap();
        manager.deposit_to_private_wallet("alice", 100).unwrap();
        let tx_hash = manager
            .send_private_transaction("alice", bob_commitment, 10)
            .unwrap();
        let tx = manager.get_private_transaction(&tx_hash).unwrap();

        assert!(manager.validate_inbound_private_transaction(&tx).is_ok());
        assert!(manager.validate_inbound_private_transaction(&tx).is_err());
    }

    #[tokio::test]
    async fn test_attach_inclusion_proofs_rejects_root_anchor_mismatch() {
        let mut manager = PrivateWalletManager::new();
        manager.create_private_wallet("alice").unwrap();
        let bob_commitment = manager.create_private_wallet("bob").unwrap();
        manager.deposit_to_private_wallet("alice", 100).unwrap();
        let tx_hash = manager
            .send_private_transaction("alice", bob_commitment, 10)
            .unwrap();

        let test_dir = ".zoka/test-suite/.test_private_wallet_attach_root_mismatch";
        let _ = std::fs::remove_dir_all(test_dir);
        let storage = Arc::new(crate::storage::RocksDbStorage::new(test_dir).unwrap());
        let state_manager = Arc::new(StateManager::new(storage));
        let input_commitment = manager
            .get_private_transaction(&tx_hash)
            .unwrap()
            .input_commitments[0]
            .clone();
        state_manager
            .append_private_commitment(input_commitment.clone())
            .await
            .unwrap();
        state_manager
            .append_private_commitment("different-state-commitment".to_string())
            .await
            .unwrap();
        let root = state_manager.get_private_commitment_root().await;
        let proof = state_manager
            .get_private_commitment_inclusion_proof(&input_commitment)
            .await
            .unwrap();

        let result = manager.attach_inclusion_proofs(&tx_hash, root, vec![proof]);
        assert!(result.is_err());

        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[test]
    fn test_private_transaction_includes_sender_ring() {
        let mut manager = PrivateWalletManager::new();
        manager.create_private_wallet("alice").unwrap();
        let bob_commitment = manager.create_private_wallet("bob").unwrap();
        manager.create_private_wallet("carol").unwrap();
        manager.deposit_to_private_wallet("alice", 100).unwrap();

        let tx_hash = manager
            .send_private_transaction("alice", bob_commitment, 25)
            .unwrap();
        let tx = manager.get_private_transaction(&tx_hash).unwrap();

        assert!(tx.ring_members.len() >= 3);
        assert_eq!(tx.ring_id, ring_id_from_members(&tx.ring_members));
        assert!(!tx.input_nullifiers.is_empty());
        assert_eq!(tx.input_public_keys.len(), tx.input_nullifiers.len());
        assert_eq!(tx.input_key_images.len(), tx.input_nullifiers.len());
        assert_eq!(tx.input_amount_commitments.len(), tx.input_nullifiers.len());
        assert_eq!(tx.outputs.len(), 2);
        assert!(tx
            .outputs
            .iter()
            .all(|output| !output.one_time_public_key.is_empty()
                && !output.amount_commitment.is_empty()));
        let backend = backend_for_ring_version(tx.ring_version).expect("known ring version");
        let ring_domain = tx_ring_domain(&tx);
        assert!(verify_ring_signature_with_backend(
            backend,
            &tx.ring_members,
            &tx.input_public_keys,
            &tx.input_key_images,
            &tx.tx_hash,
            &ring_domain,
            &ring_message(
                &tx.tx_hash,
                &tx.commitment,
                &tx.nullifier,
                &ring_domain,
                &tx.ring_members,
                &tx.input_public_keys,
                &tx.input_key_images,
            ),
            &tx.ring_signature,
        ));
        assert!(tx
            .ring_members
            .iter()
            .any(|member| member == &tx.input_public_keys[0]));
    }

    #[test]
    fn test_private_transaction_clsag_v1_backend_roundtrip() {
        let mut manager = PrivateWalletManager::new();
        manager.set_ring_signature_backend(RingSignatureBackend::ClsagV1);
        manager.create_private_wallet("alice").unwrap();
        let bob_commitment = manager.create_private_wallet("bob").unwrap();
        manager.create_private_wallet("carol").unwrap();
        manager.deposit_to_private_wallet("alice", 100).unwrap();

        let tx_hash = manager
            .send_private_transaction("alice", bob_commitment, 25)
            .unwrap();
        let tx = manager.get_private_transaction(&tx_hash).unwrap();
        assert!(manager.validate_inbound_private_transaction(&tx).is_ok());
    }

    #[test]
    fn test_private_transaction_clsag_v1_rejects_tampered_signature() {
        let mut manager = PrivateWalletManager::new();
        manager.set_ring_signature_backend(RingSignatureBackend::ClsagV1);
        manager.create_private_wallet("alice").unwrap();
        let bob_commitment = manager.create_private_wallet("bob").unwrap();
        manager.create_private_wallet("carol").unwrap();
        manager.deposit_to_private_wallet("alice", 100).unwrap();

        let tx_hash = manager
            .send_private_transaction("alice", bob_commitment, 25)
            .unwrap();
        let mut tx = manager.get_private_transaction(&tx_hash).unwrap();
        tx.ring_signature[0] ^= 0x01;
        assert!(manager.validate_inbound_private_transaction(&tx).is_err());
    }

    #[test]
    fn test_private_transaction_validation_dispatches_by_ring_version() {
        let mut manager = PrivateWalletManager::new();
        manager.set_ring_signature_backend(RingSignatureBackend::ClsagV1);
        manager.create_private_wallet("alice").unwrap();
        let bob_commitment = manager.create_private_wallet("bob").unwrap();
        manager.create_private_wallet("carol").unwrap();
        manager.deposit_to_private_wallet("alice", 100).unwrap();

        let tx_hash = manager
            .send_private_transaction("alice", bob_commitment, 15)
            .unwrap();
        let tx = manager.get_private_transaction(&tx_hash).unwrap();
        assert_eq!(tx.ring_version, 1);

        manager.set_ring_signature_backend(RingSignatureBackend::LegacyLinkable);
        assert!(
            manager.validate_inbound_private_transaction(&tx).is_ok(),
            "verification must honor tx.ring_version instead of local default backend"
        );
    }

    #[test]
    fn test_private_transaction_rejects_unknown_ring_version() {
        let mut manager = PrivateWalletManager::new();
        manager.create_private_wallet("alice").unwrap();
        let bob_commitment = manager.create_private_wallet("bob").unwrap();
        manager.create_private_wallet("carol").unwrap();
        manager.deposit_to_private_wallet("alice", 100).unwrap();

        let tx_hash = manager
            .send_private_transaction("alice", bob_commitment, 15)
            .unwrap();
        let mut tx = manager.get_private_transaction(&tx_hash).unwrap();
        tx.ring_version = 99;
        assert!(manager.validate_inbound_private_transaction(&tx).is_err());
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(16))]

        #[test]
        fn prop_private_transaction_roundtrip_and_validation(amount in 1u64..=10_000u64) {
            let mut manager = PrivateWalletManager::new();
            manager.create_private_wallet("alice").unwrap();
            let bob_commitment = manager.create_private_wallet("bob").unwrap();
            manager.create_private_wallet("carol").unwrap();
            manager.deposit_to_private_wallet("alice", amount + 5).unwrap();

            let tx_hash = manager
                .send_private_transaction("alice", bob_commitment, amount)
                .unwrap();
            let tx = manager.get_private_transaction(&tx_hash).unwrap();

            prop_assert!(manager.validate_inbound_private_transaction(&tx).is_ok());
            prop_assert_eq!(
                manager.decrypt_private_amount_for_wallet("bob", &tx).unwrap(),
                amount
            );
        }

        #[test]
        fn prop_tampered_ring_id_is_rejected(amount in 1u64..=5_000u64) {
            let mut manager = PrivateWalletManager::new();
            manager.create_private_wallet("alice").unwrap();
            let bob_commitment = manager.create_private_wallet("bob").unwrap();
            manager.create_private_wallet("carol").unwrap();
            manager.deposit_to_private_wallet("alice", amount + 5).unwrap();

            let tx_hash = manager
                .send_private_transaction("alice", bob_commitment, amount)
                .unwrap();
            let mut tx = manager.get_private_transaction(&tx_hash).unwrap();
            tx.ring_id.push('x');

            prop_assert!(manager.validate_inbound_private_transaction(&tx).is_err());
        }

        /// Any non-zero `fee` must produce a Pedersen `fee_commitment`
        /// whose Bulletproof verifies against itself, and the resulting
        /// tx must pass inbound validation (balance equation closes
        /// after subtracting `fee_blind` from the amount_balance_commit).
        #[test]
        fn prop_private_fee_produces_valid_range_proof(
            amount in 1u64..=1_000u64,
            fee in 1u64..=50u64,
        ) {
            let mut manager = PrivateWalletManager::new();
            manager.create_private_wallet("alice").unwrap();
            let bob_commitment = manager.create_private_wallet("bob").unwrap();
            manager.deposit_to_private_wallet("alice", amount + fee + 10).unwrap();

            let tx_hash = manager
                .send_private_transaction_with_fee("alice", bob_commitment, amount, fee)
                .unwrap();
            let tx = manager.get_private_transaction(&tx_hash).unwrap();

            prop_assert!(!tx.fee_commitment.is_empty());
            prop_assert!(!tx.fee_range_proof.is_empty());
            prop_assert!(verify_output_range(&tx.fee_commitment, &tx.fee_range_proof).unwrap());
            prop_assert!(manager.validate_inbound_private_transaction(&tx).is_ok());
        }

        /// Tampering with the fee range proof breaks inbound validation
        /// even though the fee_commitment point is unchanged — the
        /// validator re-verifies the Bulletproof, not just the commit.
        #[test]
        fn prop_tampered_fee_range_proof_is_rejected(
            amount in 1u64..=500u64,
            fee in 1u64..=20u64,
        ) {
            let mut manager = PrivateWalletManager::new();
            manager.create_private_wallet("alice").unwrap();
            let bob_commitment = manager.create_private_wallet("bob").unwrap();
            manager.deposit_to_private_wallet("alice", amount + fee + 5).unwrap();

            let tx_hash = manager
                .send_private_transaction_with_fee("alice", bob_commitment, amount, fee)
                .unwrap();
            let mut tx = manager.get_private_transaction(&tx_hash).unwrap();
            // Flip the first byte of the range proof. Any corruption
            // must be caught.
            if let Some(b) = tx.fee_range_proof.first_mut() {
                *b ^= 0x01;
            }

            prop_assert!(manager.validate_inbound_private_transaction(&tx).is_err());
        }

        /// Swapping the fee commitment with a random Ristretto point
        /// must break validation. This catches the class of attacks
        /// where an adversary substitutes a commitment with the same
        /// serialized shape but a different value+blind pair.
        #[test]
        fn prop_swapped_fee_commitment_is_rejected(
            amount in 1u64..=200u64,
            fee in 1u64..=15u64,
        ) {
            let mut manager = PrivateWalletManager::new();
            manager.create_private_wallet("alice").unwrap();
            let bob_commitment = manager.create_private_wallet("bob").unwrap();
            manager.deposit_to_private_wallet("alice", amount + fee + 5).unwrap();

            let tx_hash = manager
                .send_private_transaction_with_fee("alice", bob_commitment, amount, fee)
                .unwrap();
            let mut tx = manager.get_private_transaction(&tx_hash).unwrap();

            // Replace fee_commitment with a point derived from a
            // different fee value — same encoding shape, different
            // semantic meaning. Must fail balance equation.
            use curve25519_dalek::scalar::Scalar as DalekScalar;
            let rogue_blind = DalekScalar::from(fee.wrapping_add(7));
            tx.fee_commitment = amount_commitment_hex(fee + 1, rogue_blind);

            prop_assert!(manager.validate_inbound_private_transaction(&tx).is_err());
        }

        /// Double-spend detection: the same nullifier cannot clear
        /// validation twice. Ring signatures prevent using a note
        /// twice in one tx; the nullifier set prevents using it
        /// across two distinct txs.
        #[test]
        fn prop_duplicate_nullifier_across_tx_rejected(
            amount in 1u64..=100u64,
        ) {
            let mut manager = PrivateWalletManager::new();
            manager.create_private_wallet("alice").unwrap();
            let bob = manager.create_private_wallet("bob").unwrap();
            manager.deposit_to_private_wallet("alice", amount + 50).unwrap();

            let tx1_hash = manager
                .send_private_transaction("alice", bob.clone(), amount)
                .unwrap();
            let tx1 = manager.get_private_transaction(&tx1_hash).unwrap();

            // First validation succeeds and records the nullifiers.
            prop_assert!(manager.validate_inbound_private_transaction(&tx1).is_ok());

            // Rebroadcast the SAME tx — its nullifiers are already
            // marked as spent; a compliant validator must reject.
            prop_assert!(manager.validate_inbound_private_transaction(&tx1).is_err());
        }

        #[test]
        fn prop_tampered_ring_signature_is_rejected(
            amount in 1u64..=500u64,
            flip_index in 0usize..4096usize,
        ) {
            let mut manager = PrivateWalletManager::new();
            manager.create_private_wallet("alice").unwrap();
            let bob = manager.create_private_wallet("bob").unwrap();
            manager.create_private_wallet("carol").unwrap();
            manager.deposit_to_private_wallet("alice", amount + 10).unwrap();

            let tx_hash = manager
                .send_private_transaction("alice", bob, amount)
                .unwrap();
            let mut tx = manager.get_private_transaction(&tx_hash).unwrap();
            prop_assume!(!tx.ring_signature.is_empty());
            let idx = flip_index % tx.ring_signature.len();
            tx.ring_signature[idx] ^= 0x01;

            prop_assert!(manager.validate_inbound_private_transaction(&tx).is_err());
        }

        #[test]
        fn prop_new_transactions_always_emit_clsag_ring_version(
            amount in 1u64..=500u64,
            force_legacy_backend in any::<bool>(),
        ) {
            let mut manager = PrivateWalletManager::new();
            if force_legacy_backend {
                manager.set_ring_signature_backend(RingSignatureBackend::LegacyLinkable);
            } else {
                manager.set_ring_signature_backend(RingSignatureBackend::ClsagV1);
            }
            manager.create_private_wallet("alice").unwrap();
            let bob = manager.create_private_wallet("bob").unwrap();
            manager.deposit_to_private_wallet("alice", amount + 10).unwrap();

            let tx_hash = manager
                .send_private_transaction("alice", bob, amount)
                .unwrap();
            let tx = manager.get_private_transaction(&tx_hash).unwrap();
            prop_assert_eq!(tx.ring_version, 1);
        }
    }

    #[test]
    fn test_private_transaction_aggregates_multiple_notes_and_preserves_change() {
        let mut manager = PrivateWalletManager::new();
        manager.create_private_wallet("alice").unwrap();
        let bob_commitment = manager.create_private_wallet("bob").unwrap();
        manager.create_private_wallet("carol").unwrap();
        manager.deposit_to_private_wallet("alice", 30).unwrap();
        manager.deposit_to_private_wallet("alice", 20).unwrap();

        let tx_hash = manager
            .send_private_transaction("alice", bob_commitment, 40)
            .unwrap();
        let tx = manager.get_private_transaction(&tx_hash).unwrap();
        let alice_wallet = manager.get_private_wallet("alice").unwrap();

        assert_eq!(tx.input_nullifiers.len(), 2);
        assert_eq!(tx.input_public_keys.len(), 2);
        assert_eq!(tx.input_key_images.len(), 2);
        assert_eq!(tx.outputs.len(), 2);
        assert_eq!(alice_wallet.private_balance, 10);
        assert_eq!(
            tx.outputs.iter().filter(|output| output.is_change).count(),
            0,
            "public tx must not reveal which output is sender change"
        );
    }

    #[test]
    fn test_private_transaction_with_fee_preserves_fee_in_balance() {
        let mut manager = PrivateWalletManager::new();
        manager.create_private_wallet("alice").unwrap();
        let bob_commitment = manager.create_private_wallet("bob").unwrap();
        manager.deposit_to_private_wallet("alice", 50).unwrap();

        let tx_hash = manager
            .send_private_transaction_with_fee("alice", bob_commitment, 20, 3)
            .unwrap();
        let tx = manager.get_private_transaction(&tx_hash).unwrap();
        let alice_wallet = manager.get_private_wallet("alice").unwrap();

        assert_eq!(tx.fee, 0, "private-fee tx must not reveal clear fee");
        assert_eq!(tx.proof_fee, 0);
        assert_eq!(alice_wallet.private_balance, 27);

        // Non-zero fee must produce a Pedersen fee commitment + range proof.
        assert!(
            !tx.fee_commitment.is_empty(),
            "fee > 0 must emit a Pedersen fee_commitment"
        );
        assert!(
            !tx.fee_range_proof.is_empty(),
            "fee > 0 must emit a Bulletproof range proof for the fee"
        );
        assert!(
            verify_output_range(&tx.fee_commitment, &tx.fee_range_proof).unwrap(),
            "fee_range_proof must verify against fee_commitment"
        );

        // The fee_commitment must NOT decompress to `fee·G + 0·H`; if it did,
        // the fee would be trivially recoverable on-chain and we'd have
        // changed nothing. With a random blind, the recovered point differs.
        let visible_fee_point = fee_amount_commitment_point(3);
        let private_fee_point = point_from_hex(&tx.fee_commitment)
            .expect("fee_commitment must decode as a Ristretto point");
        assert_ne!(
            private_fee_point, visible_fee_point,
            "private fee commitment should not equal the visible fee·G encoding"
        );
    }

    #[test]
    fn test_private_transaction_zero_fee_skips_fee_commitment() {
        // Zero-fee tx stays on the legacy visible-fee path (empty fee_commitment,
        // validated against fee·G = identity). This keeps backwards compatibility
        // with existing tx fixtures and the many tests that don't care about fees.
        let mut manager = PrivateWalletManager::new();
        manager.create_private_wallet("alice").unwrap();
        let bob_commitment = manager.create_private_wallet("bob").unwrap();
        manager.deposit_to_private_wallet("alice", 30).unwrap();

        let tx_hash = manager
            .send_private_transaction("alice", bob_commitment, 10)
            .unwrap();
        let tx = manager.get_private_transaction(&tx_hash).unwrap();

        assert_eq!(tx.fee, 0);
        assert!(tx.fee_commitment.is_empty());
        assert!(tx.fee_range_proof.is_empty());
    }

    #[test]
    fn test_confirm_private_transaction_updates_wallet_history_copy() {
        let mut manager = PrivateWalletManager::new();
        manager.create_private_wallet("alice").unwrap();
        let bob_commitment = manager.create_private_wallet("bob").unwrap();
        manager.deposit_to_private_wallet("alice", 30).unwrap();

        let tx_hash = manager
            .send_private_transaction("alice", bob_commitment, 10)
            .unwrap();
        let before = manager
            .get_private_wallet("alice")
            .unwrap()
            .private_transactions
            .iter()
            .find(|tx| tx.tx_hash == tx_hash)
            .unwrap();
        assert_eq!(before.status, PrivacyStatus::Pending);

        manager.confirm_private_transaction(&tx_hash).unwrap();

        let after = manager
            .get_private_wallet("alice")
            .unwrap()
            .private_transactions
            .iter()
            .find(|tx| tx.tx_hash == tx_hash)
            .unwrap();
        assert_eq!(after.status, PrivacyStatus::Confirmed);
    }

    #[test]
    fn test_private_fee_opening_persists_in_local_wallet_map_only() {
        let mut manager = PrivateWalletManager::new();
        manager.create_private_wallet("alice").unwrap();
        let bob_commitment = manager.create_private_wallet("bob").unwrap();
        manager.deposit_to_private_wallet("alice", 30).unwrap();

        let tx_hash = manager
            .send_private_transaction_with_fee("alice", bob_commitment, 10, 2)
            .unwrap();
        let tx = manager.get_private_transaction(&tx_hash).unwrap();
        assert!(tx.fee_opening.is_some());

        let tx_json = serde_json::to_string(&tx).unwrap();
        assert!(
            !tx_json.contains("fee_opening"),
            "public private-tx serialization must not expose the fee opening"
        );

        let wallet = manager.get_private_wallet("alice").unwrap();
        assert!(wallet.private_fee_openings.contains_key(&tx_hash));
        let wallet_json = serde_json::to_string(wallet).unwrap();
        assert!(wallet_json.contains("private_fee_openings"));
        let restored: PrivateWallet = serde_json::from_str(&wallet_json).unwrap();
        assert!(restored.private_fee_openings.contains_key(&tx_hash));
    }

    #[test]
    fn test_local_private_fee_claims_for_block_credit_miner() {
        let mut manager = PrivateWalletManager::new();
        manager.create_private_wallet("alice").unwrap();
        let bob_commitment = manager.create_private_wallet("bob").unwrap();
        let miner_commitment = manager.create_private_wallet("miner").unwrap();

        manager.deposit_to_private_wallet("alice", 40).unwrap();
        let tx_hash = manager
            .send_private_transaction_with_fee("alice", bob_commitment, 10, 3)
            .unwrap();

        let claims = manager
            .create_local_private_fee_claims_for_block(
                std::slice::from_ref(&tx_hash),
                &miner_commitment,
            )
            .unwrap();
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].tx_hash, tx_hash);
        assert!(!claims[0].ephemeral_pubkey.is_empty());
        assert!(!claims[0].ciphertext.is_empty());

        manager.confirm_private_transaction(&tx_hash).unwrap();
        let outcome = manager
            .claim_private_fees_from_block_claims("miner", &claims)
            .unwrap();

        assert_eq!(outcome.total_fee_claimed, 3);
        assert_eq!(outcome.claimed_tx_hashes, vec![tx_hash]);
        assert!(outcome.skipped_tx_hashes.is_empty());
        assert_eq!(manager.get_private_balance("miner"), Some(3));
    }

    #[test]
    fn test_fractional_zka_private_tx_fee_claim_uses_atomic_units() {
        let mut manager = PrivateWalletManager::new();
        manager.create_private_wallet("alice").unwrap();
        let bob_commitment = manager.create_private_wallet("bob").unwrap();
        let miner_commitment = manager.create_private_wallet("miner").unwrap();

        let deposit = crate::economics::parse_zka_amount("2.5").unwrap();
        let amount = crate::economics::parse_zka_amount("0.001").unwrap();
        let fee = crate::economics::parse_zka_amount("0.0001").unwrap();
        manager.deposit_to_private_wallet("alice", deposit).unwrap();

        let tx_hash = manager
            .send_private_transaction_with_fee("alice", bob_commitment, amount, fee)
            .unwrap();
        let tx = manager.get_private_transaction(&tx_hash).unwrap().clone();

        assert_eq!(
            manager.get_private_balance("alice"),
            Some(deposit - amount - fee)
        );
        assert_eq!(tx.fee, 0, "private fee must not reveal clear atoms");
        assert!(!tx.fee_commitment.is_empty());

        let credited = manager
            .receive_private_transaction_for_wallet("bob", &tx)
            .unwrap();
        assert_eq!(credited, amount);
        assert_eq!(manager.get_private_balance("bob"), Some(amount));

        let claims = manager
            .create_local_private_fee_claims_for_block(
                std::slice::from_ref(&tx_hash),
                &miner_commitment,
            )
            .unwrap();
        assert_eq!(claims.len(), 1);

        manager.confirm_private_transaction(&tx_hash).unwrap();
        let outcome = manager
            .claim_private_fees_from_block_claims("miner", &claims)
            .unwrap();

        assert_eq!(outcome.total_fee_claimed, fee);
        assert_eq!(manager.get_private_balance("miner"), Some(fee));
        assert_eq!(crate::economics::format_zka_amount(fee), "0.0001");
    }

    #[test]
    fn test_zero_fee_transaction_creates_no_local_fee_claim() {
        let mut manager = PrivateWalletManager::new();
        manager.create_private_wallet("alice").unwrap();
        let bob_commitment = manager.create_private_wallet("bob").unwrap();
        let miner_commitment = manager.create_private_wallet("miner").unwrap();

        manager.deposit_to_private_wallet("alice", 30).unwrap();
        let tx_hash = manager
            .send_private_transaction("alice", bob_commitment, 10)
            .unwrap();

        let claims = manager
            .create_local_private_fee_claims_for_block(&[tx_hash], &miner_commitment)
            .unwrap();
        assert!(claims.is_empty());
    }

    #[test]
    fn test_miner_fee_claim_envelope_and_aggregate_claim() {
        let mut manager = PrivateWalletManager::new();
        manager.create_private_wallet("alice").unwrap();
        let bob_commitment = manager.create_private_wallet("bob").unwrap();
        let miner_commitment = manager.create_private_wallet("miner").unwrap();

        manager.deposit_to_private_wallet("alice", 80).unwrap();
        let tx_hash = manager
            .send_private_transaction_with_fee("alice", bob_commitment, 20, 4)
            .unwrap();
        manager.confirm_private_transaction(&tx_hash).unwrap();

        let envelope = manager
            .create_miner_fee_claim_envelope("alice", &tx_hash, &miner_commitment)
            .unwrap();
        let outcome = manager
            .claim_private_fees_for_wallet("miner", &[envelope])
            .unwrap();

        assert_eq!(outcome.total_fee_claimed, 4);
        assert_eq!(outcome.claimed_tx_hashes, vec![tx_hash.clone()]);
        assert!(outcome.skipped_tx_hashes.is_empty());
        assert_eq!(manager.get_private_balance("miner"), Some(4));
    }

    #[test]
    fn test_miner_fee_claim_from_block_claim_metadata() {
        let mut manager = PrivateWalletManager::new();
        manager.create_private_wallet("alice").unwrap();
        let bob_commitment = manager.create_private_wallet("bob").unwrap();
        let miner_commitment = manager.create_private_wallet("miner").unwrap();

        manager.deposit_to_private_wallet("alice", 80).unwrap();
        let tx_hash = manager
            .send_private_transaction_with_fee("alice", bob_commitment, 20, 5)
            .unwrap();
        manager.confirm_private_transaction(&tx_hash).unwrap();

        let envelope = manager
            .create_miner_fee_claim_envelope("alice", &tx_hash, &miner_commitment)
            .unwrap();
        let block_claim: crate::types::PrivateFeeClaim = envelope.into();
        let outcome = manager
            .claim_private_fees_from_block_claims("miner", &[block_claim])
            .unwrap();

        assert_eq!(outcome.total_fee_claimed, 5);
        assert_eq!(manager.get_private_balance("miner"), Some(5));
    }

    #[test]
    fn test_miner_fee_claim_rejects_duplicate_and_tampered_envelopes() {
        let mut manager = PrivateWalletManager::new();
        manager.create_private_wallet("alice").unwrap();
        let bob_commitment = manager.create_private_wallet("bob").unwrap();
        let miner_commitment = manager.create_private_wallet("miner").unwrap();

        manager.deposit_to_private_wallet("alice", 90).unwrap();
        let tx_hash = manager
            .send_private_transaction_with_fee("alice", bob_commitment, 15, 3)
            .unwrap();
        manager.confirm_private_transaction(&tx_hash).unwrap();

        let envelope = manager
            .create_miner_fee_claim_envelope("alice", &tx_hash, &miner_commitment)
            .unwrap();
        manager
            .claim_private_fees_for_wallet("miner", std::slice::from_ref(&envelope))
            .unwrap();
        assert_eq!(manager.get_private_balance("miner"), Some(3));

        let duplicate = manager.claim_private_fees_for_wallet("miner", &[envelope.clone()]);
        assert!(duplicate.is_err(), "duplicate claim must be rejected");

        let bob_commitment_2 = manager.create_private_wallet("bob2").unwrap();
        let tx_hash_2 = manager
            .send_private_transaction_with_fee("alice", bob_commitment_2, 10, 2)
            .unwrap();
        manager.confirm_private_transaction(&tx_hash_2).unwrap();

        let mut tampered = manager
            .create_miner_fee_claim_envelope("alice", &tx_hash_2, &miner_commitment)
            .unwrap();
        if let Some(first) = tampered.ciphertext.first_mut() {
            *first ^= 0x01;
        }
        let tampered_outcome = manager.claim_private_fees_for_wallet("miner", &[tampered]);
        assert!(
            tampered_outcome.is_err(),
            "tampered envelope must be rejected"
        );
        assert_eq!(manager.get_private_balance("miner"), Some(3));
    }

    #[test]
    fn test_private_coinbase_output_claims_after_maturity() {
        let mut manager = PrivateWalletManager::new();
        let miner_commitment = manager.create_private_wallet("miner").unwrap();
        let output = manager
            .create_private_coinbase_output("miner", 12, 7, "zka1miner", 67)
            .unwrap();

        assert_eq!(output.block_height, 7);
        assert_eq!(output.unlock_height, 67);
        assert_eq!(output.recipient_commitment, miner_commitment);
        assert_eq!(manager.get_private_balance("miner"), Some(0));

        let early = manager.claim_private_coinbase_output("miner", "block-hash", &output, 12, 66);
        assert!(early.is_err());
        assert_eq!(manager.get_private_balance("miner"), Some(0));

        let credited = manager
            .claim_private_coinbase_output("miner", "block-hash", &output, 12, 67)
            .unwrap();
        assert_eq!(credited, 12);
        assert_eq!(manager.get_private_balance("miner"), Some(12));
    }

    #[test]
    fn test_private_coinbase_claim_rejects_wrong_amount() {
        let mut manager = PrivateWalletManager::new();
        manager.create_private_wallet("miner").unwrap();
        let output = manager
            .create_private_coinbase_output("miner", 12, 7, "zka1miner", 67)
            .unwrap();

        let result = manager.claim_private_coinbase_output("miner", "block-hash", &output, 11, 67);
        assert!(result.is_err());
        assert_eq!(manager.get_private_balance("miner"), Some(0));
    }

    #[test]
    fn test_private_tx_hash_changes_with_output_asset_tag() {
        let mut output = PrivateOutput {
            output_id: "out-1".to_string(),
            asset_tag: "ZOKA".to_string(),
            asset_tag_commitment: asset_tag_commitment_hex("ZOKA"),
            recipient_commitment: "recipient".to_string(),
            stealth_address: "stealth".to_string(),
            ephemeral_pubkey: vec![1u8; 32],
            commitment: "commitment".to_string(),
            amount_commitment: "amount-commitment".to_string(),
            range_proof: vec![2u8; 32],
            one_time_public_key: "one-time-key".to_string(),
            encrypted_amount: vec![3u8; 8],
            commitment_blind: vec![4u8; 32],
            is_change: false,
        };

        let hash_a = private_tx_hash(
            &[String::from("nullifier-1")],
            &[String::from("commitment-1")],
            &[String::from("ZOKA")],
            "root-1",
            &[String::from("pubkey-1")],
            &[String::from("key-image-1")],
            &[String::from("amount-commitment-1")],
            "balance-commitment-1",
            "ZOKA",
            &[AssetBalanceCommitment {
                asset_tag: "ZOKA".to_string(),
                balance_commitment: "balance-commitment-1".to_string(),
            }],
            "fee-commitment-1",
            &[output.clone()],
        );

        output.asset_tag = "USDZ".to_string();
        let hash_b = private_tx_hash(
            &[String::from("nullifier-1")],
            &[String::from("commitment-1")],
            &[String::from("USDZ")],
            "root-1",
            &[String::from("pubkey-1")],
            &[String::from("key-image-1")],
            &[String::from("amount-commitment-1")],
            "balance-commitment-1",
            "USDZ",
            &[AssetBalanceCommitment {
                asset_tag: "USDZ".to_string(),
                balance_commitment: "balance-commitment-1".to_string(),
            }],
            "fee-commitment-1",
            &[output],
        );

        assert_ne!(hash_a, hash_b);
    }

    #[test]
    fn test_validate_inbound_private_transaction_rejects_asset_tag_commitment_mismatch() {
        let mut manager = PrivateWalletManager::new();
        manager.create_private_wallet("alice").unwrap();
        let bob_commitment = manager.create_private_wallet("bob").unwrap();
        manager.deposit_to_private_wallet("alice", 100).unwrap();

        let tx_hash = manager
            .send_private_transaction("alice", bob_commitment, 20)
            .unwrap();
        let mut tx = manager.get_private_transaction(&tx_hash).unwrap();
        tx.outputs[0].asset_tag_commitment = "bad-commitment".to_string();

        let result = manager.validate_inbound_private_transaction(&tx);
        assert!(result.is_err());
    }
}
