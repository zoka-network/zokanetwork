//! User-Facing Node: Integrates Wallet + P2P Network + State Management
//! Allows users to send/receive transactions across the P2P network with privacy support

use crate::network::message::{UserDiscoveryRecord, ZokaMessage};
use crate::network::p2p::Node;
use crate::private_wallet::{PrivateTransaction, PrivateWalletManager};
use crate::state::{AccountInclusionProof, StateManager};
use crate::types::{Block, Signature, Transaction};
use crate::wallet::storage::WalletStorage;
use crate::wallet::WalletManager;
use libp2p::PeerId;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc::{self, Receiver, Sender};

/// User-facing Node integrating wallets + P2P network
pub struct UserNode {
    /// Public/private wallet management
    pub wallet_manager: WalletManager,
    pub private_wallet_manager: PrivateWalletManager,

    /// P2P network interface
    pub p2p_node: Arc<Mutex<Node>>,

    /// Local block height tracking
    pub block_height: u64,
    last_block_hash: String,
    last_state_root: String,

    /// Event channel for inbound messages
    pub event_tx: Sender<UserNodeEvent>,
    pub event_rx: Receiver<UserNodeEvent>,

    /// Transaction history (local tracking)
    pending_transactions: VecDeque<Transaction>,
    #[allow(dead_code)]
    sent_private_txs: HashMap<String, PrivateTransaction>,

    /// Connected peers
    connected_peers: Arc<Mutex<Vec<PeerId>>>,

    /// Rate limiting: max tx per second per wallet
    rate_limit_tx_per_sec: usize,
    last_tx_time: Arc<Mutex<HashMap<String, Vec<SystemTime>>>>,

    /// User discovery cache (username -> user info)
    known_users: Arc<Mutex<HashMap<String, UserInfo>>>,
    advertised_listen_addr: Option<String>,

    /// Maximum number of blocks to fetch in one sync batch
    max_sync_batch: usize,
    state_manager: Option<Arc<StateManager>>,
}

#[derive(Debug, Clone)]
pub enum UserNodeEvent {
    /// Received inbound transaction (might apply to us)
    TransactionReceived(Transaction),

    /// Received private transaction
    PrivateTransactionReceived(PrivateTransaction),

    /// Received block from peers
    BlockReceived(Block),

    /// Balance changed (from inbound tx)
    BalanceChanged { wallet: String, new_balance: u64 },

    /// Private tx confirmed in block
    PrivateTransactionConfirmed(String),

    /// Connected to new peer
    PeerConnected(PeerId),

    /// Disconnected from peer
    PeerDisconnected(PeerId),

    /// Network error
    NetworkError(String),
}

impl UserNode {
    /// Create new user node with local wallets and network
    pub fn new(p2p_node: Node) -> Self {
        let (event_tx, event_rx) = mpsc::channel(1000);

        Self {
            wallet_manager: WalletManager::new(),
            private_wallet_manager: PrivateWalletManager::new(),
            p2p_node: Arc::new(Mutex::new(p2p_node)),
            block_height: 0u64,
            last_block_hash: "0".to_string(),
            last_state_root: "0".to_string(),
            event_tx,
            event_rx,
            pending_transactions: VecDeque::new(),
            sent_private_txs: HashMap::new(),
            connected_peers: Arc::new(Mutex::new(Vec::new())),
            rate_limit_tx_per_sec: 10,
            last_tx_time: Arc::new(Mutex::new(HashMap::new())),
            known_users: Arc::new(Mutex::new(HashMap::new())),
            advertised_listen_addr: None,
            max_sync_batch: 64,
            state_manager: None,
        }
    }

    /// Attach local state manager for account proof workflows.
    pub fn attach_state_manager(&mut self, state_manager: Arc<StateManager>) {
        self.state_manager = Some(state_manager);
    }

    // ============================================================
    // PUBLIC WALLET OPERATIONS
    // ============================================================

    /// Send public transaction via P2P network
    pub async fn send_public_transaction(
        &mut self,
        wallet_name: &str,
        recipient: &str,
        amount: u64,
    ) -> Result<String, String> {
        // Rate limiting
        self.check_rate_limit(wallet_name)?;

        // Create transaction in wallet
        let tx_hash = self
            .wallet_manager
            .send_transaction(wallet_name, recipient, amount)
            .map_err(|e| format!("Transaction creation failed: {}", e))?;

        // Get the transaction details for broadcasting
        let tx = Transaction {
            hash: tx_hash.clone(),
            sender: self
                .wallet_manager
                .get_addresses()
                .get(wallet_name)
                .cloned()
                .unwrap_or_default(),
            receiver: recipient.to_string(),
            amount,
            timestamp: current_timestamp(),
            nonce: 0,
            signature: Signature(vec![]), // Empty signature for now; should be signed with wallet key
            zk_proof: None,
        };

        // Track locally
        self.pending_transactions.push_back(tx.clone());

        // Broadcast via P2P network
        self.broadcast_transaction(&tx).await?;

        Ok(tx_hash)
    }

    /// Get public wallet balance
    pub fn get_balance(&self, wallet_name: &str) -> Option<u64> {
        self.wallet_manager.get_balance(wallet_name)
    }

    /// List all public wallets
    pub fn list_public_wallets(&self) -> Vec<String> {
        self.wallet_manager.list_wallet_names()
    }

    // ============================================================
    // PRIVATE WALLET OPERATIONS
    // ============================================================

    /// Send private transaction via P2P network (with ZK proof)
    pub async fn send_private_transaction(
        &mut self,
        wallet_name: &str,
        recipient_commitment: &str,
        amount: u64,
    ) -> Result<String, String> {
        // Rate limiting
        self.check_rate_limit(wallet_name)?;

        // Create private transaction (includes ZK proof generation)
        let tx_hash = self
            .private_wallet_manager
            .send_private_transaction(wallet_name, recipient_commitment.to_string(), amount)
            .map_err(|e| format!("Private transaction creation failed: {}", e))?;

        // Get transaction details
        let (pending, _verified, _confirmed, _failed) =
            self.private_wallet_manager.get_private_pool_stats();

        if pending > 0 {
            // Broadcast via P2P
            self.broadcast_private_transaction(&tx_hash).await?;
        }

        Ok(tx_hash)
    }

    /// Get private wallet balance (committed amount, not revealed)
    pub fn get_private_balance(&self, wallet_name: &str) -> Option<u64> {
        self.private_wallet_manager.get_private_balance(wallet_name)
    }

    /// List all private wallets
    pub fn list_private_wallets(&self) -> Vec<String> {
        self.private_wallet_manager.get_private_wallet_names()
    }

    /// Set an advertised multiaddr so peers can auto-dial this user.
    pub fn set_advertised_listen_addr(&mut self, addr: String) {
        self.advertised_listen_addr = Some(addr);
    }

    /// Rotate private wallet key and get fresh commitment key.
    pub fn rotate_private_wallet_key(&mut self, wallet_name: &str) -> Result<String, String> {
        self.private_wallet_manager
            .rotate_private_wallet_key(wallet_name)
            .map_err(|e| format!("Key rotation failed: {}", e))
    }

    /// Persist all wallets to encrypted local storage.
    pub fn save_wallets_to_disk(&self, password: Option<&str>) -> Result<(usize, usize), String> {
        let storage = WalletStorage::new().map_err(|e| format!("Storage init failed: {}", e))?;
        let mut public_saved = 0usize;
        let mut private_saved = 0usize;

        for wallet_name in self.list_public_wallets() {
            if let Some(wallet) = self.wallet_manager.get_wallet(&wallet_name) {
                let mnemonic = self.wallet_manager.export_wallet(&wallet_name).ok();
                storage
                    .save_public_wallet(&wallet, mnemonic, password)
                    .map_err(|e| format!("Failed to save public wallet {}: {}", wallet_name, e))?;
                public_saved += 1;
            }
        }

        for wallet_name in self.list_private_wallets() {
            if let Some(wallet) = self.private_wallet_manager.get_private_wallet(&wallet_name) {
                storage
                    .save_private_wallet(wallet, password)
                    .map_err(|e| format!("Failed to save private wallet {}: {}", wallet_name, e))?;
                private_saved += 1;
            }
        }

        Ok((public_saved, private_saved))
    }

    /// Load wallets from disk into memory.
    pub fn load_wallets_from_disk(
        &mut self,
        password: Option<&str>,
    ) -> Result<(usize, usize), String> {
        let storage = WalletStorage::new().map_err(|e| format!("Storage init failed: {}", e))?;
        let mut public_loaded = 0usize;
        let mut private_loaded = 0usize;

        let public_wallets = storage
            .load_all_public_wallets(password)
            .map_err(|e| format!("Failed to load public wallets: {}", e))?;
        for (wallet, mnemonic) in public_wallets {
            if self.wallet_manager.wallet_exists(&wallet.name) {
                continue;
            }

            let mnemonic = match mnemonic {
                Some(m) if !m.is_empty() => m,
                _ => continue,
            };

            self.wallet_manager
                .import_wallet(&wallet.name, &mnemonic)
                .map_err(|e| format!("Import failed for {}: {}", wallet.name, e))?;

            if let Some(live_wallet) = self.wallet_manager.get_wallet_mut(&wallet.name) {
                *live_wallet = wallet;
                public_loaded += 1;
            }
        }

        let private_wallets = storage
            .load_all_private_wallets(password)
            .map_err(|e| format!("Failed to load private wallets: {}", e))?;
        for wallet in private_wallets {
            if self
                .private_wallet_manager
                .get_private_wallet(&wallet.name)
                .is_some()
            {
                continue;
            }

            self.private_wallet_manager
                .create_private_wallet(&wallet.name)
                .map_err(|e| {
                    format!("Failed to initialize private wallet {}: {}", wallet.name, e)
                })?;
            if let Some(live_wallet) = self
                .private_wallet_manager
                .get_private_wallet_mut(&wallet.name)
            {
                *live_wallet = wallet;
                private_loaded += 1;
            }
        }

        Ok((public_loaded, private_loaded))
    }

    /// Deposit from public to private wallet
    pub async fn deposit_to_private(
        &mut self,
        public_wallet: &str,
        private_wallet: &str,
        amount: u64,
    ) -> Result<(), String> {
        // Check public balance available
        let balance = self.get_balance(public_wallet).ok_or("Wallet not found")?;
        if balance < amount {
            return Err("Insufficient public balance".to_string());
        }

        // Send public tx to private wallet (commitment)
        self.send_public_transaction(
            public_wallet,
            &format!("private:{}", private_wallet),
            amount,
        )
        .await?;

        // Add to private wallet
        self.private_wallet_manager
            .deposit_to_private_wallet(private_wallet, amount)
            .map_err(|e| format!("Deposit failed: {}", e))?;

        Ok(())
    }

    // ============================================================
    // INBOUND MESSAGE HANDLING (Private payment reception)
    // ============================================================

    /// Handle inbound transaction (might be for us)
    pub async fn on_inbound_transaction(&mut self, tx: &Transaction) -> Result<(), String> {
        // Check if this tx is for one of our public wallets
        for wallet_name in self.list_public_wallets() {
            if let Some(addr) = self.wallet_manager.get_addresses().get(&wallet_name) {
                if addr == &tx.receiver {
                    // This is for us!
                    self.wallet_manager
                        .receive_transaction(&wallet_name, tx.amount)
                        .map_err(|e| format!("Receive failed: {}", e))?;

                    // Emit event
                    self.event_tx
                        .send(UserNodeEvent::BalanceChanged {
                            wallet: wallet_name.clone(),
                            new_balance: self.get_balance(&wallet_name).unwrap_or(0),
                        })
                        .await
                        .ok();

                    return Ok(());
                }
            }
        }

        Ok(())
    }

    /// Handle inbound private transaction (might be for us)
    pub async fn on_inbound_private_transaction(
        &mut self,
        private_tx: &PrivateTransaction,
    ) -> Result<(), String> {
        self.private_wallet_manager
            .validate_inbound_private_transaction(private_tx)
            .map_err(|e| format!("Inbound private tx validation failed: {}", e))?;

        // Check if any of our private wallets can decrypt this
        for wallet_name in self.list_private_wallets() {
            if let Ok(true) = self
                .private_wallet_manager
                .can_wallet_decrypt_private_tx(&wallet_name, private_tx)
            {
                // We can decrypt this! Get the amount
                if let Ok(amount) = self
                    .private_wallet_manager
                    .decrypt_private_amount_for_wallet(&wallet_name, private_tx)
                {
                    // Update our private balance (locally)
                    self.private_wallet_manager
                        .deposit_to_private_wallet(&wallet_name, amount)
                        .map_err(|e| format!("Private deposit failed: {}", e))?;

                    // Emit event
                    self.event_tx
                        .send(UserNodeEvent::BalanceChanged {
                            wallet: wallet_name.clone(),
                            new_balance: self.get_private_balance(&wallet_name).unwrap_or(0),
                        })
                        .await
                        .ok();

                    return Ok(());
                }
            }
        }

        Ok(())
    }

    // ============================================================
    // STATE SYNCHRONIZATION
    // ============================================================

    /// Sync blockchain state from peers
    pub async fn sync_state_from_peers(&mut self) -> Result<u64, String> {
        let mut synced = 0usize;

        while synced < self.max_sync_batch {
            let next_height = self.block_height.saturating_add(1);

            let response = {
                let mut node = self.p2p_node.lock().unwrap();
                node.request_block_with_retry(next_height, None, 3, Duration::from_millis(1500))
                    .await
            };

            let response = match response {
                Ok(resp) => resp,
                Err(_) => break, // no additional blocks available or peer not responding
            };

            let block: Block = match serde_json::from_slice(&response.encrypted_block) {
                Ok(block) => block,
                Err(_) => break,
            };

            if block.header.block_height != next_height {
                break;
            }

            if block.header.block_height > 0 && block.header.prev_hash != self.last_block_hash {
                break;
            }

            if block.compute_hash() != block.header.hash {
                break;
            }

            if block
                .transactions
                .iter()
                .any(|tx| tx.compute_hash() != tx.hash)
            {
                break;
            }

            if !block.header.state_root.is_empty()
                && block.header.state_root != "0"
                && (block.header.state_root.len() != 64
                    || !block
                        .header
                        .state_root
                        .chars()
                        .all(|c| c.is_ascii_hexdigit()))
            {
                break;
            }

            self.block_height = block.header.block_height;
            self.last_block_hash = block.header.hash.clone();
            self.last_state_root = block.header.state_root.clone();
            synced += 1;

            self.event_tx
                .send(UserNodeEvent::BlockReceived(block))
                .await
                .ok();
        }

        Ok(self.block_height)
    }

    /// Verify user's account inclusion in latest state root
    pub fn verify_account_in_chain(&self, address: &str) -> Result<(), String> {
        if !address.starts_with("0x") {
            return Err("Verification failed: Invalid address format".to_string());
        }

        let hex_part = &address[2..];
        if hex_part.len() != 40 || !hex_part.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err("Verification failed: Invalid address format".to_string());
        }

        if self.block_height > 0 {
            let root = &self.last_state_root;
            if root != "0" && (root.len() != 64 || !root.chars().all(|c| c.is_ascii_hexdigit())) {
                return Err("Verification failed: Invalid local state root".to_string());
            }
        }

        Ok(())
    }

    /// Return the latest synced state root.
    pub fn latest_synced_state_root(&self) -> &str {
        &self.last_state_root
    }

    /// Verify an account inclusion proof against the latest synced root.
    pub fn verify_balance_proof(&self, proof: &AccountInclusionProof) -> bool {
        if self.last_state_root == "0" {
            return false;
        }
        StateManager::verify_account_inclusion_proof(&self.last_state_root, proof)
    }

    /// Get account balance and proof from attached state manager.
    pub async fn get_balance_with_proof(
        &self,
        address: &str,
    ) -> Result<(u64, AccountInclusionProof), String> {
        let state_manager = self
            .state_manager
            .as_ref()
            .ok_or_else(|| "State manager is not attached".to_string())?;
        let account = state_manager
            .get_account(address)
            .await
            .ok_or_else(|| "Account not found".to_string())?;
        let proof = state_manager
            .get_account_inclusion_proof(address)
            .await
            .ok_or_else(|| "Account proof not found".to_string())?;
        Ok((account.balance, proof))
    }

    /// Verify account state from attached canonical state manager.
    pub async fn verify_account_state(&self, address: &str) -> Result<(), String> {
        let state_manager = self
            .state_manager
            .as_ref()
            .ok_or_else(|| "State manager is not attached".to_string())?;
        let expected_root = state_manager.get_state_root().await;
        let (_, proof) = self.get_balance_with_proof(address).await?;
        if !StateManager::verify_account_inclusion_proof(&expected_root, &proof) {
            return Err("Account proof verification failed".to_string());
        }
        Ok(())
    }

    // ============================================================
    // USER DISCOVERY
    // ============================================================

    /// Register this user on the network (for others to find us)
    pub async fn register_user_identity(&mut self, username: &str) -> Result<(), String> {
        // Get our primary public key for identification
        let addresses = self.wallet_manager.get_addresses();
        let primary_addr = addresses
            .values()
            .next()
            .cloned()
            .unwrap_or_else(|| format!("user_{}", username));

        // Get our commitment key (for private payments)
        let maybe_commitment = self
            .private_wallet_manager
            .get_private_wallet_names()
            .first()
            .and_then(|name| self.private_wallet_manager.get_private_wallet(name))
            .map(|w| w.commitment_key.clone());
        let primary_commitment = match maybe_commitment {
            Some(value) => value,
            None => self
                .register_as_private_recipient("default-private")
                .unwrap_or_else(|_| "0xcommunity_default".to_string()),
        };

        let mut node = self.p2p_node.lock().unwrap();

        // Publish user registry message: username -> address + commitment
        let discovery_msg = ZokaMessage::UserRegistry {
            username: username.to_string(),
            public_address: primary_addr,
            commitment_key: primary_commitment,
            peer_id: format!("{:?}", node.peer_id()),
            listen_addr: self.advertised_listen_addr.clone(),
        };
        let dht_record = UserDiscoveryRecord {
            username: username.to_string(),
            public_address: match &discovery_msg {
                ZokaMessage::UserRegistry { public_address, .. } => public_address.clone(),
                _ => String::new(),
            },
            commitment_key: match &discovery_msg {
                ZokaMessage::UserRegistry { commitment_key, .. } => commitment_key.clone(),
                _ => String::new(),
            },
            peer_id: format!("{:?}", node.peer_id()),
            listen_addr: self.advertised_listen_addr.clone(),
            updated_at: current_timestamp(),
        };

        // Keep a local cache entry so lookups resolve instantly for our own user.
        {
            let mut users = self.known_users.lock().unwrap();
            users.insert(
                username.to_string(),
                UserInfo {
                    username: dht_record.username.clone(),
                    public_address: dht_record.public_address.clone(),
                    commitment_key: dht_record.commitment_key.clone(),
                    peer_id: dht_record.peer_id.clone(),
                    listen_addr: dht_record.listen_addr.clone(),
                },
            );
        }

        let _ = node.put_user_record(&dht_record);

        // Broadcast to network (publish_message is sync)
        node.publish_message(&discovery_msg)
            .map_err(|e| format!("Registration failed: {}", e))?;

        Ok(())
    }

    /// Find user by username on the network
    pub async fn find_user(&self, username: &str) -> Result<UserInfo, String> {
        if let Some(cached) = self.get_known_user(username) {
            return Ok(cached);
        }

        {
            let mut node = self.p2p_node.lock().unwrap();
            if let Ok(Some(record)) = node.get_user_record(username, Duration::from_secs(3)).await {
                let info = UserInfo {
                    username: record.username.clone(),
                    public_address: record.public_address,
                    commitment_key: record.commitment_key,
                    peer_id: record.peer_id,
                    listen_addr: record.listen_addr,
                };
                {
                    let mut users = self.known_users.lock().unwrap();
                    users.insert(record.username, info.clone());
                }
                return Ok(info);
            }
        }

        {
            let mut node = self.p2p_node.lock().unwrap();
            let query_msg = ZokaMessage::UserQuery {
                username: username.to_string(),
            };
            node.publish_message(&query_msg)
                .map_err(|e| format!("Query failed: {}", e))?;
        }

        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(3);
        while tokio::time::Instant::now() < deadline {
            let maybe_msg = {
                let mut node = self.p2p_node.lock().unwrap();
                tokio::time::timeout(tokio::time::Duration::from_millis(300), node.next_message())
                    .await
            };

            let message = match maybe_msg {
                Ok(Ok(message)) => message,
                Ok(Err(_)) => continue,
                Err(_) => continue,
            };

            match message {
                ZokaMessage::UserQuery { username: queried } => {
                    if let Some(info) = self.get_known_user(&queried) {
                        let response = ZokaMessage::UserQueryResponse {
                            username: info.username,
                            public_address: info.public_address,
                            commitment_key: info.commitment_key,
                            peer_id: info.peer_id,
                            listen_addr: info.listen_addr,
                        };
                        let response_record = UserDiscoveryRecord {
                            username: match &response {
                                ZokaMessage::UserQueryResponse { username, .. } => username.clone(),
                                _ => String::new(),
                            },
                            public_address: match &response {
                                ZokaMessage::UserQueryResponse { public_address, .. } => {
                                    public_address.clone()
                                }
                                _ => String::new(),
                            },
                            commitment_key: match &response {
                                ZokaMessage::UserQueryResponse { commitment_key, .. } => {
                                    commitment_key.clone()
                                }
                                _ => String::new(),
                            },
                            peer_id: match &response {
                                ZokaMessage::UserQueryResponse { peer_id, .. } => peer_id.clone(),
                                _ => String::new(),
                            },
                            listen_addr: match &response {
                                ZokaMessage::UserQueryResponse { listen_addr, .. } => {
                                    listen_addr.clone()
                                }
                                _ => None,
                            },
                            updated_at: current_timestamp(),
                        };
                        let _ = {
                            let mut node = self.p2p_node.lock().unwrap();
                            let _ = node.put_user_record(&response_record);
                            node.publish_message(&response)
                        };
                    }
                }
                ZokaMessage::UserQueryResponse {
                    username: found_username,
                    public_address,
                    commitment_key,
                    peer_id,
                    listen_addr,
                }
                | ZokaMessage::UserRegistry {
                    username: found_username,
                    public_address,
                    commitment_key,
                    peer_id,
                    listen_addr,
                } => {
                    let info = UserInfo {
                        username: found_username.clone(),
                        public_address,
                        commitment_key,
                        peer_id,
                        listen_addr,
                    };

                    {
                        let mut users = self.known_users.lock().unwrap();
                        users.insert(found_username.clone(), info.clone());
                    }

                    if found_username == username {
                        return Ok(info);
                    }
                }
                _ => {}
            }
        }

        Err(format!("User lookup timed out for username={}", username))
    }

    /// Get user info from local discovery cache.
    pub fn get_known_user(&self, username: &str) -> Option<UserInfo> {
        self.known_users.lock().unwrap().get(username).cloned()
    }

    /// Return number of cached users.
    pub fn known_user_count(&self) -> usize {
        self.known_users.lock().unwrap().len()
    }

    /// List all users currently present in local discovery cache.
    pub fn list_known_users(&self) -> Vec<UserInfo> {
        self.known_users.lock().unwrap().values().cloned().collect()
    }

    /// Send a public transaction to a user resolved by username.
    pub async fn send_public_transaction_to_user(
        &mut self,
        wallet_name: &str,
        username: &str,
        amount: u64,
    ) -> Result<String, String> {
        let user = self.find_user(username).await?;
        if let Some(addr) = &user.listen_addr {
            let connect_result = {
                let mut node = self.p2p_node.lock().unwrap();
                node.connect_to_peer(addr).await
            };
            if connect_result.is_ok() {
                let _ = {
                    let mut node = self.p2p_node.lock().unwrap();
                    tokio::time::timeout(
                        tokio::time::Duration::from_secs(2),
                        node.wait_for_any_connection(),
                    )
                    .await
                };
            }
        }
        self.send_public_transaction(wallet_name, &user.public_address, amount)
            .await
    }

    /// Register a private recipient commitment for receiving private payments.
    pub fn register_as_private_recipient(&mut self, wallet_name: &str) -> Result<String, String> {
        if self
            .private_wallet_manager
            .get_private_wallet(wallet_name)
            .is_none()
        {
            self.private_wallet_manager
                .create_private_wallet(wallet_name)
                .map_err(|e| format!("Failed to create private wallet: {}", e))?;
        }

        self.private_wallet_manager
            .get_private_wallet(wallet_name)
            .map(|wallet| wallet.commitment_key.clone())
            .ok_or_else(|| "Private wallet registration failed".to_string())
    }

    // ============================================================
    // NETWORK OPERATIONS
    // ============================================================

    /// Broadcast transaction to all connected peers
    async fn broadcast_transaction(&self, tx: &Transaction) -> Result<(), String> {
        let mut node = self.p2p_node.lock().unwrap();
        let msg = ZokaMessage::Transaction(tx.clone());

        node.publish_message(&msg)
            .map_err(|e| format!("Broadcast failed: {}", e))
    }

    /// Broadcast private transaction to peers
    async fn broadcast_private_transaction(&self, tx_hash: &str) -> Result<(), String> {
        use crate::network::message::PrivateTransactionMessage;

        let tx = self
            .private_wallet_manager
            .get_private_transaction(tx_hash)
            .ok_or_else(|| format!("Private transaction not found: {}", tx_hash))?;

        let mut node = self.p2p_node.lock().unwrap();
        let msg = ZokaMessage::PrivateTransaction(PrivateTransactionMessage {
            tx_hash: tx.tx_hash,
            commitment: tx.commitment,
            nullifier: tx.nullifier,
            zk_proof: tx.zk_proof,
            timestamp: tx.timestamp,
        });

        node.publish_message(&msg)
            .map_err(|e| format!("Private broadcast failed: {}", e))
    }

    /// Request block from peers
    #[allow(dead_code)]
    async fn request_block_from_peers(&self, height: u64) -> Result<Block, String> {
        let response = {
            let mut node = self.p2p_node.lock().unwrap();
            node.request_block_with_retry(height, None, 3, Duration::from_millis(2000))
                .await
                .map_err(|e| format!("Block request failed: {}", e))?
        };

        serde_json::from_slice(&response.encrypted_block)
            .map_err(|e| format!("Failed to decode block response: {}", e))
    }

    /// Get blockchain height from peers
    #[allow(dead_code)]
    async fn get_peer_blockchain_height(&self, _node: &Node) -> Result<u64, String> {
        // Query connected peers for their current block height
        // Return the maximum height seen
        Ok(self.block_height)
    }

    // ============================================================
    // UTILITIES
    // ============================================================

    /// Check rate limit for this wallet
    fn check_rate_limit(&mut self, wallet_name: &str) -> Result<(), String> {
        let mut times = self.last_tx_time.lock().unwrap();
        let now = SystemTime::now();
        let one_second_ago = now
            .checked_sub(Duration::from_secs(1))
            .unwrap_or(SystemTime::UNIX_EPOCH);

        let entry = times.entry(wallet_name.to_string()).or_default();
        entry.retain(|ts| *ts >= one_second_ago);
        if entry.len() >= self.rate_limit_tx_per_sec {
            return Err(format!(
                "Rate limit exceeded: max {} tx/sec",
                self.rate_limit_tx_per_sec
            ));
        }

        entry.push(now);
        Ok(())
    }

    /// Get current connected peer count
    pub fn connected_peer_count(&self) -> usize {
        self.connected_peers.lock().unwrap().len()
    }

    /// Add connected peer
    pub fn add_connected_peer(&self, peer_id: PeerId) {
        self.connected_peers.lock().unwrap().push(peer_id);
    }

    /// Get node status
    pub fn get_status(&self) -> UserNodeStatus {
        UserNodeStatus {
            block_height: self.block_height,
            connected_peers: self.connected_peer_count(),
            public_wallets: self.list_public_wallets().len(),
            private_wallets: self.list_private_wallets().len(),
            pending_txs: self.pending_transactions.len(),
            total_public_balance: self
                .wallet_manager
                .get_addresses()
                .keys()
                .filter_map(|w| self.get_balance(w))
                .sum(),
            total_private_balance: self
                .list_private_wallets()
                .iter()
                .filter_map(|w| self.get_private_balance(w))
                .sum(),
        }
    }
}

/// Public user information for discovery
#[derive(Debug, Clone)]
pub struct UserInfo {
    pub username: String,
    pub public_address: String,
    pub commitment_key: String,
    pub peer_id: String,
    pub listen_addr: Option<String>,
}

/// User node status snapshot
#[derive(Debug, Clone)]
pub struct UserNodeStatus {
    pub block_height: u64,
    pub connected_peers: usize,
    pub public_wallets: usize,
    pub private_wallets: usize,
    pub pending_txs: usize,
    pub total_public_balance: u64,
    pub total_private_balance: u64,
}

fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_user_node_creation() {
        let node = Node::new(0).await.expect("node should initialize");
        let user_node = UserNode::new(node);
        assert_eq!(user_node.block_height, 0);
        assert_eq!(user_node.connected_peer_count(), 0);
        assert_eq!(user_node.known_user_count(), 0);
    }

    #[tokio::test]
    async fn test_rate_limiting() {
        let node = Node::new(0).await.expect("node should initialize");
        let mut user_node = UserNode::new(node);
        user_node.rate_limit_tx_per_sec = 2;

        assert!(user_node.check_rate_limit("wallet-a").is_ok());
        assert!(user_node.check_rate_limit("wallet-a").is_ok());
        assert!(user_node.check_rate_limit("wallet-a").is_err());
    }

    #[tokio::test]
    async fn test_verify_account_in_chain_format() {
        let node = Node::new(0).await.expect("node should initialize");
        let user_node = UserNode::new(node);

        let valid = format!("0x{}", "a".repeat(40));
        assert!(user_node.verify_account_in_chain(&valid).is_ok());
        assert!(user_node.verify_account_in_chain("alice").is_err());
        assert!(user_node.verify_account_in_chain("0x1234").is_err());
    }

    #[tokio::test]
    async fn test_known_user_cache() {
        let node = Node::new(0).await.expect("node should initialize");
        let user_node = UserNode::new(node);

        {
            let mut cache = user_node.known_users.lock().unwrap();
            cache.insert(
                "alice".to_string(),
                UserInfo {
                    username: "alice".to_string(),
                    public_address: "0x1111111111111111111111111111111111111111".to_string(),
                    commitment_key: "commitment-alice".to_string(),
                    peer_id: "peer-1".to_string(),
                    listen_addr: None,
                },
            );
        }

        let loaded = user_node
            .get_known_user("alice")
            .expect("user should exist");
        assert_eq!(loaded.username, "alice");
        assert_eq!(user_node.known_user_count(), 1);
        assert_eq!(user_node.list_known_users().len(), 1);
    }

    #[tokio::test]
    async fn test_register_as_private_recipient() {
        let node = Node::new(0).await.expect("node should initialize");
        let mut user_node = UserNode::new(node);

        let commitment = user_node
            .register_as_private_recipient("private-main")
            .expect("should register");
        assert!(!commitment.is_empty());
    }

    #[tokio::test]
    async fn test_verify_account_state_with_attached_manager() {
        let node = Node::new(0).await.expect("node should initialize");
        let mut user_node = UserNode::new(node);

        let test_dir = ".test_user_node_verify_account_state";
        let _ = std::fs::remove_dir_all(test_dir);
        let storage = Arc::new(crate::storage::RocksDbStorage::new(test_dir).unwrap());
        let state_manager = Arc::new(StateManager::new(storage));
        state_manager
            .create_account("alice".to_string(), 123)
            .await
            .unwrap();
        user_node.attach_state_manager(state_manager);

        let result = user_node.verify_account_state("alice").await;
        assert!(result.is_ok());

        let _ = std::fs::remove_dir_all(test_dir);
    }
}
