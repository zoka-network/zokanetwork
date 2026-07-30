//! Self-custody mobile transaction construction.
//!
//! The mobile client supplies only public chain data plus its locally-held
//! recovery entropy. This module rebuilds the wallet in memory, constructs a
//! canonical private transaction locally and returns the serialized body. The
//! caller publishes that body through `/private/tx`; wallet secrets never
//! cross the network.

use crate::private_wallet::{
    PrivateTransaction, PrivateWalletManager, PRIVATE_PROOF_PROFILE_V1, PRIVATE_PROOF_VERSION_V1,
};
use crate::types::PrivateCoinbaseOutput;
use crate::zk::groth16_backend::{
    benchmark_private_tx_v2_i1_lab, ensure_trusted_setup, PRIVATE_TX_MAX_INPUTS,
    PRIVATE_TX_V2_I1_LAB_CIRCUIT, TRUSTED_SETUP_ENV,
};
use serde::{Deserialize, Serialize};
use std::time::Instant;
use zeroize::Zeroize;

const MOBILE_WALLET_NAME: &str = "mobile-self-custody";

#[derive(Debug, Clone, Deserialize)]
pub struct MobileCoinbaseInput {
    pub source_block_hash: String,
    pub expected_amount_atoms: u64,
    pub output: PrivateCoinbaseOutput,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MobilePrivateTxBuildRequest {
    pub seed_hex: String,
    pub to: String,
    pub amount_atoms: u64,
    #[serde(default)]
    pub fee_atoms: u64,
    #[serde(default = "default_private_proof_version")]
    pub private_proof_version: u8,
    #[serde(default = "default_private_proof_profile")]
    pub private_proof_profile: String,
    pub current_height: u64,
    pub trusted_setup_dir: String,
    #[serde(default)]
    pub coinbase_outputs: Vec<MobileCoinbaseInput>,
    #[serde(default)]
    pub confirmed_private_txs: Vec<PrivateTransaction>,
    #[serde(default)]
    pub pending_private_txs: Vec<PrivateTransaction>,
}

fn default_private_proof_version() -> u8 {
    PRIVATE_PROOF_VERSION_V1
}

fn default_private_proof_profile() -> String {
    PRIVATE_PROOF_PROFILE_V1.to_string()
}

#[derive(Debug, Clone, Serialize)]
pub struct MobilePrivateTxBuildResponse {
    pub hash: String,
    pub tx: PrivateTransaction,
    pub metrics: MobilePrivateTxBuildMetrics,
}

/// Local-only timings returned through JNI for mobile performance tuning.
///
/// Keep this intentionally free of wallet identifiers, addresses, note ids,
/// commitments and key material. The Android caller may aggregate these
/// values locally without creating a privacy-sensitive log.
#[derive(Debug, Clone, Default, Serialize)]
pub struct MobilePrivateTxBuildMetrics {
    pub total_ms: u64,
    pub validation_ms: u64,
    pub restore_wallet_ms: u64,
    pub replay_coinbase_ms: u64,
    pub replay_confirmed_ms: u64,
    pub replay_pending_ms: u64,
    pub rebuild_spend_set_ms: u64,
    pub prove_and_build_ms: u64,
    pub lookup_tx_ms: u64,
    pub imported_coinbase_count: usize,
    pub replayed_confirmed_count: usize,
    pub replayed_pending_count: usize,
    pub selected_input_count: usize,
    pub proof_bytes: usize,
    pub serialized_tx_bytes: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct MobilePrivateTxBenchmarkResponse {
    pub input_count: usize,
    pub total_ms: u64,
    pub proof_bytes: usize,
    pub serialized_tx_bytes: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct MobileTrustedSetupPreloadResponse {
    pub total_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct MobilePrivateTxV2I1LabBenchmarkResponse {
    pub profile: &'static str,
    pub params_bytes: u64,
    pub params_load_ms: u64,
    pub prove_ms: u64,
    pub verify_ms: u64,
    pub proof_bytes: usize,
    pub public_input_count: usize,
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

fn zpriv_from_zka1(address: &str) -> Result<String, String> {
    let encoded = address
        .strip_prefix("zka1")
        .ok_or_else(|| "recipient must be a full zka1 address".to_string())?;
    let bytes = bs58::decode(encoded)
        .into_vec()
        .map_err(|_| "recipient address is not valid base58".to_string())?;
    if bytes.len() != 68 {
        return Err(format!(
            "recipient address decoded length must be 68 bytes (got {})",
            bytes.len()
        ));
    }
    let (payload, checksum) = bytes.split_at(64);
    let expected_checksum = {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(b"zoka-private-address-v1");
        hasher.update(payload);
        hasher.finalize()[..4].to_vec()
    };
    if checksum != expected_checksum {
        return Err("recipient address checksum mismatch".to_string());
    }
    Ok(format!(
        "zpriv:{}:{}",
        hex::encode(&payload[..32]),
        hex::encode(&payload[32..])
    ))
}

pub fn build_private_transfer(
    mut request: MobilePrivateTxBuildRequest,
) -> Result<MobilePrivateTxBuildResponse, String> {
    let total_started = Instant::now();
    let mut metrics = MobilePrivateTxBuildMetrics::default();
    let validation_started = Instant::now();

    if request.amount_atoms == 0 {
        return Err("amount_atoms must be greater than zero".to_string());
    }
    if request.trusted_setup_dir.trim().is_empty() {
        return Err("trusted_setup_dir is required".to_string());
    }

    let mut seed_hex = std::mem::take(&mut request.seed_hex);
    let seed = hex::decode(seed_hex.trim_start_matches("0x"));
    seed_hex.zeroize();
    let seed = seed.map_err(|_| "seed_hex must be hexadecimal".to_string())?;
    let to_commitment = zpriv_from_zka1(&request.to)?;
    std::env::set_var(TRUSTED_SETUP_ENV, request.trusted_setup_dir);
    metrics.validation_ms = elapsed_ms(validation_started);

    let restore_started = Instant::now();
    let mut manager = PrivateWalletManager::new();
    manager
        .restore_private_wallet_from_recovery_seed(MOBILE_WALLET_NAME, seed)
        .map_err(|e| format!("failed to restore in-memory wallet: {e}"))?;
    metrics.restore_wallet_ms = elapsed_ms(restore_started);

    let replay_coinbase_started = Instant::now();
    for coinbase in request.coinbase_outputs {
        if coinbase.output.unlock_height > request.current_height {
            continue;
        }
        manager
            .claim_private_coinbase_output(
                MOBILE_WALLET_NAME,
                &coinbase.source_block_hash,
                &coinbase.output,
                coinbase.expected_amount_atoms,
                request.current_height,
            )
            .map_err(|e| format!("failed to import coinbase output: {e}"))?;
        metrics.imported_coinbase_count += 1;
    }
    metrics.replay_coinbase_ms = elapsed_ms(replay_coinbase_started);

    let replay_confirmed_started = Instant::now();
    for tx in request.confirmed_private_txs {
        manager
            .apply_relevant_private_transaction_for_wallet(MOBILE_WALLET_NAME, &tx)
            .map_err(|e| format!("failed to replay confirmed private tx {}: {e}", tx.tx_hash))?;
        metrics.replayed_confirmed_count += 1;
    }
    metrics.replay_confirmed_ms = elapsed_ms(replay_confirmed_started);

    let replay_pending_started = Instant::now();
    for tx in request.pending_private_txs {
        manager
            .apply_relevant_private_transaction_for_wallet(MOBILE_WALLET_NAME, &tx)
            .map_err(|e| format!("failed to replay pending private tx {}: {e}", tx.tx_hash))?;
        metrics.replayed_pending_count += 1;
    }
    metrics.replay_pending_ms = elapsed_ms(replay_pending_started);

    let rebuild_nullifiers_started = Instant::now();
    manager.rebuild_nullifier_set();
    manager.rebuild_claimed_fee_tx_hashes();
    metrics.rebuild_spend_set_ms = elapsed_ms(rebuild_nullifiers_started);

    let prove_started = Instant::now();
    let hash = manager
        .send_private_transaction_with_fee_and_membership_profile_at(
            MOBILE_WALLET_NAME,
            to_commitment,
            request.amount_atoms,
            request.fee_atoms,
            String::new(),
            Vec::new(),
            request.private_proof_version,
            &request.private_proof_profile,
            request.current_height,
            crate::economics::private_tx_v2_i1_activation_height(),
            crate::economics::private_tx_canonical_bundle_activation_height(),
        )
        .map_err(|e| format!("failed to build local private tx: {e}"))?;
    metrics.prove_and_build_ms = elapsed_ms(prove_started);

    let lookup_tx_started = Instant::now();
    let tx = manager
        .get_private_transaction(&hash)
        .ok_or_else(|| "local private tx body missing after build".to_string())?;
    metrics.lookup_tx_ms = elapsed_ms(lookup_tx_started);
    metrics.selected_input_count = tx.input_key_images.len();
    metrics.proof_bytes = tx.zk_proof.as_ref().map_or(0, Vec::len);
    metrics.serialized_tx_bytes = serde_json::to_vec(&tx).map_or(0, |bytes| bytes.len());
    metrics.total_ms = elapsed_ms(total_started);

    Ok(MobilePrivateTxBuildResponse { hash, tx, metrics })
}

pub fn build_private_transfer_json(request_json: &str) -> Result<String, String> {
    let request = serde_json::from_str(request_json)
        .map_err(|e| format!("invalid mobile private tx request: {e}"))?;
    let response = build_private_transfer(request)?;
    serde_json::to_string(&response)
        .map_err(|e| format!("failed to encode mobile private tx response: {e}"))
}

/// Load and validate the public trusted setup before the user starts proving.
///
/// Android can run this on its background prover queue while the user fills in
/// a transfer form. The setup contains public parameters only.
pub fn preload_trusted_setup_json(trusted_setup_dir: &str) -> Result<String, String> {
    if trusted_setup_dir.trim().is_empty() {
        return Err("trusted_setup_dir is required".to_string());
    }
    std::env::set_var(TRUSTED_SETUP_ENV, trusted_setup_dir);

    let started = Instant::now();
    ensure_trusted_setup();
    serde_json::to_string(&MobileTrustedSetupPreloadResponse {
        total_ms: elapsed_ms(started),
    })
    .map_err(|e| format!("failed to encode trusted setup preload response: {e}"))
}

/// Measure the compact i1 circuit candidate without producing a publishable tx.
///
/// This benchmark-only path proves and verifies a satisfiable synthetic
/// witness. It stays separate from node verification until v2 versioning,
/// adversarial vectors and coordinated activation are implemented.
pub fn benchmark_private_tx_v2_i1_lab_json(trusted_setup_dir: &str) -> Result<String, String> {
    if trusted_setup_dir.trim().is_empty() {
        return Err("trusted_setup_dir is required".to_string());
    }
    let result = benchmark_private_tx_v2_i1_lab(std::path::Path::new(trusted_setup_dir))?;
    serde_json::to_string(&MobilePrivateTxV2I1LabBenchmarkResponse {
        profile: PRIVATE_TX_V2_I1_LAB_CIRCUIT,
        params_bytes: result.params_bytes,
        params_load_ms: result.params_load_ms,
        prove_ms: result.prove_ms,
        verify_ms: result.verify_ms,
        proof_bytes: result.proof_bytes,
        public_input_count: result.public_input_count,
    })
    .map_err(|e| format!("failed to encode v2 i1 benchmark response: {e}"))
}

/// Build a synthetic in-memory transaction for Android performance testing.
///
/// This never reads a persisted wallet, never submits a transaction and never
/// returns synthetic key material. It exists so a connected device can measure
/// the real JNI + Groth16 path before local proving is enabled for user funds.
pub fn benchmark_private_tx_json(
    input_count: usize,
    trusted_setup_dir: &str,
) -> Result<String, String> {
    if !(1..=PRIVATE_TX_MAX_INPUTS).contains(&input_count) {
        return Err(format!(
            "input_count must be between 1 and {}",
            PRIVATE_TX_MAX_INPUTS
        ));
    }
    if trusted_setup_dir.trim().is_empty() {
        return Err("trusted_setup_dir is required".to_string());
    }
    std::env::set_var(TRUSTED_SETUP_ENV, trusted_setup_dir);

    let mut manager = PrivateWalletManager::new();
    manager
        .create_private_wallet("benchmark-sender")
        .map_err(|e| format!("failed to create synthetic sender: {e}"))?;
    let recipient = manager
        .create_private_wallet("benchmark-recipient")
        .map_err(|e| format!("failed to create synthetic recipient: {e}"))?;
    let note_atoms = 10_000u64;
    for _ in 0..input_count {
        manager
            .deposit_to_private_wallet("benchmark-sender", note_atoms)
            .map_err(|e| format!("failed to create synthetic note: {e}"))?;
    }
    let amount_atoms = note_atoms
        .saturating_mul(input_count.saturating_sub(1) as u64)
        .saturating_add(1);

    let started = Instant::now();
    let hash = manager
        .send_private_transaction_with_fee("benchmark-sender", recipient, amount_atoms, 0)
        .map_err(|e| format!("failed to build synthetic private tx: {e}"))?;
    let tx = manager
        .get_private_transaction(&hash)
        .ok_or_else(|| "synthetic private tx body missing after build".to_string())?;
    let response = MobilePrivateTxBenchmarkResponse {
        input_count,
        total_ms: elapsed_ms(started),
        proof_bytes: tx.zk_proof.as_ref().map_or(0, Vec::len),
        serialized_tx_bytes: serde_json::to_vec(&tx).map_or(0, |bytes| bytes.len()),
    };
    serde_json::to_string(&response)
        .map_err(|e| format!("failed to encode synthetic benchmark response: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn zka1_address(payload: &[u8; 64]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"zoka-private-address-v1");
        hasher.update(payload);

        let mut encoded = payload.to_vec();
        encoded.extend_from_slice(&hasher.finalize()[..4]);
        format!("zka1{}", bs58::encode(encoded).into_string())
    }

    #[test]
    fn converts_valid_zka1_recipient_to_private_commitment() {
        let payload = [7u8; 64];
        let address = zka1_address(&payload);

        assert_eq!(
            zpriv_from_zka1(&address).unwrap(),
            format!(
                "zpriv:{}:{}",
                hex::encode(&payload[..32]),
                hex::encode(&payload[32..])
            )
        );
    }

    #[test]
    fn rejects_zka1_recipient_with_modified_checksum() {
        let payload = [9u8; 64];
        let address = zka1_address(&payload);
        let mut bytes = bs58::decode(address.trim_start_matches("zka1"))
            .into_vec()
            .unwrap();
        bytes[67] ^= 1;
        let address = format!("zka1{}", bs58::encode(bytes).into_string());

        assert!(zpriv_from_zka1(&address).is_err());
    }

    #[test]
    fn mobile_metrics_json_does_not_expose_sensitive_fields() {
        let json = serde_json::to_string(&MobilePrivateTxBuildMetrics::default()).unwrap();

        assert!(json.contains("\"prove_and_build_ms\""));
        assert!(!json.contains("seed"));
        assert!(!json.contains("address"));
        assert!(!json.contains("commitment"));
        assert!(!json.contains("nullifier"));
    }

    #[test]
    fn mobile_build_request_defaults_to_historical_v1_profile() {
        let request: MobilePrivateTxBuildRequest = serde_json::from_value(serde_json::json!({
            "seed_hex": "00",
            "to": "unused",
            "amount_atoms": 1,
            "current_height": 0,
            "trusted_setup_dir": "unused"
        }))
        .expect("deserialize mobile build request");

        assert_eq!(request.private_proof_version, PRIVATE_PROOF_VERSION_V1);
        assert_eq!(request.private_proof_profile, PRIVATE_PROOF_PROFILE_V1);
    }

    #[test]
    fn synthetic_benchmark_rejects_invalid_input_count_before_loading_setup() {
        assert!(benchmark_private_tx_json(0, "unused").is_err());
        assert!(benchmark_private_tx_json(PRIVATE_TX_MAX_INPUTS + 1, "unused").is_err());
    }

    #[test]
    fn trusted_setup_preload_rejects_empty_path_before_loading_setup() {
        assert!(preload_trusted_setup_json("").is_err());
    }

    #[test]
    fn v2_i1_lab_benchmark_rejects_empty_path_before_loading_setup() {
        assert!(benchmark_private_tx_v2_i1_lab_json("").is_err());
    }

    /// Host-side baseline for the current Groth16 circuit. Run explicitly:
    ///
    /// ZOKA_TRUSTED_SETUP_DIR=./trusted-setup \
    /// cargo test --release --no-default-features \
    ///   mobile::tests::benchmark_private_tx_profiles_on_host -- --ignored --nocapture
    #[test]
    #[ignore = "manual Groth16 mobile baseline; intentionally expensive"]
    fn benchmark_private_tx_profiles_on_host() {
        std::env::var(TRUSTED_SETUP_ENV)
            .expect("set ZOKA_TRUSTED_SETUP_DIR to a verified trusted-setup bundle");
        let cases = std::env::var("ZOKA_MOBILE_BENCH_INPUTS")
            .unwrap_or_else(|_| "1,2,4,8".to_string())
            .split(',')
            .map(|raw| raw.trim().parse::<usize>().expect("input count"))
            .collect::<Vec<_>>();

        println!("inputs,total_ms,proof_bytes,serialized_tx_bytes");
        for input_count in cases {
            let mut manager = PrivateWalletManager::new();
            manager.create_private_wallet("sender").unwrap();
            let recipient = manager.create_private_wallet("recipient").unwrap();
            let note_atoms = 10_000u64;
            for _ in 0..input_count {
                manager
                    .deposit_to_private_wallet("sender", note_atoms)
                    .unwrap();
            }

            let amount_atoms = note_atoms
                .saturating_mul(input_count.saturating_sub(1) as u64)
                .saturating_add(1);
            let started = Instant::now();
            let hash = manager
                .send_private_transaction_with_fee("sender", recipient, amount_atoms, 0)
                .unwrap();
            let tx = manager.get_private_transaction(&hash).unwrap();
            println!(
                "{},{},{},{}",
                input_count,
                elapsed_ms(started),
                tx.zk_proof.as_ref().map_or(0, Vec::len),
                serde_json::to_vec(&tx).unwrap().len()
            );
        }
    }
}
