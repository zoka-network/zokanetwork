/// Private Transaction Management for Users
/// Provides confidential transaction support with commitment/nullifier/ZK proofs
use crate::transaction::commitment::Commitment;
use crate::transaction::nullifier::{Nullifier, NullifierSet};
use crate::types::BlockError;
use crate::zk::prover::Prover;
use crate::zk::verifier::Verifier;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use zeroize::Zeroize;

/// Helper function to convert bytes to hex string
fn bytes_to_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<String>()
}

/// Private transaction for confidential transfers
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PrivateTransaction {
    pub tx_hash: String,
    pub commitment: String, // Pedersen commitment of amount
    #[serde(default)]
    pub recipient_commitment: String, // Receiver commitment identifier
    #[serde(default)]
    pub encrypted_amount: Vec<u8>, // Obfuscated amount payload for receiver
    pub nullifier: String,  // Proof of spend without revealing which
    pub zk_proof: Option<Vec<u8>>, // Optional ZK-SNARK proof
    pub timestamp: u64,
    pub status: PrivacyStatus,
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
    recipient_commitment: Option<String>,
    spending_key: Vec<u8>,
    prover: Prover,
    nullifier_set: Arc<Mutex<NullifierSet>>,
}

fn xor_amount_with_stream(amount: u64, recipient_commitment: &str, nullifier: &str) -> Vec<u8> {
    let mut amount_bytes = amount.to_le_bytes();
    let mut stream = [0u8; 8];
    let mut hasher = blake3::Hasher::new();
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

impl PrivateTransactionBuilder {
    /// Create new private transaction builder
    pub fn new(
        amount: u64,
        spending_key: Vec<u8>,
        nullifier_set: Arc<Mutex<NullifierSet>>,
    ) -> Self {
        PrivateTransactionBuilder {
            amount,
            recipient_commitment: None,
            spending_key,
            prover: Prover::new("private_tx".to_string()),
            nullifier_set,
        }
    }

    /// Set recipient's commitment (public key in privacy context)
    pub fn with_recipient_commitment(mut self, commitment: String) -> Self {
        self.recipient_commitment = Some(commitment);
        self
    }

    /// Build and generate ZK proof for private transaction
    pub fn build(self) -> Result<PrivateTransaction, BlockError> {
        let recipient_commitment = self.recipient_commitment.ok_or_else(|| {
            BlockError::TransactionValidationFailed("Recipient commitment required".to_string())
        })?;

        if recipient_commitment.is_empty() {
            return Err(BlockError::TransactionValidationFailed(
                "Recipient commitment required".to_string(),
            ));
        }

        // Create amount commitment
        let commitment = Commitment::new(self.amount.to_le_bytes().to_vec());
        let commitment_hex = bytes_to_hex(commitment.as_bytes());

        // Create nullifier from spending key + amount
        let nullifier_data = [
            self.spending_key.clone(),
            self.amount.to_le_bytes().to_vec(),
        ]
        .concat();
        let nullifier = Nullifier::new(nullifier_data);
        let nullifier_hex = bytes_to_hex(nullifier.as_bytes());

        // Verify not already spent
        let mut set = self.nullifier_set.lock().unwrap();
        set.spend(nullifier.clone()).map_err(|e| {
            BlockError::TransactionValidationFailed(format!("Double spend detected: {}", e))
        })?;
        drop(set);

        // Generate ZK proof
        let proof_data = format!(
            "{}:{}:{}",
            commitment_hex, nullifier_hex, recipient_commitment
        );
        let zk_proof = self.prover.prove(proof_data.as_bytes(), &[])?;
        let encrypted_amount =
            xor_amount_with_stream(self.amount, &recipient_commitment, &nullifier_hex);

        Ok(PrivateTransaction {
            tx_hash: bytes_to_hex(&commitment.as_bytes()[0..16]),
            commitment: commitment_hex,
            recipient_commitment,
            encrypted_amount,
            nullifier: nullifier_hex,
            zk_proof: Some(zk_proof.proof_bytes),
            timestamp: current_timestamp(),
            status: PrivacyStatus::Pending,
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
            let temp_proof = crate::zk::prover::ZKProof::new(
                proof_bytes.clone(),
                "private_transaction".to_string(),
                vec![],
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
    pub historical_spending_keys: Vec<Vec<u8>>,
    pub commitment_key: String, // Public identifier
    pub private_balance: u64,
    pub private_transactions: Vec<PrivateTransaction>,
    pub created_at: u64,
}

impl PrivateWallet {
    /// Create new private wallet
    pub fn new(name: String, spending_key: Vec<u8>, commitment_key: String) -> Self {
        PrivateWallet {
            name,
            spending_key,
            historical_spending_keys: vec![],
            commitment_key,
            private_balance: 0,
            private_transactions: vec![],
            created_at: current_timestamp(),
        }
    }

    /// Create private transaction
    pub fn create_private_transaction(
        &mut self,
        amount: u64,
        recipient_commitment: String,
        nullifier_set: Arc<Mutex<NullifierSet>>,
    ) -> Result<PrivateTransaction, BlockError> {
        if amount > self.private_balance {
            return Err(BlockError::TransactionValidationFailed(
                "Insufficient private balance".to_string(),
            ));
        }

        let builder =
            PrivateTransactionBuilder::new(amount, self.spending_key.clone(), nullifier_set)
                .with_recipient_commitment(recipient_commitment);

        let tx = builder.build()?;
        self.private_transactions.push(tx.clone());
        self.private_balance -= amount;

        Ok(tx)
    }

    /// Receive private transaction (increases balance)
    pub fn receive_private_amount(&mut self, amount: u64) {
        self.private_balance += amount;
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
        let mut next_key = vec![0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut next_key);
        self.spending_key = next_key;

        let commitment = Commitment::new(self.spending_key.clone());
        self.commitment_key = bytes_to_hex(commitment.as_bytes());
        self.commitment_key.clone()
    }
}

impl Drop for PrivateWallet {
    fn drop(&mut self) {
        self.spending_key.zeroize();
        for key in &mut self.historical_spending_keys {
            key.zeroize();
        }
    }
}

/// Privacy-enabled wallet manager
pub struct PrivateWalletManager {
    private_wallets: HashMap<String, PrivateWallet>,
    private_pool: PrivateTransactionPool,
    received_nullifiers: HashSet<String>,
}

impl PrivateWalletManager {
    /// Create new private wallet manager
    pub fn new() -> Self {
        PrivateWalletManager {
            private_wallets: HashMap::new(),
            private_pool: PrivateTransactionPool::new(),
            received_nullifiers: HashSet::new(),
        }
    }

    /// Create new private wallet
    pub fn create_private_wallet(&mut self, name: &str) -> Result<String, BlockError> {
        use rand::RngCore;

        if self.private_wallets.contains_key(name) {
            return Err(BlockError::TransactionValidationFailed(
                "Private wallet already exists".to_string(),
            ));
        }

        // Generate a cryptographically random spending key.
        let mut spending_key = vec![0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut spending_key);

        // Create commitment from spending key
        let commitment = Commitment::new(spending_key.clone());
        let commitment_key = bytes_to_hex(commitment.as_bytes());

        let wallet = PrivateWallet::new(name.to_string(), spending_key, commitment_key.clone());
        self.private_wallets.insert(name.to_string(), wallet);

        Ok(commitment_key)
    }

    /// Send private transaction
    pub fn send_private_transaction(
        &mut self,
        from_wallet: &str,
        to_commitment: String,
        amount: u64,
    ) -> Result<String, BlockError> {
        let wallet = self.private_wallets.get_mut(from_wallet).ok_or_else(|| {
            BlockError::TransactionValidationFailed("Wallet not found".to_string())
        })?;

        let nullifier_set = self.private_pool.nullifier_set();
        let tx = wallet.create_private_transaction(amount, to_commitment, nullifier_set)?;

        let tx_hash = tx.tx_hash.clone();
        self.private_pool.submit_private_transaction(tx)?;

        Ok(tx_hash)
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

    /// Check if we can decrypt a private transaction (if we're the recipient)
    pub fn can_wallet_decrypt_private_tx(
        &self,
        wallet_name: &str,
        tx: &PrivateTransaction,
    ) -> Result<bool, BlockError> {
        let wallet = self.private_wallets.get(wallet_name).ok_or_else(|| {
            BlockError::TransactionValidationFailed("Wallet not found".to_string())
        })?;

        if wallet.commitment_key != tx.recipient_commitment {
            return Ok(false);
        }

        match self.decrypt_private_amount_for_wallet(wallet_name, tx) {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
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
        let wallet = self.private_wallets.get(wallet_name).ok_or_else(|| {
            BlockError::TransactionValidationFailed("Wallet not found".to_string())
        })?;

        if wallet.commitment_key != tx.recipient_commitment {
            return Err(BlockError::TransactionValidationFailed(
                "Transaction recipient does not match wallet commitment".to_string(),
            ));
        }

        let amount = decode_amount_from_payload(
            &tx.encrypted_amount,
            &tx.recipient_commitment,
            &tx.nullifier,
        )?;
        let expected_commitment = Commitment::new(amount.to_le_bytes().to_vec()).to_hex();
        if expected_commitment != tx.commitment {
            return Err(BlockError::TransactionValidationFailed(
                "Commitment mismatch while decrypting private amount".to_string(),
            ));
        }
        Ok(amount)
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

    /// Confirm a private transaction (for miners/validators)
    pub fn confirm_private_transaction(&mut self, tx_hash: &str) -> Result<(), BlockError> {
        self.private_pool.confirm_private_transaction(tx_hash)
    }

    /// Mark inbound private transaction nullifier as seen and validate optional proof.
    pub fn validate_inbound_private_transaction(
        &mut self,
        tx: &PrivateTransaction,
    ) -> Result<(), BlockError> {
        if tx.nullifier.is_empty() {
            return Err(BlockError::TransactionValidationFailed(
                "Missing nullifier in private transaction".to_string(),
            ));
        }

        if self.received_nullifiers.contains(&tx.nullifier) {
            return Err(BlockError::TransactionValidationFailed(
                "Duplicate private transaction nullifier".to_string(),
            ));
        }

        if let Some(proof_bytes) = &tx.zk_proof {
            let verifier = Verifier::new();
            let proof = crate::zk::prover::ZKProof::new(
                proof_bytes.clone(),
                "private_transaction".to_string(),
                vec![],
                current_timestamp(),
            );
            let result = verifier.verify(&proof)?;
            if !result.is_valid {
                return Err(BlockError::TransactionValidationFailed(
                    "Invalid private transaction proof".to_string(),
                ));
            }
        }

        self.received_nullifiers.insert(tx.nullifier.clone());
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

    #[test]
    fn test_private_wallet_creation() {
        let mut manager = PrivateWalletManager::new();
        let result = manager.create_private_wallet("alice");
        assert!(result.is_ok());
        assert!(manager.get_private_wallet("alice").is_some());
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

        let result = manager.send_private_transaction("alice", "bob_commitment".to_string(), 50);

        assert!(result.is_ok());
        let wallet = manager.get_private_wallet("alice").unwrap();
        assert_eq!(wallet.private_balance, 50);
    }

    #[test]
    fn test_private_pool_stats() {
        let mut manager = PrivateWalletManager::new();
        manager.create_private_wallet("alice").unwrap();
        manager.deposit_to_private_wallet("alice", 100).unwrap();

        manager
            .send_private_transaction("alice", "bob".to_string(), 30)
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
        assert!(!manager
            .can_wallet_decrypt_private_tx("alice", &tx)
            .expect("alice check should be valid"));

        let amount = manager
            .decrypt_private_amount_for_wallet("bob", &tx)
            .expect("bob should decrypt");
        assert_eq!(amount, 40);
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
        let tx = PrivateTransaction {
            tx_hash: "abc".to_string(),
            commitment: Commitment::new(10u64.to_le_bytes().to_vec()).to_hex(),
            recipient_commitment: "recipient".to_string(),
            encrypted_amount: xor_amount_with_stream(10, "recipient", "nf-1"),
            nullifier: "nf-1".to_string(),
            zk_proof: None,
            timestamp: current_timestamp(),
            status: PrivacyStatus::Pending,
        };

        assert!(manager.validate_inbound_private_transaction(&tx).is_ok());
        assert!(manager.validate_inbound_private_transaction(&tx).is_err());
    }
}
