// User transaction pool for sending and receiving
use crate::types::BlockError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// User transaction for send/receive
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct UserTransaction {
    pub hash: String,
    pub from: String,
    pub to: String,
    pub amount: u64,
    pub fee: u64,
    pub timestamp: u64,
    pub status: TxStatus,
    pub nonce: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum TxStatus {
    Pending,
    Confirmed,
    Failed,
}

impl std::fmt::Display for TxStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TxStatus::Pending => write!(f, "pending"),
            TxStatus::Confirmed => write!(f, "confirmed"),
            TxStatus::Failed => write!(f, "failed"),
        }
    }
}

/// Transaction pool manager for users
#[allow(dead_code)]
pub struct TransactionPool {
    pending: HashMap<String, UserTransaction>,
    confirmed: HashMap<String, UserTransaction>,
    failed: HashMap<String, UserTransaction>,
    user_nonces: HashMap<String, u64>,
}

impl TransactionPool {
    /// Create new transaction pool
    pub fn new() -> Self {
        TransactionPool {
            pending: HashMap::new(),
            confirmed: HashMap::new(),
            failed: HashMap::new(),
            user_nonces: HashMap::new(),
        }
    }

    /// Create and send transaction
    pub fn send_transaction(
        &mut self,
        from: &str,
        to: &str,
        amount: u64,
        fee: u64,
    ) -> Result<String, BlockError> {
        // Validate inputs
        if amount == 0 {
            return Err(BlockError::TransactionValidationFailed(
                "Amount must be > 0".to_string(),
            ));
        }

        if from == to {
            return Err(BlockError::TransactionValidationFailed(
                "Cannot send to self".to_string(),
            ));
        }

        // Get nonce
        let nonce = *self.user_nonces.get(from).unwrap_or(&0);
        let new_nonce = nonce + 1;
        self.user_nonces.insert(from.to_string(), new_nonce);

        // Create transaction
        let tx = UserTransaction {
            hash: Self::generate_hash(from, to, amount, nonce),
            from: from.to_string(),
            to: to.to_string(),
            amount,
            fee,
            timestamp: Self::current_timestamp(),
            status: TxStatus::Pending,
            nonce,
        };

        let hash = tx.hash.clone();
        self.pending.insert(hash.clone(), tx);

        Ok(hash)
    }

    /// Confirm transaction (move to confirmed)
    pub fn confirm_transaction(&mut self, hash: &str) -> Result<(), BlockError> {
        if let Some(mut tx) = self.pending.remove(hash) {
            tx.status = TxStatus::Confirmed;
            self.confirmed.insert(hash.to_string(), tx);
            Ok(())
        } else {
            Err(BlockError::TransactionValidationFailed(
                "Transaction not found".to_string(),
            ))
        }
    }

    /// Fail transaction
    pub fn fail_transaction(&mut self, hash: &str) -> Result<(), BlockError> {
        if let Some(mut tx) = self.pending.remove(hash) {
            tx.status = TxStatus::Failed;
            self.failed.insert(hash.to_string(), tx);
            Ok(())
        } else {
            Err(BlockError::TransactionValidationFailed(
                "Transaction not found".to_string(),
            ))
        }
    }

    /// Get transaction by hash
    pub fn get_transaction(&self, hash: &str) -> Option<UserTransaction> {
        self.pending
            .get(hash)
            .or_else(|| self.confirmed.get(hash))
            .or_else(|| self.failed.get(hash))
            .cloned()
    }

    /// Get user pending transactions
    pub fn get_pending_transactions(&self, address: &str) -> Vec<UserTransaction> {
        self.pending
            .values()
            .filter(|tx| tx.from == address || tx.to == address)
            .cloned()
            .collect()
    }

    /// Get user transaction history
    pub fn get_history(&self, address: &str) -> Vec<UserTransaction> {
        let mut history = Vec::new();

        for tx in self.pending.values() {
            if tx.from == address || tx.to == address {
                history.push(tx.clone());
            }
        }

        for tx in self.confirmed.values() {
            if tx.from == address || tx.to == address {
                history.push(tx.clone());
            }
        }

        for tx in self.failed.values() {
            if tx.from == address || tx.to == address {
                history.push(tx.clone());
            }
        }

        history.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        history
    }

    /// Count pending transactions from address
    pub fn pending_count(&self, address: &str) -> usize {
        self.pending
            .values()
            .filter(|tx| tx.from == address)
            .count()
    }

    /// Get pending transactions (for mining)
    pub fn get_all_pending(&self) -> Vec<UserTransaction> {
        self.pending.values().cloned().collect()
    }

    /// Count all pending transactions
    pub fn total_pending(&self) -> usize {
        self.pending.len()
    }

    /// Clear transaction
    pub fn clear_transaction(&mut self, hash: &str) -> Result<(), BlockError> {
        if self.pending.remove(hash).is_some()
            || self.confirmed.remove(hash).is_some()
            || self.failed.remove(hash).is_some()
        {
            Ok(())
        } else {
            Err(BlockError::TransactionValidationFailed(
                "Transaction not found".to_string(),
            ))
        }
    }

    /// Get user nonce
    pub fn get_nonce(&self, address: &str) -> u64 {
        *self.user_nonces.get(address).unwrap_or(&0)
    }

    /// Get statistics
    pub fn get_stats(&self) -> (usize, usize, usize) {
        (self.pending.len(), self.confirmed.len(), self.failed.len())
    }

    // Helper: Generate transaction hash
    fn generate_hash(from: &str, to: &str, amount: u64, nonce: u64) -> String {
        use sha2::{Digest, Sha256};
        let data = format!("{}{}{}{}", from, to, amount, nonce);
        let mut hasher = Sha256::new();
        hasher.update(data.as_bytes());
        format!("0x{:x}", hasher.finalize())
    }

    // Helper: Current timestamp
    fn current_timestamp() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }
}

impl Default for TransactionPool {
    fn default() -> Self {
        Self::new()
    }
}
