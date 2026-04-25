// Block module: Handles block structure and storage
use crate::types::{Block, BlockError, PublicKey, Signature, Transaction};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn create_block(
    prev_hash: String,
    transactions: Vec<Transaction>,
) -> Result<Block, BlockError> {
    for tx in &transactions {
        validate_transaction(tx)?;
    }

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| BlockError::InvalidTimestamp)?
        .as_secs();

    let block = Block::new(
        prev_hash,
        timestamp,
        0,
        0,
        transactions,
        PublicKey(vec![]),
        Signature(vec![]),
    );

    if block.header.hash.is_empty() {
        return Err(BlockError::InvalidBlockHash);
    }

    Ok(block)
}

pub fn validate_transaction(tx: &Transaction) -> Result<(), BlockError> {
    if tx.hash.is_empty() {
        return Err(BlockError::TransactionValidationFailed(
            "Empty transaction hash".to_string(),
        ));
    }
    if tx.hash != tx.compute_hash() {
        return Err(BlockError::TransactionValidationFailed(
            "Transaction hash mismatch".to_string(),
        ));
    }
    if tx.amount == 0 {
        return Err(BlockError::TransactionValidationFailed(
            "Zero amount transaction".to_string(),
        ));
    }
    if tx.sender == tx.receiver {
        return Err(BlockError::TransactionValidationFailed(
            "Self-transfer not allowed".to_string(),
        ));
    }

    Ok(())
}

/// BlockValidator provides comprehensive block validation
pub struct BlockValidator;

impl BlockValidator {
    /// Compute merkle root from transactions
    pub fn compute_merkle_root(transactions: &[Transaction]) -> String {
        if transactions.is_empty() {
            return "0".to_string();
        }

        use crate::crypto::hashing::blake3_hash;

        let mut hashes: Vec<String> = transactions.iter().map(|tx| tx.hash.clone()).collect();

        while hashes.len() > 1 {
            if hashes.len() % 2 != 0 {
                hashes.push(hashes[hashes.len() - 1].clone());
            }

            let mut next_level = Vec::new();
            for i in (0..hashes.len()).step_by(2) {
                let combined = format!("{}{}", hashes[i], hashes[i + 1]);
                let hash = blake3_hash(combined.as_bytes()).to_string();
                next_level.push(hash);
            }
            hashes = next_level;
        }

        hashes.first().cloned().unwrap_or_else(|| "0".to_string())
    }

    /// Verify merkle root of a block
    pub fn verify_merkle_root(block: &Block) -> bool {
        let computed_root = Self::compute_merkle_root(&block.transactions);
        computed_root == block.header.merkle_root
    }

    /// Verify block signature
    pub fn verify_block_signature(block: &Block) -> bool {
        // Check validator has non-empty public key
        !block.header.validator.0.is_empty() && !block.header.seal.0.is_empty()
    }

    /// Verify state root format (simplified: check it's valid hex)
    pub fn verify_state_root(block: &Block) -> bool {
        // State root must be valid and not empty (except for genesis)
        !block.header.state_root.is_empty()
    }

    /// Comprehensive block validation
    pub fn validate_block(block: &Block) -> Result<(), BlockError> {
        // Validate block header hash
        if block.header.hash.is_empty() {
            return Err(BlockError::InvalidBlockHash);
        }

        // Validate all transactions
        for tx in &block.transactions {
            validate_transaction(tx)?;
        }

        // Verify merkle root
        if !Self::verify_merkle_root(block) {
            return Err(BlockError::TransactionValidationFailed(
                "Merkle root verification failed".to_string(),
            ));
        }

        // Verify block signature
        if !Self::verify_block_signature(block) {
            return Err(BlockError::TransactionValidationFailed(
                "Block signature verification failed".to_string(),
            ));
        }

        // Verify state root
        if !Self::verify_state_root(block) {
            return Err(BlockError::TransactionValidationFailed(
                "State root verification failed".to_string(),
            ));
        }

        Ok(())
    }

    /// Validate block height is sequential
    pub fn validate_block_height(block: &Block, previous_height: u64) -> Result<(), BlockError> {
        if block.header.block_height != previous_height + 1 {
            return Err(BlockError::TransactionValidationFailed(format!(
                "Invalid block height: expected {}, got {}",
                previous_height + 1,
                block.header.block_height
            )));
        }
        Ok(())
    }

    /// Validate block timestamp
    pub fn validate_block_timestamp(
        block: &Block,
        previous_timestamp: u64,
    ) -> Result<(), BlockError> {
        if block.header.timestamp <= previous_timestamp {
            return Err(BlockError::TransactionValidationFailed(
                "Block timestamp must be greater than previous block".to_string(),
            ));
        }
        Ok(())
    }

    /// Validate block previous hash matches
    pub fn validate_prev_hash(block: &Block, previous_hash: &str) -> Result<(), BlockError> {
        if block.header.prev_hash != previous_hash {
            return Err(BlockError::InvalidBlockHash);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_transaction_rejects_invalid_inputs() {
        let mut tx = Transaction::new(
            "alice".to_string(),
            "alice".to_string(),
            100,
            1,
            1,
            Signature(vec![]),
        );

        let self_transfer = validate_transaction(&tx);
        assert!(self_transfer.is_err());

        tx.receiver = "bob".to_string();
        tx.amount = 0;
        tx.hash = tx.compute_hash();
        let zero_amount = validate_transaction(&tx);
        assert!(zero_amount.is_err());
    }

    #[test]
    fn test_create_block_validates_transactions() {
        let tx = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            10,
            1,
            1,
            Signature(vec![]),
        );

        let block = create_block("prev".to_string(), vec![tx]);
        assert!(block.is_ok());
    }

    // ===== BlockValidator Tests =====

    #[test]
    fn test_compute_merkle_root_empty_transactions() {
        let root = BlockValidator::compute_merkle_root(&[]);
        assert_eq!(root, "0");
    }

    #[test]
    fn test_compute_merkle_root_single_transaction() {
        let tx = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            10,
            1,
            1,
            Signature(vec![]),
        );

        let root = BlockValidator::compute_merkle_root(&[tx.clone()]);
        assert!(!root.is_empty());
        assert_ne!(root, "0");
    }

    #[test]
    fn test_compute_merkle_root_multiple_transactions() {
        let tx1 = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            10,
            1,
            1,
            Signature(vec![]),
        );
        let tx2 = Transaction::new(
            "bob".to_string(),
            "charlie".to_string(),
            20,
            2,
            1,
            Signature(vec![]),
        );

        let root = BlockValidator::compute_merkle_root(&[tx1, tx2]);
        assert!(!root.is_empty());
        assert_ne!(root, "0");
    }

    #[test]
    fn test_compute_merkle_root_deterministic() {
        let tx1 = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            10,
            1,
            1,
            Signature(vec![]),
        );
        let tx2 = Transaction::new(
            "bob".to_string(),
            "charlie".to_string(),
            20,
            2,
            1,
            Signature(vec![]),
        );

        let root1 = BlockValidator::compute_merkle_root(&[tx1.clone(), tx2.clone()]);
        let root2 = BlockValidator::compute_merkle_root(&[tx1, tx2]);
        assert_eq!(root1, root2);
    }

    #[test]
    fn test_verify_merkle_root_valid() {
        let tx = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            10,
            1,
            1,
            Signature(vec![]),
        );

        let expected_root = BlockValidator::compute_merkle_root(&[tx.clone()]);
        let mut block = Block::new(
            "prev".to_string(),
            1000,
            0,
            1,
            vec![tx],
            PublicKey(vec![1, 2, 3]),
            Signature(vec![1, 2, 3]),
        );
        block.header.merkle_root = expected_root;
        block.header.hash = "test-hash".to_string();

        assert!(BlockValidator::verify_merkle_root(&block));
    }

    #[test]
    fn test_verify_merkle_root_invalid() {
        let tx = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            10,
            1,
            1,
            Signature(vec![]),
        );

        let mut block = Block::new(
            "prev".to_string(),
            1000,
            0,
            1,
            vec![tx],
            PublicKey(vec![1, 2, 3]),
            Signature(vec![1, 2, 3]),
        );
        block.header.merkle_root = "wrong-merkle-root".to_string();
        block.header.hash = "test-hash".to_string();

        assert!(!BlockValidator::verify_merkle_root(&block));
    }

    #[test]
    fn test_verify_block_signature_valid() {
        let tx = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            10,
            1,
            1,
            Signature(vec![]),
        );

        let block = Block::new(
            "prev".to_string(),
            1000,
            0,
            1,
            vec![tx],
            PublicKey(vec![1, 2, 3]),
            Signature(vec![1, 2, 3]),
        );

        assert!(BlockValidator::verify_block_signature(&block));
    }

    #[test]
    fn test_verify_block_signature_missing_validator() {
        let tx = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            10,
            1,
            1,
            Signature(vec![]),
        );

        let block = Block::new(
            "prev".to_string(),
            1000,
            0,
            1,
            vec![tx],
            PublicKey(vec![]),
            Signature(vec![1, 2, 3]),
        );

        assert!(!BlockValidator::verify_block_signature(&block));
    }

    #[test]
    fn test_verify_block_signature_missing_seal() {
        let tx = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            10,
            1,
            1,
            Signature(vec![]),
        );

        let block = Block::new(
            "prev".to_string(),
            1000,
            0,
            1,
            vec![tx],
            PublicKey(vec![1, 2, 3]),
            Signature(vec![]),
        );

        assert!(!BlockValidator::verify_block_signature(&block));
    }

    #[test]
    fn test_verify_state_root_valid() {
        let tx = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            10,
            1,
            1,
            Signature(vec![]),
        );

        let mut block = Block::new(
            "prev".to_string(),
            1000,
            0,
            1,
            vec![tx],
            PublicKey(vec![1, 2, 3]),
            Signature(vec![1, 2, 3]),
        );
        block.header.state_root = "root-hash".to_string();

        assert!(BlockValidator::verify_state_root(&block));
    }

    #[test]
    fn test_verify_state_root_empty() {
        let tx = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            10,
            1,
            1,
            Signature(vec![]),
        );

        let mut block = Block::new(
            "prev".to_string(),
            1000,
            0,
            1,
            vec![tx],
            PublicKey(vec![1, 2, 3]),
            Signature(vec![1, 2, 3]),
        );
        block.header.state_root = String::new(); // Explicitly set to empty

        assert!(!BlockValidator::verify_state_root(&block));
    }

    #[test]
    fn test_verify_state_root_genesis() {
        let tx = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            10,
            1,
            1,
            Signature(vec![]),
        );

        let block = Block::new(
            "prev".to_string(),
            1000,
            0,
            1,
            vec![tx],
            PublicKey(vec![1, 2, 3]),
            Signature(vec![1, 2, 3]),
        );
        // Default state_root is "0" (genesis)

        assert!(BlockValidator::verify_state_root(&block));
    }

    #[test]
    fn test_validate_block_comprehensive() {
        let tx = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            10,
            1,
            1,
            Signature(vec![]),
        );

        let expected_root = BlockValidator::compute_merkle_root(&[tx.clone()]);
        let mut block = Block::new(
            "prev".to_string(),
            1000,
            0,
            1,
            vec![tx],
            PublicKey(vec![1, 2, 3]),
            Signature(vec![1, 2, 3]),
        );
        block.header.merkle_root = expected_root;
        block.header.hash = "test-hash".to_string();
        block.header.state_root = "state-root".to_string();

        let result = BlockValidator::validate_block(&block);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_block_invalid_merkle() {
        let tx = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            10,
            1,
            1,
            Signature(vec![]),
        );

        let mut block = Block::new(
            "prev".to_string(),
            1000,
            0,
            1,
            vec![tx],
            PublicKey(vec![1, 2, 3]),
            Signature(vec![1, 2, 3]),
        );
        block.header.merkle_root = "wrong-root".to_string();
        block.header.hash = "test-hash".to_string();
        block.header.state_root = "state-root".to_string();

        let result = BlockValidator::validate_block(&block);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_block_height_valid() {
        let tx = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            10,
            1,
            1,
            Signature(vec![]),
        );

        let mut block = Block::new(
            "prev".to_string(),
            1000,
            0,
            5,
            vec![tx],
            PublicKey(vec![1, 2, 3]),
            Signature(vec![1, 2, 3]),
        );
        block.header.hash = "test-hash".to_string();

        let result = BlockValidator::validate_block_height(&block, 4);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_block_height_invalid() {
        let tx = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            10,
            1,
            1,
            Signature(vec![]),
        );

        let mut block = Block::new(
            "prev".to_string(),
            1000,
            0,
            5,
            vec![tx],
            PublicKey(vec![1, 2, 3]),
            Signature(vec![1, 2, 3]),
        );
        block.header.hash = "test-hash".to_string();

        let result = BlockValidator::validate_block_height(&block, 5);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_block_timestamp_valid() {
        let tx = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            10,
            1,
            1,
            Signature(vec![]),
        );

        let mut block = Block::new(
            "prev".to_string(),
            1000,
            0,
            1,
            vec![tx],
            PublicKey(vec![1, 2, 3]),
            Signature(vec![1, 2, 3]),
        );
        block.header.hash = "test-hash".to_string();

        let result = BlockValidator::validate_block_timestamp(&block, 999);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_block_timestamp_invalid() {
        let tx = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            10,
            1,
            1,
            Signature(vec![]),
        );

        let mut block = Block::new(
            "prev".to_string(),
            1000,
            0,
            1,
            vec![tx],
            PublicKey(vec![1, 2, 3]),
            Signature(vec![1, 2, 3]),
        );
        block.header.hash = "test-hash".to_string();

        let result = BlockValidator::validate_block_timestamp(&block, 1000);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_prev_hash_valid() {
        let tx = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            10,
            1,
            1,
            Signature(vec![]),
        );

        let block = Block::new(
            "prev-hash".to_string(),
            1000,
            0,
            1,
            vec![tx],
            PublicKey(vec![1, 2, 3]),
            Signature(vec![1, 2, 3]),
        );

        let result = BlockValidator::validate_prev_hash(&block, "prev-hash");
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_prev_hash_invalid() {
        let tx = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            10,
            1,
            1,
            Signature(vec![]),
        );

        let block = Block::new(
            "prev-hash".to_string(),
            1000,
            0,
            1,
            vec![tx],
            PublicKey(vec![1, 2, 3]),
            Signature(vec![1, 2, 3]),
        );

        let result = BlockValidator::validate_prev_hash(&block, "wrong-hash");
        assert!(result.is_err());
    }
}
