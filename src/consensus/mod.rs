// Consensus module: Handles block validation and agreement
use crate::types::{Block, BlockError, PublicKey, Validator};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

pub mod pbft;
pub use pbft::{
    PbftConsensus, PbftEvent, PbftMessage, PbftMessageType, PbftNewView, PbftViewChange,
};

/// Proof-of-Authority consensus engine
#[allow(dead_code)]
pub struct PoAConsensus {
    validators: Arc<RwLock<HashMap<String, Validator>>>,
}

impl PoAConsensus {
    /// Create a new PoA consensus with validators
    pub fn new() -> Self {
        PoAConsensus {
            validators: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Add a validator (only for genesis/admin)
    pub async fn add_validator(&self, public_key: PublicKey, voting_power: u64) {
        let mut validators = self.validators.write().await;
        let key_str = format!("{:?}", &public_key);
        validators.insert(key_str, Validator::new(public_key, voting_power));
    }

    /// Check if a public key is an authorized validator
    pub async fn is_validator(&self, public_key: &PublicKey) -> bool {
        let validators = self.validators.read().await;
        let key_str = format!("{:?}", public_key);
        validators.contains_key(&key_str)
    }

    /// Validate a block according to PoA rules
    pub fn validate_block(&self, block: &Block) -> Result<(), BlockError> {
        // Rule 1: Block hash cannot be zero (basic check)
        if block.header.hash.is_empty() {
            return Err(BlockError::InvalidBlockHash);
        }
        if block.header.hash != block.compute_hash() {
            return Err(BlockError::InvalidBlockHash);
        }

        // Rule 2: Timestamp must be reasonable
        if block.header.timestamp == 0 {
            return Err(BlockError::InvalidTimestamp);
        }

        // Rule 3: Merkle root must match transactions
        let computed_merkle = block.compute_merkle_root();
        if computed_merkle != block.header.merkle_root {
            return Err(BlockError::InvalidMerkleRoot);
        }

        // Rule 4: State root must be present
        if block.header.state_root.is_empty() {
            return Err(BlockError::InvalidStateRoot);
        }

        // Rule 5: All transactions must be valid
        for tx in &block.transactions {
            if tx.hash.is_empty() {
                return Err(BlockError::TransactionValidationFailed(
                    "Empty transaction hash".to_string(),
                ));
            }
            if tx.sender == tx.receiver {
                return Err(BlockError::TransactionValidationFailed(
                    "Self-transfer not allowed".to_string(),
                ));
            }
        }

        // Rule 6: Every private tx hash the block references must be
        // non-empty and unique. Deeper validation (the hash actually maps
        // to a valid, range-proven, ring-signed `PrivateTransaction`)
        // happens when the body is applied from the local pool — that
        // pipeline runs outside this pure structural check.
        let mut seen_private: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for h in &block.private_tx_hashes {
            if h.is_empty() {
                return Err(BlockError::TransactionValidationFailed(
                    "Empty private_tx_hash in block body".to_string(),
                ));
            }
            if !seen_private.insert(h.as_str()) {
                return Err(BlockError::TransactionValidationFailed(format!(
                    "Duplicate private_tx_hash {} in block body",
                    h
                )));
            }
        }

        Ok(())
    }

    /// Create a new block (for validators). Keeps the legacy signature —
    /// prefer `create_block_with_private` when private tx hashes are
    /// available so they end up in the Merkle root.
    pub fn create_block(
        &self,
        prev_hash: String,
        block_height: u64,
        transactions: Vec<crate::types::Transaction>,
        validator_key: PublicKey,
    ) -> Result<Block, BlockError> {
        self.create_block_with_private(
            prev_hash,
            block_height,
            transactions,
            Vec::new(),
            validator_key,
        )
    }

    /// Create a new block that commits both the public transactions and
    /// the provided private transaction hashes. The Merkle root covers
    /// both, so tampering with either invalidates the block hash.
    pub fn create_block_with_private(
        &self,
        prev_hash: String,
        block_height: u64,
        transactions: Vec<crate::types::Transaction>,
        private_tx_hashes: Vec<String>,
        validator_key: PublicKey,
    ) -> Result<Block, BlockError> {
        use std::time::{SystemTime, UNIX_EPOCH};

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| BlockError::InvalidTimestamp)?
            .as_secs();

        let block = Block::new(
            prev_hash,
            timestamp,
            0,
            block_height,
            transactions,
            validator_key,
            crate::types::Signature(vec![]),
        )
        .with_private_tx_hashes(private_tx_hashes);

        // Validate the block we just created
        self.validate_block(&block)?;

        Ok(block)
    }

    /// Create a new block and include a state root commitment in the header.
    pub fn create_block_with_state_root(
        &self,
        prev_hash: String,
        block_height: u64,
        transactions: Vec<crate::types::Transaction>,
        validator_key: PublicKey,
        state_root: String,
    ) -> Result<Block, BlockError> {
        let block = self
            .create_block(prev_hash, block_height, transactions, validator_key)?
            .with_state_root(state_root);

        self.validate_block(&block)?;
        Ok(block)
    }

    /// Get list of active validators
    pub async fn get_validators(&self) -> Vec<Validator> {
        let validators = self.validators.read().await;
        validators.values().cloned().collect()
    }
}

impl Default for PoAConsensus {
    fn default() -> Self {
        Self::new()
    }
}

pub async fn start_consensus() -> Result<(), BlockError> {
    println!("Consensus layer (PoA) starting...");
    Ok(())
}

pub fn validate_block(block: &Block) -> Result<(), BlockError> {
    let consensus = PoAConsensus::new();
    consensus.validate_block(block)
}

/// Manages consensus finality and irreversibility guarantees
pub struct FinalityManager {
    /// Finalized block heights (irreversible)
    finalized_heights: Vec<u64>,
    /// Current view in PBFT
    current_view: u64,
    /// Quorum size (2f+1 for f faulty nodes)
    quorum_size: usize,
    /// Total validators
    total_validators: usize,
    /// Checkpoint sequence numbers
    finalized_checkpoints: Vec<u64>,
    /// View change history
    view_changes: Vec<u64>,
}

impl FinalityManager {
    /// Create a new finality manager
    pub fn new(total_validators: usize) -> Self {
        let f = if total_validators > 0 {
            (total_validators - 1) / 3
        } else {
            0
        };
        let quorum_size = 2 * f + 1;

        FinalityManager {
            finalized_heights: vec![0], // Genesis is finalized
            current_view: 0,
            quorum_size,
            total_validators,
            finalized_checkpoints: vec![0],
            view_changes: vec![],
        }
    }

    /// Mark a block as finalized
    pub fn finalize_block(&mut self, height: u64) -> Result<(), BlockError> {
        // Check if height is already finalized
        if self.finalized_heights.contains(&height) {
            return Err(BlockError::InvalidBlockHash); // Already finalized
        }

        // Check if height is sequential (must be > last finalized)
        let last_finalized = self.finalized_heights.last().cloned().unwrap_or(0);
        if height <= last_finalized {
            return Err(BlockError::InvalidBlockHash); // Out of order
        }

        self.finalized_heights.push(height);
        Ok(())
    }

    /// Finalize checkpoint with quorum verification
    pub fn finalize_checkpoint(
        &mut self,
        checkpoint_seq: u64,
        validators_agreeing: usize,
    ) -> Result<(), BlockError> {
        // Verify quorum: need 2f+1 validators
        if validators_agreeing < self.quorum_size {
            return Err(BlockError::TransactionValidationFailed(format!(
                "Checkpoint quorum not reached: {} < {}",
                validators_agreeing, self.quorum_size
            )));
        }

        if !self.finalized_checkpoints.contains(&checkpoint_seq) {
            self.finalized_checkpoints.push(checkpoint_seq);
            self.finalized_checkpoints.sort();
        }

        Ok(())
    }

    /// Handle view change recovery
    pub fn on_view_change(&mut self, new_view: u64) -> Result<(), BlockError> {
        if new_view <= self.current_view {
            return Err(BlockError::TransactionValidationFailed(
                "View number must increase".to_string(),
            ));
        }

        self.current_view = new_view;
        self.view_changes.push(new_view);

        Ok(())
    }

    /// Check if a block is irreversible
    pub fn is_irreversible(&self, height: u64) -> bool {
        self.finalized_heights.contains(&height)
    }

    /// Get the last irreversible block height
    pub fn last_irreversible_height(&self) -> u64 {
        self.finalized_heights.last().cloned().unwrap_or(0)
    }

    /// Get finalized checkpoints
    pub fn finalized_checkpoints(&self) -> &[u64] {
        &self.finalized_checkpoints
    }

    /// Get view change count
    pub fn view_change_count(&self) -> usize {
        self.view_changes.len()
    }

    /// Get current consensus view
    pub fn current_view(&self) -> u64 {
        self.current_view
    }

    /// Get quorum size requirement
    pub fn quorum_size(&self) -> usize {
        self.quorum_size
    }

    /// Check if we can tolerate f failures
    pub fn can_tolerate_failures(&self, failures: usize) -> bool {
        failures <= (self.total_validators - 1) / 3
    }

    /// Get number of validators that can be faulty without losing consensus
    pub fn fault_tolerance_count(&self) -> usize {
        if self.total_validators > 0 {
            (self.total_validators - 1) / 3
        } else {
            0
        }
    }
}

impl Default for FinalityManager {
    fn default() -> Self {
        Self::new(3) // Default: 3 validators (f=0, quorum=1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_block_validation() {
        let consensus = PoAConsensus::new();
        let block = Block::new(
            "0".to_string(),
            1000,
            0,
            1,
            vec![],
            PublicKey(vec![]),
            crate::types::Signature(vec![]),
        );

        let result = consensus.validate_block(&block);
        assert!(result.is_ok());
    }

    #[test]
    fn test_invalid_block_empty_hash() {
        let consensus = PoAConsensus::new();
        let mut block = Block::new(
            "0".to_string(),
            1000,
            0,
            1,
            vec![],
            PublicKey(vec![]),
            crate::types::Signature(vec![]),
        );
        block.header.hash = String::new();

        let result = consensus.validate_block(&block);
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_block_hash_mismatch_after_header_mutation() {
        let consensus = PoAConsensus::new();
        let mut block = Block::new(
            "0".to_string(),
            1000,
            0,
            1,
            vec![],
            PublicKey(vec![]),
            crate::types::Signature(vec![]),
        );

        block.header.state_root = "tampered-root".to_string();

        let result = consensus.validate_block(&block);
        assert!(matches!(result, Err(BlockError::InvalidBlockHash)));
    }

    #[test]
    fn test_create_block_with_state_root() {
        let consensus = PoAConsensus::new();
        let block = consensus
            .create_block_with_state_root(
                "prev".to_string(),
                2,
                vec![],
                PublicKey(vec![1]),
                "abc123".to_string(),
            )
            .expect("must create block with state root");

        assert_eq!(block.header.state_root, "abc123");
        assert!(consensus.validate_block(&block).is_ok());
    }

    #[test]
    fn test_create_block_with_private_tx_hashes_changes_merkle() {
        let consensus = PoAConsensus::new();
        let plain = consensus
            .create_block("prev".to_string(), 1, vec![], PublicKey(vec![1]))
            .expect("plain block");
        let with_private = consensus
            .create_block_with_private(
                "prev".to_string(),
                1,
                vec![],
                vec!["priv-tx-1".to_string(), "priv-tx-2".to_string()],
                PublicKey(vec![1]),
            )
            .expect("block with private hashes");

        assert_eq!(with_private.private_tx_hashes.len(), 2);
        assert_ne!(
            plain.header.merkle_root, with_private.header.merkle_root,
            "private_tx_hashes must flow into the Merkle root"
        );
        assert!(
            consensus.validate_block(&with_private).is_ok(),
            "block carrying private hashes must still validate"
        );
    }

    #[test]
    fn test_validate_block_rejects_duplicate_private_tx_hash() {
        let consensus = PoAConsensus::new();
        let block = consensus.create_block_with_private(
            "prev".to_string(),
            1,
            vec![],
            vec!["dup-hash".to_string(), "dup-hash".to_string()],
            PublicKey(vec![1]),
        );
        assert!(
            matches!(block, Err(BlockError::TransactionValidationFailed(_))),
            "duplicate private_tx_hash must be rejected by validation"
        );
    }

    #[test]
    fn test_validate_block_rejects_empty_private_tx_hash() {
        let consensus = PoAConsensus::new();
        let block = consensus.create_block_with_private(
            "prev".to_string(),
            1,
            vec![],
            vec![String::new()],
            PublicKey(vec![1]),
        );
        assert!(
            matches!(block, Err(BlockError::TransactionValidationFailed(_))),
            "empty private_tx_hash must be rejected"
        );
    }

    #[tokio::test]
    async fn test_add_validator() {
        let consensus = PoAConsensus::new();
        let pub_key = PublicKey(vec![1, 2, 3]);

        consensus.add_validator(pub_key.clone(), 100).await;
        assert!(consensus.is_validator(&pub_key).await);
    }

    // ===== FinalityManager Tests =====

    #[test]
    fn test_finality_manager_creation() {
        let fm = FinalityManager::new(4);
        assert_eq!(fm.current_view(), 0);
        assert_eq!(fm.quorum_size(), 3); // 2f+1 where f=1
        assert_eq!(fm.fault_tolerance_count(), 1);
        assert!(fm.is_irreversible(0)); // Genesis
    }

    #[test]
    fn test_finality_manager_default() {
        let fm = FinalityManager::default();
        assert!(fm.quorum_size() > 0);
        assert!(fm.is_irreversible(0));
    }

    #[test]
    fn test_finalize_block() {
        let mut fm = FinalityManager::new(4);
        let result = fm.finalize_block(1);
        assert!(result.is_ok());
        assert!(fm.is_irreversible(1));
    }

    #[test]
    fn test_finalize_block_sequential() {
        let mut fm = FinalityManager::new(4);
        assert!(fm.finalize_block(1).is_ok());
        assert!(fm.finalize_block(2).is_ok());
        assert!(fm.finalize_block(3).is_ok());
        assert_eq!(fm.last_irreversible_height(), 3);
    }

    #[test]
    fn test_finalize_block_out_of_order_fails() {
        let mut fm = FinalityManager::new(4);
        assert!(fm.finalize_block(1).is_ok());
        // Try to finalize height 0 again (already finalized as genesis)
        let result = fm.finalize_block(0);
        assert!(result.is_err());
    }

    #[test]
    fn test_finalize_block_duplicate_fails() {
        let mut fm = FinalityManager::new(4);
        assert!(fm.finalize_block(1).is_ok());
        // Try to finalize height 1 again
        let result = fm.finalize_block(1);
        assert!(result.is_err());
    }

    #[test]
    fn test_finalize_checkpoint_quorum_met() {
        let mut fm = FinalityManager::new(4);
        // Quorum for 4 validators: 2f+1 = 3
        let result = fm.finalize_checkpoint(100, 3);
        assert!(result.is_ok());
        assert!(fm.finalized_checkpoints().contains(&100));
    }

    #[test]
    fn test_finalize_checkpoint_quorum_not_met() {
        let mut fm = FinalityManager::new(4);
        // Quorum for 4 validators: 2f+1 = 3, but only 2 agree
        let result = fm.finalize_checkpoint(100, 2);
        assert!(result.is_err());
    }

    #[test]
    fn test_finalize_checkpoint_quorum_exact() {
        let mut fm = FinalityManager::new(7);
        // Quorum for 7 validators: 2f+1 = 5
        let result = fm.finalize_checkpoint(50, 5);
        assert!(result.is_ok());
    }

    #[test]
    fn test_view_change() {
        let mut fm = FinalityManager::new(4);
        assert_eq!(fm.current_view(), 0);

        let result = fm.on_view_change(1);
        assert!(result.is_ok());
        assert_eq!(fm.current_view(), 1);
        assert_eq!(fm.view_change_count(), 1);
    }

    #[test]
    fn test_view_change_sequential() {
        let mut fm = FinalityManager::new(4);
        assert!(fm.on_view_change(1).is_ok());
        assert!(fm.on_view_change(2).is_ok());
        assert!(fm.on_view_change(3).is_ok());
        assert_eq!(fm.current_view(), 3);
        assert_eq!(fm.view_change_count(), 3);
    }

    #[test]
    fn test_view_change_backwards_fails() {
        let mut fm = FinalityManager::new(4);
        assert!(fm.on_view_change(1).is_ok());
        let result = fm.on_view_change(0);
        assert!(result.is_err());
    }

    #[test]
    fn test_view_change_same_fails() {
        let mut fm = FinalityManager::new(4);
        assert!(fm.on_view_change(1).is_ok());
        let result = fm.on_view_change(1);
        assert!(result.is_err());
    }

    #[test]
    fn test_is_irreversible() {
        let mut fm = FinalityManager::new(4);
        assert!(fm.is_irreversible(0)); // Genesis
        assert!(!fm.is_irreversible(1));

        fm.finalize_block(1).ok();
        assert!(fm.is_irreversible(1));
    }

    #[test]
    fn test_last_irreversible_height() {
        let mut fm = FinalityManager::new(4);
        assert_eq!(fm.last_irreversible_height(), 0);

        fm.finalize_block(1).ok();
        assert_eq!(fm.last_irreversible_height(), 1);

        fm.finalize_block(2).ok();
        assert_eq!(fm.last_irreversible_height(), 2);
    }

    #[test]
    fn test_finalized_checkpoints() {
        let mut fm = FinalityManager::new(4);
        assert_eq!(fm.finalized_checkpoints().len(), 1); // Genesis checkpoint

        fm.finalize_checkpoint(100, 3).ok();
        assert!(fm.finalized_checkpoints().contains(&100));
    }

    #[test]
    fn test_can_tolerate_failures() {
        let fm = FinalityManager::new(4);
        // 4 validators: f=1
        assert!(fm.can_tolerate_failures(1));
        assert!(!fm.can_tolerate_failures(2));
    }

    #[test]
    fn test_can_tolerate_failures_3_validators() {
        let fm = FinalityManager::new(3);
        // 3 validators: f=0
        assert!(fm.can_tolerate_failures(0));
        assert!(!fm.can_tolerate_failures(1));
    }

    #[test]
    fn test_can_tolerate_failures_7_validators() {
        let fm = FinalityManager::new(7);
        // 7 validators: f=2
        assert!(fm.can_tolerate_failures(2));
        assert!(!fm.can_tolerate_failures(3));
    }

    #[test]
    fn test_quorum_size_calculation() {
        assert_eq!(FinalityManager::new(1).quorum_size(), 1); // 2*0+1=1
        assert_eq!(FinalityManager::new(3).quorum_size(), 1); // 2*0+1=1
        assert_eq!(FinalityManager::new(4).quorum_size(), 3); // 2*1+1=3
        assert_eq!(FinalityManager::new(7).quorum_size(), 5); // 2*2+1=5
        assert_eq!(FinalityManager::new(10).quorum_size(), 7); // 2*3+1=7
    }

    #[test]
    fn test_consensus_finality_flow() {
        let mut fm = FinalityManager::new(4);

        // Simulate block finalization flow
        for height in 1..=5 {
            assert!(fm.finalize_block(height).is_ok());
            assert!(fm.is_irreversible(height));
        }

        assert_eq!(fm.last_irreversible_height(), 5);

        // Simulate checkpoint finalization
        assert!(fm.finalize_checkpoint(100, 3).is_ok());

        // Simulate view change
        assert!(fm.on_view_change(1).is_ok());
        assert_eq!(fm.current_view(), 1);
    }
}
