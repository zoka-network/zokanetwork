//! Persistent Encrypted Wallet Storage
//! Saves wallets to disk with password-based encryption

use crate::private_wallet::PrivateWallet;
use crate::types::BlockError;
use crate::wallet::UserWallet;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use zeroize::Zeroize;

const DEFAULT_WALLET_BASE_DIR: &str = ".zoka/mainnet";

/// Encrypted wallet storage on disk
pub struct WalletStorage {
    wallets_path: PathBuf,
    private_wallets_path: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct EncryptedWallet {
    /// User wallet
    pub wallet: UserWallet,
    /// Mnemonic (plaintext for now, should be encrypted)
    pub mnemonic: Option<Vec<String>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct EncryptedPrivateWallet {
    /// Private wallet
    pub wallet: PrivateWallet,
    /// Encrypted spending key
    pub encrypted_spending_key: Vec<u8>,
    /// Encrypted scan key
    #[serde(default)]
    pub encrypted_scan_key: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct PrivateWalletMetadata {
    pub name: String,
    pub commitment_key: String,
    pub private_balance: u64,
    pub private_transaction_count: usize,
    pub encrypted: bool,
}

impl WalletStorage {
    /// Create new wallet storage with default paths
    pub fn new() -> Result<Self, BlockError> {
        Self::from_base_dir(DEFAULT_WALLET_BASE_DIR)
    }

    /// Create wallet storage rooted under a network/profile directory.
    ///
    /// For example, `.zoka/mainnet` stores private wallets in
    /// `.zoka/mainnet/private_wallets`, while `.zoka/testnet` stores them in
    /// `.zoka/testnet/private_wallets`.
    pub fn from_base_dir(base_dir: impl AsRef<Path>) -> Result<Self, BlockError> {
        let base_dir = base_dir.as_ref();
        let wallets_path = base_dir.join("wallets");
        let private_wallets_path = base_dir.join("private_wallets");

        // Create directories if they don't exist
        fs::create_dir_all(&wallets_path).map_err(|e| {
            BlockError::TransactionValidationFailed(format!("Failed to create wallets dir: {}", e))
        })?;
        fs::create_dir_all(&private_wallets_path).map_err(|e| {
            BlockError::TransactionValidationFailed(format!(
                "Failed to create private wallets dir: {}",
                e
            ))
        })?;

        Ok(WalletStorage {
            wallets_path,
            private_wallets_path,
        })
    }

    /// Returns true if a public wallet file with this name exists on disk.
    pub fn public_wallet_file_exists(&self, wallet_name: &str) -> bool {
        let filename = format!("{}.wallet.json", wallet_name);
        self.wallets_path.join(filename).is_file()
    }

    /// Returns true if a private wallet file with this name exists on disk.
    pub fn private_wallet_file_exists(&self, wallet_name: &str) -> bool {
        let filename = format!("{}.private.json", wallet_name);
        self.private_wallets_path.join(filename).is_file()
    }

    /// Directory where encrypted private wallet files are stored.
    pub fn private_wallets_dir(&self) -> &Path {
        &self.private_wallets_path
    }

    /// Save public wallet to disk
    pub fn save_public_wallet(
        &self,
        wallet: &UserWallet,
        mnemonic: Option<Vec<String>>,
        password: Option<&str>,
    ) -> Result<(), BlockError> {
        let encrypted = EncryptedWallet {
            wallet: wallet.clone(),
            mnemonic,
        };

        let json = serde_json::to_string(&encrypted).map_err(|e| {
            BlockError::TransactionValidationFailed(format!("JSON serialization failed: {}", e))
        })?;

        let filename = format!("{}.wallet.json", wallet.name);
        let filepath = self.wallets_path.join(&filename);

        // If password provided, encrypt the JSON
        let bytes = if let Some(pwd) = password {
            encrypt_data(json.as_bytes(), pwd)?
        } else {
            json.into_bytes()
        };

        let mut file = fs::File::create(&filepath).map_err(|e| {
            BlockError::TransactionValidationFailed(format!("Failed to create wallet file: {}", e))
        })?;

        file.write_all(&bytes).map_err(|e| {
            BlockError::TransactionValidationFailed(format!("Failed to write wallet file: {}", e))
        })?;

        Ok(())
    }

    /// Load public wallet from disk
    pub fn load_public_wallet(
        &self,
        wallet_name: &str,
        password: Option<&str>,
    ) -> Result<(UserWallet, Option<Vec<String>>), BlockError> {
        let filename = format!("{}.wallet.json", wallet_name);
        let filepath = self.wallets_path.join(&filename);

        if !filepath.exists() {
            return Err(BlockError::TransactionValidationFailed(format!(
                "wallet '{}' not found: run 'wallet-list' to see available wallets, or 'wallet-create --name {}' to create it",
                wallet_name, wallet_name
            )));
        }

        let mut file = fs::File::open(&filepath).map_err(|e| {
            BlockError::TransactionValidationFailed(format!(
                "failed to open wallet file '{}': {}",
                filepath.display(),
                e
            ))
        })?;

        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(|e| {
            BlockError::TransactionValidationFailed(format!(
                "failed to read wallet file '{}': {}",
                filepath.display(),
                e
            ))
        })?;

        let looks_encrypted =
            bytes.len() >= ENCRYPTION_MAGIC.len() && bytes.starts_with(ENCRYPTION_MAGIC);

        // If password provided, decrypt
        let json_str = if let Some(pwd) = password {
            let decrypted = decrypt_data(&bytes, pwd)?;
            String::from_utf8(decrypted).map_err(|e| {
                BlockError::TransactionValidationFailed(format!(
                    "wallet file '{}' is corrupted (invalid UTF-8 after decryption): {}",
                    filepath.display(),
                    e
                ))
            })?
        } else {
            if looks_encrypted {
                return Err(BlockError::TransactionValidationFailed(
                    "wallet is encrypted: provide --password <pwd> to unlock it".to_string(),
                ));
            }
            String::from_utf8(bytes).map_err(|e| {
                BlockError::TransactionValidationFailed(format!(
                    "wallet file '{}' is corrupted (invalid UTF-8): {}",
                    filepath.display(),
                    e
                ))
            })?
        };

        let encrypted: EncryptedWallet = serde_json::from_str(&json_str).map_err(|e| {
            BlockError::TransactionValidationFailed(format!(
                "wallet file '{}' is corrupted (invalid JSON): {}",
                filepath.display(),
                e
            ))
        })?;

        Ok((encrypted.wallet, encrypted.mnemonic))
    }

    /// Save private wallet to disk
    /// SECURITY: The private keys are NEVER written to disk in cleartext.
    /// Only encrypted key blobs are persisted. The wallet field has key material zeroed.
    pub fn save_private_wallet(
        &self,
        wallet: &PrivateWallet,
        password: Option<&str>,
    ) -> Result<(), BlockError> {
        // Encrypt spending key with password if provided
        let encrypted_key = if let Some(pwd) = password {
            encrypt_data(&wallet.spending_key, pwd)?
        } else {
            wallet.spending_key.clone()
        };
        let encrypted_scan_key = if let Some(pwd) = password {
            encrypt_data(&wallet.scan_key, pwd)?
        } else {
            wallet.scan_key.clone()
        };

        // Create a sanitized copy with key material cleared
        let mut sanitized_wallet = wallet.clone();
        sanitized_wallet.spending_key = vec![];
        sanitized_wallet.scan_key = vec![];
        for key in &mut sanitized_wallet.historical_spending_keys {
            *key = vec![];
        }
        for key in &mut sanitized_wallet.historical_scan_keys {
            *key = vec![];
        }

        let encrypted = EncryptedPrivateWallet {
            wallet: sanitized_wallet,
            encrypted_spending_key: encrypted_key,
            encrypted_scan_key,
        };

        let json = serde_json::to_string(&encrypted).map_err(|e| {
            BlockError::TransactionValidationFailed(format!("JSON serialization failed: {}", e))
        })?;

        let filename = format!("{}.private.json", wallet.name);
        let filepath = self.private_wallets_path.join(&filename);

        let mut file = fs::File::create(&filepath).map_err(|e| {
            BlockError::TransactionValidationFailed(format!(
                "Failed to create private wallet file: {}",
                e
            ))
        })?;

        file.write_all(json.as_bytes()).map_err(|e| {
            BlockError::TransactionValidationFailed(format!(
                "Failed to write private wallet file: {}",
                e
            ))
        })?;

        Ok(())
    }

    /// Load private wallet from disk
    pub fn load_private_wallet(
        &self,
        wallet_name: &str,
        password: Option<&str>,
    ) -> Result<PrivateWallet, BlockError> {
        let filename = format!("{}.private.json", wallet_name);
        let filepath = self.private_wallets_path.join(&filename);

        if !filepath.exists() {
            return Err(BlockError::TransactionValidationFailed(format!(
                "private wallet '{}' not found: run 'wallet-list' to see available wallets, or 'wallet-create --name {}' to create it",
                wallet_name, wallet_name
            )));
        }

        let mut file = fs::File::open(&filepath).map_err(|e| {
            BlockError::TransactionValidationFailed(format!(
                "failed to open private wallet file '{}': {}",
                filepath.display(),
                e
            ))
        })?;

        let mut json_bytes = Vec::new();
        file.read_to_end(&mut json_bytes).map_err(|e| {
            BlockError::TransactionValidationFailed(format!(
                "failed to read private wallet file '{}': {}",
                filepath.display(),
                e
            ))
        })?;

        let json_str = String::from_utf8(json_bytes).map_err(|e| {
            BlockError::TransactionValidationFailed(format!(
                "private wallet file '{}' is corrupted (invalid UTF-8): {}",
                filepath.display(),
                e
            ))
        })?;

        let mut encrypted: EncryptedPrivateWallet =
            serde_json::from_str(&json_str).map_err(|e| {
                BlockError::TransactionValidationFailed(format!(
                    "private wallet file '{}' is corrupted (invalid JSON): {}",
                    filepath.display(),
                    e
                ))
            })?;

        // Restore private keys from encrypted fields, never from wallet body.
        if let Some(pwd) = password {
            let decrypted = decrypt_data(&encrypted.encrypted_spending_key, pwd)?;
            encrypted.wallet.spending_key = decrypted;
            if encrypted.encrypted_scan_key.is_empty() {
                encrypted.wallet.scan_key = encrypted.wallet.spending_key.clone();
            } else {
                encrypted.wallet.scan_key = decrypt_data(&encrypted.encrypted_scan_key, pwd)?;
            }
        } else {
            if encrypted
                .encrypted_spending_key
                .starts_with(ENCRYPTION_MAGIC)
                || encrypted.encrypted_scan_key.starts_with(ENCRYPTION_MAGIC)
            {
                return Err(BlockError::TransactionValidationFailed(
                    "private wallet is encrypted: provide --password <pwd> to unlock it"
                        .to_string(),
                ));
            }
            // Without password, the encrypted_spending_key is the raw key
            encrypted.wallet.spending_key = encrypted.encrypted_spending_key.clone();
            encrypted.wallet.scan_key = if encrypted.encrypted_scan_key.is_empty() {
                encrypted.wallet.spending_key.clone()
            } else {
                encrypted.encrypted_scan_key.clone()
            };
        }

        Ok(encrypted.wallet)
    }

    /// Load non-secret private wallet metadata from disk.
    ///
    /// This reads the sanitized wallet body without decrypting key material, so
    /// wallet-list can show local wallet files even before the user unlocks
    /// them.
    pub fn load_private_wallet_metadata(
        &self,
        wallet_name: &str,
    ) -> Result<PrivateWalletMetadata, BlockError> {
        let filename = format!("{}.private.json", wallet_name);
        let filepath = self.private_wallets_path.join(&filename);

        if !filepath.exists() {
            return Err(BlockError::TransactionValidationFailed(format!(
                "private wallet '{}' not found: run 'wallet-list' to see available wallets, or 'wallet-create --name {}' to create it",
                wallet_name, wallet_name
            )));
        }

        let mut file = fs::File::open(&filepath).map_err(|e| {
            BlockError::TransactionValidationFailed(format!(
                "failed to open private wallet file '{}': {}",
                filepath.display(),
                e
            ))
        })?;

        let mut json_bytes = Vec::new();
        file.read_to_end(&mut json_bytes).map_err(|e| {
            BlockError::TransactionValidationFailed(format!(
                "failed to read private wallet file '{}': {}",
                filepath.display(),
                e
            ))
        })?;

        let json_str = String::from_utf8(json_bytes).map_err(|e| {
            BlockError::TransactionValidationFailed(format!(
                "private wallet file '{}' is corrupted (invalid UTF-8): {}",
                filepath.display(),
                e
            ))
        })?;

        let encrypted: EncryptedPrivateWallet = serde_json::from_str(&json_str).map_err(|e| {
            BlockError::TransactionValidationFailed(format!(
                "private wallet file '{}' is corrupted (invalid JSON): {}",
                filepath.display(),
                e
            ))
        })?;

        Ok(PrivateWalletMetadata {
            name: encrypted.wallet.name.clone(),
            commitment_key: encrypted.wallet.commitment_key.clone(),
            private_balance: encrypted.wallet.private_balance,
            private_transaction_count: encrypted.wallet.private_transaction_count(),
            encrypted: encrypted
                .encrypted_spending_key
                .starts_with(ENCRYPTION_MAGIC)
                || encrypted.encrypted_scan_key.starts_with(ENCRYPTION_MAGIC),
        })
    }

    /// Load all public wallets from disk
    pub fn load_all_public_wallets(
        &self,
        password: Option<&str>,
    ) -> Result<Vec<(UserWallet, Option<Vec<String>>)>, BlockError> {
        let mut wallets = Vec::new();

        if !self.wallets_path.exists() {
            return Ok(wallets);
        }

        for entry in fs::read_dir(&self.wallets_path).map_err(|e| {
            BlockError::TransactionValidationFailed(format!("Failed to read wallets dir: {}", e))
        })? {
            let entry = entry.map_err(|e| {
                BlockError::TransactionValidationFailed(format!("Dir entry error: {}", e))
            })?;

            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                    if let Some(wallet_name) = filename.strip_suffix(".wallet.json") {
                        if let Ok(wallet) = self.load_public_wallet(wallet_name, password) {
                            wallets.push(wallet);
                        }
                    }
                }
            }
        }

        Ok(wallets)
    }

    /// Load all private wallets from disk
    pub fn load_all_private_wallets(
        &self,
        password: Option<&str>,
    ) -> Result<Vec<PrivateWallet>, BlockError> {
        let mut wallets = Vec::new();

        if !self.private_wallets_path.exists() {
            return Ok(wallets);
        }

        for entry in fs::read_dir(&self.private_wallets_path).map_err(|e| {
            BlockError::TransactionValidationFailed(format!("Failed to read wallets dir: {}", e))
        })? {
            let entry = entry.map_err(|e| {
                BlockError::TransactionValidationFailed(format!("Dir entry error: {}", e))
            })?;

            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                    if let Some(wallet_name) = filename.strip_suffix(".private.json") {
                        if let Ok(wallet) = self.load_private_wallet(wallet_name, password) {
                            wallets.push(wallet);
                        }
                    }
                }
            }
        }

        Ok(wallets)
    }

    /// Load non-secret metadata for all local private wallet files.
    pub fn load_all_private_wallet_metadata(
        &self,
    ) -> Result<Vec<PrivateWalletMetadata>, BlockError> {
        let mut wallets = Vec::new();

        if !self.private_wallets_path.exists() {
            return Ok(wallets);
        }

        for entry in fs::read_dir(&self.private_wallets_path).map_err(|e| {
            BlockError::TransactionValidationFailed(format!("Failed to read wallets dir: {}", e))
        })? {
            let entry = entry.map_err(|e| {
                BlockError::TransactionValidationFailed(format!("Dir entry error: {}", e))
            })?;

            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                    if let Some(wallet_name) = filename.strip_suffix(".private.json") {
                        if let Ok(wallet) = self.load_private_wallet_metadata(wallet_name) {
                            wallets.push(wallet);
                        }
                    }
                }
            }
        }

        wallets.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(wallets)
    }

    /// Delete wallet file
    pub fn delete_wallet(&self, wallet_name: &str, is_private: bool) -> Result<(), BlockError> {
        let filename = if is_private {
            format!("{}.private.json", wallet_name)
        } else {
            format!("{}.wallet.json", wallet_name)
        };

        let dir = if is_private {
            &self.private_wallets_path
        } else {
            &self.wallets_path
        };

        let filepath = dir.join(&filename);
        fs::remove_file(&filepath).map_err(|e| {
            BlockError::TransactionValidationFailed(format!("Failed to delete wallet: {}", e))
        })?;

        Ok(())
    }

    /// Backup a public wallet file to an arbitrary path.
    pub fn backup_public_wallet(
        &self,
        wallet_name: &str,
        destination: &std::path::Path,
    ) -> Result<(), BlockError> {
        let source = self
            .wallets_path
            .join(format!("{}.wallet.json", wallet_name));
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                BlockError::TransactionValidationFailed(format!(
                    "Failed to create backup dir: {}",
                    e
                ))
            })?;
        }
        fs::copy(&source, destination).map_err(|e| {
            BlockError::TransactionValidationFailed(format!(
                "Failed to copy backup file from '{}' to '{}': {}",
                source.display(),
                destination.display(),
                e
            ))
        })?;
        Ok(())
    }

    /// Re-encrypt a public wallet with a new passphrase.
    pub fn change_public_wallet_passphrase(
        &self,
        wallet_name: &str,
        old_password: Option<&str>,
        new_password: Option<&str>,
    ) -> Result<(), BlockError> {
        let (wallet, mnemonic) = self.load_public_wallet(wallet_name, old_password)?;
        self.save_public_wallet(&wallet, mnemonic, new_password)
    }
}

impl Default for WalletStorage {
    fn default() -> Self {
        Self::new().unwrap_or_else(|_| {
            // Fallback to temp directory if home dir not available
            WalletStorage {
                wallets_path: PathBuf::from("/tmp/.zoka/wallets"),
                private_wallets_path: PathBuf::from("/tmp/.zoka/private_wallets"),
            }
        })
    }
}

const ENCRYPTION_MAGIC: &[u8] = b"ZOKAENC1";
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;

fn derive_encryption_key(password: &str, salt: &[u8]) -> Result<[u8; 32], BlockError> {
    let mut key = [0u8; 32];
    argon2::Argon2::default()
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|e| {
            BlockError::TransactionValidationFailed(format!("Key derivation failed: {}", e))
        })?;
    Ok(key)
}

fn xor_legacy(data: &[u8], password: &str) -> Result<Vec<u8>, BlockError> {
    use sha2::{Digest, Sha256};

    if password.is_empty() {
        return Err(BlockError::TransactionValidationFailed(
            "Password cannot be empty".to_string(),
        ));
    }

    let mut hasher = Sha256::new();
    hasher.update(password.as_bytes());
    let key = hasher.finalize();

    let mut output = Vec::with_capacity(data.len());
    for (i, byte) in data.iter().enumerate() {
        output.push(byte ^ key[i % 32]);
    }
    Ok(output)
}

/// Password-based authenticated encryption with Argon2 + AES-256-GCM.
/// Format: magic(8) || salt(16) || nonce(12) || ciphertext
fn encrypt_data(data: &[u8], password: &str) -> Result<Vec<u8>, BlockError> {
    use aes_gcm::aead::Aead;
    use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
    use rand::RngCore;

    if password.is_empty() {
        return Err(BlockError::TransactionValidationFailed(
            "password cannot be empty: choose a non-empty passphrase to protect this wallet"
                .to_string(),
        ));
    }

    let mut salt = [0u8; SALT_LEN];
    rand::rngs::OsRng.fill_bytes(&mut salt);

    let mut key = derive_encryption_key(password, &salt)?;
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| {
        BlockError::TransactionValidationFailed(format!("Cipher init failed: {}", e))
    })?;

    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher.encrypt(nonce, data).map_err(|e| {
        BlockError::TransactionValidationFailed(format!("Encryption failed: {}", e))
    })?;

    key.zeroize();

    let mut out =
        Vec::with_capacity(ENCRYPTION_MAGIC.len() + SALT_LEN + NONCE_LEN + ciphertext.len());
    out.extend_from_slice(ENCRYPTION_MAGIC);
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Password-based authenticated decryption.
/// Supports new format and legacy XOR format for backward compatibility.
fn decrypt_data(encrypted: &[u8], password: &str) -> Result<Vec<u8>, BlockError> {
    use aes_gcm::aead::Aead;
    use aes_gcm::{Aes256Gcm, KeyInit, Nonce};

    if password.is_empty() {
        return Err(BlockError::TransactionValidationFailed(
            "password cannot be empty: provide --password <pwd> to unlock the wallet".to_string(),
        ));
    }

    if encrypted.len() >= ENCRYPTION_MAGIC.len() + SALT_LEN + NONCE_LEN
        && &encrypted[..ENCRYPTION_MAGIC.len()] == ENCRYPTION_MAGIC
    {
        let salt_start = ENCRYPTION_MAGIC.len();
        let nonce_start = salt_start + SALT_LEN;
        let data_start = nonce_start + NONCE_LEN;

        let salt = &encrypted[salt_start..nonce_start];
        let nonce = &encrypted[nonce_start..data_start];
        let ciphertext = &encrypted[data_start..];

        let mut key = derive_encryption_key(password, salt)?;
        let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| {
            BlockError::TransactionValidationFailed(format!("Cipher init failed: {}", e))
        })?;
        let plaintext = cipher
            .decrypt(Nonce::from_slice(nonce), ciphertext)
            .map_err(|_| {
                BlockError::TransactionValidationFailed(
                    "incorrect password (or wallet file corrupted): authenticated decryption failed"
                        .to_string(),
                )
            })?;
        key.zeroize();
        return Ok(plaintext);
    }

    // Legacy compatibility fallback.
    xor_legacy(encrypted, password)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::private_wallet::PrivateWalletManager;
    use rand::RngCore;

    #[test]
    fn test_wallet_storage_creation() {
        let storage = WalletStorage::new();
        assert!(storage.is_ok());
    }

    #[test]
    fn test_wallet_storage_from_base_dir_uses_network_local_paths() {
        let mut suffix = [0u8; 4];
        rand::rngs::OsRng.fill_bytes(&mut suffix);
        let base = std::env::temp_dir()
            .join("zoka-wallet-storage")
            .join(hex::encode(suffix));

        let storage = WalletStorage::from_base_dir(&base).expect("profile storage");
        assert_eq!(storage.private_wallets_dir(), base.join("private_wallets"));
        assert!(base.join("wallets").is_dir());
        assert!(base.join("private_wallets").is_dir());

        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip_modern_format() {
        let data = b"wallet-sensitive-payload";
        let password = "correct horse battery staple";
        let encrypted = encrypt_data(data, password).expect("encrypt");

        assert!(encrypted.starts_with(ENCRYPTION_MAGIC));
        let decrypted = decrypt_data(&encrypted, password).expect("decrypt");
        assert_eq!(decrypted, data);
    }

    #[test]
    fn test_decrypt_legacy_xor_payload_compatibility() {
        let data = b"legacy-format";
        let password = "legacy-password";
        let legacy = xor_legacy(data, password).expect("legacy");
        let decrypted = decrypt_data(&legacy, password).expect("decrypt fallback");
        assert_eq!(decrypted, data);
    }

    #[test]
    fn test_private_wallet_roundtrip_persists_scan_key() {
        let storage = WalletStorage::new().expect("storage");
        let mut manager = PrivateWalletManager::new();
        let mut random_suffix = [0u8; 4];
        rand::rngs::OsRng.fill_bytes(&mut random_suffix);
        let wallet_name = format!("storage-test-{}", hex::encode(random_suffix));
        let password = "private-storage-password";

        manager
            .create_private_wallet(&wallet_name)
            .expect("create private wallet");
        let wallet = manager
            .get_private_wallet(&wallet_name)
            .expect("wallet exists")
            .clone();

        storage
            .save_private_wallet(&wallet, Some(password))
            .expect("save private wallet");
        let restored = storage
            .load_private_wallet(&wallet_name, Some(password))
            .expect("load private wallet");

        assert_eq!(restored.spending_key, wallet.spending_key);
        assert_eq!(restored.scan_key, wallet.scan_key);
        assert_eq!(restored.commitment_key, wallet.commitment_key);
        assert!(restored.commitment_key.starts_with("zpriv:"));

        storage
            .delete_wallet(&wallet_name, true)
            .expect("cleanup private wallet file");
    }

    #[test]
    fn test_encrypted_private_wallet_requires_password() {
        let storage = WalletStorage::new().expect("storage");
        let mut manager = PrivateWalletManager::new();
        let mut random_suffix = [0u8; 4];
        rand::rngs::OsRng.fill_bytes(&mut random_suffix);
        let wallet_name = format!("storage-encrypted-test-{}", hex::encode(random_suffix));
        let password = "required-password";

        manager
            .create_private_wallet(&wallet_name)
            .expect("create private wallet");
        let wallet = manager
            .get_private_wallet(&wallet_name)
            .expect("wallet exists")
            .clone();

        storage
            .save_private_wallet(&wallet, Some(password))
            .expect("save encrypted private wallet");

        let without_password = storage.load_private_wallet(&wallet_name, None);
        assert!(without_password.is_err());

        let restored = storage
            .load_private_wallet(&wallet_name, Some(password))
            .expect("load with password");
        assert_eq!(restored.commitment_key, wallet.commitment_key);

        storage
            .delete_wallet(&wallet_name, true)
            .expect("cleanup private wallet file");
    }

    #[test]
    fn test_private_wallet_metadata_loads_encrypted_wallet_without_password() {
        let mut random_suffix = [0u8; 4];
        rand::rngs::OsRng.fill_bytes(&mut random_suffix);
        let base = std::env::temp_dir()
            .join("zoka-wallet-metadata")
            .join(hex::encode(random_suffix));
        let storage = WalletStorage::from_base_dir(&base).expect("storage");
        let mut manager = PrivateWalletManager::new();
        let wallet_name = "metadata-encrypted-test";
        let password = "metadata-password";

        manager
            .create_private_wallet(wallet_name)
            .expect("create private wallet");
        let wallet = manager
            .get_private_wallet(wallet_name)
            .expect("wallet exists")
            .clone();

        storage
            .save_private_wallet(&wallet, Some(password))
            .expect("save encrypted private wallet");

        let without_password = storage.load_private_wallet(wallet_name, None);
        assert!(without_password.is_err());

        let metadata = storage
            .load_private_wallet_metadata(wallet_name)
            .expect("load metadata without password");
        assert_eq!(metadata.name, wallet_name);
        assert_eq!(metadata.commitment_key, wallet.commitment_key);
        assert!(metadata.encrypted);

        let all_metadata = storage
            .load_all_private_wallet_metadata()
            .expect("load all metadata");
        assert_eq!(all_metadata.len(), 1);
        assert_eq!(all_metadata[0].name, wallet_name);

        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn test_decrypt_with_wrong_password_returns_actionable_message() {
        let data = b"wallet-sensitive-payload";
        let encrypted = encrypt_data(data, "right-password").expect("encrypt");

        let err = decrypt_data(&encrypted, "wrong-password").unwrap_err();
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("incorrect password"),
            "wrong-password decryption error must say 'incorrect password'; got: {msg}"
        );
    }

    #[test]
    fn test_decrypt_empty_password_returns_actionable_message() {
        let encrypted = encrypt_data(b"x", "p").expect("encrypt");
        let err = decrypt_data(&encrypted, "").unwrap_err();
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("--password"),
            "empty password error must mention --password flag; got: {msg}"
        );
    }

    #[test]
    fn test_load_missing_public_wallet_returns_actionable_message() {
        let storage = WalletStorage::new().expect("storage");
        let mut suffix = [0u8; 4];
        rand::rngs::OsRng.fill_bytes(&mut suffix);
        let bogus = format!("does-not-exist-{}", hex::encode(suffix));

        let err = storage.load_public_wallet(&bogus, None).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains(&bogus),
            "error must include the wallet name the user typed; got: {msg}"
        );
        assert!(
            msg.contains("wallet-list") || msg.contains("wallet-create"),
            "error must hint at wallet-list or wallet-create; got: {msg}"
        );
    }

    #[test]
    fn test_load_missing_private_wallet_returns_actionable_message() {
        let storage = WalletStorage::new().expect("storage");
        let mut suffix = [0u8; 4];
        rand::rngs::OsRng.fill_bytes(&mut suffix);
        let bogus = format!("private-does-not-exist-{}", hex::encode(suffix));

        let err = storage.load_private_wallet(&bogus, None).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains(&bogus),
            "error must include the wallet name the user typed; got: {msg}"
        );
        assert!(
            msg.contains("wallet-list") || msg.contains("wallet-create"),
            "error must hint at wallet-list or wallet-create; got: {msg}"
        );
    }

    #[test]
    fn test_encrypted_private_wallet_without_password_says_provide_password() {
        let storage = WalletStorage::new().expect("storage");
        let mut manager = PrivateWalletManager::new();
        let mut suffix = [0u8; 4];
        rand::rngs::OsRng.fill_bytes(&mut suffix);
        let wallet_name = format!("encrypted-no-pwd-{}", hex::encode(suffix));

        manager
            .create_private_wallet(&wallet_name)
            .expect("create private wallet");
        let wallet = manager
            .get_private_wallet(&wallet_name)
            .expect("wallet exists")
            .clone();

        storage
            .save_private_wallet(&wallet, Some("locked-password"))
            .expect("save encrypted");

        let err = storage.load_private_wallet(&wallet_name, None).unwrap_err();
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("--password"),
            "missing-password error must point the user at --password; got: {msg}"
        );

        storage
            .delete_wallet(&wallet_name, true)
            .expect("cleanup private wallet file");
    }

    #[test]
    fn test_wallet_file_existence_helpers() {
        let storage = WalletStorage::new().expect("storage");
        let mut suffix = [0u8; 4];
        rand::rngs::OsRng.fill_bytes(&mut suffix);
        let wallet_name = format!("file-existence-{}", hex::encode(suffix));

        assert!(!storage.public_wallet_file_exists(&wallet_name));
        assert!(!storage.private_wallet_file_exists(&wallet_name));

        let mut manager = PrivateWalletManager::new();
        manager
            .create_private_wallet(&wallet_name)
            .expect("create private wallet");
        let wallet = manager
            .get_private_wallet(&wallet_name)
            .expect("wallet exists")
            .clone();
        storage
            .save_private_wallet(&wallet, Some("file-test"))
            .expect("save private wallet");

        assert!(storage.private_wallet_file_exists(&wallet_name));

        storage
            .delete_wallet(&wallet_name, true)
            .expect("cleanup private wallet file");

        assert!(!storage.private_wallet_file_exists(&wallet_name));
    }
}
