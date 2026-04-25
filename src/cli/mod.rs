/// CLI utilities for wallet and account management
use crate::types::{Signature, Transaction};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// CLI configuration file structure
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CliConfig {
    /// Configuration name
    pub name: String,
    /// Default wallet to use
    pub default_wallet: Option<String>,
    /// Storage path for wallets
    pub wallet_storage_path: PathBuf,
    /// Node endpoint
    pub node_endpoint: String,
    /// Timeout for transactions (seconds)
    pub tx_timeout_secs: u64,
    /// Enable encryption for wallets
    pub enable_wallet_encryption: bool,
}

/// Multi-profile CLI configuration
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MultiProfileConfig {
    pub profiles: HashMap<String, CliConfig>,
    pub active_profile: String,
}

impl Default for MultiProfileConfig {
    fn default() -> Self {
        let mut profiles = HashMap::new();
        profiles.insert("default".to_string(), CliConfig::default());

        MultiProfileConfig {
            profiles,
            active_profile: "default".to_string(),
        }
    }
}

impl MultiProfileConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_profile(&mut self, name: String, config: CliConfig) -> Result<(), String> {
        if self.profiles.contains_key(&name) {
            return Err(format!("Profile '{}' already exists", name));
        }
        self.profiles.insert(name, config);
        Ok(())
    }

    pub fn get_active_profile(&self) -> Option<&CliConfig> {
        self.profiles.get(&self.active_profile)
    }

    pub fn switch_profile(&mut self, name: &str) -> Result<(), String> {
        if !self.profiles.contains_key(name) {
            return Err(format!("Profile '{}' not found", name));
        }
        self.active_profile = name.to_string();
        Ok(())
    }

    pub fn list_profiles(&self) -> Vec<String> {
        self.profiles.keys().cloned().collect()
    }

    pub fn save_to_file(&self, path: &Path) -> Result<(), String> {
        let toml = toml::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize profiles: {}", e))?;

        fs::create_dir_all(path.parent().unwrap_or_else(|| Path::new(".")))
            .map_err(|e| format!("Failed to create parent directory: {}", e))?;

        fs::write(path, toml).map_err(|e| format!("Failed to write profiles file: {}", e))
    }

    pub fn load_from_file(path: &Path) -> Result<Self, String> {
        let content =
            fs::read_to_string(path).map_err(|e| format!("Failed to read profiles file: {}", e))?;

        toml::from_str::<MultiProfileConfig>(&content)
            .map_err(|e| format!("Failed to parse profiles file: {}", e))
    }
}

impl Default for CliConfig {
    fn default() -> Self {
        CliConfig {
            name: "zoka-cli".to_string(),
            default_wallet: None,
            wallet_storage_path: PathBuf::from("./wallets"),
            node_endpoint: "http://127.0.0.1:3000".to_string(),
            tx_timeout_secs: 60,
            enable_wallet_encryption: false,
        }
    }
}

impl CliConfig {
    /// Create a new configuration
    pub fn new(name: String) -> Self {
        CliConfig {
            name,
            ..Default::default()
        }
    }

    /// Load configuration from file
    pub fn load_from_file(path: &Path) -> Result<Self, String> {
        let content =
            fs::read_to_string(path).map_err(|e| format!("Failed to read config file: {}", e))?;

        // Try TOML first, then JSON
        if let Ok(config) = toml::from_str::<CliConfig>(&content) {
            return Ok(config);
        }

        serde_json::from_str::<CliConfig>(&content)
            .map_err(|e| format!("Failed to parse config file: {}", e))
    }

    /// Save configuration to file (JSON format)
    pub fn save_to_file(&self, path: &Path) -> Result<(), String> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;

        fs::create_dir_all(path.parent().unwrap_or_else(|| Path::new(".")))
            .map_err(|e| format!("Failed to create parent directory: {}", e))?;

        fs::write(path, json).map_err(|e| format!("Failed to write config file: {}", e))
    }

    /// Save configuration to TOML file
    pub fn save_to_toml(&self, path: &Path) -> Result<(), String> {
        let toml = toml::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize config to TOML: {}", e))?;

        fs::create_dir_all(path.parent().unwrap_or_else(|| Path::new(".")))
            .map_err(|e| format!("Failed to create parent directory: {}", e))?;

        fs::write(path, toml).map_err(|e| format!("Failed to write TOML config: {}", e))
    }

    /// Merge another configuration, overriding current values
    pub fn merge(&mut self, other: CliConfig) {
        if !other.name.is_empty() && other.name != "zoka-cli" {
            self.name = other.name;
        }
        if other.default_wallet.is_some() {
            self.default_wallet = other.default_wallet;
        }
        if other.wallet_storage_path != PathBuf::from("./wallets") {
            self.wallet_storage_path = other.wallet_storage_path;
        }
        if !other.node_endpoint.contains("127.0.0.1:3000") {
            self.node_endpoint = other.node_endpoint;
        }
        if other.tx_timeout_secs != 60 {
            self.tx_timeout_secs = other.tx_timeout_secs;
        }
        if other.enable_wallet_encryption {
            self.enable_wallet_encryption = true;
        }
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.name.is_empty() {
            return Err("Configuration name cannot be empty".to_string());
        }
        if self.tx_timeout_secs == 0 {
            return Err("Transaction timeout must be greater than 0".to_string());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WalletConfig {
    pub name: String,
    pub accounts: HashMap<String, WalletAccount>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WalletAccount {
    pub address: String,
    pub balance: u64,
    pub nonce: u64,
    pub created_at: u64,
}

impl WalletConfig {
    /// Create a new empty wallet configuration
    pub fn new(name: String) -> Self {
        WalletConfig {
            name,
            accounts: HashMap::new(),
        }
    }

    /// Add an account to the wallet
    pub fn add_account(&mut self, address: String, initial_balance: u64) -> Result<(), String> {
        if self.accounts.contains_key(&address) {
            return Err(format!("Account {} already exists", address));
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        self.accounts.insert(
            address.clone(),
            WalletAccount {
                address,
                balance: initial_balance,
                nonce: 0,
                created_at: now,
            },
        );

        Ok(())
    }

    /// Get account by address
    pub fn get_account(&self, address: &str) -> Option<&WalletAccount> {
        self.accounts.get(address)
    }

    /// Get mutable account
    pub fn get_account_mut(&mut self, address: &str) -> Option<&mut WalletAccount> {
        self.accounts.get_mut(address)
    }

    /// List all accounts
    pub fn list_accounts(&self) -> Vec<WalletAccount> {
        self.accounts.values().cloned().collect()
    }

    /// Update account balance
    pub fn update_balance(&mut self, address: &str, balance: u64) -> Result<(), String> {
        match self.get_account_mut(address) {
            Some(account) => {
                account.balance = balance;
                Ok(())
            }
            None => Err(format!("Account {} not found", address)),
        }
    }

    /// Update account nonce
    pub fn increment_nonce(&mut self, address: &str) -> Result<(), String> {
        match self.get_account_mut(address) {
            Some(account) => {
                account.nonce += 1;
                Ok(())
            }
            None => Err(format!("Account {} not found", address)),
        }
    }

    /// Export wallet configuration as JSON
    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self).map_err(|e| format!("Serialization failed: {}", e))
    }

    /// Import wallet configuration from JSON
    pub fn from_json(json: &str) -> Result<Self, String> {
        serde_json::from_str(json).map_err(|e| format!("Deserialization failed: {}", e))
    }

    /// Get total balance across all accounts
    pub fn total_balance(&self) -> u64 {
        self.accounts.values().map(|a| a.balance).sum()
    }
}

/// Wallet manager for managing multiple wallets and their operations
pub struct WalletManager {
    pub wallets: HashMap<String, WalletConfig>,
}

impl WalletManager {
    /// Create a new wallet manager
    pub fn new() -> Self {
        WalletManager {
            wallets: HashMap::new(),
        }
    }

    /// Create a new wallet (wallet_create command)
    pub fn create_wallet(&mut self, name: String) -> Result<WalletConfig, String> {
        if self.wallets.contains_key(&name) {
            return Err(format!("Wallet '{}' already exists", name));
        }
        let wallet = WalletConfig::new(name.clone());
        self.wallets.insert(name, wallet.clone());
        Ok(wallet)
    }

    /// List all wallet names (wallet_list command)
    pub fn list_wallets(&self) -> Vec<String> {
        self.wallets.keys().cloned().collect()
    }

    /// Get wallet by name
    pub fn get_wallet(&self, name: &str) -> Option<&WalletConfig> {
        self.wallets.get(name)
    }

    /// Get mutable wallet
    pub fn get_wallet_mut(&mut self, name: &str) -> Option<&mut WalletConfig> {
        self.wallets.get_mut(name)
    }

    /// Add account to wallet (account_create command)
    pub fn add_account(
        &mut self,
        wallet_name: String,
        address: String,
        initial_balance: u64,
    ) -> Result<(), String> {
        let wallet = self
            .get_wallet_mut(&wallet_name)
            .ok_or_else(|| format!("Wallet '{}' not found", wallet_name))?;
        wallet.add_account(address, initial_balance)?;
        Ok(())
    }

    /// Get account balance (account_balance command)
    pub fn get_account_balance(&self, wallet_name: &str, address: &str) -> Result<u64, String> {
        let wallet = self
            .get_wallet(wallet_name)
            .ok_or_else(|| format!("Wallet '{}' not found", wallet_name))?;
        let account = wallet
            .get_account(address)
            .ok_or_else(|| format!("Account '{}' not found", address))?;
        Ok(account.balance)
    }

    /// List accounts in wallet (account_list command)
    pub fn list_accounts(&self, wallet_name: &str) -> Result<Vec<WalletAccount>, String> {
        let wallet = self
            .get_wallet(wallet_name)
            .ok_or_else(|| format!("Wallet '{}' not found", wallet_name))?;
        Ok(wallet.list_accounts())
    }

    /// Transfer funds between accounts (account_transfer command)
    pub fn transfer(
        &mut self,
        wallet_name: String,
        from_address: String,
        to_address: String,
        amount: u64,
    ) -> Result<(), String> {
        if from_address == to_address {
            return Err("Cannot transfer to the same address".to_string());
        }
        if amount == 0 {
            return Err("Transfer amount must be greater than zero".to_string());
        }

        // Check if both accounts exist and have sufficient balance
        let wallet = self
            .get_wallet_mut(&wallet_name)
            .ok_or_else(|| format!("Wallet '{}' not found", wallet_name))?;

        let from_account = wallet
            .get_account(&from_address)
            .ok_or_else(|| format!("Account '{}' not found", from_address))?;

        if from_account.balance < amount {
            return Err(format!(
                "Insufficient balance: {} < {}",
                from_account.balance, amount
            ));
        }

        let to_account = wallet
            .get_account(&to_address)
            .ok_or_else(|| format!("Account '{}' not found", to_address))?;

        // Perform transfer
        let to_balance = to_account.balance;
        wallet.update_balance(&to_address, to_balance + amount)?;

        let from_balance = wallet
            .get_account(&from_address)
            .ok_or_else(|| format!("Account '{}' not found", from_address))?
            .balance;
        wallet.update_balance(&from_address, from_balance - amount)?;
        wallet.increment_nonce(&from_address)?;

        Ok(())
    }

    /// Export wallet to JSON (wallet_export command)
    pub fn export_wallet(&self, wallet_name: &str) -> Result<String, String> {
        let wallet = self
            .get_wallet(wallet_name)
            .ok_or_else(|| format!("Wallet '{}' not found", wallet_name))?;
        wallet.to_json()
    }

    /// Import wallet from JSON (wallet_import command)
    pub fn import_wallet(&mut self, wallet_name: String, json: &str) -> Result<(), String> {
        if self.wallets.contains_key(&wallet_name) {
            return Err(format!("Wallet '{}' already exists", wallet_name));
        }
        let mut wallet = WalletConfig::from_json(json)?;
        wallet.name = wallet_name.clone();
        self.wallets.insert(wallet_name, wallet);
        Ok(())
    }

    /// Get total balance across all accounts in wallet
    pub fn get_wallet_total_balance(&self, wallet_name: &str) -> Result<u64, String> {
        let wallet = self
            .get_wallet(wallet_name)
            .ok_or_else(|| format!("Wallet '{}' not found", wallet_name))?;
        Ok(wallet.total_balance())
    }

    /// Remove wallet
    pub fn delete_wallet(&mut self, wallet_name: &str) -> Result<(), String> {
        self.wallets
            .remove(wallet_name)
            .ok_or_else(|| format!("Wallet '{}' not found", wallet_name))?;
        Ok(())
    }
}

/// Transaction builder for creating transactions with proper validation
pub struct TransactionBuilder {
    sender: String,
    receiver: String,
    amount: u64,
    nonce: u64,
}

impl TransactionBuilder {
    /// Create a new transaction builder
    pub fn new(sender: String, receiver: String, amount: u64, nonce: u64) -> Self {
        TransactionBuilder {
            sender,
            receiver,
            amount,
            nonce,
        }
    }

    /// Validate the transaction parameters
    pub fn validate(&self) -> Result<(), String> {
        if self.sender.is_empty() {
            return Err("Sender address is empty".to_string());
        }
        if self.receiver.is_empty() {
            return Err("Receiver address is empty".to_string());
        }
        if self.sender == self.receiver {
            return Err("Sender and receiver cannot be the same".to_string());
        }
        if self.amount == 0 {
            return Err("Amount must be greater than zero".to_string());
        }
        Ok(())
    }

    /// Build the transaction
    pub fn build(self) -> Result<Transaction, String> {
        self.validate()?;

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Ok(Transaction::new(
            self.sender,
            self.receiver,
            self.amount,
            self.nonce,
            timestamp,
            Signature(vec![]),
        ))
    }
}

/// Transaction status tracking
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum TransactionStatus {
    Pending,
    Mined,
    Confirmed,
    Failed(String),
}

/// Transaction manager for handling transaction operations
pub struct TransactionManager {
    pub transactions: HashMap<String, (Transaction, TransactionStatus)>,
}

impl TransactionManager {
    /// Create a new transaction manager
    pub fn new() -> Self {
        TransactionManager {
            transactions: HashMap::new(),
        }
    }

    /// Send transaction (tx-send command)
    pub fn send_transaction(
        &mut self,
        tx_id: String,
        transaction: Transaction,
    ) -> Result<(), String> {
        if self.transactions.contains_key(&tx_id) {
            return Err(format!("Transaction {} already exists", tx_id));
        }
        self.transactions
            .insert(tx_id, (transaction, TransactionStatus::Pending));
        Ok(())
    }

    /// Get transaction status (tx-status command)
    pub fn get_transaction_status(&self, tx_id: &str) -> Result<TransactionStatus, String> {
        self.transactions
            .get(tx_id)
            .map(|(_, status)| status.clone())
            .ok_or_else(|| format!("Transaction {} not found", tx_id))
    }

    /// Update transaction status
    pub fn update_transaction_status(
        &mut self,
        tx_id: &str,
        new_status: TransactionStatus,
    ) -> Result<(), String> {
        match self.transactions.get_mut(tx_id) {
            Some((_, status)) => {
                *status = new_status;
                Ok(())
            }
            None => Err(format!("Transaction {} not found", tx_id)),
        }
    }

    /// Get transaction details (tx-details command)
    pub fn get_transaction_details(
        &self,
        tx_id: &str,
    ) -> Result<(Transaction, TransactionStatus), String> {
        self.transactions
            .get(tx_id)
            .map(|(tx, status)| (tx.clone(), status.clone()))
            .ok_or_else(|| format!("Transaction {} not found", tx_id))
    }

    /// List all transactions
    pub fn list_transactions(&self) -> Vec<(String, TransactionStatus)> {
        self.transactions
            .iter()
            .map(|(id, (_, status))| (id.clone(), status.clone()))
            .collect()
    }

    /// List pending transactions (tx-list-pending command)
    pub fn list_pending(&self) -> Vec<String> {
        self.transactions
            .iter()
            .filter(|(_, (_, status))| *status == TransactionStatus::Pending)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Get transaction count
    pub fn count_transactions(&self) -> usize {
        self.transactions.len()
    }

    /// Count pending transactions
    pub fn count_pending(&self) -> usize {
        self.transactions
            .values()
            .filter(|(_, status)| *status == TransactionStatus::Pending)
            .count()
    }

    /// Cancel transaction (tx-cancel command)
    pub fn cancel_transaction(&mut self, tx_id: &str) -> Result<(), String> {
        match self.transactions.get_mut(tx_id) {
            Some((_, status)) if *status == TransactionStatus::Pending => {
                *status = TransactionStatus::Failed("Cancelled by user".to_string());
                Ok(())
            }
            Some((_, status)) => Err(format!(
                "Cannot cancel transaction with status: {:?}",
                status
            )),
            None => Err(format!("Transaction {} not found", tx_id)),
        }
    }
}

impl Default for TransactionManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Block query results
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlockInfo {
    pub height: u64,
    pub hash: String,
    pub timestamp: u64,
    pub transaction_count: usize,
}

/// Block queries and chain status
pub struct BlockQuery {
    pub blocks: HashMap<u64, BlockInfo>,
}

impl BlockQuery {
    /// Create a new block query manager
    pub fn new() -> Self {
        BlockQuery {
            blocks: HashMap::new(),
        }
    }

    /// Add block info (for tracking)
    pub fn add_block(&mut self, height: u64, info: BlockInfo) -> Result<(), String> {
        if self.blocks.contains_key(&height) {
            return Err(format!("Block at height {} already exists", height));
        }
        self.blocks.insert(height, info);
        Ok(())
    }

    /// Get block by height (block-get command)
    pub fn get_block(&self, height: u64) -> Result<BlockInfo, String> {
        self.blocks
            .get(&height)
            .cloned()
            .ok_or_else(|| format!("Block at height {} not found", height))
    }

    /// Get latest block height (chain-status command)
    pub fn get_latest_height(&self) -> u64 {
        self.blocks.keys().max().copied().unwrap_or(0)
    }

    /// Get chain status (chain-status command)
    pub fn get_chain_status(&self) -> (u64, usize) {
        let height = self.get_latest_height();
        let total_blocks = self.blocks.len();
        (height, total_blocks)
    }

    /// List recent blocks (block-list command)
    pub fn list_recent_blocks(&self, limit: usize) -> Vec<BlockInfo> {
        let mut blocks: Vec<_> = self.blocks.values().cloned().collect();
        blocks.sort_by(|a, b| b.height.cmp(&a.height));
        blocks.into_iter().take(limit).collect()
    }

    /// Count transactions in blocks
    pub fn count_all_transactions(&self) -> usize {
        self.blocks.values().map(|b| b.transaction_count).sum()
    }

    /// Get average transactions per block
    pub fn avg_transactions_per_block(&self) -> f64 {
        if self.blocks.is_empty() {
            return 0.0;
        }
        self.count_all_transactions() as f64 / self.blocks.len() as f64
    }
}

impl Default for BlockQuery {
    fn default() -> Self {
        Self::new()
    }
}

/// Debug and diagnostic tools
pub struct DebugTools {
    pub created_at: u64,
}

impl DebugTools {
    /// Create new debug tools instance
    pub fn new() -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        DebugTools { created_at: now }
    }

    /// Get uptime in seconds
    pub fn get_uptime(&self) -> u64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        now.saturating_sub(self.created_at)
    }

    /// Get diagnostic information
    pub fn diagnose(&self) -> String {
        format!(
            "Uptime: {}s | Timestamp: {}",
            self.get_uptime(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
        )
    }

    /// Reset diagnostics
    pub fn reset(&mut self) {
        self.created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
    }
}

impl Default for DebugTools {
    fn default() -> Self {
        Self::new()
    }
}

/// CLI command types
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum CliCommand {
    // Wallet commands
    WalletCreate(String),
    WalletList,
    WalletImport {
        name: String,
        path: String,
    },
    WalletExport {
        name: String,
        path: String,
    },
    WalletDelete(String),

    // Account commands
    AccountCreate {
        wallet: String,
        address: String,
    },
    AccountBalance {
        wallet: String,
        address: String,
    },
    AccountList(String),
    AccountTransfer {
        wallet: String,
        from: String,
        to: String,
        amount: u64,
    },

    // Transaction commands
    TxSend {
        tx_id: String,
        sender: String,
        receiver: String,
        amount: u64,
    },
    TxStatus(String),
    TxCancel(String),
    TxList,
    TxListPending,

    // Block commands
    BlockGet(u64),
    BlockList {
        limit: usize,
    },
    ChainStatus,

    // Debug commands
    DebugUptime,
    DebugDiagnostic,
}

/// Command parser for CLI arguments
pub struct CommandParser;

impl CommandParser {
    /// Parse command string into CliCommand
    pub fn parse(input: &str) -> Result<CliCommand, String> {
        let parts: Vec<&str> = input.trim().split_whitespace().collect();

        if parts.is_empty() {
            return Err("Empty command".to_string());
        }

        match parts[0] {
            "wallet-create" => {
                if parts.len() < 2 {
                    return Err("wallet-create requires a name".to_string());
                }
                Ok(CliCommand::WalletCreate(parts[1].to_string()))
            }
            "wallet-list" => Ok(CliCommand::WalletList),
            "wallet-delete" => {
                if parts.len() < 2 {
                    return Err("wallet-delete requires a name".to_string());
                }
                Ok(CliCommand::WalletDelete(parts[1].to_string()))
            }
            "account-create" => {
                if parts.len() < 3 {
                    return Err("account-create requires wallet and address".to_string());
                }
                Ok(CliCommand::AccountCreate {
                    wallet: parts[1].to_string(),
                    address: parts[2].to_string(),
                })
            }
            "account-balance" => {
                if parts.len() < 3 {
                    return Err("account-balance requires wallet and address".to_string());
                }
                Ok(CliCommand::AccountBalance {
                    wallet: parts[1].to_string(),
                    address: parts[2].to_string(),
                })
            }
            "account-list" => {
                if parts.len() < 2 {
                    return Err("account-list requires a wallet name".to_string());
                }
                Ok(CliCommand::AccountList(parts[1].to_string()))
            }
            "tx-list" => Ok(CliCommand::TxList),
            "tx-list-pending" => Ok(CliCommand::TxListPending),
            "block-list" => {
                let limit = if parts.len() > 1 {
                    parts[1].parse().unwrap_or(10)
                } else {
                    10
                };
                Ok(CliCommand::BlockList { limit })
            }
            "chain-status" => Ok(CliCommand::ChainStatus),
            "debug-uptime" => Ok(CliCommand::DebugUptime),
            "debug-diagnostic" => Ok(CliCommand::DebugDiagnostic),
            _ => Err(format!("Unknown command: {}", parts[0])),
        }
    }

    /// Get command help text
    pub fn help() -> String {
        vec![
            "=== ZOKA CLI Commands ===",
            "",
            "Wallet Commands:",
            "  wallet-create <name>",
            "  wallet-list",
            "  wallet-delete <name>",
            "",
            "Account Commands:",
            "  account-create <wallet> <address>",
            "  account-balance <wallet> <address>",
            "  account-list <wallet>",
            "",
            "Transaction Commands:",
            "  tx-list",
            "  tx-list-pending",
            "",
            "Block Commands:",
            "  block-list [limit]",
            "  chain-status",
            "",
            "Debug Commands:",
            "  debug-uptime",
            "  debug-diagnostic",
        ]
        .join("\n")
    }
}

/// Wallet backup and restore
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WalletBackup {
    pub name: String,
    pub timestamp: u64,
    pub wallet_data: String, // JSON serialized wallet
    pub checksum: u64,
}

impl WalletBackup {
    /// Create a backup from wallet configuration
    pub fn from_wallet(wallet: &WalletConfig) -> Result<Self, String> {
        let json = wallet.to_json()?;

        // Simple checksum (sum of bytes)
        let checksum: u64 = json.bytes().map(|b| b as u64).sum();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Ok(WalletBackup {
            name: wallet.name.clone(),
            timestamp: now,
            wallet_data: json,
            checksum,
        })
    }

    /// Restore wallet from backup
    pub fn to_wallet(&self) -> Result<WalletConfig, String> {
        // Verify checksum
        let computed_checksum: u64 = self.wallet_data.bytes().map(|b| b as u64).sum();
        if computed_checksum != self.checksum {
            return Err("Checksum verification failed - backup may be corrupted".to_string());
        }

        WalletConfig::from_json(&self.wallet_data)
    }

    /// Export backup to JSON
    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self).map_err(|e| format!("Serialization failed: {}", e))
    }

    /// Import backup from JSON
    pub fn from_json(json: &str) -> Result<Self, String> {
        serde_json::from_str(json).map_err(|e| format!("Deserialization failed: {}", e))
    }
}

/// Chain sync status tracking
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncStatus {
    pub synced_height: u64,
    pub target_height: u64,
    pub is_syncing: bool,
    pub last_sync_time: u64,
}

impl Default for SyncStatus {
    fn default() -> Self {
        SyncStatus {
            synced_height: 0,
            target_height: 0,
            is_syncing: false,
            last_sync_time: 0,
        }
    }
}

/// Chain synchronization manager
pub struct ChainSync {
    pub sync_status: SyncStatus,
}

impl ChainSync {
    pub fn new() -> Self {
        ChainSync {
            sync_status: SyncStatus::default(),
        }
    }

    /// Start synchronization
    pub fn start_sync(&mut self, target_height: u64) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        self.sync_status.is_syncing = true;
        self.sync_status.target_height = target_height;
        self.sync_status.last_sync_time = now;
    }

    /// Update sync progress
    pub fn update_progress(&mut self, current_height: u64) -> bool {
        self.sync_status.synced_height = current_height;

        if current_height >= self.sync_status.target_height {
            self.sync_status.is_syncing = false;
            return true;
        }
        false
    }

    /// Get sync percentage
    pub fn get_sync_percentage(&self) -> f64 {
        if self.sync_status.target_height == 0 {
            return 0.0;
        }
        (self.sync_status.synced_height as f64 / self.sync_status.target_height as f64) * 100.0
    }

    /// Get sync progress
    pub fn get_progress(&self) -> (u64, u64) {
        (
            self.sync_status.synced_height,
            self.sync_status.target_height,
        )
    }
}

impl Default for ChainSync {
    fn default() -> Self {
        Self::new()
    }
}

/// Clap-based CLI argument parser for production use
#[derive(Parser, Debug)]
#[command(name = "zoka")]
#[command(about = "ZOKA: 100% Private Blockchain CLI", long_about = None)]
#[command(version = "0.1.0")]
pub struct CliArgs {
    #[command(subcommand)]
    pub command: Option<CliSubcommand>,
}

#[derive(Subcommand, Debug)]
pub enum CliSubcommand {
    /// Wallet operations
    Wallet {
        #[command(subcommand)]
        action: WalletAction,
    },
    /// Account operations
    Account {
        #[command(subcommand)]
        action: AccountAction,
    },
    /// Transaction operations
    Transaction {
        #[command(subcommand)]
        action: TransactionAction,
    },
    /// Block and chain operations
    Chain {
        #[command(subcommand)]
        action: ChainAction,
    },
    /// Debug and diagnostics
    Debug {
        #[command(subcommand)]
        action: DebugAction,
    },
}

#[derive(Subcommand, Debug)]
pub enum WalletAction {
    /// Create a new wallet
    Create {
        /// Wallet name
        #[arg(value_name = "NAME")]
        name: String,
    },
    /// List all wallets
    List,
    /// Delete a wallet
    Delete {
        /// Wallet name to delete
        #[arg(value_name = "NAME")]
        name: String,
    },
    /// Export wallet to file
    Export {
        /// Wallet name
        #[arg(value_name = "WALLET")]
        wallet: String,
        /// Output file path
        #[arg(value_name = "PATH")]
        path: String,
    },
    /// Import wallet from file
    Import {
        /// Wallet name
        #[arg(value_name = "NAME")]
        name: String,
        /// Input file path
        #[arg(value_name = "PATH")]
        path: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum AccountAction {
    /// Create a new account in a wallet
    Create {
        /// Wallet name
        #[arg(value_name = "WALLET")]
        wallet: String,
        /// Account address
        #[arg(value_name = "ADDRESS")]
        address: String,
    },
    /// Get account balance
    Balance {
        /// Wallet name
        #[arg(value_name = "WALLET")]
        wallet: String,
        /// Account address
        #[arg(value_name = "ADDRESS")]
        address: String,
    },
    /// List accounts in wallet
    List {
        /// Wallet name
        #[arg(value_name = "WALLET")]
        wallet: String,
    },
    /// Transfer funds between accounts
    Transfer {
        /// Wallet name
        #[arg(value_name = "WALLET")]
        wallet: String,
        /// Source address
        #[arg(value_name = "FROM")]
        from: String,
        /// Destination address
        #[arg(value_name = "TO")]
        to: String,
        /// Amount to transfer
        #[arg(value_name = "AMOUNT")]
        amount: u64,
    },
}

#[derive(Subcommand, Debug)]
pub enum TransactionAction {
    /// Send a transaction
    Send {
        /// Transaction ID
        #[arg(value_name = "ID")]
        id: String,
        /// Sender address
        #[arg(value_name = "FROM")]
        from: String,
        /// Receiver address
        #[arg(value_name = "TO")]
        to: String,
        /// Amount
        #[arg(value_name = "AMOUNT")]
        amount: u64,
    },
    /// Get transaction status
    Status {
        /// Transaction ID/hash
        #[arg(value_name = "HASH")]
        hash: String,
    },
    /// Cancel a pending transaction
    Cancel {
        /// Transaction ID/hash
        #[arg(value_name = "HASH")]
        hash: String,
    },
    /// List all transactions
    List,
    /// List pending transactions
    ListPending,
}

#[derive(Subcommand, Debug)]
pub enum ChainAction {
    /// Get block by height
    Block {
        /// Block height
        #[arg(value_name = "HEIGHT")]
        height: u64,
    },
    /// List recent blocks
    ListBlocks {
        /// Number of blocks to list
        #[arg(value_name = "LIMIT", default_value = "10")]
        limit: usize,
    },
    /// Get chain status
    Status,
}

#[derive(Subcommand, Debug)]
pub enum DebugAction {
    /// Get uptime
    Uptime,
    /// Get diagnostic information
    Diagnostic,
}

/// Convert clap CliArgs to internal CliCommand
impl From<CliArgs> for Result<CliCommand, String> {
    fn from(args: CliArgs) -> Self {
        match args.command {
            Some(CliSubcommand::Wallet { action }) => match action {
                WalletAction::Create { name } => Ok(CliCommand::WalletCreate(name)),
                WalletAction::List => Ok(CliCommand::WalletList),
                WalletAction::Delete { name } => Ok(CliCommand::WalletDelete(name)),
                WalletAction::Export { wallet, path } => {
                    Ok(CliCommand::WalletExport { name: wallet, path })
                }
                WalletAction::Import { name, path } => Ok(CliCommand::WalletImport { name, path }),
            },
            Some(CliSubcommand::Account { action }) => match action {
                AccountAction::Create { wallet, address } => {
                    Ok(CliCommand::AccountCreate { wallet, address })
                }
                AccountAction::Balance { wallet, address } => {
                    Ok(CliCommand::AccountBalance { wallet, address })
                }
                AccountAction::List { wallet } => Ok(CliCommand::AccountList(wallet)),
                AccountAction::Transfer {
                    wallet,
                    from,
                    to,
                    amount,
                } => Ok(CliCommand::AccountTransfer {
                    wallet,
                    from,
                    to,
                    amount,
                }),
            },
            Some(CliSubcommand::Transaction { action }) => match action {
                TransactionAction::Send {
                    id,
                    from,
                    to,
                    amount,
                } => Ok(CliCommand::TxSend {
                    tx_id: id,
                    sender: from,
                    receiver: to,
                    amount,
                }),
                TransactionAction::Status { hash } => Ok(CliCommand::TxStatus(hash)),
                TransactionAction::Cancel { hash } => Ok(CliCommand::TxCancel(hash)),
                TransactionAction::List => Ok(CliCommand::TxList),
                TransactionAction::ListPending => Ok(CliCommand::TxListPending),
            },
            Some(CliSubcommand::Chain { action }) => match action {
                ChainAction::Block { height } => Ok(CliCommand::BlockGet(height)),
                ChainAction::ListBlocks { limit } => Ok(CliCommand::BlockList { limit }),
                ChainAction::Status => Ok(CliCommand::ChainStatus),
            },
            Some(CliSubcommand::Debug { action }) => match action {
                DebugAction::Uptime => Ok(CliCommand::DebugUptime),
                DebugAction::Diagnostic => Ok(CliCommand::DebugDiagnostic),
            },
            None => Err("No command specified".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wallet_manager_creation() {
        let manager = WalletManager::new();
        assert_eq!(manager.list_wallets().len(), 0);
    }

    #[test]
    fn test_wallet_manager_create_wallet() {
        let mut manager = WalletManager::new();
        let result = manager.create_wallet("primary".to_string());
        assert!(result.is_ok());
        assert_eq!(manager.list_wallets().len(), 1);
    }

    #[test]
    fn test_wallet_manager_create_duplicate() {
        let mut manager = WalletManager::new();
        assert!(manager.create_wallet("primary".to_string()).is_ok());
        let result = manager.create_wallet("primary".to_string());
        assert!(result.is_err());
    }

    #[test]
    fn test_wallet_manager_add_account() {
        let mut manager = WalletManager::new();
        manager.create_wallet("primary".to_string()).unwrap();
        let result = manager.add_account("primary".to_string(), "alice".to_string(), 1000);
        assert!(result.is_ok());
    }

    #[test]
    fn test_wallet_manager_add_account_nonexistent() {
        let mut manager = WalletManager::new();
        let result = manager.add_account("nonexistent".to_string(), "alice".to_string(), 1000);
        assert!(result.is_err());
    }

    #[test]
    fn test_wallet_manager_get_balance() {
        let mut manager = WalletManager::new();
        manager.create_wallet("primary".to_string()).unwrap();
        manager
            .add_account("primary".to_string(), "alice".to_string(), 1000)
            .unwrap();
        let balance = manager.get_account_balance("primary", "alice").unwrap();
        assert_eq!(balance, 1000);
    }

    #[test]
    fn test_wallet_manager_list_accounts() {
        let mut manager = WalletManager::new();
        manager.create_wallet("primary".to_string()).unwrap();
        manager
            .add_account("primary".to_string(), "alice".to_string(), 1000)
            .unwrap();
        manager
            .add_account("primary".to_string(), "bob".to_string(), 500)
            .unwrap();
        let accounts = manager.list_accounts("primary").unwrap();
        assert_eq!(accounts.len(), 2);
    }

    #[test]
    fn test_wallet_manager_transfer() {
        let mut manager = WalletManager::new();
        manager.create_wallet("primary".to_string()).unwrap();
        manager
            .add_account("primary".to_string(), "alice".to_string(), 1000)
            .unwrap();
        manager
            .add_account("primary".to_string(), "bob".to_string(), 500)
            .unwrap();
        let result = manager.transfer(
            "primary".to_string(),
            "alice".to_string(),
            "bob".to_string(),
            200,
        );
        assert!(result.is_ok());
        let alice_balance = manager.get_account_balance("primary", "alice").unwrap();
        let bob_balance = manager.get_account_balance("primary", "bob").unwrap();
        assert_eq!(alice_balance, 800);
        assert_eq!(bob_balance, 700);
    }

    #[test]
    fn test_wallet_manager_transfer_insufficient_balance() {
        let mut manager = WalletManager::new();
        manager.create_wallet("primary".to_string()).unwrap();
        manager
            .add_account("primary".to_string(), "alice".to_string(), 100)
            .unwrap();
        manager
            .add_account("primary".to_string(), "bob".to_string(), 500)
            .unwrap();
        let result = manager.transfer(
            "primary".to_string(),
            "alice".to_string(),
            "bob".to_string(),
            200,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_wallet_manager_export_import() {
        let mut manager = WalletManager::new();
        manager.create_wallet("primary".to_string()).unwrap();
        manager
            .add_account("primary".to_string(), "alice".to_string(), 1000)
            .unwrap();
        let json = manager.export_wallet("primary").unwrap();

        let mut manager2 = WalletManager::new();
        assert!(manager2
            .import_wallet("restored".to_string(), &json)
            .is_ok());
        let balance = manager2.get_account_balance("restored", "alice").unwrap();
        assert_eq!(balance, 1000);
    }

    #[test]
    fn test_wallet_manager_delete_wallet() {
        let mut manager = WalletManager::new();
        manager.create_wallet("primary".to_string()).unwrap();
        assert_eq!(manager.list_wallets().len(), 1);
        assert!(manager.delete_wallet("primary").is_ok());
        assert_eq!(manager.list_wallets().len(), 0);
    }

    #[test]
    fn test_wallet_creation() {
        let wallet = WalletConfig::new("test_wallet".to_string());
        assert_eq!(wallet.name, "test_wallet");
        assert_eq!(wallet.accounts.len(), 0);
    }

    #[test]
    fn test_wallet_add_account() {
        let mut wallet = WalletConfig::new("test_wallet".to_string());
        assert!(wallet.add_account("alice".to_string(), 1000).is_ok());
        assert_eq!(wallet.accounts.len(), 1);
    }

    #[test]
    fn test_wallet_duplicate_account() {
        let mut wallet = WalletConfig::new("test_wallet".to_string());
        assert!(wallet.add_account("alice".to_string(), 1000).is_ok());
        let result = wallet.add_account("alice".to_string(), 500);
        assert!(result.is_err());
    }

    #[test]
    fn test_wallet_list_accounts() {
        let mut wallet = WalletConfig::new("test_wallet".to_string());
        wallet.add_account("alice".to_string(), 1000).unwrap();
        wallet.add_account("bob".to_string(), 500).unwrap();
        let accounts = wallet.list_accounts();
        assert_eq!(accounts.len(), 2);
    }

    #[test]
    fn test_wallet_total_balance() {
        let mut wallet = WalletConfig::new("test_wallet".to_string());
        wallet.add_account("alice".to_string(), 1000).unwrap();
        wallet.add_account("bob".to_string(), 500).unwrap();
        assert_eq!(wallet.total_balance(), 1500);
    }

    #[test]
    fn test_transaction_builder_valid() {
        let tx_result =
            TransactionBuilder::new("alice".to_string(), "bob".to_string(), 100, 1).build();
        assert!(tx_result.is_ok());
        let tx = tx_result.unwrap();
        assert_eq!(tx.sender, "alice");
        assert_eq!(tx.amount, 100);
    }

    #[test]
    fn test_transaction_builder_self_transfer() {
        let result =
            TransactionBuilder::new("alice".to_string(), "alice".to_string(), 100, 1).validate();
        assert!(result.is_err());
    }

    #[test]
    fn test_transaction_builder_zero_amount() {
        let result =
            TransactionBuilder::new("alice".to_string(), "bob".to_string(), 0, 1).validate();
        assert!(result.is_err());
    }

    #[test]
    fn test_wallet_serialization() {
        let mut wallet = WalletConfig::new("test".to_string());
        wallet.add_account("alice".to_string(), 1000).unwrap();
        let json = wallet.to_json().unwrap();
        let restored = WalletConfig::from_json(&json).unwrap();
        assert_eq!(restored.name, "test");
        assert_eq!(restored.accounts.len(), 1);
    }

    // === TransactionManager Tests ===

    #[test]
    fn test_transaction_manager_creation() {
        let manager = TransactionManager::new();
        assert_eq!(manager.count_transactions(), 0);
    }

    #[test]
    fn test_transaction_manager_send() {
        let mut manager = TransactionManager::new();
        let tx = TransactionBuilder::new("alice".to_string(), "bob".to_string(), 100, 0)
            .build()
            .unwrap();

        let result = manager.send_transaction("tx1".to_string(), tx);
        assert!(result.is_ok());
        assert_eq!(manager.count_transactions(), 1);
    }

    #[test]
    fn test_transaction_manager_duplicate_send() {
        let mut manager = TransactionManager::new();
        let tx = TransactionBuilder::new("alice".to_string(), "bob".to_string(), 100, 0)
            .build()
            .unwrap();

        manager
            .send_transaction("tx1".to_string(), tx.clone())
            .unwrap();
        let result = manager.send_transaction("tx1".to_string(), tx);
        assert!(result.is_err());
    }

    #[test]
    fn test_transaction_manager_get_status() {
        let mut manager = TransactionManager::new();
        let tx = TransactionBuilder::new("alice".to_string(), "bob".to_string(), 100, 0)
            .build()
            .unwrap();

        manager.send_transaction("tx1".to_string(), tx).unwrap();
        let status = manager.get_transaction_status("tx1").unwrap();
        assert_eq!(status, TransactionStatus::Pending);
    }

    #[test]
    fn test_transaction_manager_update_status() {
        let mut manager = TransactionManager::new();
        let tx = TransactionBuilder::new("alice".to_string(), "bob".to_string(), 100, 0)
            .build()
            .unwrap();

        manager.send_transaction("tx1".to_string(), tx).unwrap();
        manager
            .update_transaction_status("tx1", TransactionStatus::Mined)
            .unwrap();
        let status = manager.get_transaction_status("tx1").unwrap();
        assert_eq!(status, TransactionStatus::Mined);
    }

    #[test]
    fn test_transaction_manager_list_pending() {
        let mut manager = TransactionManager::new();
        for i in 0..3 {
            let tx = TransactionBuilder::new("alice".to_string(), "bob".to_string(), 100, i)
                .build()
                .unwrap();
            manager.send_transaction(format!("tx{}", i), tx).unwrap();
        }

        let pending = manager.list_pending();
        assert_eq!(pending.len(), 3);
    }

    #[test]
    fn test_transaction_manager_cancel() {
        let mut manager = TransactionManager::new();
        let tx = TransactionBuilder::new("alice".to_string(), "bob".to_string(), 100, 0)
            .build()
            .unwrap();

        manager.send_transaction("tx1".to_string(), tx).unwrap();
        assert!(manager.cancel_transaction("tx1").is_ok());

        let status = manager.get_transaction_status("tx1").unwrap();
        assert_eq!(
            status,
            TransactionStatus::Failed("Cancelled by user".to_string())
        );
    }

    // === BlockQuery Tests ===

    #[test]
    fn test_block_query_creation() {
        let query = BlockQuery::new();
        assert_eq!(query.get_latest_height(), 0);
    }

    #[test]
    fn test_block_query_add_block() {
        let mut query = BlockQuery::new();
        let block = BlockInfo {
            height: 0,
            hash: "hash0".to_string(),
            timestamp: 1000,
            transaction_count: 5,
        };
        assert!(query.add_block(0, block).is_ok());
    }

    #[test]
    fn test_block_query_get_block() {
        let mut query = BlockQuery::new();
        let block = BlockInfo {
            height: 1,
            hash: "hash1".to_string(),
            timestamp: 1100,
            transaction_count: 3,
        };
        query.add_block(1, block.clone()).unwrap();
        let retrieved = query.get_block(1).unwrap();
        assert_eq!(retrieved.height, 1);
    }

    #[test]
    fn test_block_query_chain_status() {
        let mut query = BlockQuery::new();
        for i in 0..5 {
            let block = BlockInfo {
                height: i,
                hash: format!("hash{}", i),
                timestamp: 1000 + i * 100,
                transaction_count: 5,
            };
            query.add_block(i, block).unwrap();
        }

        let (height, count) = query.get_chain_status();
        assert_eq!(height, 4);
        assert_eq!(count, 5);
    }

    #[test]
    fn test_block_query_list_recent() {
        let mut query = BlockQuery::new();
        for i in 0..10 {
            let block = BlockInfo {
                height: i,
                hash: format!("hash{}", i),
                timestamp: 1000 + i * 100,
                transaction_count: 5,
            };
            query.add_block(i, block).unwrap();
        }

        let recent = query.list_recent_blocks(3);
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].height, 9); // Most recent
    }

    #[test]
    fn test_block_query_avg_transactions() {
        let mut query = BlockQuery::new();
        let block1 = BlockInfo {
            height: 0,
            hash: "h0".to_string(),
            timestamp: 1000,
            transaction_count: 10,
        };
        let block2 = BlockInfo {
            height: 1,
            hash: "h1".to_string(),
            timestamp: 1100,
            transaction_count: 20,
        };

        query.add_block(0, block1).unwrap();
        query.add_block(1, block2).unwrap();

        let avg = query.avg_transactions_per_block();
        assert_eq!(avg, 15.0);
    }

    // ===  DebugTools Tests ===

    #[test]
    fn test_debug_tools_uptime() {
        let tools = DebugTools::new();
        let uptime1 = tools.get_uptime();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let uptime2 = tools.get_uptime();
        assert!(uptime2 >= uptime1);
    }

    #[test]
    fn test_debug_tools_diagnose() {
        let tools = DebugTools::new();
        let diag = tools.diagnose();
        assert!(diag.contains("Uptime:"));
    }

    #[test]
    fn test_debug_tools_reset() {
        let mut tools = DebugTools::new();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let uptime_before = tools.get_uptime();
        tools.reset();
        let uptime_after = tools.get_uptime();
        assert!(uptime_after < uptime_before || uptime_after == 0);
    }

    // === CommandParser Tests ===

    #[test]
    fn test_command_parser_wallet_create() {
        let cmd = CommandParser::parse("wallet-create primary").unwrap();
        assert_eq!(cmd, CliCommand::WalletCreate("primary".to_string()));
    }

    #[test]
    fn test_command_parser_wallet_list() {
        let cmd = CommandParser::parse("wallet-list").unwrap();
        assert_eq!(cmd, CliCommand::WalletList);
    }

    #[test]
    fn test_command_parser_account_create() {
        let cmd = CommandParser::parse("account-create primary alice").unwrap();
        assert_eq!(
            cmd,
            CliCommand::AccountCreate {
                wallet: "primary".to_string(),
                address: "alice".to_string()
            }
        );
    }

    #[test]
    fn test_command_parser_invalid_command() {
        let result = CommandParser::parse("invalid-command");
        assert!(result.is_err());
    }

    #[test]
    fn test_command_parser_help() {
        let help = CommandParser::help();
        assert!(help.contains("wallet-create"));
        assert!(help.contains("account-balance"));
    }

    // === WalletBackup Tests ===

    #[test]
    fn test_wallet_backup_creation() {
        let mut wallet = WalletConfig::new("test".to_string());
        wallet.add_account("alice".to_string(), 1000).unwrap();

        let backup = WalletBackup::from_wallet(&wallet).unwrap();
        assert_eq!(backup.name, "test");
        assert!(backup.timestamp > 0);
    }

    #[test]
    fn test_wallet_backup_restore() {
        let mut wallet = WalletConfig::new("test".to_string());
        wallet.add_account("alice".to_string(), 1000).unwrap();

        let backup = WalletBackup::from_wallet(&wallet).unwrap();
        let restored = backup.to_wallet().unwrap();

        assert_eq!(restored.name, "test");
        assert_eq!(restored.accounts.len(), 1);
    }

    #[test]
    fn test_wallet_backup_checksum() {
        let mut wallet = WalletConfig::new("test".to_string());
        wallet.add_account("alice".to_string(), 1000).unwrap();

        let mut backup = WalletBackup::from_wallet(&wallet).unwrap();
        backup.checksum = 999; // Corrupt checksum

        let result = backup.to_wallet();
        assert!(result.is_err());
    }

    #[test]
    fn test_wallet_backup_serialization() {
        let mut wallet = WalletConfig::new("test".to_string());
        wallet.add_account("alice".to_string(), 1000).unwrap();

        let backup = WalletBackup::from_wallet(&wallet).unwrap();
        let json = backup.to_json().unwrap();
        let restored = WalletBackup::from_json(&json).unwrap();

        assert_eq!(restored.name, backup.name);
        assert_eq!(restored.checksum, backup.checksum);
    }

    // === ChainSync Tests ===

    #[test]
    fn test_chain_sync_creation() {
        let sync = ChainSync::new();
        assert!(!sync.sync_status.is_syncing);
        assert_eq!(sync.sync_status.synced_height, 0);
    }

    #[test]
    fn test_chain_sync_start() {
        let mut sync = ChainSync::new();
        sync.start_sync(100);

        assert!(sync.sync_status.is_syncing);
        assert_eq!(sync.sync_status.target_height, 100);
    }

    #[test]
    fn test_chain_sync_update_progress() {
        let mut sync = ChainSync::new();
        sync.start_sync(100);

        assert!(!sync.update_progress(50));
        assert_eq!(sync.sync_status.synced_height, 50);

        let done = sync.update_progress(100);
        assert!(done);
        assert!(!sync.sync_status.is_syncing);
    }

    #[test]
    fn test_chain_sync_percentage() {
        let mut sync = ChainSync::new();
        sync.start_sync(100);

        sync.update_progress(25);
        let percentage = sync.get_sync_percentage();
        assert_eq!(percentage, 25.0);

        sync.update_progress(50);
        let percentage = sync.get_sync_percentage();
        assert_eq!(percentage, 50.0);
    }

    #[test]
    fn test_chain_sync_get_progress() {
        let mut sync = ChainSync::new();
        sync.start_sync(100);
        sync.update_progress(75);

        let (synced, target) = sync.get_progress();
        assert_eq!(synced, 75);
        assert_eq!(target, 100);
    }

    #[test]
    fn test_clap_wallet_create_conversion() {
        let args = CliArgs {
            command: Some(CliSubcommand::Wallet {
                action: WalletAction::Create {
                    name: "my-wallet".to_string(),
                },
            }),
        };

        let result: Result<CliCommand, String> = args.into();
        assert!(result.is_ok());
        let cmd = result.unwrap();
        assert_eq!(cmd, CliCommand::WalletCreate("my-wallet".to_string()));
    }

    #[test]
    fn test_clap_wallet_list_conversion() {
        let args = CliArgs {
            command: Some(CliSubcommand::Wallet {
                action: WalletAction::List,
            }),
        };

        let result: Result<CliCommand, String> = args.into();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), CliCommand::WalletList);
    }

    #[test]
    fn test_clap_account_create_conversion() {
        let args = CliArgs {
            command: Some(CliSubcommand::Account {
                action: AccountAction::Create {
                    wallet: "primary".to_string(),
                    address: "alice".to_string(),
                },
            }),
        };

        let result: Result<CliCommand, String> = args.into();
        assert!(result.is_ok());

        match result.unwrap() {
            CliCommand::AccountCreate { wallet, address } => {
                assert_eq!(wallet, "primary");
                assert_eq!(address, "alice");
            }
            _ => panic!("Wrong command type"),
        }
    }

    #[test]
    fn test_clap_transaction_send_conversion() {
        let args = CliArgs {
            command: Some(CliSubcommand::Transaction {
                action: TransactionAction::Send {
                    id: "tx-1".to_string(),
                    from: "alice".to_string(),
                    to: "bob".to_string(),
                    amount: 100,
                },
            }),
        };

        let result: Result<CliCommand, String> = args.into();
        assert!(result.is_ok());

        match result.unwrap() {
            CliCommand::TxSend {
                tx_id,
                sender,
                receiver,
                amount,
            } => {
                assert_eq!(tx_id, "tx-1");
                assert_eq!(sender, "alice");
                assert_eq!(receiver, "bob");
                assert_eq!(amount, 100);
            }
            _ => panic!("Wrong command type"),
        }
    }

    #[test]
    fn test_clap_chain_status_conversion() {
        let args = CliArgs {
            command: Some(CliSubcommand::Chain {
                action: ChainAction::Status,
            }),
        };

        let result: Result<CliCommand, String> = args.into();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), CliCommand::ChainStatus);
    }

    #[test]
    fn test_clap_debug_uptime_conversion() {
        let args = CliArgs {
            command: Some(CliSubcommand::Debug {
                action: DebugAction::Uptime,
            }),
        };

        let result: Result<CliCommand, String> = args.into();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), CliCommand::DebugUptime);
    }

    #[test]
    fn test_clap_no_command_error() {
        let args = CliArgs { command: None };
        let result: Result<CliCommand, String> = args.into();
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "No command specified");
    }

    #[test]
    fn test_clap_multi_level_command() {
        let args = CliArgs {
            command: Some(CliSubcommand::Account {
                action: AccountAction::Transfer {
                    wallet: "my-wallet".to_string(),
                    from: "alice".to_string(),
                    to: "bob".to_string(),
                    amount: 500,
                },
            }),
        };

        let result: Result<CliCommand, String> = args.into();
        assert!(result.is_ok());

        match result.unwrap() {
            CliCommand::AccountTransfer {
                wallet,
                from,
                to,
                amount,
            } => {
                assert_eq!(wallet, "my-wallet");
                assert_eq!(from, "alice");
                assert_eq!(to, "bob");
                assert_eq!(amount, 500);
            }
            _ => panic!("Wrong command type"),
        }
    }

    #[test]
    fn test_cli_config_default() {
        let config = CliConfig::default();
        assert_eq!(config.name, "zoka-cli");
        assert_eq!(config.node_endpoint, "http://127.0.0.1:3000");
        assert_eq!(config.tx_timeout_secs, 60);
        assert!(!config.enable_wallet_encryption);
    }

    #[test]
    fn test_cli_config_new() {
        let config = CliConfig::new("my-config".to_string());
        assert_eq!(config.name, "my-config");
        assert_eq!(config.node_endpoint, "http://127.0.0.1:3000");
    }

    #[test]
    fn test_cli_config_validate() {
        let config = CliConfig::default();
        assert!(config.validate().is_ok());

        let mut invalid = CliConfig::default();
        invalid.name = String::new();
        assert!(invalid.validate().is_err());

        let mut timeout_zero = CliConfig::default();
        timeout_zero.tx_timeout_secs = 0;
        assert!(timeout_zero.validate().is_err());
    }

    #[test]
    fn test_cli_config_merge() {
        let mut config1 = CliConfig::default();
        config1.default_wallet = Some("wallet1".to_string());

        let mut config2 = CliConfig::new("override".to_string());
        config2.tx_timeout_secs = 120;

        config1.merge(config2);
        assert_eq!(config1.name, "override");
        assert_eq!(config1.tx_timeout_secs, 120);
        assert_eq!(config1.default_wallet, Some("wallet1".to_string()));
    }

    #[test]
    fn test_cli_config_save_and_load_json() {
        let test_file = ".test_cli_config.json";
        let _ = fs::remove_file(test_file);

        let mut config = CliConfig::default();
        config.node_endpoint = "http://localhost:8080".to_string();
        config.default_wallet = Some("test-wallet".to_string());

        config.save_to_file(Path::new(test_file)).unwrap();
        assert!(Path::new(test_file).exists());

        let loaded = CliConfig::load_from_file(Path::new(test_file)).unwrap();
        assert_eq!(loaded.node_endpoint, "http://localhost:8080");
        assert_eq!(loaded.default_wallet, Some("test-wallet".to_string()));

        let _ = fs::remove_file(test_file);
    }

    #[test]
    fn test_cli_config_save_and_load_toml() {
        let test_file = ".test_cli_config.toml";
        let _ = fs::remove_file(test_file);

        let mut config = CliConfig::default();
        config.tx_timeout_secs = 180;
        config.enable_wallet_encryption = true;

        config.save_to_toml(Path::new(test_file)).unwrap();
        assert!(Path::new(test_file).exists());

        let loaded = CliConfig::load_from_file(Path::new(test_file)).unwrap();
        assert_eq!(loaded.tx_timeout_secs, 180);
        assert!(loaded.enable_wallet_encryption);

        let _ = fs::remove_file(test_file);
    }

    #[test]
    fn test_cli_config_custom_wallet_storage() {
        let config = CliConfig {
            name: "custom".to_string(),
            default_wallet: Some("main".to_string()),
            wallet_storage_path: PathBuf::from("/custom/wallets"),
            node_endpoint: "http://custom:9000".to_string(),
            tx_timeout_secs: 30,
            enable_wallet_encryption: true,
        };

        assert_eq!(config.wallet_storage_path, PathBuf::from("/custom/wallets"));
        assert_eq!(config.tx_timeout_secs, 30);
        assert!(config.enable_wallet_encryption);
    }

    #[test]
    fn test_cli_config_load_json_format() {
        let test_file = ".test_cli_json.json";
        let json_content = r#"{
  "name": "json-config",
  "default_wallet": "main",
  "wallet_storage_path": "./wallets",
  "node_endpoint": "http://127.0.0.1:3000",
  "tx_timeout_secs": 60,
  "enable_wallet_encryption": false
}"#;
        fs::write(test_file, json_content).unwrap();

        let config = CliConfig::load_from_file(Path::new(test_file)).unwrap();
        assert_eq!(config.name, "json-config");
        assert_eq!(config.default_wallet, Some("main".to_string()));

        let _ = fs::remove_file(test_file);
    }

    #[test]
    fn test_multi_profile_config_default() {
        let config = MultiProfileConfig::default();
        assert_eq!(config.active_profile, "default");
        assert!(config.profiles.contains_key("default"));
        assert_eq!(config.list_profiles().len(), 1);
    }

    #[test]
    fn test_multi_profile_add_profile() {
        let mut config = MultiProfileConfig::new();
        let staging = CliConfig::new("staging".to_string());

        let result = config.add_profile("staging".to_string(), staging);
        assert!(result.is_ok());
        assert_eq!(config.list_profiles().len(), 2);
    }

    #[test]
    fn test_multi_profile_duplicate_profile() {
        let mut config = MultiProfileConfig::new();
        let profile1 = CliConfig::new("test".to_string());
        let profile2 = CliConfig::new("test".to_string());

        config.add_profile("test".to_string(), profile1).unwrap();
        let result = config.add_profile("test".to_string(), profile2);
        assert!(result.is_err());
    }

    #[test]
    fn test_multi_profile_switch() {
        let mut config = MultiProfileConfig::new();
        let prod = CliConfig::new("production".to_string());
        config.add_profile("production".to_string(), prod).unwrap();

        let result = config.switch_profile("production");
        assert!(result.is_ok());
        assert_eq!(config.active_profile, "production");
    }

    #[test]
    fn test_multi_profile_switch_nonexistent() {
        let mut config = MultiProfileConfig::new();
        let result = config.switch_profile("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_multi_profile_get_active() {
        let mut config = MultiProfileConfig::new();
        let dev = CliConfig::new("development".to_string());
        config.add_profile("development".to_string(), dev).unwrap();
        config.switch_profile("development").unwrap();

        let active = config.get_active_profile();
        assert!(active.is_some());
        assert_eq!(active.unwrap().name, "development");
    }

    #[test]
    fn test_multi_profile_file_persistence() {
        let test_file = ".test_profiles.toml";
        let _ = fs::remove_file(test_file);

        let mut config = MultiProfileConfig::new();
        let prod = CliConfig::new("production".to_string());
        config.add_profile("production".to_string(), prod).unwrap();

        config.save_to_file(Path::new(test_file)).unwrap();
        assert!(Path::new(test_file).exists());

        let loaded = MultiProfileConfig::load_from_file(Path::new(test_file)).unwrap();
        assert_eq!(loaded.list_profiles().len(), 2);
        assert!(loaded.profiles.contains_key("production"));

        let _ = fs::remove_file(test_file);
    }
}
