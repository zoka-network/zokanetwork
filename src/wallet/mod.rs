// User-facing wallet management system
use crate::identity::HDWallet;
use crate::types::{BlockError, PublicKey};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// User wallet with address and balance tracking
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserWallet {
    pub name: String,
    pub address: String,
    pub public_key: Vec<u8>,
    pub balance: u64,
    pub transactions: Vec<WalletTransaction>,
    pub created_at: u64,
}

/// Transaction as seen by wallet user
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WalletTransaction {
    pub hash: String,
    pub from: String,
    pub to: String,
    pub amount: u64,
    pub fee: u64,
    pub status: TransactionStatus,
    pub timestamp: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum TransactionStatus {
    Pending,
    Confirmed,
    Failed,
}

impl std::fmt::Display for TransactionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransactionStatus::Pending => write!(f, "pending"),
            TransactionStatus::Confirmed => write!(f, "confirmed"),
            TransactionStatus::Failed => write!(f, "failed"),
        }
    }
}

/// Wallet manager for user operations
#[allow(dead_code)]
pub struct WalletManager {
    wallets: HashMap<String, UserWallet>,
    hd_wallets: HashMap<String, HDWallet>,
    default_wallet: Option<String>,
}

impl WalletManager {
    /// Create new wallet manager
    pub fn new() -> Self {
        WalletManager {
            wallets: HashMap::new(),
            hd_wallets: HashMap::new(),
            default_wallet: None,
        }
    }

    /// Create new wallet from scratch (generates seed)
    pub fn create_wallet(
        &mut self,
        wallet_name: &str,
    ) -> Result<(String, Vec<String>), BlockError> {
        if self.wallets.contains_key(wallet_name) {
            return Err(BlockError::TransactionValidationFailed(
                "Wallet already exists".to_string(),
            ));
        }

        // Generate seed
        let seed = HDWallet::generate_seed(32)?;
        let mut hd_wallet = HDWallet::new(seed.clone())?;

        // Generate mnemonic
        let mnemonic = hd_wallet.seed_to_mnemonic()?;

        // Derive first account
        let (pub_key, _priv_key) = hd_wallet.derive_account(0)?;

        // Create user wallet
        let address = Self::pubkey_to_address(&pub_key);
        let user_wallet = UserWallet {
            name: wallet_name.to_string(),
            address: address.clone(),
            public_key: pub_key.0.clone(),
            balance: 0,
            transactions: Vec::new(),
            created_at: Self::current_timestamp(),
        };

        self.wallets.insert(wallet_name.to_string(), user_wallet);
        self.hd_wallets.insert(wallet_name.to_string(), hd_wallet);

        if self.default_wallet.is_none() {
            self.default_wallet = Some(wallet_name.to_string());
        }

        Ok((address, mnemonic))
    }

    /// Import wallet from mnemonic
    pub fn import_wallet(
        &mut self,
        wallet_name: &str,
        mnemonic: &[String],
    ) -> Result<String, BlockError> {
        if self.wallets.contains_key(wallet_name) {
            return Err(BlockError::TransactionValidationFailed(
                "Wallet already exists".to_string(),
            ));
        }

        // Recover seed from mnemonic
        let seed = HDWallet::mnemonic_to_seed(mnemonic)?;
        let mut hd_wallet = HDWallet::new(seed)?;

        // Derive first account
        let (pub_key, _priv_key) = hd_wallet.derive_account(0)?;

        // Create user wallet
        let address = Self::pubkey_to_address(&pub_key);
        let user_wallet = UserWallet {
            name: wallet_name.to_string(),
            address: address.clone(),
            public_key: pub_key.0.clone(),
            balance: 0,
            transactions: Vec::new(),
            created_at: Self::current_timestamp(),
        };

        self.wallets.insert(wallet_name.to_string(), user_wallet);
        self.hd_wallets.insert(wallet_name.to_string(), hd_wallet);

        if self.default_wallet.is_none() {
            self.default_wallet = Some(wallet_name.to_string());
        }

        Ok(address)
    }

    /// Get wallet address
    pub fn get_address(&self, wallet_name: &str) -> Option<String> {
        self.wallets.get(wallet_name).map(|w| w.address.clone())
    }

    /// Get wallet mutably
    pub fn get_wallet_mut(&mut self, wallet_name: &str) -> Option<&mut UserWallet> {
        self.wallets.get_mut(wallet_name)
    }

    /// Get wallet balance
    pub fn get_balance(&self, wallet_name: &str) -> Option<u64> {
        self.wallets.get(wallet_name).map(|w| w.balance)
    }

    /// Update balance for wallet
    pub fn update_balance(
        &mut self,
        wallet_name: &str,
        new_balance: u64,
    ) -> Result<(), BlockError> {
        if let Some(wallet) = self.wallets.get_mut(wallet_name) {
            wallet.balance = new_balance;
            Ok(())
        } else {
            Err(BlockError::TransactionValidationFailed(
                "Wallet not found".to_string(),
            ))
        }
    }

    /// Add transaction to wallet history
    pub fn add_transaction(
        &mut self,
        wallet_name: &str,
        tx: WalletTransaction,
    ) -> Result<(), BlockError> {
        if let Some(wallet) = self.wallets.get_mut(wallet_name) {
            wallet.transactions.push(tx);
            Ok(())
        } else {
            Err(BlockError::TransactionValidationFailed(
                "Wallet not found".to_string(),
            ))
        }
    }

    /// Get transaction history
    pub fn get_transactions(&self, wallet_name: &str) -> Option<Vec<WalletTransaction>> {
        self.wallets
            .get(wallet_name)
            .map(|w| w.transactions.clone())
    }

    /// Get wallet info
    pub fn get_wallet(&self, wallet_name: &str) -> Option<UserWallet> {
        self.wallets.get(wallet_name).cloned()
    }

    /// List all wallets
    pub fn list_wallets(&self) -> Vec<(String, String, u64)> {
        self.wallets
            .iter()
            .map(|(name, wallet)| (name.clone(), wallet.address.clone(), wallet.balance))
            .collect()
    }

    /// Set default wallet
    pub fn set_default(&mut self, wallet_name: &str) -> Result<(), BlockError> {
        if self.wallets.contains_key(wallet_name) {
            self.default_wallet = Some(wallet_name.to_string());
            Ok(())
        } else {
            Err(BlockError::TransactionValidationFailed(
                "Wallet not found".to_string(),
            ))
        }
    }

    /// Get default wallet
    pub fn get_default(&self) -> Option<String> {
        self.default_wallet.clone()
    }

    /// Derive additional account from wallet
    pub fn derive_account(
        &mut self,
        wallet_name: &str,
        account_index: u32,
    ) -> Result<String, BlockError> {
        if let Some(hd_wallet) = self.hd_wallets.get_mut(wallet_name) {
            let (pub_key, _priv_key) = hd_wallet.derive_account(account_index)?;
            Ok(Self::pubkey_to_address(&pub_key))
        } else {
            Err(BlockError::TransactionValidationFailed(
                "Wallet not found".to_string(),
            ))
        }
    }

    /// Export wallet (seed recovery phrase)
    pub fn export_wallet(&self, wallet_name: &str) -> Result<Vec<String>, BlockError> {
        if let Some(hd_wallet) = self.hd_wallets.get(wallet_name) {
            hd_wallet.seed_to_mnemonic()
        } else {
            Err(BlockError::TransactionValidationFailed(
                "Wallet not found".to_string(),
            ))
        }
    }

    /// Delete wallet
    pub fn delete_wallet(&mut self, wallet_name: &str) -> Result<(), BlockError> {
        self.wallets.remove(wallet_name);
        self.hd_wallets.remove(wallet_name);

        if self.default_wallet.as_ref() == Some(&wallet_name.to_string()) {
            self.default_wallet = None;
        }

        Ok(())
    }

    /// Check if wallet exists
    pub fn wallet_exists(&self, wallet_name: &str) -> bool {
        self.wallets.contains_key(wallet_name)
    }

    /// Get total balance across all wallets
    pub fn total_balance(&self) -> u64 {
        self.wallets.values().map(|w| w.balance).sum()
    }

    /// Get all wallet names with their addresses
    pub fn get_addresses(&self) -> HashMap<String, String> {
        self.wallets
            .iter()
            .map(|(name, wallet)| (name.clone(), wallet.address.clone()))
            .collect()
    }

    /// List all wallet names
    pub fn list_wallet_names(&self) -> Vec<String> {
        self.wallets.keys().cloned().collect()
    }

    /// Send transaction from wallet
    pub fn send_transaction(
        &mut self,
        wallet_name: &str,
        _recipient: &str,
        amount: u64,
    ) -> Result<String, BlockError> {
        let wallet =
            self.wallets
                .get_mut(wallet_name)
                .ok_or(BlockError::TransactionValidationFailed(
                    "Wallet not found".to_string(),
                ))?;

        if wallet.balance < amount {
            return Err(BlockError::TransactionValidationFailed(
                "Insufficient balance".to_string(),
            ));
        }

        wallet.balance -= amount;

        let tx_hash = format!("0x{:x}", rand::random::<u64>());
        Ok(tx_hash)
    }

    /// Receive transaction (add to balance)
    pub fn receive_transaction(
        &mut self,
        wallet_name: &str,
        amount: u64,
    ) -> Result<(), BlockError> {
        let wallet =
            self.wallets
                .get_mut(wallet_name)
                .ok_or(BlockError::TransactionValidationFailed(
                    "Wallet not found".to_string(),
                ))?;

        wallet.balance += amount;
        Ok(())
    }

    /// Helper: Convert public key to address
    fn pubkey_to_address(pub_key: &PublicKey) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(&pub_key.0);
        let hash = hasher.finalize();
        format!("0x{}", hex::encode(&hash[..20]))
    }

    /// Helper: Current timestamp
    fn current_timestamp() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }
}

impl Default for WalletManager {
    fn default() -> Self {
        Self::new()
    }
}

pub mod storage;
