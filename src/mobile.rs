//! Self-custody mobile transaction construction.
//!
//! The mobile client supplies only public chain data plus its locally-held
//! recovery entropy. This module rebuilds the wallet in memory, constructs a
//! canonical private transaction locally and returns the serialized body. The
//! caller publishes that body through `/private/tx`; wallet secrets never
//! cross the network.

use crate::private_wallet::{PrivateTransaction, PrivateWalletManager};
use crate::types::PrivateCoinbaseOutput;
use crate::zk::groth16_backend::TRUSTED_SETUP_ENV;
use serde::{Deserialize, Serialize};

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
    pub current_height: u64,
    pub trusted_setup_dir: String,
    #[serde(default)]
    pub coinbase_outputs: Vec<MobileCoinbaseInput>,
    #[serde(default)]
    pub confirmed_private_txs: Vec<PrivateTransaction>,
    #[serde(default)]
    pub pending_private_txs: Vec<PrivateTransaction>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MobilePrivateTxBuildResponse {
    pub hash: String,
    pub tx: PrivateTransaction,
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
    request: MobilePrivateTxBuildRequest,
) -> Result<MobilePrivateTxBuildResponse, String> {
    if request.amount_atoms == 0 {
        return Err("amount_atoms must be greater than zero".to_string());
    }
    if request.trusted_setup_dir.trim().is_empty() {
        return Err("trusted_setup_dir is required".to_string());
    }

    let seed = hex::decode(request.seed_hex.trim_start_matches("0x"))
        .map_err(|_| "seed_hex must be hexadecimal".to_string())?;
    let to_commitment = zpriv_from_zka1(&request.to)?;
    std::env::set_var(TRUSTED_SETUP_ENV, request.trusted_setup_dir);

    let mut manager = PrivateWalletManager::new();
    manager
        .restore_private_wallet_from_recovery_seed(MOBILE_WALLET_NAME, seed)
        .map_err(|e| format!("failed to restore in-memory wallet: {e}"))?;

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
    }
    for tx in request.confirmed_private_txs {
        manager
            .apply_relevant_private_transaction_for_wallet(MOBILE_WALLET_NAME, &tx)
            .map_err(|e| format!("failed to replay confirmed private tx {}: {e}", tx.tx_hash))?;
    }
    for tx in request.pending_private_txs {
        manager
            .apply_relevant_private_transaction_for_wallet(MOBILE_WALLET_NAME, &tx)
            .map_err(|e| format!("failed to replay pending private tx {}: {e}", tx.tx_hash))?;
    }
    manager.rebuild_nullifier_set();

    let hash = manager
        .send_private_transaction_with_fee(
            MOBILE_WALLET_NAME,
            to_commitment,
            request.amount_atoms,
            request.fee_atoms,
        )
        .map_err(|e| format!("failed to build local private tx: {e}"))?;
    let tx = manager
        .get_private_transaction(&hash)
        .ok_or_else(|| "local private tx body missing after build".to_string())?;

    Ok(MobilePrivateTxBuildResponse { hash, tx })
}

pub fn build_private_transfer_json(request_json: &str) -> Result<String, String> {
    let request = serde_json::from_str(request_json)
        .map_err(|e| format!("invalid mobile private tx request: {e}"))?;
    let response = build_private_transfer(request)?;
    serde_json::to_string(&response)
        .map_err(|e| format!("failed to encode mobile private tx response: {e}"))
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
}
