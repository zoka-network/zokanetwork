use crate::types::{State, Transaction, TransactionError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

#[derive(Clone, Debug)]
struct PendingTx {
    tx: Transaction,
    priority_fee: u64,
    inserted_at: u64,
}

/// Mempool statistics and metrics
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MempoolMetrics {
    pub total_size: usize,
    pub max_size: usize,
    pub transactions_added: u64,
    pub transactions_removed: u64,
    pub transactions_evicted: u64,
    pub total_fees_collected: u64,
    pub average_fee: u64,
    pub rejections_insufficient_fee: u64,
    pub rejections_too_many_from_sender: u64,
    pub rejections_invalid_state: u64,
}

impl Default for MempoolMetrics {
    fn default() -> Self {
        MempoolMetrics {
            total_size: 0,
            max_size: 0,
            transactions_added: 0,
            transactions_removed: 0,
            transactions_evicted: 0,
            total_fees_collected: 0,
            average_fee: 0,
            rejections_insufficient_fee: 0,
            rejections_too_many_from_sender: 0,
            rejections_invalid_state: 0,
        }
    }
}

/// Transaction mempool for managing pending transactions
pub struct Mempool {
    pending: Arc<RwLock<HashMap<String, PendingTx>>>,
    max_size: usize,
    max_per_sender: usize,
    min_priority_fee: u64,
    metrics: Arc<RwLock<MempoolMetrics>>,
}

impl Mempool {
    /// Create a new mempool with configurable max size.
    pub fn new(max_size: usize) -> Self {
        Self::with_policy(max_size, 32, 0)
    }

    /// Create a mempool with explicit anti-spam and fee policy.
    pub fn with_policy(max_size: usize, max_per_sender: usize, min_priority_fee: u64) -> Self {
        let mut metrics = MempoolMetrics::default();
        metrics.max_size = max_size;
        Mempool {
            pending: Arc::new(RwLock::new(HashMap::new())),
            max_size,
            max_per_sender,
            min_priority_fee,
            metrics: Arc::new(RwLock::new(metrics)),
        }
    }

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    /// Add a transaction to the mempool after validation with default fee=0.
    pub async fn add_transaction(
        &self,
        tx: Transaction,
        state: &State,
    ) -> Result<(), TransactionError> {
        self.add_transaction_with_fee(tx, state, 0).await
    }

    /// Add a transaction with an explicit priority fee.
    pub async fn add_transaction_with_fee(
        &self,
        tx: Transaction,
        state: &State,
        priority_fee: u64,
    ) -> Result<(), TransactionError> {
        if priority_fee < self.min_priority_fee {
            let mut metrics = self.metrics.write().await;
            metrics.rejections_insufficient_fee += 1;
            return Err(TransactionError::InsufficientPriorityFee {
                minimum: self.min_priority_fee,
                actual: priority_fee,
            });
        }

        if let Err(e) = self.validate_transaction(&tx, state).await {
            let mut metrics = self.metrics.write().await;
            metrics.rejections_invalid_state += 1;
            return Err(e);
        }

        let mut pending = self.pending.write().await;
        if pending.contains_key(&tx.hash) {
            return Err(TransactionError::DuplicateTransaction);
        }

        let sender_pending = pending
            .values()
            .filter(|entry| entry.tx.sender == tx.sender)
            .count();
        if sender_pending >= self.max_per_sender {
            let mut metrics = self.metrics.write().await;
            metrics.rejections_too_many_from_sender += 1;
            return Err(TransactionError::TooManyPendingFromSender {
                sender: tx.sender.clone(),
                limit: self.max_per_sender,
            });
        }

        if pending.len() >= self.max_size {
            let mut lowest: Option<(String, u64, u64)> = None;
            for (hash, entry) in pending.iter() {
                let candidate = (hash.clone(), entry.priority_fee, entry.inserted_at);
                match &lowest {
                    Some((_, lowest_fee, lowest_inserted)) => {
                        if candidate.1 < *lowest_fee
                            || (candidate.1 == *lowest_fee && candidate.2 < *lowest_inserted)
                        {
                            lowest = Some(candidate);
                        }
                    }
                    None => lowest = Some(candidate),
                }
            }

            if let Some((lowest_hash, lowest_fee, _)) = lowest {
                if priority_fee <= lowest_fee {
                    return Err(TransactionError::MempoolFull);
                }
                pending.remove(&lowest_hash);
                let mut metrics = self.metrics.write().await;
                metrics.transactions_evicted += 1;
            } else {
                return Err(TransactionError::MempoolFull);
            }
        }

        pending.insert(
            tx.hash.clone(),
            PendingTx {
                tx,
                priority_fee,
                inserted_at: Self::now_secs(),
            },
        );

        // Update metrics for successful insertion
        let mut metrics = self.metrics.write().await;
        metrics.transactions_added += 1;
        metrics.total_fees_collected += priority_fee;
        metrics.total_size = pending.len();
        let total_txs = metrics.transactions_added;
        if total_txs > 0 {
            metrics.average_fee = metrics.total_fees_collected / total_txs;
        }
        Ok(())
    }

    /// Validate a transaction against current state.
    pub async fn validate_transaction(
        &self,
        tx: &Transaction,
        state: &State,
    ) -> Result<(), TransactionError> {
        let sender_account = state
            .get_account(&tx.sender)
            .ok_or(TransactionError::AccountNotFound)?;

        if tx.nonce <= sender_account.nonce {
            return Err(TransactionError::InvalidNonce {
                expected: sender_account.nonce + 1,
                actual: tx.nonce,
            });
        }

        if tx.amount == 0 {
            return Err(TransactionError::InvalidAmount);
        }

        if sender_account.balance < tx.amount {
            return Err(TransactionError::InsufficientBalance);
        }

        Ok(())
    }

    /// Get pending transactions ordered by descending priority fee and then insertion order.
    pub async fn get_pending(&self) -> Vec<Transaction> {
        let pending = self.pending.read().await;
        let mut ordered: Vec<&PendingTx> = pending.values().collect();
        ordered.sort_by(|a, b| {
            b.priority_fee
                .cmp(&a.priority_fee)
                .then_with(|| a.inserted_at.cmp(&b.inserted_at))
        });
        ordered.into_iter().map(|entry| entry.tx.clone()).collect()
    }

    /// Get best transactions for block creation with weight optimization.
    /// Selects transactions by fee priority up to max_transactions or max_weight.
    pub async fn get_block_transactions(
        &self,
        max_transactions: usize,
        max_weight: u64,
    ) -> Vec<Transaction> {
        let pending = self.pending.read().await;
        let mut ordered: Vec<&PendingTx> = pending.values().collect();
        ordered.sort_by(|a, b| {
            b.priority_fee
                .cmp(&a.priority_fee)
                .then_with(|| a.inserted_at.cmp(&b.inserted_at))
        });

        let mut selected = Vec::new();
        let mut current_weight: u64 = 0;

        for pendingtx in ordered {
            if selected.len() >= max_transactions {
                break;
            }

            // Estimate transaction weight (simplified: base + output size)
            let tx_weight: u64 = 300 + (pendingtx.tx.sender.len() as u64) * 2;
            if current_weight + tx_weight > max_weight && !selected.is_empty() {
                break;
            }

            selected.push(pendingtx.tx.clone());
            current_weight += tx_weight;
        }

        selected
    }

    /// Remove a transaction (after block inclusion).
    pub async fn remove_transaction(&self, hash: &str) -> Result<(), TransactionError> {
        let mut pending = self.pending.write().await;
        pending
            .remove(hash)
            .ok_or(TransactionError::AccountNotFound)?;

        let mut metrics = self.metrics.write().await;
        metrics.transactions_removed += 1;
        metrics.total_size = pending.len();

        Ok(())
    }

    /// Replace a pending transaction with a higher-fee version (fee-bumping).
    /// The new transaction must have the same sender and receiver as the original.
    /// Returns error if original not found, new fee is not higher, or nonce differs.
    pub async fn replace_transaction(
        &self,
        old_hash: &str,
        new_tx: Transaction,
        new_priority_fee: u64,
        state: &State,
    ) -> Result<(), TransactionError> {
        let pending = self.pending.read().await;

        // Find and validate the old transaction
        let old_entry = pending
            .get(old_hash)
            .ok_or(TransactionError::AccountNotFound)?;

        // Copy values we need before dropping the lock
        let old_sender = old_entry.tx.sender.clone();
        let old_receiver = old_entry.tx.receiver.clone();
        let old_nonce = old_entry.tx.nonce;
        let old_fee = old_entry.priority_fee;

        // Validate replacement conditions
        if old_sender != new_tx.sender || old_receiver != new_tx.receiver {
            return Err(TransactionError::InvalidAmount);
        }

        if new_tx.nonce != old_nonce {
            return Err(TransactionError::InvalidNonce {
                expected: old_nonce,
                actual: new_tx.nonce,
            });
        }

        if new_priority_fee <= old_fee {
            return Err(TransactionError::InsufficientPriorityFee {
                minimum: old_fee + 1,
                actual: new_priority_fee,
            });
        }

        // Drop the read lock
        drop(pending);

        // Validate new transaction against state
        self.validate_transaction(&new_tx, state).await?;

        // Re-acquire write lock and perform replacement
        let mut pending = self.pending.write().await;
        pending.remove(old_hash);
        pending.insert(
            new_tx.hash.clone(),
            PendingTx {
                tx: new_tx,
                priority_fee: new_priority_fee,
                inserted_at: Self::now_secs(),
            },
        );

        // Update metrics
        let mut metrics = self.metrics.write().await;
        metrics.total_fees_collected += new_priority_fee - old_fee;
        let total_txs = metrics.transactions_added;
        if total_txs > 0 {
            metrics.average_fee = metrics.total_fees_collected / total_txs;
        }

        Ok(())
    }

    /// Get a specific transaction.
    pub async fn get_transaction(&self, hash: &str) -> Option<Transaction> {
        let pending = self.pending.read().await;
        pending.get(hash).map(|entry| entry.tx.clone())
    }

    /// Get the number of pending transactions.
    pub async fn len(&self) -> usize {
        let pending = self.pending.read().await;
        pending.len()
    }

    /// Get current mempool metrics
    pub async fn get_metrics(&self) -> MempoolMetrics {
        let mut metrics = self.metrics.read().await.clone();
        // Update current size snapshot
        let pending = self.pending.read().await;
        metrics.total_size = pending.len();
        metrics
    }

    /// Reset metrics (useful for testing)
    pub async fn reset_metrics(&self) {
        let mut metrics = self.metrics.write().await;
        *metrics = MempoolMetrics {
            max_size: metrics.max_size,
            ..Default::default()
        };
    }

    /// Clear all pending transactions (for testing).
    pub async fn clear(&self) {
        let mut pending = self.pending.write().await;
        pending.clear();
    }
}

/// Advanced mempool manager with priority queues, eviction, and TTL support
#[derive(Clone, Debug)]
pub struct MempoolManager {
    /// Mempool transactions with priorities
    transactions: HashMap<String, (Transaction, u64)>, // hash -> (tx, priority)
    /// Timestamp when transaction was added
    timestamps: HashMap<String, u64>, // hash -> inserted_at
    /// Transaction TTL in seconds
    default_ttl: u64,
    /// Maximum mempool size
    max_size: usize,
    /// Eviction strategy: "lowest_fee", "oldest", "random"
    eviction_strategy: String,
}

impl MempoolManager {
    /// Create a new mempool manager
    pub fn new(max_size: usize, default_ttl: u64) -> Self {
        MempoolManager {
            transactions: HashMap::new(),
            timestamps: HashMap::new(),
            default_ttl,
            max_size,
            eviction_strategy: "lowest_fee".to_string(),
        }
    }

    /// Add transaction with priority
    pub fn add_transaction(
        &mut self,
        hash: String,
        tx: Transaction,
        priority: u64,
    ) -> Result<(), String> {
        // Check if mempool is full, needs eviction
        if self.transactions.len() >= self.max_size {
            if !self.evict_one() {
                return Err("Cannot add transaction: mempool full and eviction failed".to_string());
            }
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        self.transactions.insert(hash.clone(), (tx, priority));
        self.timestamps.insert(hash, now);
        Ok(())
    }

    /// Evict one transaction based on strategy
    fn evict_one(&mut self) -> bool {
        if self.transactions.is_empty() {
            return false;
        }

        let to_remove = match self.eviction_strategy.as_str() {
            "lowest_fee" => {
                // Find transaction with lowest priority
                let (hash, _) = self
                    .transactions
                    .iter()
                    .min_by_key(|(_, (_, priority))| *priority)
                    .map(|(h, (_, p))| (h.clone(), *p))
                    .unwrap_or_default();
                hash
            }
            "oldest" => {
                // Find oldest transaction
                let (hash, _) = self
                    .timestamps
                    .iter()
                    .min_by_key(|(_, timestamp)| *timestamp)
                    .map(|(h, t)| (h.clone(), *t))
                    .unwrap_or_default();
                hash
            }
            _ => {
                // Random (just take first)
                self.transactions.keys().next().cloned().unwrap_or_default()
            }
        };

        if !to_remove.is_empty() {
            self.transactions.remove(&to_remove);
            self.timestamps.remove(&to_remove);
            true
        } else {
            false
        }
    }

    /// Remove transaction by hash
    pub fn remove(&mut self, hash: &str) -> Option<(Transaction, u64)> {
        self.timestamps.remove(hash);
        self.transactions.remove(hash)
    }

    /// Get transaction by hash
    pub fn get(&self, hash: &str) -> Option<&(Transaction, u64)> {
        self.transactions.get(hash)
    }

    /// Get transactions by sender (pending)
    pub fn get_by_sender(&self, sender: &str) -> Vec<(String, u64)> {
        self.transactions
            .iter()
            .filter(|(_, (tx, _))| tx.sender == sender)
            .map(|(h, (_, p))| (h.clone(), *p))
            .collect()
    }

    /// Get top N transactions by priority
    pub fn get_top_by_priority(&self, count: usize) -> Vec<(String, u64)> {
        let mut txs: Vec<_> = self
            .transactions
            .iter()
            .map(|(h, (_, p))| (h.clone(), *p))
            .collect();
        txs.sort_by(|a, b| b.1.cmp(&a.1)); // Sort descending by priority
        txs.into_iter().take(count).collect()
    }

    /// Expire transactions older than TTL
    pub fn expire_old_transactions(&mut self) -> usize {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let expired: Vec<String> = self
            .timestamps
            .iter()
            .filter(|(_, timestamp)| now - **timestamp > self.default_ttl)
            .map(|(h, _)| h.clone())
            .collect();

        let count = expired.len();
        for hash in expired {
            self.remove(&hash);
        }
        count
    }

    /// Get transaction count
    pub fn count(&self) -> usize {
        self.transactions.len()
    }

    /// Get memory size estimate (rough)
    pub fn size_estimate(&self) -> usize {
        self.transactions.len() * 256 // Approximate
    }

    /// Set eviction strategy
    pub fn set_eviction_strategy(&mut self, strategy: String) {
        self.eviction_strategy = strategy;
    }

    /// Get current eviction strategy
    pub fn get_eviction_strategy(&self) -> &str {
        &self.eviction_strategy
    }

    /// Check if transaction exists
    pub fn contains(&self, hash: &str) -> bool {
        self.transactions.contains_key(hash)
    }

    /// Clear all transactions
    pub fn clear(&mut self) {
        self.transactions.clear();
        self.timestamps.clear();
    }

    /// Get mempool statistics
    pub fn stats(&self) -> (usize, usize, f64) {
        let count = self.transactions.len();
        let avg_priority = if count > 0 {
            let sum: u64 = self.transactions.values().map(|(_, p)| p).sum();
            sum as f64 / count as f64
        } else {
            0.0
        };
        (count, self.max_size, avg_priority)
    }
}

impl Default for MempoolManager {
    fn default() -> Self {
        Self::new(10000, 600) // Default: 10K txs, 600s TTL
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AccountState, Signature};

    fn base_state(balance: u64, nonce: u64) -> State {
        let mut state = State::new();
        let mut alice = AccountState::new("alice".to_string());
        alice.balance = balance;
        alice.nonce = nonce;
        state.accounts.insert("alice".to_string(), alice);
        state
    }

    #[tokio::test]
    async fn test_mempool_add_transaction() {
        let mempool = Mempool::new(100);
        let state = base_state(1000, 0);

        let tx = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            100,
            1,
            0,
            Signature(vec![]),
        );

        let result = mempool.add_transaction(tx.clone(), &state).await;
        assert!(result.is_ok());
        assert_eq!(mempool.len().await, 1);
    }

    #[tokio::test]
    async fn test_mempool_duplicate_transaction() {
        let mempool = Mempool::new(100);
        let state = base_state(1000, 0);

        let tx = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            100,
            1,
            0,
            Signature(vec![]),
        );

        mempool.add_transaction(tx.clone(), &state).await.unwrap();
        let result = mempool.add_transaction(tx, &state).await;
        assert!(matches!(
            result,
            Err(TransactionError::DuplicateTransaction)
        ));
    }

    #[tokio::test]
    async fn test_mempool_invalid_nonce() {
        let mempool = Mempool::new(100);
        let state = base_state(1000, 5);

        let tx = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            100,
            5,
            0,
            Signature(vec![]),
        );

        let result = mempool.add_transaction(tx, &state).await;
        assert!(matches!(result, Err(TransactionError::InvalidNonce { .. })));
    }

    #[tokio::test]
    async fn test_mempool_insufficient_balance() {
        let mempool = Mempool::new(100);
        let state = base_state(50, 0);

        let tx = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            100,
            1,
            0,
            Signature(vec![]),
        );

        let result = mempool.add_transaction(tx, &state).await;
        assert!(matches!(result, Err(TransactionError::InsufficientBalance)));
    }

    #[tokio::test]
    async fn test_mempool_remove_transaction() {
        let mempool = Mempool::new(100);
        let state = base_state(1000, 0);

        let tx = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            100,
            1,
            0,
            Signature(vec![]),
        );

        let hash = tx.hash.clone();
        mempool.add_transaction(tx, &state).await.unwrap();
        assert_eq!(mempool.len().await, 1);

        let result = mempool.remove_transaction(&hash).await;
        assert!(result.is_ok());
        assert_eq!(mempool.len().await, 0);
    }

    #[tokio::test]
    async fn test_mempool_replaces_low_fee_when_full() {
        let mempool = Mempool::with_policy(1, 32, 0);
        let state = base_state(5000, 0);

        let tx_low = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            100,
            1,
            1,
            Signature(vec![]),
        );
        mempool
            .add_transaction_with_fee(tx_low.clone(), &state, 1)
            .await
            .unwrap();

        let tx_high = Transaction::new(
            "alice".to_string(),
            "carol".to_string(),
            150,
            2,
            2,
            Signature(vec![]),
        );
        mempool
            .add_transaction_with_fee(tx_high.clone(), &state, 5)
            .await
            .unwrap();

        assert!(mempool.get_transaction(&tx_low.hash).await.is_none());
        assert!(mempool.get_transaction(&tx_high.hash).await.is_some());
    }

    #[tokio::test]
    async fn test_mempool_sender_spam_limit() {
        let mempool = Mempool::with_policy(10, 1, 0);
        let state = base_state(5000, 0);

        let tx1 = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            100,
            1,
            1,
            Signature(vec![]),
        );
        mempool.add_transaction(tx1, &state).await.unwrap();

        let tx2 = Transaction::new(
            "alice".to_string(),
            "carol".to_string(),
            100,
            2,
            2,
            Signature(vec![]),
        );
        let result = mempool.add_transaction(tx2, &state).await;
        assert!(matches!(
            result,
            Err(TransactionError::TooManyPendingFromSender { .. })
        ));
    }

    #[tokio::test]
    async fn test_mempool_min_priority_fee_policy() {
        let mempool = Mempool::with_policy(10, 32, 2);
        let state = base_state(5000, 0);

        let tx = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            100,
            1,
            1,
            Signature(vec![]),
        );
        let result = mempool.add_transaction_with_fee(tx, &state, 1).await;
        assert!(matches!(
            result,
            Err(TransactionError::InsufficientPriorityFee {
                minimum: 2,
                actual: 1
            })
        ));
    }

    // ===== MempoolManager Tests =====

    #[test]
    fn test_mempool_manager_creation() {
        let manager = MempoolManager::new(100, 600);
        assert_eq!(manager.count(), 0);
    }

    #[test]
    fn test_mempool_manager_default() {
        let manager = MempoolManager::default();
        assert!(manager.count() == 0);
    }

    #[test]
    fn test_add_transaction() {
        let mut manager = MempoolManager::new(100, 600);
        let tx = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            100,
            1,
            1,
            Signature(vec![]),
        );

        let result = manager.add_transaction("tx1".to_string(), tx, 10);
        assert!(result.is_ok());
        assert_eq!(manager.count(), 1);
    }

    #[test]
    fn test_get_transaction() {
        let mut manager = MempoolManager::new(100, 600);
        let tx = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            100,
            1,
            1,
            Signature(vec![]),
        );

        manager
            .add_transaction("tx1".to_string(), tx.clone(), 10)
            .ok();
        let retrieved = manager.get("tx1");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().0.hash, tx.hash);
    }

    #[test]
    fn test_remove_transaction() {
        let mut manager = MempoolManager::new(100, 600);
        let tx = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            100,
            1,
            1,
            Signature(vec![]),
        );

        manager.add_transaction("tx1".to_string(), tx, 10).ok();
        assert_eq!(manager.count(), 1);

        let removed = manager.remove("tx1");
        assert!(removed.is_some());
        assert_eq!(manager.count(), 0);
    }

    #[test]
    fn test_get_by_sender() {
        let mut manager = MempoolManager::new(100, 600);

        for i in 1..=3 {
            let tx = Transaction::new(
                "alice".to_string(),
                format!("receiver-{}", i),
                100,
                i as u64,
                1,
                Signature(vec![]),
            );
            manager
                .add_transaction(format!("tx-{}", i), tx, i as u64 * 10)
                .ok();
        }

        let alice_txs = manager.get_by_sender("alice");
        assert_eq!(alice_txs.len(), 3);
    }

    #[test]
    fn test_get_top_by_priority() {
        let mut manager = MempoolManager::new(100, 600);

        for i in 1..=5 {
            let tx = Transaction::new(
                "alice".to_string(),
                format!("bob-{}", i),
                100,
                i as u64,
                1,
                Signature(vec![]),
            );
            manager
                .add_transaction(format!("tx-{}", i), tx, i as u64 * 10)
                .ok();
        }

        let top = manager.get_top_by_priority(2);
        assert_eq!(top.len(), 2);
        // Should be tx-5 (priority 50) and tx-4 (priority 40)
        assert_eq!(top[0].1, 50);
        assert_eq!(top[1].1, 40);
    }

    #[test]
    fn test_eviction_lowest_fee() {
        let mut manager = MempoolManager::new(3, 600);
        manager.set_eviction_strategy("lowest_fee".to_string());

        for i in 1..=4 {
            let tx = Transaction::new(
                "alice".to_string(),
                format!("bob-{}", i),
                100,
                i as u64,
                1,
                Signature(vec![]),
            );
            manager
                .add_transaction(format!("tx-{}", i), tx, i as u64)
                .ok();
        }

        // tx-1 should have been evicted (lowest priority)
        assert_eq!(manager.count(), 3);
        assert!(!manager.contains("tx-1"));
    }

    #[test]
    fn test_eviction_oldest() {
        let mut manager = MempoolManager::new(2, 600);
        manager.set_eviction_strategy("oldest".to_string());

        let tx1 = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            100,
            1,
            1,
            Signature(vec![]),
        );
        manager.add_transaction("tx-1".to_string(), tx1, 10).ok();

        // Add small delay or just add another
        let tx2 = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            100,
            2,
            1,
            Signature(vec![]),
        );
        manager.add_transaction("tx-2".to_string(), tx2, 20).ok();

        let tx3 = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            100,
            3,
            1,
            Signature(vec![]),
        );
        manager.add_transaction("tx-3".to_string(), tx3, 30).ok();

        // tx-1 should have been evicted (oldest)
        assert_eq!(manager.count(), 2);
    }

    #[test]
    fn test_set_eviction_strategy() {
        let mut manager = MempoolManager::new(100, 600);
        assert_eq!(manager.get_eviction_strategy(), "lowest_fee");

        manager.set_eviction_strategy("oldest".to_string());
        assert_eq!(manager.get_eviction_strategy(), "oldest");
    }

    #[test]
    fn test_contains() {
        let mut manager = MempoolManager::new(100, 600);
        let tx = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            100,
            1,
            1,
            Signature(vec![]),
        );

        manager.add_transaction("tx1".to_string(), tx, 10).ok();
        assert!(manager.contains("tx1"));
        assert!(!manager.contains("tx2"));
    }

    #[test]
    fn test_clear() {
        let mut manager = MempoolManager::new(100, 600);

        for i in 1..=5 {
            let tx = Transaction::new(
                "alice".to_string(),
                format!("bob-{}", i),
                100,
                i as u64,
                1,
                Signature(vec![]),
            );
            manager
                .add_transaction(format!("tx-{}", i), tx, i as u64)
                .ok();
        }

        assert_eq!(manager.count(), 5);
        manager.clear();
        assert_eq!(manager.count(), 0);
    }

    #[test]
    fn test_size_estimate() {
        let mut manager = MempoolManager::new(100, 600);

        for i in 1..=5 {
            let tx = Transaction::new(
                "alice".to_string(),
                format!("bob-{}", i),
                100,
                i as u64,
                1,
                Signature(vec![]),
            );
            manager
                .add_transaction(format!("tx-{}", i), tx, i as u64)
                .ok();
        }

        let size = manager.size_estimate();
        assert!(size > 0);
    }

    #[test]
    fn test_stats() {
        let mut manager = MempoolManager::new(100, 600);

        for i in 1..=3 {
            let tx = Transaction::new(
                "alice".to_string(),
                format!("bob-{}", i),
                100,
                i as u64,
                1,
                Signature(vec![]),
            );
            manager
                .add_transaction(format!("tx-{}", i), tx, i as u64 * 10)
                .ok();
        }

        let (count, max, avg) = manager.stats();
        assert_eq!(count, 3);
        assert_eq!(max, 100);
        assert!(avg > 0.0);
    }

    #[test]
    fn test_mempool_manager_full() {
        let mut manager = MempoolManager::new(2, 600);

        let tx1 = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            100,
            1,
            1,
            Signature(vec![]),
        );
        let tx2 = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            100,
            2,
            1,
            Signature(vec![]),
        );
        let tx3 = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            100,
            3,
            1,
            Signature(vec![]),
        );

        assert!(manager.add_transaction("tx-1".to_string(), tx1, 5).is_ok());
        assert!(manager.add_transaction("tx-2".to_string(), tx2, 10).is_ok());
        // Adding tx-3 should trigger eviction
        assert!(manager.add_transaction("tx-3".to_string(), tx3, 15).is_ok());
        assert_eq!(manager.count(), 2);
    }
}
