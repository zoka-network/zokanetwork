//! Persistent Encrypted Wallet Storage
//! Saves wallets to disk with password-based encryption

use crate::private_wallet::PrivateWallet;
use crate::types::BlockError;
use crate::wallet::UserWallet;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use zeroize::Zeroize;

const WALLET_DIR: &str = ".zoka/wallets";
const PRIVATE_WALLET_DIR: &str = ".zoka/private_wallets";

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
}

impl WalletStorage {
    /// Create new wallet storage with default paths
    pub fn new() -> Result<Self, BlockError> {
        let home_dir = home::home_dir().ok_or(BlockError::TransactionValidationFailed(
            "Cannot determine home directory".to_string(),
        ))?;

        let wallets_path = home_dir.join(WALLET_DIR);
        let private_wallets_path = home_dir.join(PRIVATE_WALLET_DIR);

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
            return Err(BlockError::TransactionValidationFailed(
                "Wallet file not found".to_string(),
            ));
        }

        let mut file = fs::File::open(&filepath).map_err(|e| {
            BlockError::TransactionValidationFailed(format!("Failed to open wallet file: {}", e))
        })?;

        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(|e| {
            BlockError::TransactionValidationFailed(format!("Failed to read wallet file: {}", e))
        })?;

        // If password provided, decrypt
        let json_str = if let Some(pwd) = password {
            let decrypted = decrypt_data(&bytes, pwd)?;
            String::from_utf8(decrypted).map_err(|e| {
                BlockError::TransactionValidationFailed(format!("UTF-8 error: {}", e))
            })?
        } else {
            String::from_utf8(bytes).map_err(|e| {
                BlockError::TransactionValidationFailed(format!("UTF-8 error: {}", e))
            })?
        };

        let encrypted: EncryptedWallet = serde_json::from_str(&json_str).map_err(|e| {
            BlockError::TransactionValidationFailed(format!("JSON parse failed: {}", e))
        })?;

        Ok((encrypted.wallet, encrypted.mnemonic))
    }

    /// Save private wallet to disk
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

        let encrypted = EncryptedPrivateWallet {
            wallet: wallet.clone(),
            encrypted_spending_key: encrypted_key,
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
            return Err(BlockError::TransactionValidationFailed(
                "Private wallet file not found".to_string(),
            ));
        }

        let mut file = fs::File::open(&filepath).map_err(|e| {
            BlockError::TransactionValidationFailed(format!("Failed to open wallet file: {}", e))
        })?;

        let mut json_bytes = Vec::new();
        file.read_to_end(&mut json_bytes).map_err(|e| {
            BlockError::TransactionValidationFailed(format!("Failed to read wallet file: {}", e))
        })?;

        let json_str = String::from_utf8(json_bytes)
            .map_err(|e| BlockError::TransactionValidationFailed(format!("UTF-8 error: {}", e)))?;

        let mut encrypted: EncryptedPrivateWallet =
            serde_json::from_str(&json_str).map_err(|e| {
                BlockError::TransactionValidationFailed(format!("JSON parse failed: {}", e))
            })?;

        // Decrypt spending key if password-protected
        if let Some(pwd) = password {
            let decrypted = decrypt_data(&encrypted.encrypted_spending_key, pwd)?;
            encrypted.wallet.spending_key = decrypted;
        }

        Ok(encrypted.wallet)
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
            "Password cannot be empty".to_string(),
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
            "Password cannot be empty".to_string(),
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
            .map_err(|e| {
                BlockError::TransactionValidationFailed(format!("Decryption failed: {}", e))
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

    #[test]
    fn test_wallet_storage_creation() {
        let storage = WalletStorage::new();
        assert!(storage.is_ok());
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
}
