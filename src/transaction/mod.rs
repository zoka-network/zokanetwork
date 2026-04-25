//! Transacciones Privadas para ZOKA
//!
//! Proporciona:
//! - Transacciones encriptadas
//! - Commitments Pedersen
//! - Nullifiers anti-doble-gasto
//! - Validación de transacciones

pub mod commitment;
pub mod nullifier;
pub mod private_tx;
pub mod validation;

pub use commitment::Commitment;
pub use nullifier::Nullifier;
pub use private_tx::PrivateTransaction;
pub use validation::TransactionValidator;

use serde::{Deserialize, Serialize};

/// Estado de una transacción
#[derive(Clone, Debug, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransactionStatus {
    /// Creada pero no validada
    Pending,
    /// Validada pero no confirmada
    Validated,
    /// Incluida en bloque
    Confirmed,
    /// Rechazada
    Rejected,
    /// Gastada (para transacciones privadas)
    Spent,
}

/// Transaction indexing and query support
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub struct TxIndexEntry {
    pub hash: String,
    pub sender: String,
    pub receiver: String,
    pub block_height: u64,
    pub timestamp: u64,
    pub status: TransactionStatus,
}

/// Transaction indexer for efficient queries
pub struct TransactionIndexer {
    /// Index by transaction hash
    by_hash: HashMap<String, TxIndexEntry>,
    /// Index by sender address
    by_sender: HashMap<String, Vec<String>>, // sender -> list of tx hashes
    /// Index by receiver address
    by_receiver: HashMap<String, Vec<String>>, // receiver -> list of tx hashes
    /// Index by block height
    by_height: HashMap<u64, Vec<String>>, // height -> list of tx hashes
}

impl TransactionIndexer {
    /// Create a new transaction indexer
    pub fn new() -> Self {
        TransactionIndexer {
            by_hash: HashMap::new(),
            by_sender: HashMap::new(),
            by_receiver: HashMap::new(),
            by_height: HashMap::new(),
        }
    }

    /// Index a transaction
    pub fn index(&mut self, entry: TxIndexEntry) {
        let hash = entry.hash.clone();
        let sender = entry.sender.clone();
        let receiver = entry.receiver.clone();
        let height = entry.block_height;

        // Index by hash
        self.by_hash.insert(hash.clone(), entry);

        // Index by sender
        self.by_sender
            .entry(sender)
            .or_insert_with(Vec::new)
            .push(hash.clone());

        // Index by receiver
        self.by_receiver
            .entry(receiver)
            .or_insert_with(Vec::new)
            .push(hash.clone());

        // Index by height
        self.by_height
            .entry(height)
            .or_insert_with(Vec::new)
            .push(hash);
    }

    /// Query transaction by hash
    pub fn get_by_hash(&self, hash: &str) -> Option<TxIndexEntry> {
        self.by_hash.get(hash).cloned()
    }

    /// Query all transactions by sender
    pub fn get_by_sender(&self, sender: &str) -> Vec<TxIndexEntry> {
        self.by_sender
            .get(sender)
            .map(|hashes| {
                hashes
                    .iter()
                    .filter_map(|h| self.by_hash.get(h).cloned())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Query all transactions by receiver
    pub fn get_by_receiver(&self, receiver: &str) -> Vec<TxIndexEntry> {
        self.by_receiver
            .get(receiver)
            .map(|hashes| {
                hashes
                    .iter()
                    .filter_map(|h| self.by_hash.get(h).cloned())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Query all transactions in a block
    pub fn get_by_height(&self, height: u64) -> Vec<TxIndexEntry> {
        self.by_height
            .get(&height)
            .map(|hashes| {
                hashes
                    .iter()
                    .filter_map(|h| self.by_hash.get(h).cloned())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Query transactions in a height range
    pub fn get_by_height_range(&self, from: u64, to: u64) -> Vec<TxIndexEntry> {
        let mut result = Vec::new();
        for height in from..=to {
            result.extend(self.get_by_height(height));
        }
        result
    }

    /// Query transactions by sender and status
    pub fn get_by_sender_and_status(
        &self,
        sender: &str,
        status: TransactionStatus,
    ) -> Vec<TxIndexEntry> {
        self.get_by_sender(sender)
            .into_iter()
            .filter(|tx| tx.status == status)
            .collect()
    }

    /// Get total transaction count
    pub fn count(&self) -> usize {
        self.by_hash.len()
    }

    /// Get transaction count by sender
    pub fn count_by_sender(&self, sender: &str) -> usize {
        self.by_sender
            .get(sender)
            .map(|hashes| hashes.len())
            .unwrap_or(0)
    }

    /// Get transaction count in block
    pub fn count_by_height(&self, height: u64) -> usize {
        self.by_height
            .get(&height)
            .map(|hashes| hashes.len())
            .unwrap_or(0)
    }

    /// Update transaction status
    pub fn update_status(&mut self, hash: &str, status: TransactionStatus) -> bool {
        if let Some(entry) = self.by_hash.get_mut(hash) {
            entry.status = status;
            true
        } else {
            false
        }
    }

    /// Remove a transaction (for cleanup)
    pub fn remove(&mut self, hash: &str) -> Option<TxIndexEntry> {
        if let Some(entry) = self.by_hash.remove(hash) {
            // Clean up other indices
            if let Some(hashes) = self.by_sender.get_mut(&entry.sender) {
                hashes.retain(|h| h != hash);
            }
            if let Some(hashes) = self.by_receiver.get_mut(&entry.receiver) {
                hashes.retain(|h| h != hash);
            }
            if let Some(hashes) = self.by_height.get_mut(&entry.block_height) {
                hashes.retain(|h| h != hash);
            }
            Some(entry)
        } else {
            None
        }
    }

    /// Clear all indices
    pub fn clear(&mut self) {
        self.by_hash.clear();
        self.by_sender.clear();
        self.by_receiver.clear();
        self.by_height.clear();
    }
}

impl Default for TransactionIndexer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transaction_status() {
        assert_ne!(TransactionStatus::Pending, TransactionStatus::Confirmed);
    }

    // ===== TransactionIndexer Tests =====

    #[test]
    fn test_indexer_creation() {
        let indexer = TransactionIndexer::new();
        assert_eq!(indexer.count(), 0);
    }

    #[test]
    fn test_indexer_default() {
        let indexer = TransactionIndexer::default();
        assert_eq!(indexer.count(), 0);
    }

    #[test]
    fn test_index_and_get_by_hash() {
        let mut indexer = TransactionIndexer::new();
        let entry = TxIndexEntry {
            hash: "tx-hash-1".to_string(),
            sender: "alice".to_string(),
            receiver: "bob".to_string(),
            block_height: 1,
            timestamp: 1000,
            status: TransactionStatus::Confirmed,
        };

        indexer.index(entry.clone());
        let retrieved = indexer.get_by_hash("tx-hash-1");
        assert_eq!(retrieved, Some(entry));
    }

    #[test]
    fn test_get_by_nonexistent_hash() {
        let indexer = TransactionIndexer::new();
        let retrieved = indexer.get_by_hash("nonexistent");
        assert_eq!(retrieved, None);
    }

    #[test]
    fn test_get_by_sender() {
        let mut indexer = TransactionIndexer::new();
        let entry1 = TxIndexEntry {
            hash: "tx-1".to_string(),
            sender: "alice".to_string(),
            receiver: "bob".to_string(),
            block_height: 1,
            timestamp: 1000,
            status: TransactionStatus::Confirmed,
        };
        let entry2 = TxIndexEntry {
            hash: "tx-2".to_string(),
            sender: "alice".to_string(),
            receiver: "charlie".to_string(),
            block_height: 2,
            timestamp: 1001,
            status: TransactionStatus::Confirmed,
        };

        indexer.index(entry1);
        indexer.index(entry2);

        let by_alice = indexer.get_by_sender("alice");
        assert_eq!(by_alice.len(), 2);
    }

    #[test]
    fn test_get_by_receiver() {
        let mut indexer = TransactionIndexer::new();
        let entry1 = TxIndexEntry {
            hash: "tx-1".to_string(),
            sender: "alice".to_string(),
            receiver: "bob".to_string(),
            block_height: 1,
            timestamp: 1000,
            status: TransactionStatus::Confirmed,
        };
        let entry2 = TxIndexEntry {
            hash: "tx-2".to_string(),
            sender: "charlie".to_string(),
            receiver: "bob".to_string(),
            block_height: 2,
            timestamp: 1001,
            status: TransactionStatus::Confirmed,
        };

        indexer.index(entry1);
        indexer.index(entry2);

        let by_bob = indexer.get_by_receiver("bob");
        assert_eq!(by_bob.len(), 2);
    }

    #[test]
    fn test_get_by_height() {
        let mut indexer = TransactionIndexer::new();
        let entry1 = TxIndexEntry {
            hash: "tx-1".to_string(),
            sender: "alice".to_string(),
            receiver: "bob".to_string(),
            block_height: 5,
            timestamp: 1000,
            status: TransactionStatus::Confirmed,
        };
        let entry2 = TxIndexEntry {
            hash: "tx-2".to_string(),
            sender: "charlie".to_string(),
            receiver: "dave".to_string(),
            block_height: 5,
            timestamp: 1001,
            status: TransactionStatus::Confirmed,
        };

        indexer.index(entry1);
        indexer.index(entry2);

        let at_height_5 = indexer.get_by_height(5);
        assert_eq!(at_height_5.len(), 2);

        let at_height_4 = indexer.get_by_height(4);
        assert_eq!(at_height_4.len(), 0);
    }

    #[test]
    fn test_get_by_height_range() {
        let mut indexer = TransactionIndexer::new();
        for h in 1..=5 {
            let entry = TxIndexEntry {
                hash: format!("tx-{}", h),
                sender: "alice".to_string(),
                receiver: "bob".to_string(),
                block_height: h,
                timestamp: 1000 + h,
                status: TransactionStatus::Confirmed,
            };
            indexer.index(entry);
        }

        let range = indexer.get_by_height_range(2, 4);
        assert_eq!(range.len(), 3); // heights 2, 3, 4
    }

    #[test]
    fn test_get_by_sender_and_status() {
        let mut indexer = TransactionIndexer::new();
        let entry1 = TxIndexEntry {
            hash: "tx-1".to_string(),
            sender: "alice".to_string(),
            receiver: "bob".to_string(),
            block_height: 1,
            timestamp: 1000,
            status: TransactionStatus::Confirmed,
        };
        let entry2 = TxIndexEntry {
            hash: "tx-2".to_string(),
            sender: "alice".to_string(),
            receiver: "charlie".to_string(),
            block_height: 2,
            timestamp: 1001,
            status: TransactionStatus::Pending,
        };

        indexer.index(entry1);
        indexer.index(entry2);

        let confirmed = indexer.get_by_sender_and_status("alice", TransactionStatus::Confirmed);
        assert_eq!(confirmed.len(), 1);

        let pending = indexer.get_by_sender_and_status("alice", TransactionStatus::Pending);
        assert_eq!(pending.len(), 1);
    }

    #[test]
    fn test_count() {
        let mut indexer = TransactionIndexer::new();
        assert_eq!(indexer.count(), 0);

        for i in 1..=5 {
            let entry = TxIndexEntry {
                hash: format!("tx-{}", i),
                sender: "alice".to_string(),
                receiver: "bob".to_string(),
                block_height: i,
                timestamp: 1000,
                status: TransactionStatus::Confirmed,
            };
            indexer.index(entry);
        }

        assert_eq!(indexer.count(), 5);
    }

    #[test]
    fn test_count_by_sender() {
        let mut indexer = TransactionIndexer::new();
        for i in 1..=3 {
            let entry = TxIndexEntry {
                hash: format!("tx-{}", i),
                sender: "alice".to_string(),
                receiver: "bob".to_string(),
                block_height: i,
                timestamp: 1000,
                status: TransactionStatus::Confirmed,
            };
            indexer.index(entry);
        }

        assert_eq!(indexer.count_by_sender("alice"), 3);
        assert_eq!(indexer.count_by_sender("charlie"), 0);
    }

    #[test]
    fn test_count_by_height() {
        let mut indexer = TransactionIndexer::new();
        let entry1 = TxIndexEntry {
            hash: "tx-1".to_string(),
            sender: "alice".to_string(),
            receiver: "bob".to_string(),
            block_height: 5,
            timestamp: 1000,
            status: TransactionStatus::Confirmed,
        };
        let entry2 = TxIndexEntry {
            hash: "tx-2".to_string(),
            sender: "charlie".to_string(),
            receiver: "dave".to_string(),
            block_height: 5,
            timestamp: 1001,
            status: TransactionStatus::Confirmed,
        };

        indexer.index(entry1);
        indexer.index(entry2);

        assert_eq!(indexer.count_by_height(5), 2);
        assert_eq!(indexer.count_by_height(6), 0);
    }

    #[test]
    fn test_update_status() {
        let mut indexer = TransactionIndexer::new();
        let entry = TxIndexEntry {
            hash: "tx-1".to_string(),
            sender: "alice".to_string(),
            receiver: "bob".to_string(),
            block_height: 1,
            timestamp: 1000,
            status: TransactionStatus::Pending,
        };

        indexer.index(entry);
        assert!(indexer.update_status("tx-1", TransactionStatus::Confirmed));

        let updated = indexer.get_by_hash("tx-1").unwrap();
        assert_eq!(updated.status, TransactionStatus::Confirmed);
    }

    #[test]
    fn test_update_status_nonexistent() {
        let mut indexer = TransactionIndexer::new();
        assert!(!indexer.update_status("nonexistent", TransactionStatus::Confirmed));
    }

    #[test]
    fn test_remove() {
        let mut indexer = TransactionIndexer::new();
        let entry = TxIndexEntry {
            hash: "tx-1".to_string(),
            sender: "alice".to_string(),
            receiver: "bob".to_string(),
            block_height: 1,
            timestamp: 1000,
            status: TransactionStatus::Confirmed,
        };

        indexer.index(entry.clone());
        assert_eq!(indexer.count(), 1);

        let removed = indexer.remove("tx-1");
        assert_eq!(removed, Some(entry));
        assert_eq!(indexer.count(), 0);
    }

    #[test]
    fn test_remove_cleans_all_indices() {
        let mut indexer = TransactionIndexer::new();
        let entry = TxIndexEntry {
            hash: "tx-1".to_string(),
            sender: "alice".to_string(),
            receiver: "bob".to_string(),
            block_height: 1,
            timestamp: 1000,
            status: TransactionStatus::Confirmed,
        };

        indexer.index(entry);
        indexer.remove("tx-1");

        assert!(indexer.get_by_hash("tx-1").is_none());
        assert_eq!(indexer.get_by_sender("alice").len(), 0);
        assert_eq!(indexer.get_by_receiver("bob").len(), 0);
        assert_eq!(indexer.get_by_height(1).len(), 0);
    }

    #[test]
    fn test_clear() {
        let mut indexer = TransactionIndexer::new();
        for i in 1..=5 {
            let entry = TxIndexEntry {
                hash: format!("tx-{}", i),
                sender: "alice".to_string(),
                receiver: "bob".to_string(),
                block_height: i,
                timestamp: 1000,
                status: TransactionStatus::Confirmed,
            };
            indexer.index(entry);
        }

        assert_eq!(indexer.count(), 5);
        indexer.clear();
        assert_eq!(indexer.count(), 0);
    }

    #[test]
    fn test_multiple_transactions_same_sender() {
        let mut indexer = TransactionIndexer::new();
        for i in 1..=10 {
            let entry = TxIndexEntry {
                hash: format!("tx-{}", i),
                sender: "alice".to_string(),
                receiver: format!("user-{}", i),
                block_height: i,
                timestamp: 1000 + i,
                status: TransactionStatus::Confirmed,
            };
            indexer.index(entry);
        }

        assert_eq!(indexer.count_by_sender("alice"), 10);
        let txs = indexer.get_by_sender("alice");
        assert_eq!(txs.len(), 10);
    }
}
