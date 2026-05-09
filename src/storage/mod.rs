use crate::types::{AccountState, Block, State, StorageError, Transaction};
use rocksdb::{IteratorMode, Options, WriteBatch, DB};
use std::path::Path;
use std::sync::Arc;

const PENDING_PRIVATE_TX_PREFIX: &str = "mempool:private:pending:";
const PENDING_PRIVATE_TX_COUNT_KEY: &str = "mempool:private:count";

/// Storage trait defining the interface for blockchain persistence
pub trait StorageBackend: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError>;
    fn put(&self, key: &str, value: Vec<u8>) -> Result<(), StorageError>;
    fn delete(&self, key: &str) -> Result<(), StorageError>;
    fn exists(&self, key: &str) -> Result<bool, StorageError>;
}

/// RocksDB-based implementation of storage backend
pub struct RocksDbStorage {
    db: Arc<DB>,
}

impl RocksDbStorage {
    /// Initialize RocksDB storage at the specified path
    pub fn new(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let mut opts = Options::default();
        opts.create_if_missing(true);

        let db = DB::open(&opts, path).map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        Ok(RocksDbStorage { db: Arc::new(db) })
    }

    /// Get raw bytes from storage
    pub fn get_raw(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
        self.db
            .get(key.as_bytes())
            .map_err(|e| StorageError::DatabaseError(e.to_string()))
    }

    /// Put raw bytes to storage
    pub fn put_raw(&self, key: &str, value: Vec<u8>) -> Result<(), StorageError> {
        self.db
            .put(key.as_bytes(), &value)
            .map_err(|e| StorageError::DatabaseError(e.to_string()))
    }

    /// Save a block to storage
    pub fn save_block(&self, block: &Block) -> Result<(), StorageError> {
        self.save_block_body(block)?;
        self.set_block_hash_at_height(block.header.block_height, &block.header.hash)?;
        Ok(())
    }

    /// Save only the block body by hash. This is used for side branches:
    /// a block can be known without becoming canonical at its height.
    pub fn save_block_body(&self, block: &Block) -> Result<(), StorageError> {
        let key = format!("block:{}", block.header.hash);
        let value = serde_json::to_vec(block)
            .map_err(|e| StorageError::SerializationError(e.to_string()))?;
        self.put_raw(&key, value)
    }

    /// Set the canonical block hash for a height.
    pub fn set_block_hash_at_height(&self, height: u64, hash: &str) -> Result<(), StorageError> {
        let height_key = format!("chain:block_height:{}", height);
        self.put_raw(&height_key, hash.as_bytes().to_vec())
    }

    /// Return the canonical block hash for a height, if the direct
    /// height index exists. This deliberately does not scan block bodies:
    /// side-branch bodies may exist at the same height but are not
    /// canonical unless the height index points at them.
    pub fn get_canonical_block_hash_at_height(
        &self,
        height: u64,
    ) -> Result<Option<String>, StorageError> {
        let height_key = format!("chain:block_height:{}", height);
        let Some(hash_raw) = self.get_raw(&height_key)? else {
            return Ok(None);
        };
        let hash = String::from_utf8(hash_raw)
            .map_err(|_| StorageError::SerializationError("Invalid UTF-8".to_string()))?;
        Ok(Some(hash))
    }

    /// Retrieve the canonical block body for a height using only the
    /// direct height index.
    pub fn get_canonical_block_by_height(
        &self,
        height: u64,
    ) -> Result<Option<Block>, StorageError> {
        let Some(hash) = self.get_canonical_block_hash_at_height(height)? else {
            return Ok(None);
        };
        self.get_block(&hash)
    }

    /// Remove the canonical height index entry. Used when a reorg selects
    /// a shorter but heavier chain.
    pub fn delete_block_hash_at_height(&self, height: u64) -> Result<(), StorageError> {
        let height_key = format!("chain:block_height:{}", height);
        self.db
            .delete(height_key.as_bytes())
            .map_err(|e| StorageError::DatabaseError(e.to_string()))
    }

    /// Delete a block body by hash. Used by the pruner when a block is
    /// older than the retention window. Caller is responsible for
    /// removing height-index and per-block metadata too — see
    /// `state::chain::prune_below_head`.
    pub fn delete_block_body(&self, hash: &str) -> Result<(), StorageError> {
        let key = format!("block:{}", hash);
        self.db
            .delete(key.as_bytes())
            .map_err(|e| StorageError::DatabaseError(e.to_string()))
    }

    /// Delete an arbitrary key from storage. Used by the pruner to
    /// remove `chain:block_meta:<hash>` entries.
    pub fn delete_raw(&self, key: &str) -> Result<(), StorageError> {
        self.db
            .delete(key.as_bytes())
            .map_err(|e| StorageError::DatabaseError(e.to_string()))
    }

    /// Persist block, chain metadata, and all account states atomically.
    pub fn persist_block_and_state(
        &self,
        block: &Block,
        state: &State,
    ) -> Result<(), StorageError> {
        let mut batch = WriteBatch::default();

        let block_key = format!("block:{}", block.header.hash);
        let block_value = serde_json::to_vec(block)
            .map_err(|e| StorageError::SerializationError(e.to_string()))?;
        batch.put(block_key.as_bytes(), block_value);

        let height_key = format!("chain:block_height:{}", block.header.block_height);
        batch.put(height_key.as_bytes(), block.header.hash.as_bytes());

        batch.put(
            "chain:height".as_bytes(),
            state.current_height.to_string().as_bytes(),
        );
        batch.put("chain:last_hash".as_bytes(), state.current_hash.as_bytes());
        batch.put(
            "chain:state_root".as_bytes(),
            state.current_state_root.as_bytes(),
        );
        batch.put(
            "chain:private_commitment_root".as_bytes(),
            state.private_commitment_root.as_bytes(),
        );
        let private_commitments = serde_json::to_vec(&state.private_commitments)
            .map_err(|e| StorageError::SerializationError(e.to_string()))?;
        batch.put("chain:private_commitments".as_bytes(), private_commitments);

        for (address, account) in &state.accounts {
            let key = format!("state:{}", address);
            let value = serde_json::to_vec(account)
                .map_err(|e| StorageError::SerializationError(e.to_string()))?;
            batch.put(key.as_bytes(), value);
        }

        self.db
            .write(batch)
            .map_err(|e| StorageError::DatabaseError(e.to_string()))
    }

    /// Retrieve a block by hash
    pub fn get_block(&self, hash: &str) -> Result<Option<Block>, StorageError> {
        let key = format!("block:{}", hash);
        match self.get_raw(&key)? {
            Some(data) => {
                let block = serde_json::from_slice(&data)
                    .map_err(|e| StorageError::SerializationError(e.to_string()))?;
                Ok(Some(block))
            }
            None => Ok(None),
        }
    }

    /// Retrieve a block by height.
    ///
    /// Uses the direct height index when present, and falls back to block scan
    /// for compatibility with data persisted before index support.
    pub fn get_block_by_height(&self, height: u64) -> Result<Option<Block>, StorageError> {
        if let Some(hash) = self.get_canonical_block_hash_at_height(height)? {
            return self.get_block(&hash);
        }

        let blocks = self.iterate_blocks()?;
        Ok(blocks
            .into_iter()
            .find(|block| block.header.block_height == height))
    }

    /// Save a transaction to storage
    pub fn save_transaction(&self, tx: &Transaction) -> Result<(), StorageError> {
        let key = format!("tx:{}", tx.hash);
        let value =
            serde_json::to_vec(tx).map_err(|e| StorageError::SerializationError(e.to_string()))?;
        self.put_raw(&key, value)
    }

    // ===== Private transaction bodies =====
    //
    // Private transactions are held in-memory by `PrivateTransactionPool`.
    // Persisting their bodies here lets a node survive restarts without
    // losing the pool, and lets the `/private/tx/:hash` endpoint succeed
    // after a cold boot. The storage layer stays generic: we take and
    // return raw bytes, and the caller (private_wallet) handles
    // serialization. This avoids a cyclic `storage -> private_wallet`
    // dependency.
    //
    // Keys live under the `priv_tx:` prefix so they share the RocksDB
    // instance with public data but cannot be confused with it during
    // key scans.

    /// Persist the serialized body of a private transaction.
    pub fn save_private_transaction_bytes(
        &self,
        hash: &str,
        body: &[u8],
    ) -> Result<(), StorageError> {
        let key = format!("priv_tx:{}", hash);
        self.put_raw(&key, body.to_vec())
    }

    /// Retrieve a private transaction body, if any.
    pub fn get_private_transaction_bytes(
        &self,
        hash: &str,
    ) -> Result<Option<Vec<u8>>, StorageError> {
        let key = format!("priv_tx:{}", hash);
        self.get_raw(&key)
    }

    /// Remove a private transaction body from storage (used by retention
    /// sweeps and rollback).
    pub fn delete_private_transaction(&self, hash: &str) -> Result<(), StorageError> {
        let key = format!("priv_tx:{}", hash);
        self.db
            .delete(key.as_bytes())
            .map_err(|e| StorageError::DatabaseError(e.to_string()))
    }

    /// Iterate over every persisted private transaction body, yielding
    /// `(hash, body_bytes)` pairs. Useful for warm-loading the pool at
    /// node startup.
    pub fn iterate_private_transactions(&self) -> Result<Vec<(String, Vec<u8>)>, StorageError> {
        let prefix = b"priv_tx:";
        let mut out = Vec::new();
        for item in self
            .db
            .iterator(IteratorMode::From(prefix, rocksdb::Direction::Forward))
        {
            let (key, value) = item.map_err(|e| StorageError::DatabaseError(e.to_string()))?;
            if !key.starts_with(prefix) {
                break;
            }
            let hash = String::from_utf8(key[prefix.len()..].to_vec()).map_err(|_| {
                StorageError::SerializationError("Invalid UTF-8 in priv_tx key".to_string())
            })?;
            out.push((hash, value.to_vec()));
        }
        Ok(out)
    }

    /// Mark a private transaction body as pending for local miner inclusion.
    ///
    /// The body itself remains under `priv_tx:<hash>` so APIs can still serve
    /// it after block inclusion; this marker is only the local mempool index.
    pub fn save_pending_private_transaction_hash(&self, hash: &str) -> Result<(), StorageError> {
        let hash = hash.trim();
        if hash.is_empty() {
            return Err(StorageError::SerializationError(
                "pending private tx hash cannot be empty".to_string(),
            ));
        }
        let key = format!("{PENDING_PRIVATE_TX_PREFIX}{hash}");
        self.put_raw(&key, b"1".to_vec())?;
        self.refresh_pending_private_transaction_count()
    }

    /// Remove a private transaction from the local pending index after it is
    /// committed to a block. The persisted body is intentionally preserved.
    pub fn delete_pending_private_transaction_hash(&self, hash: &str) -> Result<(), StorageError> {
        let key = format!("{PENDING_PRIVATE_TX_PREFIX}{}", hash.trim());
        self.db
            .delete(key.as_bytes())
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        self.refresh_pending_private_transaction_count()
    }

    /// Return pending private tx hashes in deterministic key order.
    pub fn iterate_pending_private_transaction_hashes(&self) -> Result<Vec<String>, StorageError> {
        let prefix = PENDING_PRIVATE_TX_PREFIX.as_bytes();
        let mut out = Vec::new();
        for item in self
            .db
            .iterator(IteratorMode::From(prefix, rocksdb::Direction::Forward))
        {
            let (key, _value) = item.map_err(|e| StorageError::DatabaseError(e.to_string()))?;
            if !key.starts_with(prefix) {
                break;
            }
            let hash = String::from_utf8(key[prefix.len()..].to_vec()).map_err(|_| {
                StorageError::SerializationError(
                    "Invalid UTF-8 in pending private tx key".to_string(),
                )
            })?;
            out.push(hash);
        }
        Ok(out)
    }

    /// Return the cached pending private tx count, repairing it from the
    /// marker index when older DBs do not have the count key yet.
    pub fn pending_private_transaction_count(&self) -> Result<u64, StorageError> {
        if let Some(raw) = self.get_raw(PENDING_PRIVATE_TX_COUNT_KEY)? {
            if let Ok(text) = std::str::from_utf8(&raw) {
                if let Ok(value) = text.trim().parse::<u64>() {
                    return Ok(value);
                }
            }
        }
        let count = self.iterate_pending_private_transaction_hashes()?.len() as u64;
        self.put_raw(PENDING_PRIVATE_TX_COUNT_KEY, count.to_string().into_bytes())?;
        Ok(count)
    }

    fn refresh_pending_private_transaction_count(&self) -> Result<(), StorageError> {
        let count = self.iterate_pending_private_transaction_hashes()?.len() as u64;
        self.put_raw(PENDING_PRIVATE_TX_COUNT_KEY, count.to_string().into_bytes())
    }

    /// Retrieve a transaction by hash
    pub fn get_transaction(&self, hash: &str) -> Result<Option<Transaction>, StorageError> {
        let key = format!("tx:{}", hash);
        match self.get_raw(&key)? {
            Some(data) => {
                let tx = serde_json::from_slice(&data)
                    .map_err(|e| StorageError::SerializationError(e.to_string()))?;
                Ok(Some(tx))
            }
            None => Ok(None),
        }
    }

    /// Save account state to storage
    pub fn save_account_state(
        &self,
        address: &str,
        state: &AccountState,
    ) -> Result<(), StorageError> {
        let key = format!("state:{}", address);
        let value = serde_json::to_vec(state)
            .map_err(|e| StorageError::SerializationError(e.to_string()))?;
        self.put_raw(&key, value)
    }

    /// Retrieve account state by address
    pub fn get_account_state(&self, address: &str) -> Result<Option<AccountState>, StorageError> {
        let key = format!("state:{}", address);
        match self.get_raw(&key)? {
            Some(data) => {
                let state = serde_json::from_slice(&data)
                    .map_err(|e| StorageError::SerializationError(e.to_string()))?;
                Ok(Some(state))
            }
            None => Ok(None),
        }
    }

    /// Get the current blockchain height
    pub fn get_chain_height(&self) -> Result<u64, StorageError> {
        match self.get_raw("chain:height")? {
            Some(data) => {
                let height = String::from_utf8(data)
                    .ok()
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(0);
                Ok(height)
            }
            None => Ok(0),
        }
    }

    /// Set the current blockchain height
    pub fn set_chain_height(&self, height: u64) -> Result<(), StorageError> {
        self.put_raw("chain:height", height.to_string().into_bytes())
    }

    /// Get the latest persisted state root
    pub fn get_state_root(&self) -> Result<String, StorageError> {
        match self.get_raw("chain:state_root")? {
            Some(data) => {
                let root = String::from_utf8(data)
                    .map_err(|_| StorageError::SerializationError("Invalid UTF-8".to_string()))?;
                Ok(root)
            }
            None => Ok("0".to_string()),
        }
    }

    /// Set the latest persisted state root
    pub fn set_state_root(&self, root: &str) -> Result<(), StorageError> {
        self.put_raw("chain:state_root", root.as_bytes().to_vec())
    }

    /// Get the latest persisted private commitment root
    pub fn get_private_commitment_root(&self) -> Result<String, StorageError> {
        match self.get_raw("chain:private_commitment_root")? {
            Some(data) => {
                let root = String::from_utf8(data)
                    .map_err(|_| StorageError::SerializationError("Invalid UTF-8".to_string()))?;
                Ok(root)
            }
            None => Ok("0".to_string()),
        }
    }

    /// Set the latest private commitment root
    pub fn set_private_commitment_root(&self, root: &str) -> Result<(), StorageError> {
        self.put_raw("chain:private_commitment_root", root.as_bytes().to_vec())
    }

    /// Get persisted private commitments list
    pub fn get_private_commitments(&self) -> Result<Vec<String>, StorageError> {
        match self.get_raw("chain:private_commitments")? {
            Some(data) => serde_json::from_slice(&data)
                .map_err(|e| StorageError::SerializationError(e.to_string())),
            None => Ok(Vec::new()),
        }
    }

    /// Persist private commitments list
    pub fn set_private_commitments(&self, commitments: &[String]) -> Result<(), StorageError> {
        let serialized = serde_json::to_vec(commitments)
            .map_err(|e| StorageError::SerializationError(e.to_string()))?;
        self.put_raw("chain:private_commitments", serialized)
    }

    /// Get the last block hash
    pub fn get_last_block_hash(&self) -> Result<String, StorageError> {
        match self.get_raw("chain:last_hash")? {
            Some(data) => {
                let hash = String::from_utf8(data)
                    .map_err(|_| StorageError::SerializationError("Invalid UTF-8".to_string()))?;
                Ok(hash)
            }
            None => Ok("0".to_string()),
        }
    }

    /// Set the last block hash
    pub fn set_last_block_hash(&self, hash: &str) -> Result<(), StorageError> {
        self.put_raw("chain:last_hash", hash.as_bytes().to_vec())
    }

    /// Cumulative ZKA emission across the canonical chain (sum of all
    /// coinbase amounts up to and including the chain tip). Defaults to 0
    /// when the key is absent so a fresh DB looks like genesis.
    pub fn get_total_supply_emitted(&self) -> Result<u64, StorageError> {
        match self.get_raw("chain:total_supply_emitted")? {
            Some(data) => Ok(String::from_utf8(data)
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0)),
            None => Ok(0),
        }
    }

    pub fn set_total_supply_emitted(&self, total: u64) -> Result<(), StorageError> {
        self.put_raw("chain:total_supply_emitted", total.to_string().into_bytes())
    }

    /// Cumulative PoW work across the canonical chain, measured as the sum
    /// of leading-zero-bits per block hash. Used by fork-choice to pick
    /// the heaviest chain. Defaults to 0 on a fresh DB.
    pub fn get_cumulative_work(&self) -> Result<u128, StorageError> {
        match self.get_raw("chain:cumulative_work")? {
            Some(data) => Ok(String::from_utf8(data)
                .ok()
                .and_then(|s| s.parse::<u128>().ok())
                .unwrap_or(0)),
            None => Ok(0),
        }
    }

    pub fn set_cumulative_work(&self, work: u128) -> Result<(), StorageError> {
        self.put_raw("chain:cumulative_work", work.to_string().into_bytes())
    }

    /// Difficulty bits used for the most recent block on the canonical
    /// chain. Defaults to 0 (= "no chain yet"); callers must treat 0 as
    /// "use the canonical genesis difficulty from `economics`".
    pub fn get_current_difficulty_bits(&self) -> Result<u32, StorageError> {
        match self.get_raw("chain:current_difficulty_bits")? {
            Some(data) => Ok(String::from_utf8(data)
                .ok()
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(0)),
            None => Ok(0),
        }
    }

    pub fn set_current_difficulty_bits(&self, bits: u32) -> Result<(), StorageError> {
        self.put_raw(
            "chain:current_difficulty_bits",
            bits.to_string().into_bytes(),
        )
    }

    /// Check if a block exists
    pub fn block_exists(&self, hash: &str) -> Result<bool, StorageError> {
        let key = format!("block:{}", hash);
        self.get_raw(&key).map(|v| v.is_some())
    }

    /// Iterate over all blocks
    pub fn iterate_blocks(&self) -> Result<Vec<Block>, StorageError> {
        let mut blocks = Vec::new();
        let iter = self
            .db
            .iterator(IteratorMode::From(b"block:", rocksdb::Direction::Forward));

        for result in iter {
            match result {
                Ok((key, value)) => {
                    let key_str = String::from_utf8_lossy(&key);
                    if !key_str.starts_with("block:") {
                        break;
                    }

                    if let Ok(block) = serde_json::from_slice::<Block>(&value) {
                        blocks.push(block);
                    }
                }
                Err(_) => continue,
            }
        }

        Ok(blocks)
    }

    /// Iterate over all persisted account states.
    pub fn iterate_account_states(&self) -> Result<Vec<AccountState>, StorageError> {
        let mut accounts = Vec::new();
        let iter = self
            .db
            .iterator(IteratorMode::From(b"state:", rocksdb::Direction::Forward));

        for result in iter {
            match result {
                Ok((key, value)) => {
                    let key_str = String::from_utf8_lossy(&key);
                    if !key_str.starts_with("state:") {
                        break;
                    }
                    let account: AccountState = serde_json::from_slice(&value)
                        .map_err(|e| StorageError::SerializationError(e.to_string()))?;
                    accounts.push(account);
                }
                Err(err) => {
                    return Err(StorageError::DatabaseError(err.to_string()));
                }
            }
        }

        Ok(accounts)
    }

    /// Delete a transaction (after block confirmation)
    pub fn delete_transaction(&self, hash: &str) -> Result<(), StorageError> {
        let key = format!("tx:{}", hash);
        self.db
            .delete(key.as_bytes())
            .map_err(|e| StorageError::DatabaseError(e.to_string()))
    }

    /// Clear all data (for testing only)
    pub fn clear_all(&self) -> Result<(), StorageError> {
        let opts = Options::default();

        // Get the path from the database
        let path = self.db.path();
        drop(self.db.clone());

        // Destroy and recreate
        DB::destroy(&opts, path).map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        Ok(())
    }
}

impl StorageBackend for RocksDbStorage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
        self.get_raw(key)
    }

    fn put(&self, key: &str, value: Vec<u8>) -> Result<(), StorageError> {
        self.put_raw(key, value)
    }

    fn delete(&self, key: &str) -> Result<(), StorageError> {
        self.db
            .delete(key.as_bytes())
            .map_err(|e| StorageError::DatabaseError(e.to_string()))
    }

    fn exists(&self, key: &str) -> Result<bool, StorageError> {
        self.get(key).map(|v| v.is_some())
    }
}

/// Encryption configuration for at-rest encryption
#[derive(Debug, Clone)]
pub struct EncryptionAtRestConfig {
    pub enabled: bool,
    pub master_key: Vec<u8>,
    pub algorithm: EncryptionAlgorithm,
}

#[derive(Debug, Clone, Copy)]
pub enum EncryptionAlgorithm {
    Xor,    // Simple XOR (for demo)
    Aes256, // AES-256 (production)
}

impl EncryptionAtRestConfig {
    pub fn new(master_key: Vec<u8>) -> Self {
        EncryptionAtRestConfig {
            enabled: true,
            master_key,
            algorithm: EncryptionAlgorithm::Xor,
        }
    }

    pub fn aes256(master_key: Vec<u8>) -> Self {
        EncryptionAtRestConfig {
            enabled: true,
            master_key,
            algorithm: EncryptionAlgorithm::Aes256,
        }
    }

    pub fn disabled() -> Self {
        EncryptionAtRestConfig {
            enabled: false,
            master_key: vec![],
            algorithm: EncryptionAlgorithm::Xor,
        }
    }
}

/// Encryption utility for at-rest encryption
pub struct EncryptionAtRest {
    config: EncryptionAtRestConfig,
}

impl EncryptionAtRest {
    pub fn new(config: EncryptionAtRestConfig) -> Self {
        EncryptionAtRest { config }
    }

    pub fn encrypt(&self, data: &[u8]) -> Result<Vec<u8>, StorageError> {
        if !self.config.enabled {
            return Ok(data.to_vec());
        }

        match self.config.algorithm {
            EncryptionAlgorithm::Xor => self.encrypt_xor(data),
            EncryptionAlgorithm::Aes256 => {
                // Placeholder for AES implementation
                self.encrypt_xor(data)
            }
        }
    }

    pub fn decrypt(&self, data: &[u8]) -> Result<Vec<u8>, StorageError> {
        if !self.config.enabled {
            return Ok(data.to_vec());
        }

        match self.config.algorithm {
            EncryptionAlgorithm::Xor => self.decrypt_xor(data),
            EncryptionAlgorithm::Aes256 => {
                // Placeholder for AES implementation
                self.decrypt_xor(data)
            }
        }
    }

    fn encrypt_xor(&self, data: &[u8]) -> Result<Vec<u8>, StorageError> {
        if self.config.master_key.is_empty() {
            return Ok(data.to_vec());
        }

        Ok(data
            .iter()
            .enumerate()
            .map(|(i, &byte)| byte ^ self.config.master_key[i % self.config.master_key.len()])
            .collect())
    }

    fn decrypt_xor(&self, data: &[u8]) -> Result<Vec<u8>, StorageError> {
        // XOR is symmetric
        self.encrypt_xor(data)
    }

    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }
}

/// Encrypted storage backend wrapper
pub struct EncryptedStorage {
    backend: Arc<dyn StorageBackend>,
    encryption: EncryptionAtRest,
}

impl EncryptedStorage {
    pub fn new(backend: Arc<dyn StorageBackend>, config: EncryptionAtRestConfig) -> Self {
        EncryptedStorage {
            backend,
            encryption: EncryptionAtRest::new(config),
        }
    }

    pub fn with_rocksdb(
        path: impl AsRef<Path>,
        config: EncryptionAtRestConfig,
    ) -> Result<Self, StorageError> {
        let storage = RocksDbStorage::new(path)?;
        Ok(EncryptedStorage::new(Arc::new(storage), config))
    }

    pub fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
        match self.backend.get(key)? {
            Some(encrypted_data) => {
                let decrypted = self.encryption.decrypt(&encrypted_data)?;
                Ok(Some(decrypted))
            }
            None => Ok(None),
        }
    }

    pub fn put(&self, key: &str, value: Vec<u8>) -> Result<(), StorageError> {
        let encrypted_data = self.encryption.encrypt(&value)?;
        self.backend.put(key, encrypted_data)
    }

    pub fn delete(&self, key: &str) -> Result<(), StorageError> {
        self.backend.delete(key)
    }

    pub fn exists(&self, key: &str) -> Result<bool, StorageError> {
        self.backend.exists(key)
    }

    pub fn get_encrypted_raw(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
        self.backend.get(key)
    }
}

impl StorageBackend for EncryptedStorage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
        EncryptedStorage::get(self, key)
    }

    fn put(&self, key: &str, value: Vec<u8>) -> Result<(), StorageError> {
        EncryptedStorage::put(self, key, value)
    }

    fn delete(&self, key: &str) -> Result<(), StorageError> {
        EncryptedStorage::delete(self, key)
    }

    fn exists(&self, key: &str) -> Result<bool, StorageError> {
        EncryptedStorage::exists(self, key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{PublicKey, Signature};
    use std::fs;

    #[test]
    fn test_storage_basic_operations() {
        let test_dir = ".zoka/test-suite/.test_storage";
        let _ = fs::remove_dir_all(test_dir);

        let storage = RocksDbStorage::new(test_dir).expect("Failed to create storage");

        // Test put and get
        storage.put("test_key", b"test_value".to_vec()).unwrap();
        let value = storage.get("test_key").unwrap();
        assert_eq!(value, Some(b"test_value".to_vec()));

        // Test exists
        assert!(storage.exists("test_key").unwrap());
        assert!(!storage.exists("nonexistent").unwrap());

        // Test delete
        storage.delete("test_key").unwrap();
        assert!(!storage.exists("test_key").unwrap());

        // Test state root metadata
        storage.set_state_root("root-1").unwrap();
        assert_eq!(storage.get_state_root().unwrap(), "root-1");

        let _ = fs::remove_dir_all(test_dir);
    }

    #[test]
    fn test_storage_private_commitment_metadata() {
        let test_dir = ".zoka/test-suite/.test_storage_private_commitment";
        let _ = fs::remove_dir_all(test_dir);

        let storage = RocksDbStorage::new(test_dir).expect("Failed to create storage");
        let commitments = vec!["c1".to_string(), "c2".to_string()];

        storage.set_private_commitments(&commitments).unwrap();
        storage
            .set_private_commitment_root("private-root-1")
            .unwrap();

        assert_eq!(storage.get_private_commitments().unwrap(), commitments);
        assert_eq!(
            storage.get_private_commitment_root().unwrap(),
            "private-root-1".to_string()
        );

        let _ = fs::remove_dir_all(test_dir);
    }

    #[test]
    fn test_storage_private_tx_bodies_roundtrip() {
        let test_dir = ".zoka/test-suite/.test_storage_priv_tx";
        let _ = fs::remove_dir_all(test_dir);

        let storage = RocksDbStorage::new(test_dir).expect("storage");
        // Two opaque bodies — storage treats them as byte blobs.
        storage
            .save_private_transaction_bytes("hash_alpha", b"serialized-tx-alpha")
            .unwrap();
        storage
            .save_private_transaction_bytes("hash_beta", b"serialized-tx-beta")
            .unwrap();

        assert_eq!(
            storage
                .get_private_transaction_bytes("hash_alpha")
                .unwrap()
                .as_deref(),
            Some(b"serialized-tx-alpha".as_slice())
        );
        assert_eq!(
            storage
                .get_private_transaction_bytes("hash_beta")
                .unwrap()
                .as_deref(),
            Some(b"serialized-tx-beta".as_slice())
        );
        assert!(storage
            .get_private_transaction_bytes("missing")
            .unwrap()
            .is_none());

        let mut all = storage.iterate_private_transactions().unwrap();
        all.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].0, "hash_alpha");
        assert_eq!(all[1].0, "hash_beta");

        // Deletion leaves the other body intact and does not leak into
        // the public `tx:` namespace.
        storage.delete_private_transaction("hash_alpha").unwrap();
        assert!(storage
            .get_private_transaction_bytes("hash_alpha")
            .unwrap()
            .is_none());
        assert!(storage
            .get_private_transaction_bytes("hash_beta")
            .unwrap()
            .is_some());
        assert!(
            storage.get_transaction("hash_beta").unwrap().is_none(),
            "private body must NOT be reachable via the public tx: namespace"
        );

        let _ = fs::remove_dir_all(test_dir);
    }

    #[test]
    fn test_storage_pending_private_tx_index_roundtrip() {
        let test_dir = ".zoka/test-suite/.test_storage_pending_priv_tx";
        let _ = fs::remove_dir_all(test_dir);

        let storage = RocksDbStorage::new(test_dir).expect("storage");
        assert_eq!(storage.pending_private_transaction_count().unwrap(), 0);

        storage
            .save_private_transaction_bytes("hash_alpha", b"serialized-tx-alpha")
            .unwrap();
        storage
            .save_private_transaction_bytes("hash_beta", b"serialized-tx-beta")
            .unwrap();
        storage
            .save_pending_private_transaction_hash("hash_beta")
            .unwrap();
        storage
            .save_pending_private_transaction_hash("hash_alpha")
            .unwrap();
        storage
            .save_pending_private_transaction_hash("hash_alpha")
            .unwrap();

        let pending = storage
            .iterate_pending_private_transaction_hashes()
            .unwrap();
        assert_eq!(
            pending,
            vec!["hash_alpha".to_string(), "hash_beta".to_string()]
        );
        assert_eq!(storage.pending_private_transaction_count().unwrap(), 2);

        storage
            .delete_pending_private_transaction_hash("hash_alpha")
            .unwrap();
        assert_eq!(
            storage
                .iterate_pending_private_transaction_hashes()
                .unwrap(),
            vec!["hash_beta".to_string()]
        );
        assert_eq!(storage.pending_private_transaction_count().unwrap(), 1);
        assert!(
            storage
                .get_private_transaction_bytes("hash_alpha")
                .unwrap()
                .is_some(),
            "committed private tx bodies remain available after pending marker deletion"
        );

        let _ = fs::remove_dir_all(test_dir);
    }

    #[test]
    fn test_storage_account_state() {
        let test_dir = ".zoka/test-suite/.test_storage_account";
        let _ = fs::remove_dir_all(test_dir);

        let storage = RocksDbStorage::new(test_dir).expect("Failed to create storage");

        let account = AccountState {
            address: "alice".to_string(),
            balance: 1000,
            nonce: 0,
            last_updated_height: 0,
        };

        storage.save_account_state("alice", &account).unwrap();
        let retrieved = storage.get_account_state("alice").unwrap();
        assert_eq!(retrieved, Some(account));

        let all_accounts = storage.iterate_account_states().unwrap();
        assert_eq!(all_accounts.len(), 1);
        assert_eq!(all_accounts[0].address, "alice");

        let _ = fs::remove_dir_all(test_dir);
    }

    #[test]
    fn test_storage_get_block_by_height() {
        let test_dir = ".zoka/test-suite/.test_storage_block_index";
        let _ = fs::remove_dir_all(test_dir);

        let storage = RocksDbStorage::new(test_dir).expect("Failed to create storage");
        let block = Block::new(
            "prev-hash".to_string(),
            1234,
            0,
            7,
            vec![],
            PublicKey(vec![1, 2, 3]),
            Signature(vec![]),
        );

        storage.save_block(&block).unwrap();

        let by_height = storage
            .get_block_by_height(7)
            .unwrap()
            .expect("block by height");
        assert_eq!(by_height.header.hash, block.header.hash);

        let _ = fs::remove_dir_all(test_dir);
    }

    #[test]
    fn test_storage_get_block_by_height_fallback_scan() {
        let test_dir = ".zoka/test-suite/.test_storage_block_fallback";
        let _ = fs::remove_dir_all(test_dir);

        let storage = RocksDbStorage::new(test_dir).expect("Failed to create storage");
        let legacy_block = Block::new(
            "legacy-prev".to_string(),
            55,
            0,
            12,
            vec![],
            PublicKey(vec![9]),
            Signature(vec![]),
        );

        let block_key = format!("block:{}", legacy_block.header.hash);
        let encoded = serde_json::to_vec(&legacy_block).unwrap();
        storage.put_raw(&block_key, encoded).unwrap();

        let by_height = storage
            .get_block_by_height(12)
            .unwrap()
            .expect("fallback block by height");
        assert_eq!(by_height.header.hash, legacy_block.header.hash);

        let _ = fs::remove_dir_all(test_dir);
    }

    #[test]
    fn test_storage_canonical_block_by_height_ignores_body_only_side_blocks() {
        let test_dir = ".zoka/test-suite/.test_storage_canonical_block_index";
        let _ = fs::remove_dir_all(test_dir);

        let storage = RocksDbStorage::new(test_dir).expect("Failed to create storage");
        let side_block = Block::new(
            "side-prev".to_string(),
            88,
            0,
            14,
            vec![],
            PublicKey(vec![4]),
            Signature(vec![]),
        );

        storage.save_block_body(&side_block).unwrap();
        assert!(storage.get_canonical_block_by_height(14).unwrap().is_none());
        assert!(storage.get_block_by_height(14).unwrap().is_some());

        storage
            .set_block_hash_at_height(14, &side_block.header.hash)
            .unwrap();
        let canonical = storage
            .get_canonical_block_by_height(14)
            .unwrap()
            .expect("canonical block");
        assert_eq!(canonical.header.hash, side_block.header.hash);

        let _ = fs::remove_dir_all(test_dir);
    }

    #[test]
    fn test_encryption_at_rest_xor() {
        let key = vec![1, 2, 3, 4, 5];
        let config = EncryptionAtRestConfig::new(key);
        let encryption = EncryptionAtRest::new(config);

        let plaintext = b"sensitive data";
        let encrypted = encryption.encrypt(plaintext).unwrap();

        assert_ne!(encrypted, plaintext);

        let decrypted = encryption.decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_encryption_at_rest_disabled() {
        let config = EncryptionAtRestConfig::disabled();
        let encryption = EncryptionAtRest::new(config);

        let plaintext = b"sensitive data";
        let encrypted = encryption.encrypt(plaintext).unwrap();

        // When disabled, should return original data
        assert_eq!(encrypted, plaintext);
    }

    #[test]
    fn test_encryption_at_rest_empty_key() {
        let config = EncryptionAtRestConfig::new(vec![]);
        let encryption = EncryptionAtRest::new(config);

        let plaintext = b"sensitive data";
        let encrypted = encryption.encrypt(plaintext).unwrap();

        // With empty key, should return original
        assert_eq!(encrypted, plaintext);
    }

    #[test]
    fn test_encryption_at_rest_symmetry() {
        let key = vec![42, 84, 126];
        let config = EncryptionAtRestConfig::new(key);
        let encryption = EncryptionAtRest::new(config);

        let plaintext = b"test data for encryption";
        let encrypted = encryption.encrypt(plaintext).unwrap();
        let encrypted_again = encryption.encrypt(&encrypted).unwrap();

        // XOR applied twice returns original
        assert_eq!(encrypted_again, plaintext);
    }

    #[test]
    fn test_encrypted_storage_basic() {
        let test_dir = ".zoka/test-suite/.test_encrypted_storage";
        let _ = fs::remove_dir_all(test_dir);

        let config = EncryptionAtRestConfig::new(vec![42, 84, 126, 210]);
        let storage = EncryptedStorage::with_rocksdb(test_dir, config).unwrap();

        let plaintext = b"secret value";
        storage.put("secret_key", plaintext.to_vec()).unwrap();

        // Retrieve decrypted
        let retrieved = storage.get("secret_key").unwrap();
        assert_eq!(retrieved, Some(plaintext.to_vec()));

        // Verify raw storage is encrypted
        if let Ok(Some(encrypted_raw)) = storage.get_encrypted_raw("secret_key") {
            assert_ne!(encrypted_raw, plaintext);
        }

        let _ = fs::remove_dir_all(test_dir);
    }

    #[test]
    fn test_encrypted_storage_multiple_keys() {
        let test_dir = ".zoka/test-suite/.test_encrypted_storage_multi";
        let _ = fs::remove_dir_all(test_dir);

        let config = EncryptionAtRestConfig::new(vec![1, 2, 3, 4, 5]);
        let storage = EncryptedStorage::with_rocksdb(test_dir, config).unwrap();

        storage.put("key1", b"value1".to_vec()).unwrap();
        storage.put("key2", b"value2".to_vec()).unwrap();
        storage.put("key3", b"value3".to_vec()).unwrap();

        assert_eq!(storage.get("key1").unwrap(), Some(b"value1".to_vec()));
        assert_eq!(storage.get("key2").unwrap(), Some(b"value2".to_vec()));
        assert_eq!(storage.get("key3").unwrap(), Some(b"value3".to_vec()));

        let _ = fs::remove_dir_all(test_dir);
    }

    #[test]
    fn test_encrypted_storage_delete() {
        let test_dir = ".zoka/test-suite/.test_encrypted_storage_delete";
        let _ = fs::remove_dir_all(test_dir);

        let config = EncryptionAtRestConfig::disabled();
        let storage = EncryptedStorage::with_rocksdb(test_dir, config).unwrap();

        storage.put("to_delete", b"content".to_vec()).unwrap();
        assert!(storage.exists("to_delete").unwrap());

        storage.delete("to_delete").unwrap();
        assert!(!storage.exists("to_delete").unwrap());

        let _ = fs::remove_dir_all(test_dir);
    }

    #[test]
    fn test_encryption_at_rest_aes256_config() {
        let key = vec![
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
            25, 26, 27, 28, 29, 30, 31, 32,
        ];
        let config = EncryptionAtRestConfig::aes256(key);

        assert!(config.enabled);
        assert_eq!(config.master_key.len(), 32);
        match config.algorithm {
            EncryptionAlgorithm::Aes256 => assert!(true),
            _ => panic!("Expected Aes256"),
        }
    }

    #[test]
    fn test_encrypted_storage_account_state() {
        let test_dir = ".zoka/test-suite/.test_encrypted_account";
        let _ = fs::remove_dir_all(test_dir);

        let config = EncryptionAtRestConfig::new(vec![111, 222, 33, 44]);
        let encrypted = EncryptedStorage::with_rocksdb(test_dir, config).unwrap();

        let account_json = serde_json::to_vec(&AccountState {
            address: "alice".to_string(),
            balance: 5000,
            nonce: 10,
            last_updated_height: 100,
        })
        .unwrap();

        encrypted.put("state:alice", account_json.clone()).unwrap();
        let retrieved = encrypted.get("state:alice").unwrap();
        assert_eq!(retrieved, Some(account_json));

        let _ = fs::remove_dir_all(test_dir);
    }
}
