// User-facing mining system
use crate::types::{Block, BlockError, PublicKey, Signature, Transaction};
use std::sync::{Arc, Mutex};

pub mod block_validator;
pub mod difficulty;
pub mod pow;

/// Mining configuration and rewards
#[allow(dead_code)]
pub struct MiningConfig {
    pub block_reward: u64,
    pub mining_difficulty: u32,
    pub block_time_target_secs: u64,
    pub max_transactions_per_block: usize,
}

impl Default for MiningConfig {
    fn default() -> Self {
        MiningConfig {
            block_reward: 10,           // 10 coins per block
            mining_difficulty: 4,       // Leading zeros required
            block_time_target_secs: 60, // Target 1 block per minute
            max_transactions_per_block: 1000,
        }
    }
}

/// Miner for users to produce blocks
#[allow(dead_code)]
pub struct UserMiner {
    config: MiningConfig,
    mined_blocks: Arc<Mutex<u64>>,
    total_rewards: Arc<Mutex<u64>>,
    is_mining: Arc<Mutex<bool>>,
    current_block_number: Arc<Mutex<u64>>,
}

impl UserMiner {
    /// Create new miner with default config
    pub fn new() -> Self {
        UserMiner {
            config: MiningConfig::default(),
            mined_blocks: Arc::new(Mutex::new(0)),
            total_rewards: Arc::new(Mutex::new(0)),
            is_mining: Arc::new(Mutex::new(false)),
            current_block_number: Arc::new(Mutex::new(0)),
        }
    }

    /// Create miner with custom config
    pub fn with_config(config: MiningConfig) -> Self {
        UserMiner {
            config,
            mined_blocks: Arc::new(Mutex::new(0)),
            total_rewards: Arc::new(Mutex::new(0)),
            is_mining: Arc::new(Mutex::new(false)),
            current_block_number: Arc::new(Mutex::new(0)),
        }
    }

    /// Mine a new block with given transactions
    pub fn mine_block(
        &self,
        miner_pubkey: PublicKey,
        transactions: Vec<Transaction>,
        previous_hash: &str,
    ) -> Result<(Block, u64), BlockError> {
        *self.is_mining.lock().unwrap() = true;

        // Limit transactions
        let txs: Vec<Transaction> = transactions
            .into_iter()
            .take(self.config.max_transactions_per_block)
            .collect();

        // Create block
        let block_height = {
            if let Ok(mut num) = self.current_block_number.lock() {
                let h = *num;
                *num += 1;
                h
            } else {
                0
            }
        };

        let timestamp = Self::current_timestamp();
        let mut nonce = 0u64;

        // Proof of Work: find nonce
        let mut block = Block {
            header: crate::types::BlockHeader {
                hash: "0".to_string(),
                prev_hash: previous_hash.to_string(),
                merkle_root: "0".to_string(),
                state_root: "0".to_string(),
                timestamp,
                nonce,
                block_height,
                validator: miner_pubkey.clone(),
                seal: Signature(vec![]),
            },
            transactions: txs.clone(),
            private_tx_hashes: Vec::new(),
            private_fee_claims: Vec::new(),
            private_coinbase: None,
        };

        loop {
            let block_str = format!("{:?}{}", block, nonce);
            let hash = Self::calculate_hash(&block_str);

            if self.is_hash_valid(&hash) {
                block.header.nonce = nonce;
                block.header.hash = hash;
                block.header.seal = Signature(format!("mined:{}", nonce).into_bytes());
                break;
            }

            nonce += 1;
            if nonce % 1000 == 0 {
                if let Ok(mining) = self.is_mining.lock() {
                    if !*mining {
                        return Err(BlockError::TransactionValidationFailed(
                            "Mining cancelled".to_string(),
                        ));
                    }
                }
            }
        }

        // Record mining reward
        let reward = self.config.block_reward;
        if let Ok(mut rewards) = self.total_rewards.lock() {
            *rewards += reward;
        }
        if let Ok(mut blocks) = self.mined_blocks.lock() {
            *blocks += 1;
        }

        *self.is_mining.lock().unwrap() = false;

        Ok((block, reward))
    }

    /// Start mining (in real apps, this would be async)
    pub fn start_mining(&self) -> Result<(), BlockError> {
        *self.is_mining.lock().unwrap() = true;
        Ok(())
    }

    /// Stop mining
    pub fn stop_mining(&self) -> Result<(), BlockError> {
        *self.is_mining.lock().unwrap() = false;
        Ok(())
    }

    /// Get mining stats
    pub fn get_stats(&self) -> (u64, u64) {
        let blocks = self.mined_blocks.lock().unwrap().clone();
        let rewards = self.total_rewards.lock().unwrap().clone();
        (blocks, rewards)
    }

    /// Get if currently mining
    pub fn is_mining(&self) -> bool {
        *self.is_mining.lock().unwrap()
    }

    /// Set difficulty
    pub fn set_difficulty(&mut self, difficulty: u32) {
        self.config.mining_difficulty = difficulty;
    }

    /// Set block reward
    pub fn set_block_reward(&mut self, reward: u64) {
        self.config.block_reward = reward;
    }

    // Helper: Calculate merkle root from transactions
    #[allow(dead_code)]
    fn calculate_merkle_root(_transactions: &[Transaction]) -> String {
        "0".to_string() // Simplified for users
    }

    // Helper: Calculate block hash
    fn calculate_hash(block_str: &str) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(block_str.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    // Helper: Check if hash meets difficulty requirement
    fn is_hash_valid(&self, hash: &str) -> bool {
        let required_zeros = self.config.mining_difficulty as usize;
        hash.starts_with(&"0".repeat(required_zeros))
    }

    // Helper: Current timestamp
    fn current_timestamp() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }
}

impl Default for UserMiner {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for UserMiner {
    fn clone(&self) -> Self {
        UserMiner {
            config: self.config.clone(),
            mined_blocks: Arc::clone(&self.mined_blocks),
            total_rewards: Arc::clone(&self.total_rewards),
            is_mining: Arc::clone(&self.is_mining),
            current_block_number: Arc::clone(&self.current_block_number),
        }
    }
}

impl Clone for MiningConfig {
    fn clone(&self) -> Self {
        MiningConfig {
            block_reward: self.block_reward,
            mining_difficulty: self.mining_difficulty,
            block_time_target_secs: self.block_time_target_secs,
            max_transactions_per_block: self.max_transactions_per_block,
        }
    }
}
