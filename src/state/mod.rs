pub mod chain;
pub mod sync;

use crate::crypto::{
    private_tree_hash_commitment, private_tree_hash_pair, private_tree_scalar_from_hex,
    private_tree_scalar_to_hex, PRIVATE_TREE_DEPTH,
};
use crate::storage::RocksDbStorage;
use crate::types::{AccountState, Block, State, StateError, Transaction};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MerkleProofStep {
    pub sibling_hash: String,
    pub sibling_on_left: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AccountInclusionProof {
    pub address: String,
    pub account: AccountState,
    pub leaf_hash: String,
    pub root_hash: String,
    pub steps: Vec<MerkleProofStep>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PrivateCommitmentInclusionProof {
    pub commitment: String,
    pub leaf_hash: String,
    pub root_hash: String,
    pub steps: Vec<MerkleProofStep>,
}

fn hash_account_leaf(address: &str, account: &AccountState) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(address.as_bytes());
    hasher.update(account.balance.to_le_bytes());
    hasher.update(account.nonce.to_le_bytes());
    hasher.update(account.last_updated_height.to_le_bytes());
    hasher.finalize().to_vec()
}

fn hash_pair(left: &[u8], right: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(left);
    hasher.update(right);
    hasher.finalize().to_vec()
}

fn compute_merkle_root_from_sorted_leaves(mut leaves: Vec<(String, Vec<u8>)>) -> String {
    if leaves.is_empty() {
        return "0".to_string();
    }

    leaves.sort_by(|a, b| a.0.cmp(&b.0));
    let mut level: Vec<Vec<u8>> = leaves.into_iter().map(|(_, leaf)| leaf).collect();
    while level.len() > 1 {
        let mut next_level = Vec::with_capacity(level.len().div_ceil(2));
        for chunk in level.chunks(2) {
            let left = &chunk[0];
            let right = if chunk.len() == 2 {
                &chunk[1]
            } else {
                &chunk[0]
            };
            next_level.push(hash_pair(left, right));
        }
        level = next_level;
    }

    hex::encode(&level[0])
}

fn compute_account_root_for_state(state: &State) -> String {
    if state.accounts.is_empty() {
        return "0".to_string();
    }

    let leaves: Vec<(String, Vec<u8>)> = state
        .accounts
        .iter()
        .map(|(address, account)| (address.clone(), hash_account_leaf(address, account)))
        .collect();
    compute_merkle_root_from_sorted_leaves(leaves)
}

pub fn hash_private_commitment_leaf_bytes(commitment: &str) -> Vec<u8> {
    private_tree_hash_commitment(commitment).to_bytes().to_vec()
}

pub fn hash_private_commitment_leaf_hex(commitment: &str) -> String {
    private_tree_scalar_to_hex(&private_tree_hash_commitment(commitment))
}

fn build_private_commitment_layers(commitments: &[String]) -> Vec<Vec<bls12_381::Scalar>> {
    if commitments.is_empty() {
        return Vec::new();
    }

    let mut leaves: Vec<(String, bls12_381::Scalar)> = commitments
        .iter()
        .cloned()
        .map(|commitment| {
            (
                commitment.clone(),
                private_tree_hash_commitment(&commitment),
            )
        })
        .collect();
    leaves.sort_by(|a, b| a.0.cmp(&b.0));

    let mut layers = Vec::with_capacity(PRIVATE_TREE_DEPTH + 1);
    let mut current: Vec<bls12_381::Scalar> = leaves.into_iter().map(|(_, leaf)| leaf).collect();
    layers.push(current.clone());

    for _ in 0..PRIVATE_TREE_DEPTH {
        let mut next = Vec::with_capacity(current.len().div_ceil(2).max(1));
        for chunk in current.chunks(2) {
            let left = chunk[0];
            let right = if chunk.len() == 2 { chunk[1] } else { chunk[0] };
            next.push(private_tree_hash_pair(left, right));
        }
        current = next;
        layers.push(current.clone());
    }

    layers
}

fn compute_private_commitment_root(commitments: &[String]) -> String {
    if commitments.is_empty() {
        return "0".to_string();
    }

    let layers = build_private_commitment_layers(commitments);
    private_tree_scalar_to_hex(layers.last().and_then(|level| level.first()).unwrap())
}

pub fn build_private_commitment_inclusion_proofs(
    commitments: &[String],
    targets: &[String],
) -> Option<(String, Vec<PrivateCommitmentInclusionProof>)> {
    if commitments.is_empty() {
        return None;
    }

    let mut leaves: Vec<(String, bls12_381::Scalar)> = commitments
        .iter()
        .cloned()
        .map(|commitment| {
            (
                commitment.clone(),
                private_tree_hash_commitment(&commitment),
            )
        })
        .collect();
    leaves.sort_by(|a, b| a.0.cmp(&b.0));

    let root_hash = compute_private_commitment_root(commitments);
    let mut proofs = Vec::with_capacity(targets.len());

    for target in targets {
        let mut index = leaves.iter().position(|(c, _)| c == target)?;
        let (proof_commitment, proof_leaf) = leaves[index].clone();
        let mut level: Vec<bls12_381::Scalar> = leaves.iter().map(|(_, leaf)| *leaf).collect();
        let mut steps = Vec::new();

        for _ in 0..PRIVATE_TREE_DEPTH {
            let sibling_index = if index % 2 == 0 {
                if index + 1 < level.len() {
                    index + 1
                } else {
                    index
                }
            } else {
                index - 1
            };
            let sibling = level[sibling_index];
            steps.push(MerkleProofStep {
                sibling_hash: private_tree_scalar_to_hex(&sibling),
                sibling_on_left: sibling_index < index,
            });

            let mut next_level = Vec::with_capacity(level.len().div_ceil(2).max(1));
            for chunk in level.chunks(2) {
                let left = chunk[0];
                let right = if chunk.len() == 2 { chunk[1] } else { chunk[0] };
                next_level.push(private_tree_hash_pair(left, right));
            }
            index /= 2;
            level = next_level;
        }

        proofs.push(PrivateCommitmentInclusionProof {
            commitment: proof_commitment,
            leaf_hash: private_tree_scalar_to_hex(&proof_leaf),
            root_hash: root_hash.clone(),
            steps,
        });
    }

    Some((root_hash, proofs))
}

fn compute_state_root_for_state(state: &State) -> String {
    // Canonical state root remains account-based for backward compatibility.
    // Private commitments are tracked via `private_commitment_root`.
    compute_account_root_for_state(state)
}

/// Blockchain state machine managing all account states
pub struct StateManager {
    state: Arc<RwLock<State>>,
    storage: Arc<RocksDbStorage>,
    state_history: Arc<RwLock<Vec<(u64, State)>>>, // (height, state) snapshots for rollback
}

impl StateManager {
    /// Create a new state manager with storage
    pub fn new(storage: Arc<RocksDbStorage>) -> Self {
        StateManager {
            state: Arc::new(RwLock::new(State::new())),
            storage,
            state_history: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Expose underlying storage for block sync handlers.
    pub fn raw_storage(&self) -> Arc<RocksDbStorage> {
        Arc::clone(&self.storage)
    }

    /// Load state from storage on node startup
    pub async fn load_from_storage(&self) -> Result<(), StateError> {
        let mut state = self.state.write().await;

        // Load chain height
        state.current_height = self
            .storage
            .get_chain_height()
            .map_err(|e| StateError::StorageError(e.to_string()))?;

        // Load last block hash
        state.current_hash = self
            .storage
            .get_last_block_hash()
            .map_err(|e| StateError::StorageError(e.to_string()))?;

        // Load latest state root
        state.current_state_root = self
            .storage
            .get_state_root()
            .map_err(|e| StateError::StorageError(e.to_string()))?;

        // Load all persisted account states.
        state.accounts.clear();
        let persisted_accounts = self
            .storage
            .iterate_account_states()
            .map_err(|e| StateError::StorageError(e.to_string()))?;
        for account in persisted_accounts {
            state.accounts.insert(account.address.clone(), account);
        }

        state.private_commitments = self
            .storage
            .get_private_commitments()
            .map_err(|e| StateError::StorageError(e.to_string()))?;
        state.private_commitment_root = self
            .storage
            .get_private_commitment_root()
            .map_err(|e| StateError::StorageError(e.to_string()))?;

        let recomputed_private_root = compute_private_commitment_root(&state.private_commitments);
        if state.private_commitment_root == "0"
            || state.private_commitment_root != recomputed_private_root
        {
            state.private_commitment_root = recomputed_private_root;
        }

        // Recompute combined state root from loaded account + private commitment sets.
        let recomputed_root = compute_state_root_for_state(&state);
        if state.current_state_root == "0" || state.current_state_root != recomputed_root {
            state.current_state_root = recomputed_root;
        }

        Ok(())
    }

    /// Apply a complete block to the state
    pub async fn apply_block(&self, block: &Block) -> Result<(), StateError> {
        let mut state_guard = self.state.write().await;
        let mut next_state = state_guard.clone();
        next_state.current_height = block.header.block_height;

        // Execute each transaction in the block
        for tx in &block.transactions {
            self.apply_transaction_internal(&mut next_state, tx)?;
        }

        let computed_state_root = compute_state_root_for_state(&next_state);
        if block.header.state_root != "0" && block.header.state_root != computed_state_root {
            return Err(StateError::BlockApplicationFailed(
                "block state root does not match post-state transition".to_string(),
            ));
        }

        // Update state metadata
        next_state.current_hash = block.header.hash.clone();
        next_state.current_state_root = computed_state_root.clone();
        *state_guard = next_state;

        // Persist block + metadata + account state atomically.
        self.storage
            .persist_block_and_state(block, &state_guard)
            .map_err(|e| StateError::StorageError(e.to_string()))?;

        // Create snapshot for rollback capability
        drop(state_guard); // Release write lock
        self.snapshot_state(block.header.block_height).await?;

        Ok(())
    }

    /// Apply a single transaction to the state
    fn apply_transaction_internal(
        &self,
        state: &mut State,
        tx: &Transaction,
    ) -> Result<(), StateError> {
        // Private transactions live in `private_wallet::PrivateTransaction`
        // and never enter this path — the public `Transaction` type no longer
        // has a `zk_proof` field, so the type system already enforces that
        // anything arriving here is a plaintext public transfer.

        // Get or create sender account
        let sender = state
            .accounts
            .entry(tx.sender.clone())
            .or_insert_with(|| AccountState::new(tx.sender.clone()));

        // Check sufficient balance
        if sender.balance < tx.amount {
            return Err(StateError::InvalidStateTransition);
        }

        // Deduct from sender
        sender.balance -= tx.amount;
        sender.nonce += 1;
        sender.last_updated_height = state.current_height;

        // Get or create receiver account
        let receiver = state
            .accounts
            .entry(tx.receiver.clone())
            .or_insert_with(|| AccountState::new(tx.receiver.clone()));

        // Add to receiver
        receiver.balance += tx.amount;
        receiver.last_updated_height = state.current_height;

        Ok(())
    }

    /// Compute the post-state root that would result from applying a block.
    pub async fn preview_state_root_for_block(&self, block: &Block) -> Result<String, StateError> {
        let current = self.state.read().await;
        let mut preview = current.clone();
        preview.current_height = block.header.block_height;

        for tx in &block.transactions {
            self.apply_transaction_internal(&mut preview, tx)?;
        }

        Ok(compute_state_root_for_state(&preview))
    }

    /// Get a copy of the current state
    pub async fn get_state(&self) -> State {
        self.state.read().await.clone()
    }

    /// Get account state
    pub async fn get_account(&self, address: &str) -> Option<AccountState> {
        if let Ok(Some(account)) = self.storage.get_account_state(address) {
            return Some(account);
        }

        let state = self.state.read().await;
        state.get_account(&address.to_string()).cloned()
    }

    /// Get current blockchain height
    pub async fn get_height(&self) -> u64 {
        if let Ok(height) = self.storage.get_chain_height() {
            return height;
        }

        let state = self.state.read().await;
        state.current_height
    }

    /// Get the PoW difficulty (leading-zero bits) required for the next block.
    pub fn get_next_difficulty(&self) -> u32 {
        crate::mining::difficulty::expected_for_next_block(&self.storage)
            .unwrap_or(crate::economics::POW_INITIAL_DIFFICULTY_BITS)
    }

    /// Get current block hash
    pub async fn get_current_hash(&self) -> String {
        if let Ok(hash) = self.storage.get_last_block_hash() {
            return hash;
        }

        let state = self.state.read().await;
        state.current_hash.clone()
    }

    /// Get current state root
    pub async fn get_state_root(&self) -> String {
        if let Ok(root) = self.storage.get_state_root() {
            if root != "0" {
                return root;
            }
        }

        let state = self.state.read().await;
        state.current_state_root.clone()
    }

    /// Get a block by height
    pub async fn get_block_by_height(&self, height: u64) -> Result<Option<Block>, StateError> {
        self.storage
            .get_block_by_height(height)
            .map_err(|e| StateError::StorageError(e.to_string()))
    }

    /// Get a block by hash
    pub async fn get_block_by_hash(&self, hash: &str) -> Result<Option<Block>, StateError> {
        self.storage
            .get_block(hash)
            .map_err(|e| StateError::StorageError(e.to_string()))
    }

    /// Get the persisted PoW difficulty bits for a block, when metadata is available.
    pub fn get_block_difficulty_bits(&self, hash: &str) -> Result<Option<u32>, StateError> {
        let key = format!("chain:block_meta:{hash}");
        let Some(bytes) = self
            .storage
            .get_raw(&key)
            .map_err(|e| StateError::StorageError(e.to_string()))?
        else {
            return Ok(None);
        };

        let value: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|e| StateError::StorageError(format!("block meta decode failed: {e}")))?;
        Ok(value
            .get("difficulty_bits")
            .and_then(|bits| bits.as_u64())
            .and_then(|bits| u32::try_from(bits).ok()))
    }

    /// Get the latest block
    pub async fn get_latest_block(&self) -> Result<Option<Block>, StateError> {
        let height = self.get_height().await;
        if height == 0 {
            return Ok(None);
        }
        self.get_block_by_height(height).await
    }

    /// Get blocks in a height range
    pub async fn get_blocks_in_range(
        &self,
        from_height: u64,
        to_height: u64,
    ) -> Result<Vec<Block>, StateError> {
        let mut blocks = Vec::new();
        for height in from_height..=to_height {
            if let Some(block) = self.get_block_by_height(height).await? {
                blocks.push(block);
            }
        }
        Ok(blocks)
    }

    /// Get transactions from a specific block
    pub async fn get_block_transactions(
        &self,
        height: u64,
    ) -> Result<Vec<Transaction>, StateError> {
        match self.get_block_by_height(height).await? {
            Some(block) => Ok(block.transactions),
            None => Err(StateError::StorageError("Block not found".to_string())),
        }
    }

    /// Return the private-transaction hashes committed at this block
    /// height. The actual `PrivateTransaction` bodies live in
    /// `PrivateTransactionPool`; callers that want the full private tx
    /// must look each hash up there.
    pub async fn get_block_private_tx_hashes(
        &self,
        height: u64,
    ) -> Result<Vec<String>, StateError> {
        match self.get_block_by_height(height).await? {
            Some(block) => Ok(block.private_tx_hashes),
            None => Err(StateError::StorageError("Block not found".to_string())),
        }
    }

    /// Get the encrypted/canonical private transaction body by hash.
    pub fn get_private_transaction_bytes(&self, hash: &str) -> Result<Option<Vec<u8>>, StateError> {
        self.storage
            .get_private_transaction_bytes(hash)
            .map_err(|e| StateError::StorageError(e.to_string()))
    }

    /// Persist the encrypted/canonical private transaction body by hash.
    pub fn save_private_transaction_bytes(
        &self,
        hash: &str,
        body: &[u8],
    ) -> Result<(), StateError> {
        self.storage
            .save_private_transaction_bytes(hash, body)
            .map_err(|e| StateError::StorageError(e.to_string()))
    }

    /// Mark a private transaction body as pending for local miner inclusion.
    pub fn save_pending_private_transaction_hash(&self, hash: &str) -> Result<(), StateError> {
        self.storage
            .save_pending_private_transaction_hash(hash)
            .map_err(|e| StateError::StorageError(e.to_string()))
    }

    /// Remove a private transaction from the local pending index.
    pub fn delete_pending_private_transaction_hash(&self, hash: &str) -> Result<(), StateError> {
        self.storage
            .delete_pending_private_transaction_hash(hash)
            .map_err(|e| StateError::StorageError(e.to_string()))
    }

    /// Return locally pending private transaction hashes.
    pub fn iterate_pending_private_transaction_hashes(&self) -> Result<Vec<String>, StateError> {
        self.storage
            .iterate_pending_private_transaction_hashes()
            .map_err(|e| StateError::StorageError(e.to_string()))
    }

    /// Remove a persisted private transaction body.
    pub fn delete_private_transaction(&self, hash: &str) -> Result<(), StateError> {
        self.storage
            .delete_private_transaction(hash)
            .map_err(|e| StateError::StorageError(e.to_string()))
    }

    /// Return whether the chain already commits this private transaction hash.
    pub async fn has_private_transaction_hash(&self, hash: &str) -> Result<bool, StateError> {
        let height = self.get_height().await;
        for h in 1..=height {
            if let Some(block) = self.get_block_by_height(h).await? {
                if block
                    .private_tx_hashes
                    .iter()
                    .any(|candidate| candidate == hash)
                {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// Create an account with initial balance (for testing/genesis)
    pub async fn create_account(
        &self,
        address: String,
        initial_balance: u64,
    ) -> Result<(), StateError> {
        let mut state = self.state.write().await;

        if state.accounts.contains_key(&address) {
            return Err(StateError::InvalidStateTransition);
        }

        let mut account = AccountState::new(address.clone());
        account.balance = initial_balance;
        state.accounts.insert(address, account);
        state.current_state_root = compute_state_root_for_state(&state);
        Ok(())
    }

    /// Compute current state root from the in-memory state
    pub async fn compute_state_root(&self) -> String {
        let state = self.state.read().await;
        compute_state_root_for_state(&state)
    }

    /// Create a snapshot of current state at a specific height
    pub async fn snapshot_state(&self, height: u64) -> Result<(), StateError> {
        let state = self.state.read().await;
        let mut history = self.state_history.write().await;

        // Keep only last 100 snapshots to prevent memory bloat
        if history.len() >= 100 {
            history.remove(0);
        }

        history.push((height, state.clone()));
        Ok(())
    }

    /// Rollback state to a previous height (for chain reorganization)
    pub async fn rollback_to_height(&self, target_height: u64) -> Result<(), StateError> {
        let history = self.state_history.read().await;

        // Find the snapshot at or before the target height
        let snapshot = history
            .iter()
            .rev()
            .find(|(h, _)| h <= &target_height)
            .ok_or(StateError::RollbackFailed)?;

        let mut state = self.state.write().await;
        *state = snapshot.1.clone();

        // Update storage with rolled-back state
        self.storage
            .set_chain_height(state.current_height)
            .map_err(|e| StateError::StorageError(e.to_string()))?;

        self.storage
            .set_last_block_hash(&state.current_hash)
            .map_err(|e| StateError::StorageError(e.to_string()))?;

        self.storage
            .set_state_root(&state.current_state_root)
            .map_err(|e| StateError::StorageError(e.to_string()))?;
        self.storage
            .set_private_commitment_root(&state.private_commitment_root)
            .map_err(|e| StateError::StorageError(e.to_string()))?;
        self.storage
            .set_private_commitments(&state.private_commitments)
            .map_err(|e| StateError::StorageError(e.to_string()))?;

        Ok(())
    }

    /// Get the current height of the longest chain we have a snapshot for
    pub async fn get_rollback_points(&self) -> Vec<u64> {
        let history = self.state_history.read().await;
        history.iter().map(|(h, _)| *h).collect()
    }

    /// Create an explicit snapshot at current height
    pub async fn create_snapshot(&self) -> Result<u64, StateError> {
        let state = self.state.read().await;
        let mut history = self.state_history.write().await;

        let height = state.current_height;
        history.push((height, state.clone()));

        // Keep only last 100 snapshots to manage memory
        if history.len() > 100 {
            history.remove(0);
        }

        Ok(height)
    }

    /// List all available snapshots with metadata
    pub async fn list_snapshots(&self) -> Vec<(u64, String, u64)> {
        let _state = self.state.read().await;
        let history = self.state_history.read().await;

        history
            .iter()
            .map(|(h, snapshot_state)| {
                (
                    *h,
                    snapshot_state.current_state_root.clone(),
                    snapshot_state.accounts.len() as u64,
                )
            })
            .collect()
    }

    /// Restore from specific snapshot
    pub async fn restore_from_snapshot(&self, height: u64) -> Result<(), StateError> {
        self.rollback_to_height(height).await
    }

    /// Get snapshot count
    pub async fn snapshot_count(&self) -> usize {
        let history = self.state_history.read().await;
        history.len()
    }

    /// Build a Merkle inclusion proof for an account in the current state tree
    pub async fn get_account_inclusion_proof(
        &self,
        address: &str,
    ) -> Option<AccountInclusionProof> {
        let state = self.state.read().await;
        let mut leaves: Vec<(String, AccountState, Vec<u8>)> = state
            .accounts
            .iter()
            .map(|(addr, account)| {
                (
                    addr.clone(),
                    account.clone(),
                    hash_account_leaf(addr, account),
                )
            })
            .collect();
        leaves.sort_by(|a, b| a.0.cmp(&b.0));

        let mut index = leaves.iter().position(|(addr, _, _)| addr == address)?;
        let (proof_address, proof_account, proof_leaf) = leaves[index].clone();
        let mut level: Vec<Vec<u8>> = leaves.into_iter().map(|(_, _, leaf)| leaf).collect();
        let mut steps = Vec::new();

        while level.len() > 1 {
            let sibling_index = if index % 2 == 0 {
                if index + 1 < level.len() {
                    index + 1
                } else {
                    index
                }
            } else {
                index - 1
            };

            let sibling = level[sibling_index].clone();
            steps.push(MerkleProofStep {
                sibling_hash: hex::encode(&sibling),
                sibling_on_left: sibling_index < index,
            });

            let mut next_level = Vec::with_capacity(level.len().div_ceil(2));
            for chunk in level.chunks(2) {
                let left = &chunk[0];
                let right = if chunk.len() == 2 {
                    &chunk[1]
                } else {
                    &chunk[0]
                };
                next_level.push(hash_pair(left, right));
            }

            index /= 2;
            level = next_level;
        }

        let root_hash = if level.is_empty() {
            "0".to_string()
        } else {
            hex::encode(&level[0])
        };

        Some(AccountInclusionProof {
            address: proof_address,
            account: proof_account,
            leaf_hash: hex::encode(&proof_leaf),
            root_hash,
            steps,
        })
    }

    /// Verify an account inclusion proof against an expected root
    pub fn verify_account_inclusion_proof(
        expected_root: &str,
        proof: &AccountInclusionProof,
    ) -> bool {
        if proof.address != proof.account.address {
            return false;
        }

        let mut current = hash_account_leaf(&proof.address, &proof.account);
        if hex::encode(&current) != proof.leaf_hash {
            return false;
        }

        for step in &proof.steps {
            let sibling = match hex::decode(&step.sibling_hash) {
                Ok(bytes) => bytes,
                Err(_) => return false,
            };

            current = if step.sibling_on_left {
                hash_pair(&sibling, &current)
            } else {
                hash_pair(&current, &sibling)
            };
        }

        let recomputed_root = hex::encode(current);
        recomputed_root == expected_root && recomputed_root == proof.root_hash
    }

    /// Append a private commitment to the state commitment tree.
    pub async fn append_private_commitment(
        &self,
        commitment: String,
    ) -> Result<String, StateError> {
        if commitment.is_empty() {
            return Err(StateError::InvalidStateTransition);
        }

        let mut state = self.state.write().await;
        state.private_commitments.push(commitment);
        state.private_commitment_root = compute_private_commitment_root(&state.private_commitments);
        state.current_state_root = compute_state_root_for_state(&state);

        self.storage
            .set_private_commitments(&state.private_commitments)
            .map_err(|e| StateError::StorageError(e.to_string()))?;
        self.storage
            .set_private_commitment_root(&state.private_commitment_root)
            .map_err(|e| StateError::StorageError(e.to_string()))?;
        self.storage
            .set_state_root(&state.current_state_root)
            .map_err(|e| StateError::StorageError(e.to_string()))?;

        Ok(state.private_commitment_root.clone())
    }

    /// Return the current private commitment Merkle root.
    pub async fn get_private_commitment_root(&self) -> String {
        let state = self.state.read().await;
        state.private_commitment_root.clone()
    }

    /// Build inclusion proof for a private commitment currently in state.
    pub async fn get_private_commitment_inclusion_proof(
        &self,
        commitment: &str,
    ) -> Option<PrivateCommitmentInclusionProof> {
        let state = self.state.read().await;
        let (_, mut proofs) = build_private_commitment_inclusion_proofs(
            &state.private_commitments,
            &[commitment.to_string()],
        )?;
        proofs.pop()
    }

    /// Verify a private commitment inclusion proof against expected root.
    pub fn verify_private_commitment_inclusion_proof(
        expected_root: &str,
        proof: &PrivateCommitmentInclusionProof,
    ) -> bool {
        let mut current = private_tree_hash_commitment(&proof.commitment);
        if private_tree_scalar_to_hex(&current) != proof.leaf_hash {
            return false;
        }

        for step in &proof.steps {
            let sibling = match private_tree_scalar_from_hex(&step.sibling_hash) {
                Some(scalar) => scalar,
                None => return false,
            };
            current = if step.sibling_on_left {
                private_tree_hash_pair(sibling, current)
            } else {
                private_tree_hash_pair(current, sibling)
            };
        }

        let recomputed_root = private_tree_scalar_to_hex(&current);
        recomputed_root == expected_root && recomputed_root == proof.root_hash
    }
}

/// State pruning and garbage collection
#[derive(Clone, Debug)]
pub struct StatePruner {
    /// Retention period in blocks
    retention_blocks: u64,
    /// Current block height
    current_height: u64,
    /// Pruned accounts (garbage collected)
    pruned_count: usize,
    /// Archive mode enabled
    archive_mode: bool,
}

impl StatePruner {
    /// Create a new state pruner
    pub fn new(retention_blocks: u64) -> Self {
        StatePruner {
            retention_blocks,
            current_height: 0,
            pruned_count: 0,
            archive_mode: false,
        }
    }

    /// Check if an account should be pruned
    pub fn should_prune_account(&self, last_updated_height: u64) -> bool {
        if self.archive_mode {
            return false; // Never prune in archive mode
        }

        if self.current_height < self.retention_blocks {
            return false; // Too early to prune
        }

        let prune_before = self.current_height - self.retention_blocks;
        last_updated_height < prune_before
    }

    /// Update current block height
    pub fn update_height(&mut self, height: u64) {
        self.current_height = height;
    }

    /// Get number of accounts that would be pruned
    pub fn count_pruneable(&self, accounts: &[(String, AccountState)]) -> usize {
        accounts
            .iter()
            .filter(|(_, acc)| self.should_prune_account(acc.last_updated_height))
            .count()
    }

    /// Perform pruning on state
    pub fn prune_state(&mut self, accounts: &mut Vec<(String, AccountState)>) -> usize {
        let before = accounts.len();
        accounts.retain(|(_, acc)| !self.should_prune_account(acc.last_updated_height));
        let after = accounts.len();
        let pruned = before - after;
        self.pruned_count += pruned;
        pruned
    }

    /// Enable/disable archive mode (keep all history)
    pub fn set_archive_mode(&mut self, enabled: bool) {
        self.archive_mode = enabled;
    }

    /// Check if archive mode is enabled
    pub fn is_archive_mode(&self) -> bool {
        self.archive_mode
    }

    /// Get total pruned count
    pub fn get_pruned_count(&self) -> usize {
        self.pruned_count
    }

    /// Get current height
    pub fn get_current_height(&self) -> u64 {
        self.current_height
    }

    /// Get retention period
    pub fn get_retention_blocks(&self) -> u64 {
        self.retention_blocks
    }

    /// Calculate prunable accounts based on live set (for statistics)
    pub fn estimate_prunable_size(&self, accounts: &[(String, AccountState)]) -> (usize, u64) {
        let count = self.count_pruneable(accounts);
        let estimated_bytes = count * 200; // Rough estimate: 200 bytes per account
        (count, estimated_bytes as u64)
    }

    /// Get pruning status
    pub fn get_status(&self) -> (u64, u64, bool, usize) {
        (
            self.current_height,
            self.retention_blocks,
            self.archive_mode,
            self.pruned_count,
        )
    }
}

impl Default for StatePruner {
    fn default() -> Self {
        Self::new(1000) // Default: keep 1000 blocks of history
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{PublicKey, Signature};
    use std::fs;

    #[tokio::test]
    async fn test_state_apply_transaction() {
        let test_dir = ".zoka/test-suite/.test_state_tx";
        let _ = fs::remove_dir_all(test_dir);

        let storage = Arc::new(RocksDbStorage::new(test_dir).unwrap());
        let manager = StateManager::new(storage.clone());

        // Create account
        manager
            .create_account("alice".to_string(), 1000)
            .await
            .unwrap();
        manager.create_account("bob".to_string(), 0).await.unwrap();

        // Create transaction
        let tx = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            100,
            0,
            0,
            Signature(vec![]),
        );

        // Apply block with transaction
        let block = Block::new(
            "0".to_string(),
            0,
            0,
            1,
            vec![tx],
            PublicKey(vec![]),
            Signature(vec![]),
        );

        manager.apply_block(&block).await.unwrap();

        let persisted = storage
            .get_block_by_height(1)
            .unwrap()
            .expect("block should be persisted by height");
        assert_eq!(persisted.header.hash, block.header.hash);

        // Check final balances
        let alice = manager.get_account("alice").await.unwrap();
        let bob = manager.get_account("bob").await.unwrap();

        assert_eq!(alice.balance, 900);
        assert_eq!(bob.balance, 100);

        let _ = fs::remove_dir_all(test_dir);
    }

    #[tokio::test]
    async fn test_state_insufficient_balance() {
        let test_dir = ".zoka/test-suite/.test_state_balance";
        let _ = fs::remove_dir_all(test_dir);

        let storage = Arc::new(RocksDbStorage::new(test_dir).unwrap());
        let manager = StateManager::new(storage);

        // Create account with small balance
        manager
            .create_account("alice".to_string(), 50)
            .await
            .unwrap();
        manager.create_account("bob".to_string(), 0).await.unwrap();

        // Create transaction with amount > balance
        let tx = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            100,
            0,
            0,
            Signature(vec![]),
        );

        let block = Block::new(
            "0".to_string(),
            0,
            0,
            1,
            vec![tx],
            PublicKey(vec![]),
            Signature(vec![]),
        );

        // Apply block should fail
        let result = manager.apply_block(&block).await;
        assert!(result.is_err());

        let _ = fs::remove_dir_all(test_dir);
    }

    #[tokio::test]
    async fn test_state_root_validated_during_block_apply() {
        let test_dir = ".zoka/test-suite/.test_state_root_validation";
        let _ = fs::remove_dir_all(test_dir);

        let storage = Arc::new(RocksDbStorage::new(test_dir).unwrap());
        let manager = StateManager::new(storage);

        manager
            .create_account("alice".to_string(), 1000)
            .await
            .unwrap();
        manager.create_account("bob".to_string(), 0).await.unwrap();

        let tx = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            100,
            1,
            0,
            Signature(vec![]),
        );

        let invalid_block = Block::new(
            "0".to_string(),
            0,
            0,
            1,
            vec![tx.clone()],
            PublicKey(vec![]),
            Signature(vec![]),
        )
        .with_state_root("deadbeef".to_string());

        let invalid_result = manager.apply_block(&invalid_block).await;
        assert!(invalid_result.is_err());

        let draft_block = Block::new(
            "0".to_string(),
            0,
            0,
            1,
            vec![tx],
            PublicKey(vec![]),
            Signature(vec![]),
        );
        let expected_root = manager
            .preview_state_root_for_block(&draft_block)
            .await
            .expect("preview state root");

        let valid_block = draft_block.with_state_root(expected_root.clone());

        manager.apply_block(&valid_block).await.unwrap();
        assert_eq!(manager.get_state_root().await, expected_root);

        let _ = fs::remove_dir_all(test_dir);
    }

    #[tokio::test]
    async fn test_account_inclusion_proof_roundtrip() {
        let test_dir = ".zoka/test-suite/.test_state_merkle_proof";
        let _ = fs::remove_dir_all(test_dir);

        let storage = Arc::new(RocksDbStorage::new(test_dir).unwrap());
        let manager = StateManager::new(storage);

        manager
            .create_account("alice".to_string(), 1000)
            .await
            .unwrap();
        manager
            .create_account("bob".to_string(), 250)
            .await
            .unwrap();
        manager
            .create_account("carol".to_string(), 500)
            .await
            .unwrap();

        let root = manager.compute_state_root().await;
        let proof = manager
            .get_account_inclusion_proof("bob")
            .await
            .expect("proof must exist");

        assert_eq!(proof.root_hash, root);
        assert!(StateManager::verify_account_inclusion_proof(&root, &proof));

        let mut tampered = proof.clone();
        tampered.account.balance += 1;
        assert!(!StateManager::verify_account_inclusion_proof(
            &root, &tampered
        ));

        let _ = fs::remove_dir_all(test_dir);
    }

    #[tokio::test]
    async fn test_load_from_storage_restores_accounts() {
        let test_dir = ".zoka/test-suite/.test_state_load_storage";
        let _ = fs::remove_dir_all(test_dir);

        let storage = Arc::new(RocksDbStorage::new(test_dir).unwrap());
        storage
            .save_account_state(
                "alice",
                &AccountState {
                    address: "alice".to_string(),
                    balance: 700,
                    nonce: 2,
                    last_updated_height: 4,
                },
            )
            .unwrap();
        storage
            .save_account_state(
                "bob",
                &AccountState {
                    address: "bob".to_string(),
                    balance: 300,
                    nonce: 1,
                    last_updated_height: 4,
                },
            )
            .unwrap();
        storage.set_chain_height(4).unwrap();
        storage.set_last_block_hash("hash-4").unwrap();

        let manager = StateManager::new(storage);
        manager.load_from_storage().await.unwrap();

        let alice = manager.get_account("alice").await;
        let bob = manager.get_account("bob").await;
        assert!(alice.is_some());
        assert!(bob.is_some());
        assert_ne!(manager.get_state_root().await, "0");

        let _ = fs::remove_dir_all(test_dir);
    }

    #[tokio::test]
    async fn test_private_commitment_tree_roundtrip() {
        let test_dir = ".zoka/test-suite/.test_private_commitment_tree";
        let _ = fs::remove_dir_all(test_dir);

        let storage = Arc::new(RocksDbStorage::new(test_dir).unwrap());
        let manager = StateManager::new(storage.clone());

        let root_1 = manager
            .append_private_commitment("cmt-a".to_string())
            .await
            .unwrap();
        assert_ne!(root_1, "0");

        let root_2 = manager
            .append_private_commitment("cmt-b".to_string())
            .await
            .unwrap();
        assert_ne!(root_1, root_2);
        assert_eq!(manager.get_private_commitment_root().await, root_2);

        manager.load_from_storage().await.unwrap();
        assert_eq!(manager.get_private_commitment_root().await, root_2);

        let _ = fs::remove_dir_all(test_dir);
    }

    #[tokio::test]
    async fn test_private_commitment_inclusion_proof_roundtrip() {
        let test_dir = ".zoka/test-suite/.test_private_commitment_proof";
        let _ = fs::remove_dir_all(test_dir);

        let storage = Arc::new(RocksDbStorage::new(test_dir).unwrap());
        let manager = StateManager::new(storage);
        manager
            .append_private_commitment("cmt-a".to_string())
            .await
            .unwrap();
        manager
            .append_private_commitment("cmt-b".to_string())
            .await
            .unwrap();
        manager
            .append_private_commitment("cmt-c".to_string())
            .await
            .unwrap();

        let root = manager.get_private_commitment_root().await;
        let proof = manager
            .get_private_commitment_inclusion_proof("cmt-b")
            .await
            .expect("proof must exist");
        assert!(StateManager::verify_private_commitment_inclusion_proof(
            &root, &proof
        ));

        let mut tampered = proof.clone();
        tampered.commitment = "cmt-x".to_string();
        assert!(!StateManager::verify_private_commitment_inclusion_proof(
            &root, &tampered
        ));

        let _ = fs::remove_dir_all(test_dir);
    }

    // ===== StatePruner Tests =====

    #[test]
    fn test_state_pruner_creation() {
        let pruner = StatePruner::new(1000);
        assert_eq!(pruner.get_retention_blocks(), 1000);
        assert_eq!(pruner.get_current_height(), 0);
        assert!(!pruner.is_archive_mode());
    }

    #[test]
    fn test_state_pruner_default() {
        let pruner = StatePruner::default();
        assert!(pruner.get_retention_blocks() > 0);
    }

    #[test]
    fn test_should_not_prune_young_accounts() {
        let pruner = StatePruner::new(1000);
        let account = AccountState {
            address: "alice".to_string(),
            balance: 1000,
            nonce: 1,
            last_updated_height: 500,
        };
        assert!(!pruner.should_prune_account(account.last_updated_height));
    }

    #[test]
    fn test_should_prune_old_accounts() {
        let mut pruner = StatePruner::new(1000);
        pruner.update_height(2500);
        let account = AccountState {
            address: "alice".to_string(),
            balance: 1000,
            nonce: 1,
            last_updated_height: 1000,
        };
        assert!(pruner.should_prune_account(account.last_updated_height));
    }

    #[test]
    fn test_archive_mode_prevents_pruning() {
        let mut pruner = StatePruner::new(1000);
        pruner.update_height(2500);
        pruner.set_archive_mode(true);
        let account = AccountState {
            address: "alice".to_string(),
            balance: 1000,
            nonce: 1,
            last_updated_height: 1000,
        };
        assert!(!pruner.should_prune_account(account.last_updated_height));
    }

    #[test]
    fn test_update_height() {
        let mut pruner = StatePruner::new(1000);
        assert_eq!(pruner.get_current_height(), 0);
        pruner.update_height(100);
        assert_eq!(pruner.get_current_height(), 100);
    }

    #[test]
    fn test_count_pruneable() {
        let mut pruner = StatePruner::new(100);
        pruner.update_height(500);

        let accounts = vec![
            (
                "alice".to_string(),
                AccountState {
                    address: "alice".to_string(),
                    balance: 1000,
                    nonce: 1,
                    last_updated_height: 200,
                },
            ),
            (
                "bob".to_string(),
                AccountState {
                    address: "bob".to_string(),
                    balance: 1000,
                    nonce: 1,
                    last_updated_height: 450,
                },
            ),
        ];

        let pruneable = pruner.count_pruneable(&accounts);
        assert!(pruneable > 0);
    }

    #[test]
    fn test_prune_state() {
        let mut pruner = StatePruner::new(100);
        pruner.update_height(500);

        let mut accounts = vec![
            (
                "alice".to_string(),
                AccountState {
                    address: "alice".to_string(),
                    balance: 1000,
                    nonce: 1,
                    last_updated_height: 200,
                },
            ),
            (
                "bob".to_string(),
                AccountState {
                    address: "bob".to_string(),
                    balance: 1000,
                    nonce: 1,
                    last_updated_height: 450,
                },
            ),
        ];

        let before = accounts.len();
        let pruned = pruner.prune_state(&mut accounts);
        assert!(pruned > 0);
        assert!(accounts.len() < before);
    }

    #[test]
    fn test_pruned_count_tracking() {
        let mut pruner = StatePruner::new(100);
        assert_eq!(pruner.get_pruned_count(), 0);

        pruner.update_height(500);
        let mut accounts = vec![(
            "alice".to_string(),
            AccountState {
                address: "alice".to_string(),
                balance: 1000,
                nonce: 1,
                last_updated_height: 200,
            },
        )];

        pruner.prune_state(&mut accounts);
        assert!(pruner.get_pruned_count() > 0);
    }

    #[test]
    fn test_archive_mode_toggle() {
        let mut pruner = StatePruner::new(1000);
        assert!(!pruner.is_archive_mode());

        pruner.set_archive_mode(true);
        assert!(pruner.is_archive_mode());

        pruner.set_archive_mode(false);
        assert!(!pruner.is_archive_mode());
    }

    #[test]
    fn test_estimate_prunable_size() {
        let mut pruner = StatePruner::new(100);
        pruner.update_height(500);

        let accounts = vec![
            (
                "alice".to_string(),
                AccountState {
                    address: "alice".to_string(),
                    balance: 1000,
                    nonce: 1,
                    last_updated_height: 200,
                },
            ),
            (
                "bob".to_string(),
                AccountState {
                    address: "bob".to_string(),
                    balance: 1000,
                    nonce: 1,
                    last_updated_height: 300,
                },
            ),
        ];

        let (count, size) = pruner.estimate_prunable_size(&accounts);
        assert!(count > 0);
        assert!(size > 0);
    }

    #[test]
    fn test_get_status() {
        let mut pruner = StatePruner::new(1000);
        pruner.update_height(500);
        pruner.set_archive_mode(true);

        let (height, retention, archive, pruned) = pruner.get_status();
        assert_eq!(height, 500);
        assert_eq!(retention, 1000);
        assert!(archive);
        assert_eq!(pruned, 0);
    }
}
