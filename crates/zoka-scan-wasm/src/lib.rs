//! In-browser private scan crypto for the Zoka explorer.
//!
//! Every function here is a byte-exact mirror of `src/private_wallet/mod.rs`
//! so that a scan/view key can be used entirely client-side: the key never
//! leaves the browser. Step 1 (this file) implements payment-address
//! derivation, which is the foundation the full output scan builds on. It is
//! verified against the node's `payment_address_from_scan_key` (same input →
//! same `zpriv:...:...` address) before we trust it.

use curve25519_dalek::constants::RISTRETTO_BASEPOINT_POINT;
use curve25519_dalek::ristretto::RistrettoPoint;
use curve25519_dalek::scalar::Scalar;
use sha2::{Digest, Sha256};
use wasm_bindgen::prelude::*;

// ── Mirror of src/private_wallet/mod.rs (address derivation) ────────

fn signing_seed_from_bytes(bytes: &[u8]) -> [u8; 32] {
    let digest = Sha256::digest(bytes);
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&digest[..32]);
    seed
}

fn scalar_from_secret_bytes(bytes: &[u8]) -> Scalar {
    Scalar::from_bytes_mod_order(signing_seed_from_bytes(bytes))
}

fn point_to_hex(point: &RistrettoPoint) -> String {
    hex::encode(point.compress().to_bytes())
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

// ── WASM exports ────────────────────────────────────────────────────

/// Derive the `zpriv:scan_pub:spend_pub` payment address from a hex scan key,
/// entirely in the browser. Same output as the node's
/// `payment_address_from_scan_key`.
#[wasm_bindgen]
pub fn derive_payment_address(scan_key_hex: &str) -> Result<String, JsValue> {
    let clean = scan_key_hex.trim().trim_start_matches("0x");
    let bytes = hex::decode(clean).map_err(|e| JsValue::from_str(&format!("bad hex: {e}")))?;
    payment_address_from_scan_key_bytes(&bytes).map_err(|e| JsValue::from_str(&e))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Byte-exact match against the node's payment_address_from_scan_key for
    // scan key = 32 bytes of 0x11 (captured live from the node's
    // /private/view-scan-key). If this passes, our in-browser crypto is
    // identical to the node's and the scan key can stay client-side.
    #[test]
    fn matches_node_derivation_for_known_key() {
        let key = [0x11u8; 32];
        let addr = payment_address_from_scan_key_bytes(&key).unwrap();
        assert_eq!(
            addr,
            "zpriv:22922f406fc98f6b303679a6db5d2e91d27f0f1861ecc3128bcf26efe958094f:46291d19cbb708cca5a68c23141d241eeb875107fbe1a4fe917bcfc061640966"
        );
    }
}
