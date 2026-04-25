use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

// ==================== Error Types ====================

#[derive(Error, Debug, Clone)]
#[allow(dead_code)]
pub enum TransactionError {
    #[error("Invalid signature")]
    InvalidSignature,
    #[error("Invalid nonce: expected {expected}, got {actual}")]
    InvalidNonce { expected: u64, actual: u64 },
    #[error("Negative or zero amount")]
    InvalidAmount,
    #[error("Insufficient balance")]
    InsufficientBalance,
    #[error("Sender account does not exist")]
    AccountNotFound,
    #[error("Transaction already exists")]
    DuplicateTransaction,
    #[error("Mempool is full")]
    MempoolFull,
    #[error("Too many pending transactions for sender {sender}: limit {limit}")]
    TooManyPendingFromSender { sender: String, limit: usize },
    #[error("Insufficient priority fee: minimum {minimum}, got {actual}")]
    InsufficientPriorityFee { minimum: u64, actual: u64 },
    #[error("Serialization error: {0}")]
    SerializationError(String),
}

#[derive(Error, Debug, Clone)]
#[allow(dead_code)]
pub enum BlockError {
    #[error("Invalid block hash")]
    InvalidBlockHash,
    #[error("Invalid parent hash link")]
    InvalidParentHash,
    #[error("Invalid merkle root")]
    InvalidMerkleRoot,
    #[error("Invalid state root")]
    InvalidStateRoot,
    #[error("Invalid commitment")]
    InvalidCommitment,
    #[error("Invalid timestamp")]
    InvalidTimestamp,
    #[error("Transaction validation failed: {0}")]
    TransactionValidationFailed(String),
    #[error("Unauthorized validator")]
    UnauthorizedValidator,
    #[error("Invalid block signature")]
    InvalidBlockSignature,
    #[error("Serialization error: {0}")]
    SerializationError(String),
}

#[derive(Error, Debug, Clone)]
pub enum StateError {
    #[error("Account not found: {0}")]
    AccountNotFound(String),
    #[error("Invalid state transition")]
    InvalidStateTransition,
    #[error("Block application failed: {0}")]
    BlockApplicationFailed(String),
    #[error("Rollback failed")]
    RollbackFailed,
    #[error("Storage error: {0}")]
    StorageError(String),
}

#[derive(Error, Debug, Clone)]
pub enum StorageError {
    #[error("Key not found")]
    KeyNotFound,
    #[error("Database error: {0}")]
    DatabaseError(String),
    #[error("Serialization error: {0}")]
    SerializationError(String),
    #[error("IO error: {0}")]
    IoError(String),
}

// ==================== Cryptographic Types ====================

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct PublicKey(pub Vec<u8>);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PrivateKey(pub Vec<u8>);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Signature(pub Vec<u8>);

// ==================== Identity & Permissions ====================

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Permission {
    CanPropose,
    CanValidate,
    CanVote,
    CanManagePeers,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Identity {
    Validator {
        public_key: PublicKey,
        voting_power: u64,
        permissions: Vec<Permission>,
    },
    User {
        public_key: PublicKey,
        permissions: Vec<Permission>,
    },
}

impl Identity {
    pub fn public_key(&self) -> &PublicKey {
        match self {
            Identity::Validator { public_key, .. } => public_key,
            Identity::User { public_key, .. } => public_key,
        }
    }

    pub fn is_validator(&self) -> bool {
        matches!(self, Identity::Validator { .. })
    }

    pub fn permissions(&self) -> &[Permission] {
        match self {
            Identity::Validator { permissions, .. } => permissions,
            Identity::User { permissions, .. } => permissions,
        }
    }
}

// ==================== Transaction ====================

pub type Address = String;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Transaction {
    pub hash: String,
    pub sender: Address,
    pub receiver: Address,
    pub amount: u64,
    pub nonce: u64,
    pub timestamp: u64,
    pub signature: Signature,
    pub zk_proof: Option<Vec<u8>>,
}

impl Transaction {
    pub fn new(
        sender: Address,
        receiver: Address,
        amount: u64,
        nonce: u64,
        timestamp: u64,
        signature: Signature,
    ) -> Self {
        let mut tx = Transaction {
            hash: String::new(),
            sender,
            receiver,
            amount,
            nonce,
            timestamp,
            signature,
            zk_proof: None,
        };
        tx.hash = tx.compute_hash();
        tx
    }

    pub fn compute_hash(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        let data = format!(
            "{}:{}:{}:{}:{}",
            self.sender, self.receiver, self.amount, self.nonce, self.timestamp
        );
        hasher.update(data.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    pub fn with_zk_proof(mut self, proof: Vec<u8>) -> Self {
        self.zk_proof = Some(proof);
        self
    }
}

// ==================== Block ====================

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlockHeader {
    pub hash: String,
    pub prev_hash: String,
    pub merkle_root: String,
    pub state_root: String,
    pub timestamp: u64,
    pub nonce: u64,
    pub block_height: u64,
    pub validator: PublicKey,
    pub seal: Signature,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Block {
    pub header: BlockHeader,
    pub transactions: Vec<Transaction>,
    pub zk_proof: Option<Vec<u8>>,
}

impl Block {
    pub fn new(
        prev_hash: String,
        timestamp: u64,
        nonce: u64,
        block_height: u64,
        transactions: Vec<Transaction>,
        validator: PublicKey,
        seal: Signature,
    ) -> Self {
        let mut block = Block {
            header: BlockHeader {
                hash: String::new(),
                prev_hash,
                merkle_root: String::new(),
                state_root: String::from("0"),
                timestamp,
                nonce,
                block_height,
                validator,
                seal,
            },
            transactions,
            zk_proof: None,
        };

        block.header.merkle_root = block.compute_merkle_root();
        block.header.hash = block.compute_hash();
        block
    }

    pub fn compute_merkle_root(&self) -> String {
        use sha2::{Digest, Sha256};

        if self.transactions.is_empty() {
            return String::from("0");
        }

        let mut hashes: Vec<Vec<u8>> = self
            .transactions
            .iter()
            .map(|tx| {
                let mut hasher = Sha256::new();
                hasher.update(tx.hash.as_bytes());
                hasher.finalize().to_vec()
            })
            .collect();

        while hashes.len() > 1 {
            let mut next_level = Vec::new();
            for chunk in hashes.chunks(2) {
                let mut hasher = Sha256::new();
                hasher.update(&chunk[0]);
                if chunk.len() > 1 {
                    hasher.update(&chunk[1]);
                } else {
                    hasher.update(&chunk[0]);
                }
                next_level.push(hasher.finalize().to_vec());
            }
            hashes = next_level;
        }

        format!("{:x}", sha2::Sha256::digest(&hashes[0]))
    }

    pub fn compute_hash(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        let data = format!(
            "{}:{}:{}:{}:{}:{}",
            self.header.prev_hash,
            self.header.merkle_root,
            self.header.state_root,
            self.header.timestamp,
            self.header.nonce,
            self.header.block_height
        );
        hasher.update(data.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    pub fn with_state_root(mut self, state_root: String) -> Self {
        self.header.state_root = state_root;
        self.header.hash = self.compute_hash();
        self
    }

    pub fn with_zk_proof(mut self, proof: Vec<u8>) -> Self {
        self.zk_proof = Some(proof);
        self
    }
}

// ==================== Account State ====================

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AccountState {
    pub address: Address,
    pub balance: u64,
    pub nonce: u64,
    pub last_updated_height: u64,
}

impl AccountState {
    pub fn new(address: Address) -> Self {
        AccountState {
            address,
            balance: 0,
            nonce: 0,
            last_updated_height: 0,
        }
    }

    pub fn with_balance(mut self, balance: u64) -> Self {
        self.balance = balance;
        self
    }
}

// ==================== Validator ====================

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Validator {
    pub public_key: PublicKey,
    pub voting_power: u64,
    pub is_active: bool,
}

impl Validator {
    pub fn new(public_key: PublicKey, voting_power: u64) -> Self {
        Validator {
            public_key,
            voting_power,
            is_active: true,
        }
    }
}

// ==================== State & Global State ====================

#[derive(Clone, Debug)]
pub struct State {
    pub accounts: HashMap<Address, AccountState>,
    pub current_height: u64,
    pub current_hash: String,
    pub current_state_root: String,
}

impl State {
    pub fn new() -> Self {
        State {
            accounts: HashMap::new(),
            current_height: 0,
            current_hash: String::from("0"),
            current_state_root: String::from("0"),
        }
    }

    pub fn get_account(&self, address: &Address) -> Option<&AccountState> {
        self.accounts.get(address)
    }

    pub fn get_account_mut(&mut self, address: &Address) -> Option<&mut AccountState> {
        self.accounts.get_mut(address)
    }

    pub fn create_account(&mut self, address: Address) -> Result<(), StateError> {
        if self.accounts.contains_key(&address) {
            return Err(StateError::InvalidStateTransition);
        }
        self.accounts
            .insert(address.clone(), AccountState::new(address));
        Ok(())
    }
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

// ==================== Traits ====================

pub trait Hashable {
    fn compute_hash(&self) -> String;
}

impl Hashable for Transaction {
    fn compute_hash(&self) -> String {
        Transaction::compute_hash(self)
    }
}

impl Hashable for Block {
    fn compute_hash(&self) -> String {
        Block::compute_hash(self)
    }
}

// ==================== Network Types ====================

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PeerInfo {
    pub peer_id: String,
    pub address: String,
    pub port: u16,
    pub connected_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeInfo {
    pub version: String,
    pub block_height: u64,
    pub block_hash: String,
    pub peer_count: usize,
    pub pending_transactions: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Metrics {
    pub block_height: u64,
    pub pending_transactions: u64,
    pub connected_peers: u64,
    pub valid_transactions: u64,
    pub rejected_transactions: u64,
}

// ==================== API Types ====================

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TransactionRequest {
    pub sender: Address,
    pub receiver: Address,
    pub amount: u64,
    pub nonce: u64,
    pub priority_fee: Option<u64>,
    pub signature_hex: Option<String>,
    pub public_key_hex: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TransactionResponse {
    pub hash: String,
    pub status: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlockResponse {
    pub height: u64,
    pub hash: String,
    pub prev_hash: String,
    pub timestamp: u64,
    pub transactions_count: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccountResponse {
    pub address: Address,
    pub balance: u64,
    pub nonce: u64,
}
