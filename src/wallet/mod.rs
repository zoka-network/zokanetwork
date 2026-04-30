// User-facing wallet data types persisted by wallet storage.
use serde::{Deserialize, Serialize};

/// User wallet with address and balance tracking.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserWallet {
    pub name: String,
    pub address: String,
    pub public_key: Vec<u8>,
    pub balance: u64,
    #[serde(default)]
    pub restore_height: u64,
    #[serde(default)]
    pub subaddresses: Vec<WalletSubAddress>,
    #[serde(default)]
    pub next_subaddress_index: u32,
    pub transactions: Vec<WalletTransaction>,
    pub created_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WalletSubAddress {
    pub index: u32,
    pub label: String,
    pub address: String,
    pub created_at: u64,
}

/// Transaction as seen by wallet user.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WalletTransaction {
    pub hash: String,
    pub from: String,
    pub to: String,
    pub amount: u64,
    pub fee: u64,
    pub status: TransactionStatus,
    #[serde(default)]
    pub block_height: Option<u64>,
    #[serde(default)]
    pub unlock_height: Option<u64>,
    pub timestamp: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WalletKeyMaterial {
    pub wallet_name: String,
    pub account_index: u32,
    pub address: String,
    pub public_key_hex: String,
    pub private_key_hex: String,
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

pub mod storage;
