use crate::types::{AccountState, Block, State, StorageError, Transaction};
use rocksdb::{IteratorMode, Options, WriteBatch, DB};
use std::path::Path;
use std::sync::Arc;

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
        let key = format!("block:{}", block.header.hash);
        let value = serde_json::to_vec(block)
            .map_err(|e| StorageError::SerializationError(e.to_string()))?;
        self.put_raw(&key, value)?;
        let height_key = format!("chain:block_height:{}", block.header.block_height);
        self.put_raw(&height_key, block.header.hash.as_bytes().to_vec())?;
        Ok(())
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
        let height_key = format!("chain:block_height:{}", height);
        if let Some(hash_raw) = self.get_raw(&height_key)? {
            let hash = String::from_utf8(hash_raw)
                .map_err(|_| StorageError::SerializationError("Invalid UTF-8".to_string()))?;
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
        let test_dir = ".test_storage";
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
    fn test_storage_account_state() {
        let test_dir = ".test_storage_account";
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
        let test_dir = ".test_storage_block_index";
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
        let test_dir = ".test_storage_block_fallback";
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
        let test_dir = ".test_encrypted_storage";
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
        let test_dir = ".test_encrypted_storage_multi";
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
        let test_dir = ".test_encrypted_storage_delete";
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
        let test_dir = ".test_encrypted_account";
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
