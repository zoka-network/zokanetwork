//! In-browser private scan crypto for the Zoka explorer.
//!
//! This module mirrors the minimal read-only crypto from
//! `src/private_wallet/mod.rs` so the public explorer can scan encrypted
//! private transaction bodies without sending a scan key to any server.

use curve25519_dalek::constants::RISTRETTO_BASEPOINT_POINT;
use curve25519_dalek::ristretto::{CompressedRistretto, RistrettoPoint};
use curve25519_dalek::scalar::Scalar;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256, Sha512};
use wasm_bindgen::prelude::*;

const PRIVATE_NOTE_PAYLOAD_V2_LEN: usize = 40;

fn default_asset_tag() -> String {
    "ZOKA".to_string()
}

fn constant_time_eq_str(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (left, right) in a.iter().zip(b.iter()) {
        diff |= left ^ right;
    }
    diff == 0
}

fn signing_seed_from_bytes(bytes: &[u8]) -> [u8; 32] {
    let digest = Sha256::digest(bytes);
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&digest[..32]);
    seed
}

fn scalar_from_secret_bytes(bytes: &[u8]) -> Scalar {
    Scalar::from_bytes_mod_order(signing_seed_from_bytes(bytes))
}

fn hash_to_scalar(parts: &[&[u8]]) -> Scalar {
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

fn derive_spending_key_from_recovery_seed(seed_entropy: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(b"zoka-private-wallet-spend-v1");
    hasher.update([0x1f]);
    hasher.update(seed_entropy);
    hasher.finalize().to_vec()
}

fn payment_address_from_keys(scan_key: &[u8], spending_key: &[u8]) -> String {
    format!(
        "zpriv:{}:{}",
        public_key_hex(scan_key),
        public_key_hex(spending_key)
    )
}

fn payment_address_from_scan_key_bytes(scan_key: &[u8]) -> Result<String, String> {
    if scan_key.len() != 16 && scan_key.len() != 32 {
        return Err("scan key must encode 16 or 32 bytes".to_string());
    }
    let spending_key = derive_spending_key_from_recovery_seed(scan_key);
    Ok(payment_address_from_keys(scan_key, &spending_key))
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

fn validate_view_scan_key_for_address(
    scan_key: &[u8],
    payment_address: &str,
) -> Result<(RistrettoPoint, RistrettoPoint), String> {
    let (scan_public, spend_public) = parse_payment_address(payment_address)
        .ok_or_else(|| "payment address is not a valid zpriv address".to_string())?;
    let derived_scan_public = scalar_from_secret_bytes(scan_key) * RISTRETTO_BASEPOINT_POINT;
    if derived_scan_public != scan_public {
        return Err("scan private key does not match this address".to_string());
    }
    Ok((scan_public, spend_public))
}

fn amount_blind_from_shared(shared_bytes: &[u8], commitment: &str) -> Scalar {
    hash_to_scalar(&[b"zoka-amount-blind", shared_bytes, commitment.as_bytes()])
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

fn decrypt_private_note_payload_v2(
    payload: &[u8],
    amount_blind: Scalar,
    commitment: &str,
    nullifier_domain: &str,
) -> Result<(u64, Vec<u8>), String> {
    if payload.len() != PRIVATE_NOTE_PAYLOAD_V2_LEN {
        return Err("Invalid private note payload length".to_string());
    }
    let key = private_note_payload_key(amount_blind, commitment, nullifier_domain);
    let plaintext = xor_with_blake3_stream(payload, &key, b"zoka-private-note-payload-v2");
    let mut amount_bytes = [0u8; 8];
    amount_bytes.copy_from_slice(&plaintext[..8]);
    Ok((u64::from_le_bytes(amount_bytes), plaintext[8..].to_vec()))
}

fn decode_amount_from_payload(
    payload: &[u8],
    commitment_blind: &[u8],
    recipient_commitment: &str,
    nullifier: &str,
) -> Result<u64, String> {
    if payload.len() != 8 {
        return Err("Invalid encrypted amount payload".to_string());
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

fn commitment_new_blinded_hex(data: &[u8], blindness: &[u8]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(data);
    hasher.update(blindness);
    hex::encode(hasher.finalize().as_bytes())
}

fn derive_output_amount_blind_from_note(
    scan_key: &[u8],
    output: &PrivateOutput,
) -> Option<Scalar> {
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

fn derive_output_public_key(
    recipient_commitment: &str,
    commitment: &str,
    ephemeral_pubkey: &[u8],
) -> Option<String> {
    let (scan_public, spend_public) = parse_payment_address(recipient_commitment)?;
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
    Some(point_to_hex(
        &(spend_public + derived_scalar * RISTRETTO_BASEPOINT_POINT),
    ))
}

#[derive(Clone, Debug, Deserialize)]
struct PrivateOutput {
    #[serde(default)]
    output_id: String,
    #[serde(default = "default_asset_tag")]
    asset_tag: String,
    #[serde(default)]
    recipient_commitment: String,
    #[serde(default)]
    stealth_address: String,
    #[serde(default)]
    ephemeral_pubkey: Vec<u8>,
    #[serde(default)]
    commitment: String,
    #[serde(default)]
    one_time_public_key: String,
    #[serde(default)]
    encrypted_amount: Vec<u8>,
    #[serde(default)]
    commitment_blind: Vec<u8>,
    #[serde(default)]
    is_change: bool,
}

#[derive(Clone, Debug, Deserialize)]
struct PrivateTransaction {
    #[serde(default)]
    tx_hash: String,
    #[serde(default)]
    commitment: String,
    #[serde(default)]
    recipient_commitment: String,
    #[serde(default)]
    stealth_address: String,
    #[serde(default)]
    ephemeral_pubkey: Vec<u8>,
    #[serde(default)]
    encrypted_amount: Vec<u8>,
    #[serde(default)]
    outputs: Vec<PrivateOutput>,
    #[serde(default)]
    nullifier: String,
    #[serde(default)]
    timestamp: u64,
    #[serde(default)]
    commitment_blind: Vec<u8>,
}

#[derive(Debug, Serialize)]
struct PrivateViewScanMatch {
    tx_hash: String,
    output_id: String,
    output_index: usize,
    asset_tag: String,
    amount: u64,
    commitment: String,
    stealth_address: String,
    one_time_public_key: String,
    is_change: bool,
    timestamp: u64,
}

#[derive(Debug, Serialize)]
struct PrivateTxScanResponse {
    address: String,
    scanned_transactions: usize,
    matching_outputs: usize,
    total_amount_atoms: u64,
    matches: Vec<PrivateViewScanMatch>,
}

fn legacy_output_from_tx(tx: &PrivateTransaction) -> Option<PrivateOutput> {
    if tx.commitment.is_empty() {
        return None;
    }
    let one_time_public_key = derive_output_public_key(
        &tx.recipient_commitment,
        &tx.commitment,
        &tx.ephemeral_pubkey,
    )?;
    Some(PrivateOutput {
        output_id: format!("{}:0", tx.tx_hash),
        asset_tag: default_asset_tag(),
        recipient_commitment: tx.recipient_commitment.clone(),
        stealth_address: tx.stealth_address.clone(),
        ephemeral_pubkey: tx.ephemeral_pubkey.clone(),
        commitment: tx.commitment.clone(),
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

fn scan_private_transaction_inner(
    scan_key: &[u8],
    payment_address: &str,
    tx: &PrivateTransaction,
) -> Result<Vec<PrivateViewScanMatch>, String> {
    let (_, spend_public) = validate_view_scan_key_for_address(scan_key, payment_address)?;
    let mut matches = Vec::new();

    for (output_index, output) in tx_outputs(tx).into_iter().enumerate() {
        let Some(amount_blind) = derive_output_amount_blind_from_note(scan_key, &output) else {
            continue;
        };
        let Some(ephemeral_public) = output
            .ephemeral_pubkey
            .as_slice()
            .try_into()
            .ok()
            .and_then(|bytes: [u8; 32]| CompressedRistretto(bytes).decompress())
        else {
            continue;
        };
        let shared_point = scalar_from_secret_bytes(scan_key) * ephemeral_public;
        let derived_scalar = hash_to_scalar(&[
            b"zoka-output-key",
            shared_point.compress().as_bytes(),
            output.commitment.as_bytes(),
        ]);
        let derived_public =
            point_to_hex(&(spend_public + derived_scalar * RISTRETTO_BASEPOINT_POINT));
        if !constant_time_eq_str(&derived_public, &output.one_time_public_key) {
            continue;
        }

        let (amount, commitment_blind) =
            if output.encrypted_amount.len() == PRIVATE_NOTE_PAYLOAD_V2_LEN {
                decrypt_private_note_payload_v2(
                    &output.encrypted_amount,
                    amount_blind,
                    &output.commitment,
                    &tx.nullifier,
                )?
            } else {
                let recipient_commitment = if output.recipient_commitment.is_empty() {
                    payment_address
                } else {
                    output.recipient_commitment.as_str()
                };
                if output.commitment_blind.is_empty() {
                    return Err("Legacy private output is missing commitment blind".to_string());
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

        let expected_commitment = commitment_new_blinded_hex(&amount.to_le_bytes(), &commitment_blind);
        if !constant_time_eq_str(&expected_commitment, &output.commitment) {
            return Err("Commitment mismatch while scanning private amount".to_string());
        }

        matches.push(PrivateViewScanMatch {
            tx_hash: tx.tx_hash.clone(),
            output_id: output.output_id.clone(),
            output_index,
            asset_tag: output.asset_tag.clone(),
            amount,
            commitment: output.commitment.clone(),
            stealth_address: output.stealth_address.clone(),
            one_time_public_key: output.one_time_public_key.clone(),
            is_change: output.is_change,
            timestamp: tx.timestamp,
        });
    }

    Ok(matches)
}

/// Derive the `zpriv:scan_pub:spend_pub` payment address from a hex scan key,
/// entirely in the browser. Same output as the node's
/// `payment_address_from_scan_key`.
#[wasm_bindgen]
pub fn derive_payment_address(scan_key_hex: &str) -> Result<String, JsValue> {
    let clean = scan_key_hex.trim().trim_start_matches("0x");
    let bytes = hex::decode(clean).map_err(|e| JsValue::from_str(&format!("bad hex: {e}")))?;
    payment_address_from_scan_key_bytes(&bytes).map_err(|e| JsValue::from_str(&e))
}

/// Scan one archived private transaction body using only a scan key.
///
/// The input transaction body is public encrypted chain data. The scan key is
/// used only inside this WASM module and never leaves the browser.
#[wasm_bindgen]
pub fn scan_private_transaction(scan_key_hex: &str, tx_json: &str) -> Result<String, JsValue> {
    let clean = scan_key_hex.trim().trim_start_matches("0x");
    let scan_key = hex::decode(clean).map_err(|e| JsValue::from_str(&format!("bad hex: {e}")))?;
    let address = payment_address_from_scan_key_bytes(&scan_key).map_err(|e| JsValue::from_str(&e))?;
    let tx: PrivateTransaction =
        serde_json::from_str(tx_json).map_err(|e| JsValue::from_str(&format!("bad tx json: {e}")))?;
    let matches = scan_private_transaction_inner(&scan_key, &address, &tx)
        .map_err(|e| JsValue::from_str(&e))?;
    let total_amount_atoms = matches
        .iter()
        .fold(0u64, |total, item| total.saturating_add(item.amount));
    serde_json::to_string(&PrivateTxScanResponse {
        address,
        scanned_transactions: 1,
        matching_outputs: matches.len(),
        total_amount_atoms,
        matches,
    })
    .map_err(|e| JsValue::from_str(&format!("serialize scan result: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_node_derivation_for_known_key() {
        let key = [0x11u8; 32];
        let addr = payment_address_from_scan_key_bytes(&key).unwrap();
        assert_eq!(
            addr,
            "zpriv:22922f406fc98f6b303679a6db5d2e91d27f0f1861ecc3128bcf26efe958094f:46291d19cbb708cca5a68c23141d241eeb875107fbe1a4fe917bcfc061640966"
        );
    }

    #[test]
    fn scans_private_tx_output_with_scan_key_only() {
        let scan_key = [0x11u8; 32];
        let address = payment_address_from_scan_key_bytes(&scan_key).unwrap();
        let (_, spend_public) = parse_payment_address(&address).unwrap();
        let amount = 42_000_000u64;
        let commitment_blind = vec![0x33u8; 32];
        let commitment = commitment_new_blinded_hex(&amount.to_le_bytes(), &commitment_blind);
        let nullifier = "test-nullifier".to_string();

        let ephemeral_scalar = Scalar::from_bytes_mod_order_wide(&[0x22u8; 64]);
        let ephemeral_pubkey = (ephemeral_scalar * RISTRETTO_BASEPOINT_POINT)
            .compress()
            .to_bytes()
            .to_vec();
        let shared_point = ephemeral_scalar * parse_payment_address(&address).unwrap().0;
        let derived_scalar = hash_to_scalar(&[
            b"zoka-output-key",
            shared_point.compress().as_bytes(),
            commitment.as_bytes(),
        ]);
        let one_time_public_key =
            point_to_hex(&(spend_public + derived_scalar * RISTRETTO_BASEPOINT_POINT));
        let amount_blind = amount_blind_from_shared(shared_point.compress().as_bytes(), &commitment);

        let mut payload = Vec::new();
        payload.extend_from_slice(&amount.to_le_bytes());
        payload.extend_from_slice(&commitment_blind);
        let encrypted_amount = xor_with_blake3_stream(
            &payload,
            &private_note_payload_key(amount_blind, &commitment, &nullifier),
            b"zoka-private-note-payload-v2",
        );

        let tx = PrivateTransaction {
            tx_hash: "abc123".to_string(),
            commitment: String::new(),
            recipient_commitment: String::new(),
            stealth_address: String::new(),
            ephemeral_pubkey: Vec::new(),
            encrypted_amount: Vec::new(),
            outputs: vec![PrivateOutput {
                output_id: "out-1".to_string(),
                asset_tag: default_asset_tag(),
                recipient_commitment: String::new(),
                stealth_address: "stlth_test".to_string(),
                ephemeral_pubkey,
                commitment,
                one_time_public_key,
                encrypted_amount,
                commitment_blind: Vec::new(),
                is_change: false,
            }],
            nullifier,
            timestamp: 123,
            commitment_blind: Vec::new(),
        };

        let matches = scan_private_transaction_inner(&scan_key, &address, &tx).unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].amount, amount);
        assert_eq!(matches[0].tx_hash, "abc123");
    }
}
