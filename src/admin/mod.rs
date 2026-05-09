// Admin module: Tools for monitoring and admin
use crate::types::{Metrics, NodeInfo};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Node monitoring and metrics
#[allow(dead_code)]
pub struct NodeMonitor {
    block_height: Arc<AtomicU64>,
    pending_transactions: Arc<AtomicU64>,
    connected_peers: Arc<AtomicU64>,
    valid_transactions: Arc<AtomicU64>,
    rejected_transactions: Arc<AtomicU64>,
    status: Arc<RwLock<String>>,
}

impl NodeMonitor {
    pub fn new() -> Self {
        NodeMonitor {
            block_height: Arc::new(AtomicU64::new(0)),
            pending_transactions: Arc::new(AtomicU64::new(0)),
            connected_peers: Arc::new(AtomicU64::new(0)),
            valid_transactions: Arc::new(AtomicU64::new(0)),
            rejected_transactions: Arc::new(AtomicU64::new(0)),
            status: Arc::new(RwLock::new("initializing".to_string())),
        }
    }

    pub fn set_block_height(&self, height: u64) {
        self.block_height.store(height, Ordering::SeqCst);
    }

    pub fn set_pending_transactions(&self, count: u64) {
        self.pending_transactions.store(count, Ordering::SeqCst);
    }

    pub fn set_connected_peers(&self, count: u64) {
        self.connected_peers.store(count, Ordering::SeqCst);
    }

    pub fn increment_valid_transactions(&self) {
        self.valid_transactions.fetch_add(1, Ordering::SeqCst);
    }

    pub fn increment_rejected_transactions(&self) {
        self.rejected_transactions.fetch_add(1, Ordering::SeqCst);
    }

    pub async fn set_status(&self, status: String) {
        let mut s = self.status.write().await;
        *s = status;
    }

    pub fn get_metrics(&self) -> Metrics {
        Metrics {
            block_height: self.block_height.load(Ordering::SeqCst),
            pending_transactions: self.pending_transactions.load(Ordering::SeqCst),
            connected_peers: self.connected_peers.load(Ordering::SeqCst),
            valid_transactions: self.valid_transactions.load(Ordering::SeqCst),
            rejected_transactions: self.rejected_transactions.load(Ordering::SeqCst),
        }
    }
}

impl Default for NodeMonitor {
    fn default() -> Self {
        Self::new()
    }
}

pub fn get_node_info() -> NodeInfo {
    NodeInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        block_height: 0,
        block_hash: "0".to_string(),
        peer_count: 0,
        pending_transactions: 0,
    }
}

pub fn get_metrics() -> Metrics {
    Metrics {
        block_height: 0,
        pending_transactions: 0,
        connected_peers: 0,
        valid_transactions: 0,
        rejected_transactions: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_monitor_creation() {
        let monitor = NodeMonitor::new();
        monitor.set_block_height(100);
        assert_eq!(monitor.block_height.load(Ordering::SeqCst), 100);
    }

    #[test]
    fn test_metrics() {
        let monitor = NodeMonitor::new();
        monitor.set_block_height(5);
        monitor.set_pending_transactions(10);
        monitor.set_connected_peers(3);

        let metrics = monitor.get_metrics();
        assert_eq!(metrics.block_height, 5);
        assert_eq!(metrics.pending_transactions, 10);
        assert_eq!(metrics.connected_peers, 3);
    }

    #[test]
    fn test_transaction_counters() {
        let monitor = NodeMonitor::new();
        monitor.increment_valid_transactions();
        monitor.increment_valid_transactions();
        monitor.increment_rejected_transactions();

        let metrics = monitor.get_metrics();
        assert_eq!(metrics.valid_transactions, 2);
        assert_eq!(metrics.rejected_transactions, 1);
    }
}
