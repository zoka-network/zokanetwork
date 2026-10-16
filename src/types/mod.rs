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

/// Public transaction on the ledger. Private transactions live in
/// `crate::private_wallet::PrivateTransaction` and never cross this type —
/// there used to be a `zk_proof: Option<Vec<u8>>` field here together with
/// API-layer redaction logic for "private-looking" rows, but no code path
/// ever actually set it to `Some(...)`, so the redaction was a placebo.
/// Removing the field enforces the rule at the type level: anything on a
/// `Transaction` is, by construction, public. If you want privacy, you must
/// build a `PrivateTransaction`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Transaction {
    pub hash: String,
    pub sender: Address,
    pub receiver: Address,
    pub amount: u64,
    pub nonce: u64,
    pub timestamp: u64,
    pub signature: Signature,
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
        };
        tx.hash = tx.compute_hash();
        tx
    }

    pub fn compute_hash(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        // Timestamp is excluded so clients can pre-compute and sign the hash
        // before submission (timestamp is server-assigned on receipt).
        let data = format!(
            "{}:{}:{}:{}",
            self.sender, self.receiver, self.amount, self.nonce
        );
        hasher.update(data.as_bytes());
        format!("{:x}", hasher.finalize())
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

/// Encrypted opening that lets the block producer claim a private tx fee
/// without revealing the fee amount to observers. The referenced private
/// transaction hash must be committed in the same block.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PrivateFeeClaim {
    pub tx_hash: String,
    pub ephemeral_pubkey: Vec<u8>,
    pub ciphertext: Vec<u8>,
}

/// Privacy-native block reward output committed inside the block body.
///
/// Mainnet v1 keeps the subsidy amount public in the canonical coinbase
/// transaction so emission remains independently auditable. This output is
/// the spendable private note metadata the winning miner later imports after
/// coinbase maturity.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PrivateCoinbaseOutput {
    pub block_height: u64,
    pub recipient_address: Address,
    pub unlock_height: u64,
    pub output_id: String,
    pub asset_tag: String,
    pub asset_tag_commitment: String,
    pub recipient_commitment: String,
    pub stealth_address: String,
    pub ephemeral_pubkey: Vec<u8>,
    pub commitment: String,
    pub amount_commitment: String,
    pub range_proof: Vec<u8>,
    pub one_time_public_key: String,
    pub encrypted_amount: Vec<u8>,
    pub commitment_blind: Vec<u8>,
}

/// Block body on the ledger.
///
/// Public transactions live in `transactions`. Historical private
/// transactions are referenced by hash only. A future height-gated format
/// also carries their serialized bodies as bytes, keeping this type free of
/// a cyclic dependency on the private wallet module.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Block {
    pub header: BlockHeader,
    pub transactions: Vec<Transaction>,
    /// Hashes of private transactions committed at this height. Defaults
    /// to empty for backwards compatibility with pre-split blocks.
    #[serde(default)]
    pub private_tx_hashes: Vec<String>,
    /// Canonical serialized private transaction bodies. This field remains
    /// empty until the reviewed activation height is fixed in consensus.
    #[serde(default)]
    pub private_tx_bodies: Vec<Vec<u8>>,
    /// Spent nullifiers (primary + input nullifiers) of every private tx
    /// committed in this block, sorted and de-duplicated. Committed in the
    /// Merkle root so every node — even one that never received the tx
    /// bodies — can enforce the no-double-spend consensus rule from the
    /// block alone. Defaults to empty for backwards compatibility.
    #[serde(default)]
    pub private_nullifiers: Vec<String>,
    /// Encrypted fee-opening envelopes for private txs included in this
    /// block. These are committed by Merkle root but hide the fee amount.
    #[serde(default)]
    pub private_fee_claims: Vec<PrivateFeeClaim>,
    /// Spendable private output for the block subsidy. Present whenever
    /// the block subsidy is non-zero.
    #[serde(default)]
    pub private_coinbase: Option<PrivateCoinbaseOutput>,
    /// Zoka Quantic transactions, coinbase first, each in the binary format of
    /// `crates/zoka-quantic` (hex in the JSON). Always empty on Zenith, and then
    /// omitted, so Zenith blocks keep their exact JSON, Merkle root and hash.
    #[serde(default, skip_serializing_if = "Vec::is_empty", with = "hex_byte_vecs")]
    pub quantic_transactions: Vec<Vec<u8>>,
}

/// `Vec<Vec<u8>>` as a list of hex strings: a Quantic transaction is about
/// 11 KB, and a JSON array of numbers would make it four times larger.
mod hex_byte_vecs {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(items: &[Vec<u8>], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(items.iter().map(hex::encode))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<Vec<u8>>, D::Error> {
        let items = Vec::<String>::deserialize(deserializer)?;
        items
            .iter()
            .map(|item| hex::decode(item).map_err(serde::de::Error::custom))
            .collect()
    }
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
            private_tx_hashes: Vec::new(),
            private_tx_bodies: Vec::new(),
            private_nullifiers: Vec::new(),
            private_fee_claims: Vec::new(),
            private_coinbase: None,
            quantic_transactions: Vec::new(),
        };

        block.header.merkle_root = block.compute_merkle_root();
        block.header.hash = block.compute_hash();
        block
    }

    /// Attach a set of private-transaction hashes. Recomputes the Merkle
    /// root and block hash, so call this *before* signing the block.
    pub fn with_private_tx_hashes(mut self, private_tx_hashes: Vec<String>) -> Self {
        self.private_tx_hashes = private_tx_hashes;
        self.header.merkle_root = self.compute_merkle_root();
        self.header.hash = self.compute_hash();
        self
    }

    /// Attach canonical private transaction bodies for the future
    /// height-gated format. Recomputes the Merkle root and block hash.
    pub fn with_private_tx_bodies(mut self, private_tx_bodies: Vec<Vec<u8>>) -> Self {
        self.private_tx_bodies = private_tx_bodies;
        self.header.merkle_root = self.compute_merkle_root();
        self.header.hash = self.compute_hash();
        self
    }

    /// Attach encrypted private-fee claims. Recomputes the Merkle root and
    /// block hash, so call this before finalizing PoW/signature.
    pub fn with_private_fee_claims(mut self, private_fee_claims: Vec<PrivateFeeClaim>) -> Self {
        self.private_fee_claims = private_fee_claims;
        self.header.merkle_root = self.compute_merkle_root();
        self.header.hash = self.compute_hash();
        self
    }

    /// Attach the private spendable reward output. Recomputes the Merkle
    /// root and block hash, so call this before finalizing PoW/signature.
    pub fn with_private_coinbase(mut self, private_coinbase: PrivateCoinbaseOutput) -> Self {
        self.private_coinbase = Some(private_coinbase);
        self.header.merkle_root = self.compute_merkle_root();
        self.header.hash = self.compute_hash();
        self
    }

    /// Attach the spent-nullifier set for the private txs in this block.
    /// The list is sorted and de-duplicated so the commitment is canonical.
    /// Recomputes the Merkle root and block hash, so call this before
    /// finalizing PoW/signature.
    pub fn with_private_nullifiers(mut self, private_nullifiers: Vec<String>) -> Self {
        let mut nullifiers = private_nullifiers;
        nullifiers.sort();
        nullifiers.dedup();
        self.private_nullifiers = nullifiers;
        self.header.merkle_root = self.compute_merkle_root();
        self.header.hash = self.compute_hash();
        self
    }

    pub fn compute_merkle_root(&self) -> String {
        use sha2::{Digest, Sha256};

        if self.transactions.is_empty()
            && self.private_tx_hashes.is_empty()
            && self.private_tx_bodies.is_empty()
            && self.private_nullifiers.is_empty()
            && self.private_fee_claims.is_empty()
            && self.private_coinbase.is_none()
            && self.quantic_transactions.is_empty()
        {
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
            .chain(self.private_tx_hashes.iter().map(|hash| {
                // Prefix private tx hashes so they can't collide with a
                // public-tx hash whose string value happens to match.
                let mut hasher = Sha256::new();
                hasher.update(b"private:");
                hasher.update(hash.as_bytes());
                hasher.finalize().to_vec()
            }))
            .chain(self.private_tx_bodies.iter().map(|body| {
                let mut hasher = Sha256::new();
                hasher.update(b"private-body-v1:");
                hasher.update(body);
                hasher.finalize().to_vec()
            }))
            .chain(self.private_nullifiers.iter().map(|nullifier| {
                // Commit each spent nullifier so the no-double-spend rule is
                // anchored by PoW and verifiable from the block alone.
                let mut hasher = Sha256::new();
                hasher.update(b"private-nullifier:");
                hasher.update(nullifier.as_bytes());
                hasher.finalize().to_vec()
            }))
            .chain(self.private_fee_claims.iter().map(|claim| {
                let mut hasher = Sha256::new();
                hasher.update(b"private-fee-claim:");
                hasher.update(claim.tx_hash.as_bytes());
                hasher.update([0x1f]);
                hasher.update(&claim.ephemeral_pubkey);
                hasher.update([0x1f]);
                hasher.update(&claim.ciphertext);
                hasher.finalize().to_vec()
            }))
            .collect();
        if let Some(coinbase) = &self.private_coinbase {
            let mut hasher = Sha256::new();
            hasher.update(b"private-coinbase-v1:");
            hasher.update(coinbase.block_height.to_le_bytes());
            hasher.update([0x1f]);
            hasher.update(coinbase.recipient_address.as_bytes());
            hasher.update([0x1f]);
            hasher.update(coinbase.unlock_height.to_le_bytes());
            hasher.update([0x1f]);
            hasher.update(coinbase.output_id.as_bytes());
            hasher.update([0x1f]);
            hasher.update(coinbase.asset_tag.as_bytes());
            hasher.update([0x1f]);
            hasher.update(coinbase.asset_tag_commitment.as_bytes());
            hasher.update([0x1f]);
            hasher.update(coinbase.recipient_commitment.as_bytes());
            hasher.update([0x1f]);
            hasher.update(coinbase.stealth_address.as_bytes());
            hasher.update([0x1f]);
            hasher.update(&coinbase.ephemeral_pubkey);
            hasher.update([0x1f]);
            hasher.update(coinbase.commitment.as_bytes());
            hasher.update([0x1f]);
            hasher.update(coinbase.amount_commitment.as_bytes());
            hasher.update([0x1f]);
            hasher.update(&coinbase.range_proof);
            hasher.update([0x1f]);
            hasher.update(coinbase.one_time_public_key.as_bytes());
            hasher.update([0x1f]);
            hasher.update(&coinbase.encrypted_amount);
            hasher.update([0x1f]);
            hasher.update(&coinbase.commitment_blind);
            hashes.push(hasher.finalize().to_vec());
        }
        // Zoka Quantic. Absent on Zenith, so its roots do not change.
        for tx in &self.quantic_transactions {
            let mut hasher = Sha256::new();
            hasher.update(b"quantic-tx-v1:");
            hasher.update(tx);
            hashes.push(hasher.finalize().to_vec());
        }

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
    pub private_commitments: Vec<String>,
    pub private_commitment_root: String,
    pub current_height: u64,
    pub current_hash: String,
    pub current_state_root: String,
}

impl State {
    pub fn new() -> Self {
        State {
            accounts: HashMap::new(),
            private_commitments: Vec::new(),
            private_commitment_root: String::from("0"),
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

#[cfg(test)]
mod block_body_split_tests {
    use super::*;

    fn dummy_validator() -> PublicKey {
        PublicKey(vec![1u8; 32])
    }

    fn dummy_seal() -> Signature {
        Signature(vec![2u8; 64])
    }

    #[test]
    fn private_tx_hashes_default_to_empty() {
        let block = Block::new(
            "prev".to_string(),
            0,
            0,
            0,
            vec![],
            dummy_validator(),
            dummy_seal(),
        );
        assert!(block.private_tx_hashes.is_empty());
        assert!(block.private_tx_bodies.is_empty());
    }

    #[test]
    fn private_tx_hashes_change_merkle_root() {
        let empty = Block::new(
            "prev".to_string(),
            0,
            0,
            0,
            vec![],
            dummy_validator(),
            dummy_seal(),
        );
        let with_private = empty
            .clone()
            .with_private_tx_hashes(vec!["private-tx-1".to_string()]);
        assert_ne!(
            empty.header.merkle_root, with_private.header.merkle_root,
            "adding a private tx hash must change the Merkle root"
        );
        assert_ne!(
            empty.header.hash, with_private.header.hash,
            "adding a private tx hash must change the block hash"
        );
    }

    #[test]
    fn canonical_private_tx_bodies_change_merkle_root() {
        let empty = Block::new(
            "prev".to_string(),
            0,
            0,
            0,
            vec![],
            dummy_validator(),
            dummy_seal(),
        );
        let with_body = empty
            .clone()
            .with_private_tx_bodies(vec![b"private-body".to_vec()]);

        assert_ne!(empty.header.merkle_root, with_body.header.merkle_root);
        assert_ne!(empty.header.hash, with_body.header.hash);
    }

    #[test]
    fn public_and_private_hash_namespaces_do_not_collide() {
        // A public tx whose hash string happens to match a private tx hash
        // string must still produce different Merkle roots — otherwise a
        // malicious peer could substitute one for the other unnoticed.
        let shared = "collision-candidate";
        let public_tx =
            Transaction::new("a".to_string(), "b".to_string(), 1, 1, 0, Signature(vec![]));
        let mut block_public = Block::new(
            "prev".to_string(),
            0,
            0,
            0,
            vec![public_tx],
            dummy_validator(),
            dummy_seal(),
        );
        // Force the public tx hash to `shared` so it would equal the
        // private tx hash string below.
        block_public.transactions[0].hash = shared.to_string();
        block_public.header.merkle_root = block_public.compute_merkle_root();

        let block_private = Block::new(
            "prev".to_string(),
            0,
            0,
            0,
            vec![],
            dummy_validator(),
            dummy_seal(),
        )
        .with_private_tx_hashes(vec![shared.to_string()]);

        assert_ne!(
            block_public.header.merkle_root, block_private.header.merkle_root,
            "namespace prefix must prevent same-hex string collision"
        );
    }
}
