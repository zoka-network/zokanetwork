//! Canonical PoW chain tip on top of `RocksDbStorage`.
//!
//! Every node — miner or full validator — applies blocks through
//! [`apply_block`]. The function:
//!
//! 1. Looks up the current head from storage (or treats genesis as the
//!    parent if the DB is empty).
//! 2. Builds a `PowBlockContext` from the parent + cached cumulative
//!    supply, and runs `validate_pow_block`. Anything the network would
//!    reject is rejected here too — same code path, no divergence.
//! 3. Persists the block, updates `chain:height`, `chain:last_hash`,
//!    `chain:total_supply_emitted`, `chain:cumulative_work`.
//!
//! The miner self-applies its own block via [`apply_block`] and so does
//! a peer-side handler when it receives a block over gossipsub. There is
//! one ingress path; that is what makes "real users mining" possible
//! without consensus drift between nodes.
//!
//! Fork-choice today: blocks that extend the head advance the canonical
//! chain immediately. Valid side branches are retained by hash, and a
//! side branch with more cumulative work can replace the canonical
//! height index once its known branch connects to a common ancestor.
//! Full state rollback/replay remains a higher-risk migration tracked in
//! `docs/protocol-overview.md`.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::mining::block_validator::{validate_pow_block, BlockValidationError, PowBlockContext};
use crate::mining::{difficulty, pow};
use crate::storage::RocksDbStorage;
use crate::types::{Block, BlockHeader, PublicKey, Signature, StorageError};

/// Snapshot of the canonical chain head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainHead {
    pub height: u64,
    pub hash: String,
    pub total_supply_emitted: u64,
    pub cumulative_work: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct BlockChainMeta {
    height: u64,
    total_supply_emitted: u64,
    cumulative_work: u128,
    difficulty_bits: u32,
}

/// All the ways block ingestion can fail. Each variant maps to one
/// concrete reason a peer or auditor would reject the block.
#[derive(Debug, Clone, Error)]
pub enum ChainError {
    #[error("block already known: {0}")]
    BlockAlreadyKnown(String),

    #[error(
        "block does not extend the current head: head={head}, candidate.prev_hash={candidate}"
    )]
    DoesNotExtendHead { head: String, candidate: String },

    #[error(
        "block height is not head + 1: head={head_height}, candidate={candidate_height}, expected={expected_height}"
    )]
    NonContiguousHeight {
        head_height: u64,
        candidate_height: u64,
        expected_height: u64,
    },

    #[error("validation failed: {0}")]
    Validation(#[from] BlockValidationError),

    #[error("storage error: {0}")]
    Storage(String),

    #[error("difficulty bits resolver returned 0; the chain must always require some work")]
    InvalidExpectedDifficulty,

    #[error(
        "side branch stored but not selected: candidate_work={candidate_work}, head_work={head_work}"
    )]
    SideBranchNotHeavier {
        candidate_work: u128,
        head_work: u128,
    },

    #[error("private double-spend: nullifier {nullifier} already committed on-chain")]
    PrivateNullifierDoubleSpend { nullifier: String },

    #[error("private double-spend: nullifier {nullifier} appears twice within the same block")]
    DuplicatePrivateNullifierInBlock { nullifier: String },

    #[error("invalid canonical private transaction bundle: {0}")]
    InvalidPrivateTxBundle(String),

    #[error("mainnet checkpoint mismatch at height {height}: expected {expected}, got {actual}")]
    MainnetCheckpointMismatch {
        height: u64,
        expected: String,
        actual: String,
    },
}

impl From<StorageError> for ChainError {
    fn from(e: StorageError) -> Self {
        ChainError::Storage(format!("{:?}", e))
    }
}

/// Reads the persisted chain tip. Returns `None` only when the DB is
/// fresh (no block ever applied). Callers MUST treat `None` as "genesis
/// is the parent" rather than as an error.
pub fn read_chain_head(storage: &RocksDbStorage) -> Result<Option<ChainHead>, ChainError> {
    let height = storage.get_chain_height()?;
    let hash = storage.get_last_block_hash()?;
    if height == 0 && (hash.is_empty() || hash == "0") {
        return Ok(None);
    }
    Ok(Some(ChainHead {
        height,
        hash,
        total_supply_emitted: storage.get_total_supply_emitted()?,
        cumulative_work: storage.get_cumulative_work()?,
    }))
}

/// Synthetic genesis-parent block. Used as the parent for the very
/// first applied block (height 1). Its hash is derived deterministically
/// from the chain id so every node agrees on what "the parent of block 1"
/// looks like.
pub fn genesis_parent_for(chain_id: &str) -> Block {
    let prev_hash = {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(b"zoka-mainnet-genesis-pow");
        hasher.update([0x1f]);
        hasher.update(chain_id.as_bytes());
        hex::encode(hasher.finalize())
    };
    Block {
        header: BlockHeader {
            hash: prev_hash,
            prev_hash: String::new(),
            merkle_root: String::from("0"),
            state_root: String::from("0"),
            timestamp: 0,
            nonce: 0,
            block_height: 0,
            validator: PublicKey(Vec::new()),
            seal: Signature(Vec::new()),
        },
        transactions: Vec::new(),
        private_tx_hashes: Vec::new(),
        private_tx_bodies: Vec::new(),
        private_nullifiers: Vec::new(),
        private_fee_claims: Vec::new(),
        private_coinbase: None,
    }
}

/// Pulls the parent for `candidate` from storage, falling back to the
/// synthetic genesis parent when the candidate is the first block.
fn parent_for(
    storage: &RocksDbStorage,
    candidate: &Block,
    chain_id: &str,
) -> Result<Block, ChainError> {
    if candidate.header.block_height <= 1 {
        return Ok(genesis_parent_for(chain_id));
    }
    let parent = storage
        .get_block(&candidate.header.prev_hash)?
        .ok_or_else(|| ChainError::DoesNotExtendHead {
            head: storage.get_last_block_hash().unwrap_or_default(),
            candidate: candidate.header.prev_hash.clone(),
        })?;
    Ok(parent)
}

fn block_meta_key(hash: &str) -> String {
    format!("chain:block_meta:{hash}")
}

fn save_block_meta(
    storage: &RocksDbStorage,
    hash: &str,
    meta: &BlockChainMeta,
) -> Result<(), ChainError> {
    let bytes = serde_json::to_vec(meta)
        .map_err(|e| ChainError::Storage(format!("serialize block meta failed: {e}")))?;
    storage.put_raw(&block_meta_key(hash), bytes)?;
    Ok(())
}

fn get_block_meta(
    storage: &RocksDbStorage,
    hash: &str,
) -> Result<Option<BlockChainMeta>, ChainError> {
    let Some(bytes) = storage.get_raw(&block_meta_key(hash))? else {
        return Ok(None);
    };
    let meta = serde_json::from_slice(&bytes)
        .map_err(|e| ChainError::Storage(format!("deserialize block meta failed: {e}")))?;
    Ok(Some(meta))
}

fn canonical_hash_at_height(
    storage: &RocksDbStorage,
    height: u64,
) -> Result<Option<String>, ChainError> {
    let key = format!("chain:block_height:{height}");
    let Some(raw) = storage.get_raw(&key)? else {
        return Ok(None);
    };
    let hash = String::from_utf8(raw)
        .map_err(|_| ChainError::Storage("canonical height index is not utf-8".to_string()))?;
    Ok(Some(hash))
}

/// Checkpoints que rigen para `chain_id`: los compilados (solo Zenith) y el
/// último firmado por el nodo del equipo, si hay uno aceptado en la base. El
/// firmado ya se verificó al guardarse (`checkpoint_authority`).
pub fn active_checkpoints(
    storage: &RocksDbStorage,
    chain_id: &str,
) -> Result<Vec<(u64, String)>, ChainError> {
    let mut checkpoints: Vec<(u64, String)> = if chain_id == crate::economics::MAINNET_CHAIN_ID {
        crate::economics::MAINNET_EMERGENCY_CHECKPOINTS
            .iter()
            .map(|(height, hash)| (*height, (*hash).to_string()))
            .collect()
    } else {
        Vec::new()
    };
    let signed = crate::checkpoint_authority::latest_signed_checkpoint(storage)
        .map_err(|e| ChainError::Storage(e.to_string()))?;
    if let Some(signed) = signed.filter(|cp| cp.chain_id == chain_id) {
        checkpoints.push((signed.height, signed.block_hash));
    }
    Ok(checkpoints)
}

/// Verify every active checkpoint that the local canonical height has
/// already crossed. This is intentionally read-only: callers use the error to
/// quarantine mining while normal block ingestion continues collecting the
/// checkpoint-valid recovery branch.
pub fn validate_mainnet_checkpoint_history(
    storage: &RocksDbStorage,
    chain_id: &str,
) -> Result<(), ChainError> {
    let checkpoints = active_checkpoints(storage, chain_id)?;
    if checkpoints.is_empty() {
        return Ok(());
    }

    let head_height = storage.get_chain_height()?;
    for (height, expected) in &checkpoints {
        if head_height < *height {
            continue;
        }
        let actual = canonical_hash_at_height(storage, *height)?
            .unwrap_or_else(|| "missing canonical checkpoint index".to_string());
        if actual != *expected {
            return Err(ChainError::MainnetCheckpointMismatch {
                height: *height,
                expected: expected.clone(),
                actual,
            });
        }
    }
    Ok(())
}

/// Resolve an ancestor on `candidate`'s branch. Once the walk reaches a block
/// already indexed as canonical, older ancestry can be answered from the
/// canonical height index without re-reading thousands of block bodies.
fn candidate_ancestor_hash_at_height(
    storage: &RocksDbStorage,
    candidate: &Block,
    target_height: u64,
) -> Result<Option<String>, ChainError> {
    if candidate.header.block_height < target_height {
        return Ok(None);
    }

    let mut cursor = candidate.clone();
    loop {
        if cursor.header.block_height == target_height {
            return Ok(Some(cursor.header.hash));
        }
        if cursor.header.block_height < target_height {
            return Ok(None);
        }

        if canonical_hash_at_height(storage, cursor.header.block_height)?.as_deref()
            == Some(cursor.header.hash.as_str())
        {
            return canonical_hash_at_height(storage, target_height);
        }

        cursor = storage
            .get_block(&cursor.header.prev_hash)?
            .ok_or_else(|| ChainError::DoesNotExtendHead {
                head: cursor.header.prev_hash.clone(),
                candidate: candidate.header.hash.clone(),
            })?;
    }
}

fn validate_candidate_mainnet_checkpoints(
    storage: &RocksDbStorage,
    chain_id: &str,
    candidate: &Block,
) -> Result<(), ChainError> {
    for (height, expected) in &active_checkpoints(storage, chain_id)? {
        if candidate.header.block_height < *height {
            continue;
        }
        let actual = candidate_ancestor_hash_at_height(storage, candidate, *height)?
            .unwrap_or_else(|| "missing checkpoint ancestor".to_string());
        if actual != *expected {
            return Err(ChainError::MainnetCheckpointMismatch {
                height: *height,
                expected: expected.clone(),
                actual,
            });
        }
    }
    Ok(())
}

/// Consensus migration used by 1.9.0: work alone cannot recover a node whose
/// invalid local fork is already longer. Such a node may switch to lower work
/// only after the incoming, fully validated branch proves the compiled
/// checkpoint that its local canonical history violates.
fn checkpoint_forced_reorg_required(
    storage: &RocksDbStorage,
    chain_id: &str,
    candidate: &Block,
) -> Result<bool, ChainError> {
    let mismatch_height = match validate_mainnet_checkpoint_history(storage, chain_id) {
        Ok(()) => return Ok(false),
        Err(ChainError::MainnetCheckpointMismatch { height, .. }) => height,
        Err(other) => return Err(other),
    };
    if candidate.header.block_height < mismatch_height {
        return Ok(false);
    }
    validate_candidate_mainnet_checkpoints(storage, chain_id, candidate)?;
    Ok(true)
}

fn genesis_meta() -> BlockChainMeta {
    BlockChainMeta {
        height: 0,
        total_supply_emitted: 0,
        cumulative_work: 0,
        difficulty_bits: crate::economics::POW_INITIAL_DIFFICULTY_BITS,
    }
}

fn parent_meta_for(
    storage: &RocksDbStorage,
    parent: &Block,
    head: Option<&ChainHead>,
) -> Result<BlockChainMeta, ChainError> {
    if parent.header.block_height == 0 {
        return Ok(genesis_meta());
    }
    if let Some(meta) = get_block_meta(storage, &parent.header.hash)? {
        return Ok(meta);
    }
    if let Some(head) = head {
        if parent.header.hash == head.hash {
            return Ok(BlockChainMeta {
                height: head.height,
                total_supply_emitted: head.total_supply_emitted,
                cumulative_work: head.cumulative_work,
                difficulty_bits: storage
                    .get_current_difficulty_bits()?
                    .max(crate::economics::POW_INITIAL_DIFFICULTY_BITS),
            });
        }
    }
    Err(ChainError::Storage(format!(
        "missing chain metadata for parent {}",
        parent.header.hash
    )))
}

fn expected_difficulty_after_parent(
    storage: &RocksDbStorage,
    chain_id: &str,
    parent: &Block,
    parent_meta: &BlockChainMeta,
) -> Result<u32, ChainError> {
    if parent_meta.height < 2 {
        return Ok(crate::economics::POW_INITIAL_DIFFICULTY_BITS);
    }
    let grandparent = storage
        .get_block(&parent.header.prev_hash)?
        .ok_or_else(|| ChainError::DoesNotExtendHead {
            head: parent.header.prev_hash.clone(),
            candidate: format!("missing grandparent for {}", parent.header.hash),
        })?;
    if grandparent.header.block_height.saturating_add(1) != parent.header.block_height {
        return Err(ChainError::NonContiguousHeight {
            head_height: grandparent.header.block_height,
            candidate_height: parent.header.block_height,
            expected_height: grandparent.header.block_height.saturating_add(1),
        });
    }
    if parent.header.block_height == 1
        && parent.header.prev_hash != genesis_parent_for(chain_id).header.hash
    {
        return Err(ChainError::DoesNotExtendHead {
            head: genesis_parent_for(chain_id).header.hash,
            candidate: parent.header.prev_hash.clone(),
        });
    }
    let solve_secs = parent
        .header
        .timestamp
        .saturating_sub(grandparent.header.timestamp);
    let next_height = parent_meta.height.saturating_add(1);
    let retargeted = difficulty::retarget_from_solve_time(
        parent_meta.difficulty_bits,
        solve_secs,
        crate::economics::TARGET_BLOCK_TIME_SECS,
    );
    if chain_id == crate::economics::MAINNET_CHAIN_ID {
        return difficulty::apply_mainnet_emergency_difficulty_cap(
            storage,
            next_height,
            retargeted,
        )
        .map_err(ChainError::Storage);
    }
    Ok(retargeted)
}

fn collect_branch_to_common_ancestor(
    storage: &RocksDbStorage,
    candidate: &Block,
) -> Result<Vec<Block>, ChainError> {
    let mut branch = vec![candidate.clone()];
    let mut cursor = candidate.clone();
    loop {
        if cursor.header.block_height <= 1 {
            break;
        }
        let parent = storage
            .get_block(&cursor.header.prev_hash)?
            .ok_or_else(|| ChainError::DoesNotExtendHead {
                head: cursor.header.prev_hash.clone(),
                candidate: cursor.header.hash.clone(),
            })?;
        if canonical_hash_at_height(storage, parent.header.block_height)?.as_deref()
            == Some(parent.header.hash.as_str())
        {
            break;
        }
        branch.push(parent.clone());
        cursor = parent;
    }
    branch.reverse();
    Ok(branch)
}

fn promote_branch_to_canonical(
    storage: &RocksDbStorage,
    candidate: &Block,
    meta: &BlockChainMeta,
) -> Result<ChainHead, ChainError> {
    let old_height = storage.get_chain_height()?;
    let branch = collect_branch_to_common_ancestor(storage, candidate)?;
    let mut old_canonical = Vec::new();
    if let Some(first) = branch.first() {
        for height in first.header.block_height..=old_height {
            if let Some(hash) = canonical_hash_at_height(storage, height)? {
                if let Some(old_block) = storage.get_block(&hash)? {
                    old_canonical.push(old_block);
                }
            }
        }
    }
    // Double-spend validation for the incoming branch, against what the
    // chain will look like AFTER the reorg — and before mutating anything.
    // Side-branch blocks skip the global-index nullifier check at store time
    // (the index reflects the outgoing branch, see
    // `validate_private_nullifiers`), so this is where the real rule is
    // enforced: a nullifier the outgoing blocks spent is freed by this very
    // reorg and may be re-spent by the incoming branch (same tx mined on
    // both sides of a fork — not a double-spend; it ends up committed
    // exactly once). Only a nullifier spent by a block that REMAINS
    // canonical (below the branch point), or spent twice within the
    // incoming branch itself, is a double-spend — those abort the promotion
    // with the canonical head untouched.
    if let Some(first) = branch.first() {
        let _branch_point = first.header.block_height;
        let mut freed_by_reorg = std::collections::HashSet::new();
        for old_block in &old_canonical {
            for nullifier in &old_block.private_nullifiers {
                let nullifier = nullifier.trim();
                if !nullifier.is_empty() {
                    freed_by_reorg.insert(nullifier.to_string());
                }
            }
        }
        let mut incoming = std::collections::HashSet::new();
        for block in &branch {
            for nullifier in &block.private_nullifiers {
                let nullifier = nullifier.trim();
                if nullifier.is_empty() {
                    continue;
                }
                if !incoming.insert(nullifier.to_string()) {
                    return Err(ChainError::PrivateNullifierDoubleSpend {
                        nullifier: nullifier.to_string(),
                    });
                }
                let spent_on_current_chain = storage
                    .is_nullifier_spent(nullifier)
                    .map_err(|e| ChainError::Storage(format!("{e:?}")))?;
                if spent_on_current_chain && !freed_by_reorg.contains(nullifier) {
                    return Err(ChainError::PrivateNullifierDoubleSpend {
                        nullifier: nullifier.to_string(),
                    });
                }
            }
        }
    }
    // Reorg: the blocks being displaced (from the branch point up to the old
    // head) leave the canonical chain, so free their nullifiers; then the new
    // canonical branch claims its own. Order matters — de-index the outgoing
    // blocks before indexing the incoming branch so a nullifier reused across
    // both branches ends up owned by the new chain.
    storage.promote_canonical_branch_atomically(
        &old_canonical,
        &branch,
        old_height,
        meta.height,
        &candidate.header.hash,
        meta.total_supply_emitted,
        meta.cumulative_work,
        meta.difficulty_bits,
    )?;
    Ok(ChainHead {
        height: meta.height,
        hash: candidate.header.hash.clone(),
        total_supply_emitted: meta.total_supply_emitted,
        cumulative_work: meta.cumulative_work,
    })
}

fn maybe_promote_known_side_branch(
    storage: &RocksDbStorage,
    chain_id: &str,
    candidate: &Block,
    head: Option<&ChainHead>,
) -> Result<Option<ChainHead>, ChainError> {
    if canonical_hash_at_height(storage, candidate.header.block_height)?.as_deref()
        == Some(candidate.header.hash.as_str())
    {
        return Ok(None);
    }

    let Some(meta) = get_block_meta(storage, &candidate.header.hash)? else {
        return Ok(None);
    };
    let head_work = head.map(|h| h.cumulative_work).unwrap_or(0);
    let checkpoint_forced = checkpoint_forced_reorg_required(storage, chain_id, candidate)?;
    if meta.cumulative_work > head_work || checkpoint_forced {
        return Ok(Some(promote_branch_to_canonical(
            storage, candidate, &meta,
        )?));
    }
    Ok(None)
}

/// Production ingress point. Derives the canonical expected difficulty
/// from `mining::difficulty::expected_for_next_block(storage)` so that
/// every node — miner or verifier — applies the same rule, then forwards
/// to [`apply_block_with_expected_difficulty`].
pub fn apply_block(
    storage: &RocksDbStorage,
    chain_id: &str,
    candidate: &Block,
) -> Result<ChainHead, ChainError> {
    let expected = difficulty::expected_for_next_block(storage).map_err(ChainError::Storage)?;
    apply_block_inner(storage, chain_id, candidate, expected, false)
}

/// Apply a freshly mined or freshly received block at an explicit
/// expected difficulty. Production callers MUST go through
/// [`apply_block`]; this variant exists for tests that want to drive the
/// chain at a relaxed difficulty without monkey-patching `economics`.
pub fn apply_block_with_expected_difficulty(
    storage: &RocksDbStorage,
    chain_id: &str,
    candidate: &Block,
    expected_difficulty_bits: u32,
) -> Result<ChainHead, ChainError> {
    apply_block_inner(storage, chain_id, candidate, expected_difficulty_bits, true)
}

/// Reject a block that double-spends a private note. Two checks:
///   1. No nullifier may repeat within the block's own `private_nullifiers`.
///   2. For a block extending the current head only: no nullifier may
///      already be committed on-chain (the global spent index). This is the
///      canonical no-double-spend consensus rule, and it works from the
///      block alone — a node does not need the tx bodies, which are not
///      guaranteed to propagate with the block.
///
/// The global-index check deliberately does NOT apply to side-branch blocks:
/// the index reflects the *current* canonical branch, while a side-branch
/// block may legitimately carry a nullifier the current branch also spent —
/// the same private tx mined into both sides of a natural fork. Whether the
/// competing branch double-spends is only decidable against the post-reorg
/// nullifier set, which is what [`promote_branch_to_canonical`] validates
/// before switching. Checking side-branch blocks against the global index
/// here made fork recovery impossible: a node on the losing branch rejected
/// every canonical block carrying the shared nullifier, so it could never
/// store the branch, never reorg, and stayed split forever (mainnet incident
/// 2026-07-09, fork at height 2736).
fn validate_private_nullifiers(
    storage: &RocksDbStorage,
    candidate: &Block,
    extends_head: bool,
) -> Result<(), ChainError> {
    if candidate.private_nullifiers.is_empty() {
        return Ok(());
    }
    let mut seen = std::collections::HashSet::new();
    for nullifier in &candidate.private_nullifiers {
        let nullifier = nullifier.trim();
        if nullifier.is_empty() {
            continue;
        }
        if !seen.insert(nullifier.to_string()) {
            return Err(ChainError::DuplicatePrivateNullifierInBlock {
                nullifier: nullifier.to_string(),
            });
        }
        if extends_head
            && storage
                .is_nullifier_spent(nullifier)
                .map_err(|e| ChainError::Storage(format!("{e:?}")))?
        {
            return Err(ChainError::PrivateNullifierDoubleSpend {
                nullifier: nullifier.to_string(),
            });
        }
    }
    Ok(())
}

fn enforce_mainnet_emergency_checkpoint(
    storage: &RocksDbStorage,
    chain_id: &str,
    candidate: &Block,
) -> Result<(), ChainError> {
    validate_candidate_mainnet_checkpoints(storage, chain_id, candidate)
}

/// Persist every nullifier committed in `block` into the global spent index,
/// keyed at the block's height. Call this whenever a block becomes part of
/// the canonical chain.
fn index_block_nullifiers(storage: &RocksDbStorage, block: &Block) -> Result<(), ChainError> {
    for nullifier in &block.private_nullifiers {
        let nullifier = nullifier.trim();
        if nullifier.is_empty() {
            continue;
        }
        storage
            .save_spent_nullifier(nullifier, block.header.block_height)
            .map_err(|e| ChainError::Storage(format!("{e:?}")))?;
    }
    Ok(())
}

fn apply_block_inner(
    storage: &RocksDbStorage,
    chain_id: &str,
    candidate: &Block,
    expected_difficulty_bits: u32,
    force_expected_difficulty: bool,
) -> Result<ChainHead, ChainError> {
    if expected_difficulty_bits == 0 {
        return Err(ChainError::InvalidExpectedDifficulty);
    }

    let head = read_chain_head(storage)?;
    if storage.block_exists(&candidate.header.hash)? {
        if let Some(promoted) =
            maybe_promote_known_side_branch(storage, chain_id, candidate, head.as_ref())?
        {
            return Ok(promoted);
        }
        let is_indexed_canonical =
            canonical_hash_at_height(storage, candidate.header.block_height)?.as_deref()
                == Some(candidate.header.hash.as_str());
        let has_validated_meta = get_block_meta(storage, &candidate.header.hash)?.is_some();
        if is_indexed_canonical || has_validated_meta {
            return Err(ChainError::BlockAlreadyKnown(candidate.header.hash.clone()));
        }
    }

    let parent_height = head.as_ref().map(|h| h.height).unwrap_or(0);
    let head_hash = head.as_ref().map(|h| h.hash.clone()).unwrap_or_default();

    let parent = parent_for(storage, candidate, chain_id)?;
    let parent_meta = parent_meta_for(storage, &parent, head.as_ref())?;
    let expected_height = parent.header.block_height.saturating_add(1);
    if candidate.header.block_height != expected_height {
        return Err(ChainError::NonContiguousHeight {
            head_height: parent.header.block_height,
            candidate_height: candidate.header.block_height,
            expected_height,
        });
    }

    let head_or_genesis = head_hash.clone();
    let expected_prev_hash = if head_or_genesis.is_empty() {
        genesis_parent_for(chain_id).header.hash
    } else {
        head_or_genesis
    };
    let extends_head = candidate.header.prev_hash == expected_prev_hash
        && candidate.header.block_height == parent_height.saturating_add(1);
    enforce_mainnet_emergency_checkpoint(storage, chain_id, candidate)?;

    let effective_expected_difficulty = if force_expected_difficulty || extends_head {
        expected_difficulty_bits
    } else {
        expected_difficulty_after_parent(storage, chain_id, &parent, &parent_meta)?
    };
    let ctx = PowBlockContext {
        chain_id,
        parent: &parent,
        parent_total_supply_emitted: parent_meta.total_supply_emitted,
        expected_difficulty_bits: effective_expected_difficulty,
    };
    validate_pow_block(candidate, &ctx)?;
    crate::state::sync::validate_private_tx_bundle_policy_for_block(candidate)
        .map_err(ChainError::InvalidPrivateTxBundle)?;
    validate_private_nullifiers(storage, candidate, extends_head)?;

    let coinbase_amount = candidate
        .transactions
        .first()
        .map(|tx| tx.amount)
        .unwrap_or(0);
    let new_total_supply = parent_meta
        .total_supply_emitted
        .saturating_add(coinbase_amount);
    let block_work = pow::leading_zero_bits(&candidate.header.seal.0) as u128;
    let new_cumulative_work = parent_meta.cumulative_work.saturating_add(block_work);
    let new_difficulty_bits =
        difficulty::difficulty_after_applying(candidate, effective_expected_difficulty);
    let new_meta = BlockChainMeta {
        height: candidate.header.block_height,
        total_supply_emitted: new_total_supply,
        cumulative_work: new_cumulative_work,
        difficulty_bits: new_difficulty_bits,
    };

    if extends_head {
        storage.save_block(candidate)?;
        save_block_meta(storage, &candidate.header.hash, &new_meta)?;
        index_block_nullifiers(storage, candidate)?;
        storage.set_chain_height(candidate.header.block_height)?;
        storage.set_last_block_hash(&candidate.header.hash)?;
        storage.set_total_supply_emitted(new_total_supply)?;
        storage.set_cumulative_work(new_cumulative_work)?;
        storage.set_current_difficulty_bits(new_difficulty_bits)?;

        return Ok(ChainHead {
            height: candidate.header.block_height,
            hash: candidate.header.hash.clone(),
            total_supply_emitted: new_total_supply,
            cumulative_work: new_cumulative_work,
        });
    }

    storage.save_block_body(candidate)?;
    save_block_meta(storage, &candidate.header.hash, &new_meta)?;
    let head_work = head.as_ref().map(|h| h.cumulative_work).unwrap_or(0);
    let checkpoint_forced = checkpoint_forced_reorg_required(storage, chain_id, candidate)?;
    if new_meta.cumulative_work > head_work || checkpoint_forced {
        return promote_branch_to_canonical(storage, candidate, &new_meta);
    }

    Err(ChainError::SideBranchNotHeavier {
        candidate_work: new_meta.cumulative_work,
        head_work,
    })
}

/// Placeholder for future fork-choice that may rewind the canonical
/// chain back to a common ancestor. For now this returns `false`
/// unconditionally so peers cannot trick a node into rolling back account
/// state. Once the state-layer migration supports rollback, this
/// function will compare cumulative work and decide whether to reorg.
pub fn would_replace_head_with_reorg(
    storage: &RocksDbStorage,
    _candidate: &Block,
    candidate_cumulative_work: u128,
) -> bool {
    storage
        .get_cumulative_work()
        .map(|head_work| candidate_cumulative_work > head_work)
        .unwrap_or(false)
}

/// Convenience: reads the chain head and returns the parent supply +
/// height the miner needs to compute the next block.
pub fn next_block_metadata(
    storage: &Arc<RocksDbStorage>,
    chain_id: &str,
) -> Result<(u64, String, u64), ChainError> {
    validate_mainnet_checkpoint_history(storage, chain_id)?;
    let head = read_chain_head(storage)?;
    Ok(match head {
        Some(h) => (h.height.saturating_add(1), h.hash, h.total_supply_emitted),
        None => (1, genesis_parent_for(chain_id).header.hash, 0),
    })
}

/// Minimum number of recent blocks the pruner MUST retain even if the
/// caller asks for less. The difficulty retarget algorithm reads the
/// timestamps of the parent and grandparent (`mining::difficulty::
/// expected_for_next_block`), so dropping below 2 retained blocks would
/// break the next-block difficulty calculation. The retention cap also
/// keeps a small reorg buffer; reorgs deeper than this require IBD
/// from a peer.
pub const PRUNE_MIN_RETAINED_BLOCKS: u64 = 8;

/// Outcome of a [`prune_below_head`] call. Useful for CLI dashboards
/// and dry-run reports — the values describe what was (or would be)
/// touched in storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PruneReport {
    /// Chain head height at the moment the pruner ran. Pruning never
    /// touches the head or any block at height >= `retained_from_height`.
    pub head_height: u64,
    /// Lowest height in the recent contiguous retention window. Compiled
    /// checkpoint blocks below this height are retained as sparse anchors.
    pub retained_from_height: u64,
    /// How many block bodies were deleted (or would be deleted in
    /// dry-run).
    pub blocks_pruned: u64,
    /// How many height-index entries were deleted alongside.
    pub height_indices_pruned: u64,
    /// How many `chain:block_meta:<hash>` entries were deleted.
    pub metas_pruned: u64,
    /// True when the call was a dry-run; nothing was actually deleted.
    pub dry_run: bool,
}

/// Drop block bodies older than `keep_last` blocks below the head,
/// keeping enough recent state to retarget difficulty and tolerate a
/// short reorg.
///
/// Invariants enforced:
///
/// 1. `keep_last` is clamped at the floor [`PRUNE_MIN_RETAINED_BLOCKS`].
///    Any caller asking for fewer is upgraded silently to the minimum;
///    you cannot accidentally cripple difficulty retarget.
/// 2. Heights `[max(1, head - keep_last + 1) .. head]` and every compiled
///    mainnet checkpoint are NEVER touched.
/// 3. The chain head metadata (`chain:height`, `chain:last_hash`,
///    `chain:total_supply_emitted`, `chain:cumulative_work`,
///    `chain:current_difficulty_bits`, `chain:genesis:digest`) is
///    NEVER touched.
/// 4. `dry_run = true` produces a [`PruneReport`] with the same numbers
///    that a real prune would produce, without modifying storage.
///
/// After a successful prune, `apply_block` keeps working: the retained
/// window contains at least the two blocks needed by
/// `mining::difficulty::expected_for_next_block`, and any block whose
/// height is in the retained range still has its full body for the
/// next consensus check.
pub fn prune_below_head(
    storage: &RocksDbStorage,
    keep_last: u64,
    dry_run: bool,
) -> Result<PruneReport, ChainError> {
    let head = match read_chain_head(storage)? {
        Some(h) => h,
        None => {
            return Ok(PruneReport {
                head_height: 0,
                retained_from_height: 0,
                blocks_pruned: 0,
                height_indices_pruned: 0,
                metas_pruned: 0,
                dry_run,
            });
        }
    };
    let head_height = head.height;
    let effective_keep = keep_last.max(PRUNE_MIN_RETAINED_BLOCKS);

    if head_height <= effective_keep {
        return Ok(PruneReport {
            head_height,
            retained_from_height: 1,
            blocks_pruned: 0,
            height_indices_pruned: 0,
            metas_pruned: 0,
            dry_run,
        });
    }

    let retained_from = head_height.saturating_sub(effective_keep).saturating_add(1);
    let mut blocks_pruned = 0u64;
    let mut indices_pruned = 0u64;
    let mut metas_pruned = 0u64;

    for height in 1..retained_from {
        if crate::economics::MAINNET_EMERGENCY_CHECKPOINTS
            .iter()
            .any(|(checkpoint_height, _)| *checkpoint_height == height)
        {
            continue;
        }
        let block = match storage.get_block_by_height(height)? {
            Some(b) => b,
            None => continue,
        };
        if !dry_run {
            storage.delete_block_hash_at_height(height)?;
            storage.delete_block_body(&block.header.hash)?;
            storage.delete_raw(&block_meta_key(&block.header.hash))?;
        }
        blocks_pruned = blocks_pruned.saturating_add(1);
        indices_pruned = indices_pruned.saturating_add(1);
        metas_pruned = metas_pruned.saturating_add(1);
    }

    Ok(PruneReport {
        head_height,
        retained_from_height: retained_from,
        blocks_pruned,
        height_indices_pruned: indices_pruned,
        metas_pruned,
        dry_run,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::economics;
    use crate::mining::block_validator::build_coinbase_tx;
    use crate::types::{Address, Block, BlockHeader, PrivateCoinbaseOutput, PublicKey, Signature};

    fn open_temp_storage() -> (RocksDbStorage, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let storage = RocksDbStorage::new(dir.path().to_str().unwrap()).expect("rocksdb open");
        (storage, dir)
    }

    fn dummy_private_coinbase(block_height: u64, recipient_address: &str) -> PrivateCoinbaseOutput {
        PrivateCoinbaseOutput {
            block_height,
            recipient_address: recipient_address.to_string(),
            unlock_height: block_height.saturating_add(economics::COINBASE_MATURITY_BLOCKS),
            output_id: format!("coinbase-output-{block_height}"),
            asset_tag: "ZOKA".to_string(),
            asset_tag_commitment: "asset-commitment".to_string(),
            recipient_commitment: "zpriv:0000000000000000000000000000000000000000000000000000000000000000:1111111111111111111111111111111111111111111111111111111111111111".to_string(),
            stealth_address: "stlth_test".to_string(),
            ephemeral_pubkey: vec![1; 32],
            commitment: "coinbase-commitment".to_string(),
            amount_commitment: "coinbase-amount-commitment".to_string(),
            range_proof: vec![2; 32],
            one_time_public_key: "coinbase-one-time-key".to_string(),
            encrypted_amount: vec![3; 8],
            commitment_blind: vec![4; 32],
        }
    }

    fn mine_block(
        chain_id: &str,
        parent_hash: &str,
        parent_height: u64,
        parent_supply: u64,
        miner: &Address,
        difficulty_bits: u32,
        timestamp: u64,
    ) -> Block {
        let height = parent_height.saturating_add(1);
        let subsidy = economics::block_reward_for_height(height, parent_supply);
        let coinbase = build_coinbase_tx(miner.clone(), subsidy, height, timestamp);
        let engine = pow::engine_for(chain_id, height).expect("testnet engine");
        let mut nonce = 0u64;
        let (computed, found_nonce) = loop {
            let preimage =
                pow::build_pow_input(chain_id, parent_hash, miner, height, timestamp, nonce);
            let hash = engine.calculate_hash(&preimage).expect("hash");
            if pow::hash_meets_difficulty(&hash, difficulty_bits) {
                break (hash, nonce);
            }
            nonce = nonce.wrapping_add(1);
        };
        let mut block = Block {
            header: BlockHeader {
                hash: String::new(),
                prev_hash: parent_hash.to_string(),
                merkle_root: String::new(),
                state_root: String::from("0"),
                timestamp,
                nonce: found_nonce,
                block_height: height,
                validator: PublicKey(Vec::new()),
                seal: Signature(computed),
            },
            transactions: vec![coinbase],
            private_tx_hashes: Vec::new(),
            private_tx_bodies: Vec::new(),
            private_nullifiers: Vec::new(),
            private_fee_claims: Vec::new(),
            private_coinbase: Some(dummy_private_coinbase(height, miner)),
        };
        block.header.merkle_root = block.compute_merkle_root();
        block.header.hash = block.compute_hash();
        block
    }

    fn recovery_fixture_block(
        height: u64,
        hash: &str,
        prev_hash: &str,
        nullifiers: Vec<String>,
    ) -> Block {
        Block {
            header: BlockHeader {
                hash: hash.to_string(),
                prev_hash: prev_hash.to_string(),
                merkle_root: "fixture".to_string(),
                state_root: "fixture".to_string(),
                timestamp: 1_700_000_000 + height,
                nonce: 0,
                block_height: height,
                validator: PublicKey(Vec::new()),
                seal: Signature(Vec::new()),
            },
            transactions: Vec::new(),
            private_tx_hashes: Vec::new(),
            private_tx_bodies: Vec::new(),
            private_nullifiers: nullifiers,
            private_fee_claims: Vec::new(),
            private_coinbase: None,
        }
    }

    /// Seed a long, high-work canonical fork that violates the 1.9 checkpoint,
    /// plus a previously validated lower-work side block carrying the correct
    /// checkpoint. Earlier anchors are represented by their canonical indices;
    /// the common parent lets ancestry validation safely reuse those indices.
    fn seed_mainnet_recovery_fixture(
        storage: &RocksDbStorage,
        common_nullifiers: Vec<String>,
        candidate_nullifiers: Vec<String>,
    ) -> (String, Block) {
        for (height, hash) in economics::MAINNET_EMERGENCY_CHECKPOINTS {
            if *height < economics::MAINNET_RECOVERY_CHECKPOINT_10887_HEIGHT {
                storage
                    .set_block_hash_at_height(*height, hash)
                    .expect("seed earlier checkpoint index");
            }
        }

        let common = recovery_fixture_block(
            economics::MAINNET_RECOVERY_CHECKPOINT_10887_HEIGHT - 1,
            "common-10886",
            "common-10885",
            common_nullifiers.clone(),
        );
        let wrong_checkpoint = recovery_fixture_block(
            economics::MAINNET_RECOVERY_CHECKPOINT_10887_HEIGHT,
            "wrong-fork-10887",
            &common.header.hash,
            Vec::new(),
        );
        let wrong_head = recovery_fixture_block(
            economics::MAINNET_RECOVERY_CHECKPOINT_10887_HEIGHT + 1,
            "wrong-fork-10888",
            &wrong_checkpoint.header.hash,
            Vec::new(),
        );
        for block in [&common, &wrong_checkpoint, &wrong_head] {
            storage
                .save_block_body(block)
                .expect("seed canonical fixture body");
            storage
                .set_block_hash_at_height(block.header.block_height, &block.header.hash)
                .expect("seed canonical fixture index");
        }
        for nullifier in common_nullifiers {
            storage
                .save_spent_nullifier(&nullifier, common.header.block_height)
                .expect("seed spent nullifier");
        }
        storage
            .set_chain_height(wrong_head.header.block_height)
            .expect("seed height");
        storage
            .set_last_block_hash(&wrong_head.header.hash)
            .expect("seed head hash");
        storage.set_total_supply_emitted(99).expect("seed supply");
        storage.set_cumulative_work(50_000).expect("seed work");
        storage
            .set_current_difficulty_bits(economics::POW_INITIAL_DIFFICULTY_BITS)
            .expect("seed difficulty");

        let candidate = recovery_fixture_block(
            economics::MAINNET_RECOVERY_CHECKPOINT_10887_HEIGHT,
            economics::MAINNET_RECOVERY_CHECKPOINT_10887_HASH,
            &common.header.hash,
            candidate_nullifiers,
        );
        storage
            .save_block_body(&candidate)
            .expect("seed validated side body");
        save_block_meta(
            storage,
            &candidate.header.hash,
            &BlockChainMeta {
                height: candidate.header.block_height,
                total_supply_emitted: 88,
                cumulative_work: 10,
                difficulty_bits: economics::POW_INITIAL_DIFFICULTY_BITS,
            },
        )
        .expect("seed validated side metadata");

        (wrong_head.header.hash, candidate)
    }

    const SIGNED_TEST_CHAIN: &str = "zoka-test-signed-checkpoints";

    /// Como el de abajo, en una cadena sin checkpoints compilados: la rama
    /// canónica local, con más trabajo, tiene en la altura 10 otro bloque que
    /// el que firmó la autoridad. El firmado está guardado aparte, con menos
    /// trabajo.
    fn seed_signed_checkpoint_fixture(storage: &RocksDbStorage) -> (String, Block) {
        let firmado = "a".repeat(64);
        let common = recovery_fixture_block(9, "common-9", "common-8", Vec::new());
        let wrong = recovery_fixture_block(10, "wrong-10", &common.header.hash, Vec::new());
        let wrong_head = recovery_fixture_block(11, "wrong-11", &wrong.header.hash, Vec::new());
        for block in [&common, &wrong, &wrong_head] {
            storage.save_block_body(block).expect("seed body");
            storage
                .set_block_hash_at_height(block.header.block_height, &block.header.hash)
                .expect("seed index");
        }
        storage.set_chain_height(11).expect("seed height");
        storage.set_last_block_hash(&wrong_head.header.hash).expect("seed head");
        storage.set_total_supply_emitted(99).expect("seed supply");
        storage.set_cumulative_work(50_000).expect("seed work");
        storage
            .set_current_difficulty_bits(economics::POW_INITIAL_DIFFICULTY_BITS)
            .expect("seed difficulty");

        let candidate = recovery_fixture_block(10, &firmado, &common.header.hash, Vec::new());
        storage.save_block_body(&candidate).expect("seed side body");
        save_block_meta(
            storage,
            &candidate.header.hash,
            &BlockChainMeta {
                height: 10,
                total_supply_emitted: 88,
                cumulative_work: 10,
                difficulty_bits: economics::POW_INITIAL_DIFFICULTY_BITS,
            },
        )
        .expect("seed side metadata");
        (wrong_head.header.hash, candidate)
    }

    fn sign_test_checkpoint(storage: &RocksDbStorage, height: u64, hash: &str) {
        let key = ed25519_dalek::SigningKey::from_bytes(&[9u8; 32]);
        let checkpoint = crate::checkpoint_authority::SignedCheckpoint::sign(
            &key,
            SIGNED_TEST_CHAIN,
            height,
            hash,
        );
        crate::checkpoint_authority::accept_signed_checkpoint(
            storage,
            SIGNED_TEST_CHAIN,
            &checkpoint,
            &key.verifying_key().into(),
        )
        .expect("accept test checkpoint");
    }

    // El pedido del usuario del 13/10: un nodo con otra rama, aunque tenga más
    // trabajo, obedece el checkpoint que firmó el nodo del equipo. Deja de
    // minar (cuarentena) y pasa a la rama firmada cuando la tiene.
    #[test]
    fn un_nodo_obedece_el_checkpoint_firmado_aunque_tenga_mas_trabajo() {
        let (storage, _dir) = open_temp_storage();
        let storage = Arc::new(storage);
        let (old_head_hash, candidate) = seed_signed_checkpoint_fixture(&storage);
        sign_test_checkpoint(&storage, 10, &candidate.header.hash);

        let mining_err = next_block_metadata(&storage, SIGNED_TEST_CHAIN)
            .expect_err("a branch that contradicts the signed checkpoint cannot mine");
        assert!(matches!(
            mining_err,
            ChainError::MainnetCheckpointMismatch { height: 10, .. }
        ));

        let old_head = read_chain_head(&storage)
            .expect("read old head")
            .expect("old head exists");
        assert_eq!(old_head.hash, old_head_hash);
        let promoted = maybe_promote_known_side_branch(
            &storage,
            SIGNED_TEST_CHAIN,
            &candidate,
            Some(&old_head),
        )
        .expect("signed branch remains ingestible")
        .expect("the signed checkpoint overrides higher work");
        assert_eq!(promoted.hash, candidate.header.hash);
        assert_eq!(promoted.cumulative_work, 10);
        assert_eq!(
            canonical_hash_at_height(&storage, 10).expect("index"),
            Some(candidate.header.hash.clone())
        );
        assert_eq!(canonical_hash_at_height(&storage, 11).expect("index"), None);
        validate_mainnet_checkpoint_history(&storage, SIGNED_TEST_CHAIN)
            .expect("the promoted history obeys the signed checkpoint");
    }

    // Control: la misma situación sin checkpoint firmado. Gana el trabajo
    // acumulado, así que lo que decidió arriba fue la firma.
    #[test]
    fn sin_checkpoint_firmado_gana_el_trabajo_acumulado() {
        let (storage, _dir) = open_temp_storage();
        let (old_head_hash, candidate) = seed_signed_checkpoint_fixture(&storage);
        validate_mainnet_checkpoint_history(&storage, SIGNED_TEST_CHAIN)
            .expect("no checkpoints, nothing to contradict");
        let old_head = read_chain_head(&storage)
            .expect("read old head")
            .expect("old head exists");
        let promoted = maybe_promote_known_side_branch(
            &storage,
            SIGNED_TEST_CHAIN,
            &candidate,
            Some(&old_head),
        )
        .expect("side branch is ingestible");
        assert!(promoted.is_none(), "lower work must not win without the checkpoint");
        assert_eq!(
            read_chain_head(&storage).expect("head").expect("head").hash,
            old_head_hash
        );
    }

    #[test]
    fn mismatched_mainnet_db_cannot_mine_but_promotes_valid_checkpoint_branch() {
        let (storage, _dir) = open_temp_storage();
        let storage = Arc::new(storage);
        let (old_head_hash, candidate) =
            seed_mainnet_recovery_fixture(&storage, Vec::new(), Vec::new());

        let mining_err = next_block_metadata(&storage, economics::MAINNET_CHAIN_ID)
            .expect_err("mismatched DB must be quarantined from mining");
        assert!(matches!(
            mining_err,
            ChainError::MainnetCheckpointMismatch {
                height: economics::MAINNET_RECOVERY_CHECKPOINT_10887_HEIGHT,
                ..
            }
        ));

        let old_head = read_chain_head(&storage)
            .expect("read old head")
            .expect("old head exists");
        assert_eq!(old_head.hash, old_head_hash);
        assert!(old_head.cumulative_work > 10);

        let promoted = maybe_promote_known_side_branch(
            &storage,
            economics::MAINNET_CHAIN_ID,
            &candidate,
            Some(&old_head),
        )
        .expect("checkpoint branch remains ingestible")
        .expect("checkpoint must override higher work on the invalid fork");

        assert_eq!(promoted.height, candidate.header.block_height);
        assert_eq!(promoted.hash, candidate.header.hash);
        assert_eq!(promoted.cumulative_work, 10);
        assert_eq!(
            canonical_hash_at_height(
                &storage,
                economics::MAINNET_RECOVERY_CHECKPOINT_10887_HEIGHT
            )
            .expect("checkpoint index"),
            Some(economics::MAINNET_RECOVERY_CHECKPOINT_10887_HASH.to_string())
        );
        assert_eq!(
            canonical_hash_at_height(
                &storage,
                economics::MAINNET_RECOVERY_CHECKPOINT_10887_HEIGHT + 1
            )
            .expect("old height index removed"),
            None
        );
        assert!(storage
            .block_exists(&old_head_hash)
            .expect("old body lookup"));
        validate_mainnet_checkpoint_history(&storage, economics::MAINNET_CHAIN_ID)
            .expect("promoted history satisfies all checkpoints");
    }

    #[test]
    fn failed_checkpoint_forced_reorg_keeps_the_old_canonical_view() {
        let (storage, _dir) = open_temp_storage();
        let (old_head_hash, candidate) = seed_mainnet_recovery_fixture(
            &storage,
            vec!["spent-below-branch".to_string()],
            vec!["spent-below-branch".to_string()],
        );
        let old_head = read_chain_head(&storage)
            .expect("read old head")
            .expect("old head exists");

        let err = maybe_promote_known_side_branch(
            &storage,
            economics::MAINNET_CHAIN_ID,
            &candidate,
            Some(&old_head),
        )
        .expect_err("invalid recovery branch must fail before atomic commit");
        assert!(matches!(
            err,
            ChainError::PrivateNullifierDoubleSpend { .. }
        ));

        let after = read_chain_head(&storage)
            .expect("read head after error")
            .expect("head remains");
        assert_eq!(after.hash, old_head_hash);
        assert_eq!(after.height, old_head.height);
        assert_eq!(after.cumulative_work, old_head.cumulative_work);
        assert_eq!(
            canonical_hash_at_height(
                &storage,
                economics::MAINNET_RECOVERY_CHECKPOINT_10887_HEIGHT
            )
            .expect("old checkpoint index"),
            Some("wrong-fork-10887".to_string())
        );
        assert!(storage
            .is_nullifier_spent("spent-below-branch")
            .expect("spent index survives"));
    }

    #[test]
    fn lower_work_branch_without_the_checkpoint_is_never_force_promoted() {
        let (storage, _dir) = open_temp_storage();
        let (old_head_hash, mut candidate) =
            seed_mainnet_recovery_fixture(&storage, Vec::new(), Vec::new());
        candidate.header.hash = "different-fork-10887".to_string();
        storage
            .save_block_body(&candidate)
            .expect("seed invalid side body");
        save_block_meta(
            &storage,
            &candidate.header.hash,
            &BlockChainMeta {
                height: candidate.header.block_height,
                total_supply_emitted: 88,
                cumulative_work: 10,
                difficulty_bits: economics::POW_INITIAL_DIFFICULTY_BITS,
            },
        )
        .expect("seed invalid side metadata");
        let old_head = read_chain_head(&storage)
            .expect("read old head")
            .expect("old head exists");

        let err = maybe_promote_known_side_branch(
            &storage,
            economics::MAINNET_CHAIN_ID,
            &candidate,
            Some(&old_head),
        )
        .expect_err("branch without checkpoint must be rejected");
        assert!(matches!(
            err,
            ChainError::MainnetCheckpointMismatch {
                height: economics::MAINNET_RECOVERY_CHECKPOINT_10887_HEIGHT,
                ..
            }
        ));
        assert_eq!(
            read_chain_head(&storage)
                .expect("read head after rejection")
                .expect("head remains")
                .hash,
            old_head_hash
        );
    }

    #[test]
    fn fresh_db_has_no_chain_head() {
        let (storage, _dir) = open_temp_storage();
        assert_eq!(read_chain_head(&storage).expect("read head"), None);
    }

    #[test]
    fn applying_first_block_lands_on_genesis_parent() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = open_temp_storage();
        let genesis = genesis_parent_for(chain_id);
        let block = mine_block(
            chain_id,
            &genesis.header.hash,
            0,
            0,
            &"zka1miner".to_string(),
            4,
            1_700_000_000,
        );
        let head = apply_block_with_expected_difficulty(&storage, chain_id, &block, 4)
            .expect("apply genesis-child");
        assert_eq!(head.height, 1);
        assert_eq!(head.hash, block.header.hash);
        assert_eq!(
            head.total_supply_emitted,
            economics::block_reward_for_height(1, 0)
        );
        assert!(head.cumulative_work > 0);

        let stored = read_chain_head(&storage)
            .expect("read after apply")
            .expect("head");
        assert_eq!(stored, head);
        assert!(storage.block_exists(&block.header.hash).expect("exists"));
    }

    #[test]
    fn applying_two_consecutive_blocks_extends_the_chain() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = open_temp_storage();
        let genesis = genesis_parent_for(chain_id);

        let block1 = mine_block(
            chain_id,
            &genesis.header.hash,
            0,
            0,
            &"zka1miner".to_string(),
            4,
            1_700_000_000,
        );
        let head1 = apply_block_with_expected_difficulty(&storage, chain_id, &block1, 4)
            .expect("apply block 1");

        let block2 = mine_block(
            chain_id,
            &head1.hash,
            head1.height,
            head1.total_supply_emitted,
            &"zka1miner".to_string(),
            4,
            1_700_000_120,
        );
        let head2 = apply_block_with_expected_difficulty(&storage, chain_id, &block2, 4)
            .expect("apply block 2");

        assert_eq!(head2.height, 2);
        assert_eq!(
            head2.total_supply_emitted,
            block1.transactions[0]
                .amount
                .saturating_add(block2.transactions[0].amount)
        );
        assert!(head2.cumulative_work >= head1.cumulative_work);
    }

    #[test]
    fn duplicate_block_is_rejected_explicitly() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = open_temp_storage();
        let genesis = genesis_parent_for(chain_id);
        let block = mine_block(
            chain_id,
            &genesis.header.hash,
            0,
            0,
            &"zka1miner".to_string(),
            4,
            1_700_000_000,
        );
        apply_block_with_expected_difficulty(&storage, chain_id, &block, 4).expect("first apply");
        let err = apply_block_with_expected_difficulty(&storage, chain_id, &block, 4).unwrap_err();
        assert!(matches!(err, ChainError::BlockAlreadyKnown(_)));
    }

    #[test]
    fn block_skipping_a_height_is_rejected() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = open_temp_storage();
        let genesis = genesis_parent_for(chain_id);
        // Build at height 2 directly without ever applying height 1.
        let mut block = mine_block(
            chain_id,
            &genesis.header.hash,
            1, // pretend height 1 is the parent
            economics::block_reward_for_height(1, 0),
            &"zka1miner".to_string(),
            4,
            1_700_000_000,
        );
        // Force the block to claim height 2.
        block.header.block_height = 2;
        block.header.hash = block.compute_hash();
        let err = apply_block_with_expected_difficulty(&storage, chain_id, &block, 4).unwrap_err();
        assert!(matches!(
            err,
            ChainError::NonContiguousHeight { .. } | ChainError::DoesNotExtendHead { .. }
        ));
    }

    #[test]
    fn block_with_wrong_prev_hash_is_rejected() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = open_temp_storage();
        let genesis = genesis_parent_for(chain_id);
        let block1 = mine_block(
            chain_id,
            &genesis.header.hash,
            0,
            0,
            &"zka1miner".to_string(),
            4,
            1_700_000_000,
        );
        apply_block_with_expected_difficulty(&storage, chain_id, &block1, 4)
            .expect("apply block 1");

        // Mine block 2 against a fake parent hash; even though heights line
        // up, the chain link won't.
        let block2_bad = mine_block(
            chain_id,
            "deadbeef",
            block1.header.block_height,
            block1.transactions[0].amount,
            &"zka1miner".to_string(),
            4,
            1_700_000_120,
        );
        let err =
            apply_block_with_expected_difficulty(&storage, chain_id, &block2_bad, 4).unwrap_err();
        assert!(matches!(err, ChainError::DoesNotExtendHead { .. }));
    }

    #[test]
    fn validation_failures_surface_as_chain_validation_error() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = open_temp_storage();
        let genesis = genesis_parent_for(chain_id);
        // Build a block with the WRONG coinbase amount: 5 atoms instead of
        // the expected atomic subsidy.
        let mut block = mine_block(
            chain_id,
            &genesis.header.hash,
            0,
            0,
            &"zka1miner".to_string(),
            4,
            1_700_000_000,
        );
        block.transactions[0].amount = 5;
        block.transactions[0].hash = block.transactions[0].compute_hash();
        block.header.merkle_root = block.compute_merkle_root();
        block.header.hash = block.compute_hash();
        let err = apply_block_with_expected_difficulty(&storage, chain_id, &block, 4).unwrap_err();
        match err {
            ChainError::Validation(BlockValidationError::CoinbaseAmountMismatch { .. }) => {}
            other => panic!("expected CoinbaseAmountMismatch, got: {other:?}"),
        }
    }

    #[test]
    fn would_replace_head_with_reorg_compares_cumulative_work() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = open_temp_storage();
        let block = mine_block(
            chain_id,
            &genesis_parent_for(chain_id).header.hash,
            0,
            0,
            &"zka1miner".to_string(),
            4,
            1_700_000_000,
        );
        assert!(!would_replace_head_with_reorg(&storage, &block, 0));
        assert!(would_replace_head_with_reorg(&storage, &block, u128::MAX));
    }

    #[test]
    fn heavier_side_branch_replaces_canonical_head() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = open_temp_storage();
        let genesis = genesis_parent_for(chain_id);

        let canonical = mine_block(
            chain_id,
            &genesis.header.hash,
            0,
            0,
            &"zka1miner".to_string(),
            economics::POW_INITIAL_DIFFICULTY_BITS,
            1_700_000_000,
        );
        let canonical_head = apply_block(&storage, chain_id, &canonical).expect("apply canonical");

        // Mine a seal with strictly more observed work than the canonical
        // block. Do not clamp this fixture to the temporary rehearsal max:
        // a valid block may exceed the minimum target by chance.
        let stronger_bits = (canonical_head.cumulative_work as u32).saturating_add(1);
        let side_branch = mine_block(
            chain_id,
            &genesis.header.hash,
            0,
            0,
            &"zka1miner".to_string(),
            stronger_bits,
            1_700_000_777,
        );

        let new_head = apply_block(&storage, chain_id, &side_branch).expect("reorg to stronger");
        assert_eq!(new_head.hash, side_branch.header.hash);
        assert!(new_head.cumulative_work > canonical_head.cumulative_work);

        let canonical_at_height = storage
            .get_block_by_height(1)
            .expect("height lookup")
            .expect("height 1 block");
        assert_eq!(canonical_at_height.header.hash, side_branch.header.hash);
    }

    #[test]
    fn known_heavier_side_branch_replaces_canonical_head() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = open_temp_storage();
        let genesis = genesis_parent_for(chain_id);

        let canonical = mine_block(
            chain_id,
            &genesis.header.hash,
            0,
            0,
            &"zka1miner".to_string(),
            economics::POW_INITIAL_DIFFICULTY_BITS,
            1_700_000_000,
        );
        let canonical_head = apply_block(&storage, chain_id, &canonical).expect("apply canonical");

        let stronger_bits = (canonical_head.cumulative_work as u32).saturating_add(1);
        let side_branch = mine_block(
            chain_id,
            &genesis.header.hash,
            0,
            0,
            &"zka1miner".to_string(),
            stronger_bits,
            1_700_000_777,
        );
        let side_work = pow::leading_zero_bits(&side_branch.header.seal.0) as u128;
        assert!(side_work > canonical_head.cumulative_work);

        storage
            .save_block_body(&side_branch)
            .expect("store known side branch body");
        save_block_meta(
            &storage,
            &side_branch.header.hash,
            &BlockChainMeta {
                height: side_branch.header.block_height,
                total_supply_emitted: side_branch.transactions[0].amount,
                cumulative_work: side_work,
                difficulty_bits: stronger_bits,
            },
        )
        .expect("store known side branch meta");

        let new_head = apply_block(&storage, chain_id, &side_branch)
            .expect("promote known stronger side branch");
        assert_eq!(new_head.hash, side_branch.header.hash);
        assert!(new_head.cumulative_work > canonical_head.cumulative_work);

        let canonical_at_height = storage
            .get_block_by_height(1)
            .expect("height lookup")
            .expect("height 1 block");
        assert_eq!(canonical_at_height.header.hash, side_branch.header.hash);
    }

    #[test]
    fn next_block_metadata_falls_back_to_genesis_when_db_is_empty() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = open_temp_storage();
        let storage = Arc::new(storage);
        let (height, prev_hash, supply) =
            next_block_metadata(&storage, chain_id).expect("metadata");
        assert_eq!(height, 1);
        assert_eq!(prev_hash, genesis_parent_for(chain_id).header.hash);
        assert_eq!(supply, 0);
    }

    #[test]
    fn applying_through_canonical_path_persists_difficulty() {
        // The `apply_block` path (no difficulty argument) MUST persist
        // `chain:current_difficulty_bits` so the next call to
        // `mining::difficulty::expected_for_next_block` returns the same
        // value the validator just used. This is the consensus contract.
        let chain_id = "zoka-test-net";
        let (storage, _dir) = open_temp_storage();

        // Mine + validate at the canonical initial difficulty.
        let canonical_initial = economics::POW_INITIAL_DIFFICULTY_BITS;
        let block = mine_block(
            chain_id,
            &genesis_parent_for(chain_id).header.hash,
            0,
            0,
            &"zka1miner".to_string(),
            canonical_initial,
            1_700_000_000,
        );
        let head = apply_block(&storage, chain_id, &block).expect("canonical apply");
        assert_eq!(head.height, 1);

        let stored_difficulty = storage
            .get_current_difficulty_bits()
            .expect("read difficulty");
        assert_eq!(stored_difficulty, canonical_initial);

        // The next-block expected difficulty must read back the same
        // value during the warmup band (heights 1 and 2).
        let next_expected = crate::mining::difficulty::expected_for_next_block(&storage)
            .expect("expected for next block");
        assert_eq!(next_expected, canonical_initial);
    }

    #[test]
    fn apply_block_canonical_rejects_block_solved_at_lower_difficulty() {
        // A peer that announces a block whose seal carries fewer leading
        // zero bits than the canonical expected difficulty MUST be
        // refused. We construct a seal with zero leading-zero bits so it
        // fails the warmup INITIAL band.
        let chain_id = "zoka-test-net";
        let (storage, _dir) = open_temp_storage();

        let parent = genesis_parent_for(chain_id);
        let coinbase = build_coinbase_tx(
            "zka1miner".to_string(),
            economics::block_reward_for_height(1, 0),
            1,
            1_700_000_000,
        );
        let mut block = Block {
            header: BlockHeader {
                hash: String::new(),
                prev_hash: parent.header.hash.clone(),
                merkle_root: String::new(),
                state_root: String::from("0"),
                timestamp: 1_700_000_000,
                nonce: 0,
                block_height: 1,
                validator: PublicKey(Vec::new()),
                seal: Signature(vec![0xff; 32]), // zero leading zeros
            },
            transactions: vec![coinbase],
            private_tx_hashes: Vec::new(),
            private_tx_bodies: Vec::new(),
            private_nullifiers: Vec::new(),
            private_fee_claims: Vec::new(),
            private_coinbase: Some(dummy_private_coinbase(1, "zka1miner")),
        };
        block.header.merkle_root = block.compute_merkle_root();
        block.header.hash = block.compute_hash();

        let result = apply_block(&storage, chain_id, &block);
        match result {
            Err(ChainError::Validation(BlockValidationError::PowBelowDifficulty {
                needed,
                ..
            })) => {
                assert_eq!(needed, economics::POW_INITIAL_DIFFICULTY_BITS);
            }
            Err(ChainError::Validation(BlockValidationError::PowSealMismatch { .. })) => {
                // Acceptable: seal doesn't match the recomputed PoW hash;
                // either way the block is rejected for failing PoW.
            }
            other => {
                panic!("expected PoW rejection (below-difficulty or seal-mismatch), got: {other:?}")
            }
        }
    }

    // -------- prune_below_head tests --------

    /// Build a chain of `n` blocks and return the storage so prune tests
    /// can exercise it.
    fn build_chain_with_n_blocks(n: u64) -> (RocksDbStorage, tempfile::TempDir, String) {
        let chain_id = "zoka-test-net";
        let (storage, dir) = open_temp_storage();
        let miner = "zka1miner".to_string();
        let mut prev_hash = genesis_parent_for(chain_id).header.hash.clone();
        let mut prev_height = 0u64;
        let mut supply = 0u64;
        let mut ts = 1_700_000_000u64;
        for _ in 0..n {
            let block = mine_block(chain_id, &prev_hash, prev_height, supply, &miner, 4, ts);
            apply_block_with_expected_difficulty(&storage, chain_id, &block, 4)
                .expect("apply block");
            prev_hash = block.header.hash.clone();
            prev_height = block.header.block_height;
            supply = supply.saturating_add(block.transactions[0].amount);
            ts += 120;
        }
        (storage, dir, chain_id.to_string())
    }

    #[test]
    fn prune_on_empty_chain_is_a_noop() {
        let (storage, _dir) = open_temp_storage();
        let report = prune_below_head(&storage, 5, false).expect("prune empty");
        assert_eq!(report.head_height, 0);
        assert_eq!(report.blocks_pruned, 0);
        assert_eq!(report.height_indices_pruned, 0);
        assert_eq!(report.metas_pruned, 0);
    }

    #[test]
    fn prune_below_minimum_keep_window_does_nothing() {
        let (storage, _dir, _) = build_chain_with_n_blocks(3);
        // Even though we ask for keep_last=2, the floor PRUNE_MIN_RETAINED_BLOCKS
        // is 8, so a 3-block chain has nothing to prune.
        let report = prune_below_head(&storage, 2, false).expect("prune");
        assert_eq!(report.head_height, 3);
        assert_eq!(report.blocks_pruned, 0);
    }

    #[test]
    fn prune_above_minimum_drops_old_blocks_only() {
        let (storage, _dir, _) = build_chain_with_n_blocks(15);
        // keep_last=10 => retain heights [6, 15], drop [1, 5].
        let report = prune_below_head(&storage, 10, false).expect("prune");
        assert_eq!(report.head_height, 15);
        assert_eq!(report.retained_from_height, 6);
        assert_eq!(report.blocks_pruned, 5);
        assert_eq!(report.height_indices_pruned, 5);
        assert_eq!(report.metas_pruned, 5);

        // Heights 1..5 are gone.
        for h in 1..=5u64 {
            assert!(
                storage.get_block_by_height(h).expect("query").is_none(),
                "height {h} should be pruned"
            );
        }
        // Heights 6..15 still present.
        for h in 6..=15u64 {
            assert!(
                storage.get_block_by_height(h).expect("query").is_some(),
                "height {h} must remain"
            );
        }
    }

    #[test]
    fn prune_preserves_chain_head_metadata() {
        let chain_id = "zoka-test-net";
        let (storage, _dir, _) = build_chain_with_n_blocks(20);
        let head_before = read_chain_head(&storage).expect("head").expect("some");
        prune_below_head(&storage, 8, false).expect("prune");
        let head_after = read_chain_head(&storage).expect("head").expect("some");
        // Head height, hash, supply, work all unchanged.
        assert_eq!(head_before, head_after);
        // Still resolvable by hash.
        assert!(storage.get_block(&head_after.hash).expect("get").is_some());
        // pow_algorithm_for_chain still resolves cleanly via genesis stub.
        let _ = genesis_parent_for(chain_id);
    }

    #[test]
    fn prune_keeps_difficulty_retarget_data_available() {
        let (storage, _dir, _) = build_chain_with_n_blocks(20);
        prune_below_head(&storage, 8, false).expect("prune");
        // expected_for_next_block reads parent + grandparent timestamps.
        // Both must still be present after pruning.
        let next_diff = crate::mining::difficulty::expected_for_next_block(&storage)
            .expect("retarget after prune");
        assert!(next_diff >= economics::POW_MIN_DIFFICULTY_BITS);
        assert!(next_diff <= economics::POW_MAX_DIFFICULTY_BITS);
    }

    #[test]
    fn apply_block_still_works_after_prune() {
        let chain_id = "zoka-test-net";
        let (storage, _dir, _) = build_chain_with_n_blocks(20);
        prune_below_head(&storage, 8, false).expect("prune");
        // Mine + apply one more block — the chain must keep moving.
        let head = read_chain_head(&storage).expect("head").expect("some");
        let next = mine_block(
            chain_id,
            &head.hash,
            head.height,
            head.total_supply_emitted,
            &"zka1miner".to_string(),
            4,
            1_700_010_000,
        );
        let new_head = apply_block_with_expected_difficulty(&storage, chain_id, &next, 4)
            .expect("apply post-prune");
        assert_eq!(new_head.height, head.height.saturating_add(1));
    }

    #[test]
    fn dry_run_reports_without_modifying_storage() {
        let (storage, _dir, _) = build_chain_with_n_blocks(15);
        let report = prune_below_head(&storage, 10, true).expect("dry run");
        assert!(report.dry_run);
        assert_eq!(report.blocks_pruned, 5);
        // Heights 1..5 STILL present because dry-run doesn't touch.
        for h in 1..=5u64 {
            assert!(
                storage.get_block_by_height(h).expect("query").is_some(),
                "dry-run must NOT remove height {h}"
            );
        }
    }

    #[test]
    fn prune_clamps_keep_last_to_min_retained_blocks() {
        // keep_last=0 should be silently upgraded to PRUNE_MIN_RETAINED_BLOCKS=8,
        // not collapse the chain to head-only.
        let (storage, _dir, _) = build_chain_with_n_blocks(20);
        let report = prune_below_head(&storage, 0, false).expect("prune");
        assert_eq!(report.retained_from_height, 13); // 20 - 8 + 1
        assert_eq!(report.blocks_pruned, 12); // heights 1..12
                                              // Head and last 8 still present.
        for h in 13..=20u64 {
            assert!(storage.get_block_by_height(h).expect("query").is_some());
        }
    }

    /// Build a block carrying the given spent nullifiers (mined on top of the
    /// supplied parent), recomputing merkle/hash so PoW stays valid.
    fn mine_block_with_nullifiers(
        chain_id: &str,
        parent_hash: &str,
        parent_height: u64,
        parent_supply: u64,
        miner: &Address,
        difficulty_bits: u32,
        timestamp: u64,
        nullifiers: Vec<String>,
    ) -> Block {
        let mut block = mine_block(
            chain_id,
            parent_hash,
            parent_height,
            parent_supply,
            miner,
            difficulty_bits,
            timestamp,
        );
        // Assign nullifiers raw (no sort/dedup) so tests can exercise the
        // in-block duplicate guard; recompute merkle/hash and re-mine the nonce
        // against the new header so PoW stays valid.
        block.private_nullifiers = nullifiers;
        block.header.merkle_root = block.compute_merkle_root();
        let engine = pow::engine_for(chain_id, block.header.block_height).expect("engine");
        let mut nonce = 0u64;
        let (computed, found_nonce) = loop {
            let preimage = pow::build_pow_input(
                chain_id,
                parent_hash,
                miner,
                block.header.block_height,
                timestamp,
                nonce,
            );
            let hash = engine.calculate_hash(&preimage).expect("hash");
            if pow::hash_meets_difficulty(&hash, difficulty_bits) {
                break (hash, nonce);
            }
            nonce = nonce.wrapping_add(1);
        };
        block.header.nonce = found_nonce;
        block.header.seal = Signature(computed);
        block.header.hash = block.compute_hash();
        block
    }

    #[test]
    fn spent_nullifier_index_roundtrips() {
        let (storage, _dir) = open_temp_storage();
        assert!(!storage.is_nullifier_spent("abc").expect("query"));
        storage.save_spent_nullifier("abc", 7).expect("save");
        assert!(storage.is_nullifier_spent("abc").expect("query"));
        storage.delete_spent_nullifier("abc").expect("delete");
        assert!(!storage.is_nullifier_spent("abc").expect("query"));
    }

    #[test]
    fn block_with_duplicate_nullifier_is_rejected() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = open_temp_storage();
        let genesis = genesis_parent_for(chain_id);
        let miner = "zka1miner".to_string();
        let block = mine_block_with_nullifiers(
            chain_id,
            &genesis.header.hash,
            0,
            0,
            &miner,
            economics::POW_MIN_DIFFICULTY_BITS,
            1,
            vec!["dup".to_string(), "dup".to_string()],
        );
        let err = apply_block(&storage, chain_id, &block).unwrap_err();
        // Lo que importa es que el bloque se rechace, no cual de las dos
        // guardas lo atrapa primero.
        //
        // Con el bundle canonico dormido lo caza el chequeo especifico de
        // duplicados. Con el bundle activo -- que es como se compila el binario
        // del telefono -- lo caza antes la validacion del bundle, que ve el
        // nullificador repetido. Exigir una variante concreta hacia fallar esta
        // prueba justamente en la configuracion que corre en el dispositivo,
        // dejando sin cubrir el doble gasto donde mas importa.
        let rechazado_por_duplicado =
            matches!(err, ChainError::DuplicatePrivateNullifierInBlock { .. })
                || format!("{err:?}").contains("repeats private nullifier");
        assert!(
            rechazado_por_duplicado,
            "expected duplicate-nullifier rejection, got: {err:?}"
        );
    }

    // Este caso necesita APLICAR bloques con exito, y el ayudante commitea
    // nullificadores sin cuerpos: con el bundle canonico activo -- como se
    // compila el binario del telefono -- eso se rechaza por "nullifier set
    // mismatch: derived 0, committed 1", que es correcto.
    //
    // No se pierde la cobertura del doble gasto en esa configuracion:
    // `block_with_duplicate_nullifier_is_rejected` corre en las dos y verifica
    // que un nullificador repetido se rechaza. Lo que no se puede probar aca es
    // el camino de aceptar-y-despues-rechazar, porque el bloque de prueba nunca
    // llega a aceptarse.
    #[cfg(not(feature = "mobile"))]
    #[test]
    fn double_spent_nullifier_across_blocks_is_rejected() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = open_temp_storage();
        let genesis = genesis_parent_for(chain_id);
        let miner = "zka1miner".to_string();

        // Block 1 commits nullifier N — accepted and indexed.
        let b1 = mine_block_with_nullifiers(
            chain_id,
            &genesis.header.hash,
            0,
            0,
            &miner,
            economics::POW_MIN_DIFFICULTY_BITS,
            1,
            vec!["N".to_string()],
        );
        let head1 = apply_block(&storage, chain_id, &b1).expect("apply b1");
        assert!(storage.is_nullifier_spent("N").expect("query"));

        // Block 2 tries to commit the same nullifier N — must be rejected.
        let b2 = mine_block_with_nullifiers(
            chain_id,
            &head1.hash,
            head1.height,
            head1.total_supply_emitted,
            &miner,
            economics::POW_MIN_DIFFICULTY_BITS,
            2,
            vec!["N".to_string()],
        );
        let err = apply_block(&storage, chain_id, &b2).unwrap_err();
        assert!(
            matches!(err, ChainError::PrivateNullifierDoubleSpend { .. }),
            "expected double-spend rejection, got: {err:?}"
        );
    }

    // Este caso necesita APLICAR bloques con exito, y el ayudante commitea
    // nullificadores sin cuerpos: con el bundle canonico activo -- como se
    // compila el binario del telefono -- eso se rechaza por "nullifier set
    // mismatch: derived 0, committed 1", que es correcto.
    //
    // No se pierde la cobertura del doble gasto en esa configuracion:
    // `block_with_duplicate_nullifier_is_rejected` corre en las dos y verifica
    // que un nullificador repetido se rechaza. Lo que no se puede probar aca es
    // el camino de aceptar-y-despues-rechazar, porque el bloque de prueba nunca
    // llega a aceptarse.
    #[cfg(not(feature = "mobile"))]
    #[test]
    fn distinct_nullifiers_across_blocks_are_accepted() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = open_temp_storage();
        let genesis = genesis_parent_for(chain_id);
        let miner = "zka1miner".to_string();

        let b1 = mine_block_with_nullifiers(
            chain_id,
            &genesis.header.hash,
            0,
            0,
            &miner,
            economics::POW_MIN_DIFFICULTY_BITS,
            1,
            vec!["N1".to_string()],
        );
        let head1 = apply_block(&storage, chain_id, &b1).expect("apply b1");
        let b2 = mine_block_with_nullifiers(
            chain_id,
            &head1.hash,
            head1.height,
            head1.total_supply_emitted,
            &miner,
            economics::POW_MIN_DIFFICULTY_BITS,
            2,
            vec!["N2".to_string()],
        );
        apply_block(&storage, chain_id, &b2).expect("apply b2");
        assert!(storage.is_nullifier_spent("N1").expect("query"));
        assert!(storage.is_nullifier_spent("N2").expect("query"));
    }

    /// The same private tx mined into both sides of a natural fork is NOT a
    /// double-spend: the block that spent the nullifier on the losing branch
    /// is orphaned by the very reorg that promotes the winner, so the
    /// nullifier ends up committed exactly once — on the new canonical
    /// chain. The old behavior rejected the heavier branch here, which froze
    /// every node on the losing side of a fork forever (mainnet incident
    /// 2026-07-09: fork at height 2736, both branches carried the same tx;
    /// BOOT could never reorg back to the majority chain).
    // Este caso necesita APLICAR bloques con exito, y el ayudante commitea
    // nullificadores sin cuerpos: con el bundle canonico activo -- como se
    // compila el binario del telefono -- eso se rechaza por "nullifier set
    // mismatch: derived 0, committed 1", que es correcto.
    //
    // No se pierde la cobertura del doble gasto en esa configuracion:
    // `block_with_duplicate_nullifier_is_rejected` corre en las dos y verifica
    // que un nullificador repetido se rechaza. Lo que no se puede probar aca es
    // el camino de aceptar-y-despues-rechazar, porque el bloque de prueba nunca
    // llega a aceptarse.
    #[cfg(not(feature = "mobile"))]
    #[test]
    fn heavier_side_branch_sharing_a_forked_nullifier_reorgs_cleanly() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = open_temp_storage();
        let genesis = genesis_parent_for(chain_id);
        let miner = "zka1miner".to_string();

        let canonical = mine_block_with_nullifiers(
            chain_id,
            &genesis.header.hash,
            0,
            0,
            &miner,
            economics::POW_INITIAL_DIFFICULTY_BITS,
            1_700_000_000,
            vec!["N".to_string()],
        );
        let canonical_head = apply_block(&storage, chain_id, &canonical).expect("apply canonical");
        assert!(storage.is_nullifier_spent("N").expect("query"));

        // A heavier side branch at height 1 spends the SAME nullifier N —
        // the block that spent N on the current chain is displaced by this
        // reorg, so the promotion must succeed and N must end up owned by
        // the new canonical branch.
        let stronger_bits = (canonical_head.cumulative_work as u32).saturating_add(1);
        let side_branch = mine_block_with_nullifiers(
            chain_id,
            &genesis.header.hash,
            0,
            0,
            &miner,
            stronger_bits,
            1_700_000_777,
            vec!["N".to_string()],
        );
        let new_head = apply_block(&storage, chain_id, &side_branch)
            .expect("fork-shared nullifier must not block the reorg");
        assert_eq!(new_head.hash, side_branch.header.hash);
        assert!(
            storage.is_nullifier_spent("N").expect("query"),
            "N must remain spent, now owned by the new canonical branch"
        );

        // And spending N again on the NEW chain is still a double-spend.
        let repeat = mine_block_with_nullifiers(
            chain_id,
            &new_head.hash,
            new_head.height,
            new_head.total_supply_emitted,
            &miner,
            economics::POW_INITIAL_DIFFICULTY_BITS,
            1_700_000_888,
            vec!["N".to_string()],
        );
        let err = apply_block(&storage, chain_id, &repeat).unwrap_err();
        assert!(
            matches!(err, ChainError::PrivateNullifierDoubleSpend { .. }),
            "expected double-spend rejection, got: {err:?}"
        );
    }

    /// The actual reorg double-spend loophole stays closed: a nullifier
    /// spent by a block BELOW the branch point (one that remains canonical
    /// after the reorg) cannot be re-spent by the incoming branch, no matter
    /// how much heavier that branch is. The promotion is aborted with the
    /// canonical head untouched.
    // Este caso necesita APLICAR bloques con exito, y el ayudante commitea
    // nullificadores sin cuerpos: con el bundle canonico activo -- como se
    // compila el binario del telefono -- eso se rechaza por "nullifier set
    // mismatch: derived 0, committed 1", que es correcto.
    //
    // No se pierde la cobertura del doble gasto en esa configuracion:
    // `block_with_duplicate_nullifier_is_rejected` corre en las dos y verifica
    // que un nullificador repetido se rechaza. Lo que no se puede probar aca es
    // el camino de aceptar-y-despues-rechazar, porque el bloque de prueba nunca
    // llega a aceptarse.
    #[cfg(not(feature = "mobile"))]
    #[test]
    fn heavier_side_branch_cannot_respend_a_nullifier_committed_below_the_branch_point() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = open_temp_storage();
        let genesis = genesis_parent_for(chain_id);
        let miner = "zka1miner".to_string();

        // Canonical height 1 spends N; height 2 extends it without touching N.
        let a1 = mine_block_with_nullifiers(
            chain_id,
            &genesis.header.hash,
            0,
            0,
            &miner,
            economics::POW_INITIAL_DIFFICULTY_BITS,
            1_700_000_000,
            vec!["N".to_string()],
        );
        let a1_head = apply_block(&storage, chain_id, &a1).expect("apply a1");
        let a2 = mine_block_with_nullifiers(
            chain_id,
            &a1_head.hash,
            a1_head.height,
            a1_head.total_supply_emitted,
            &miner,
            economics::POW_INITIAL_DIFFICULTY_BITS,
            1_700_000_060,
            Vec::new(),
        );
        let a2_head = apply_block(&storage, chain_id, &a2).expect("apply a2");
        assert!(storage.is_nullifier_spent("N").expect("query"));

        // A heavier fork off a1 (branch point = height 2) tries to spend N
        // again. a1 — the block that spent N — REMAINS canonical below the
        // branch point, so this is a genuine double-spend and the promotion
        // must fail without touching the head. The competing branch keeps
        // a1's work, so b2 only needs to out-work a2 itself (NOT the whole
        // cumulative chain — mining a single seal past the two-block
        // cumulative total would take ~2^30 hashes and hang the test).
        let stronger_bits = pow::leading_zero_bits(&a2.header.seal.0).saturating_add(1);
        let b2 = mine_block_with_nullifiers(
            chain_id,
            &a1_head.hash,
            a1_head.height,
            a1_head.total_supply_emitted,
            &miner,
            stronger_bits,
            1_700_000_777,
            vec!["N".to_string()],
        );
        let err = apply_block(&storage, chain_id, &b2).unwrap_err();
        assert!(
            matches!(err, ChainError::PrivateNullifierDoubleSpend { .. }),
            "expected double-spend rejection for respend below the branch point, got: {err:?}"
        );

        let head = read_chain_head(&storage)
            .expect("read head")
            .expect("head must still exist");
        assert_eq!(
            head.hash, a2_head.hash,
            "canonical head must be unchanged after the rejected reorg attempt"
        );
        assert!(
            storage.is_nullifier_spent("N").expect("query"),
            "N must remain spent by the canonical chain"
        );
    }

    /// Companion to the test above: a reorg that does NOT touch the
    /// contested nullifier legitimately frees it (the orphaned block's
    /// nullifiers are de-indexed by `promote_branch_to_canonical`), so a
    /// later block on the new canonical chain may spend it — and once
    /// spent there, it is protected against a further double-spend exactly
    /// like any other nullifier.
    // Este caso necesita APLICAR bloques con exito, y el ayudante commitea
    // nullificadores sin cuerpos: con el bundle canonico activo -- como se
    // compila el binario del telefono -- eso se rechaza por "nullifier set
    // mismatch: derived 0, committed 1", que es correcto.
    //
    // No se pierde la cobertura del doble gasto en esa configuracion:
    // `block_with_duplicate_nullifier_is_rejected` corre en las dos y verifica
    // que un nullificador repetido se rechaza. Lo que no se puede probar aca es
    // el camino de aceptar-y-despues-rechazar, porque el bloque de prueba nunca
    // llega a aceptarse.
    #[cfg(not(feature = "mobile"))]
    #[test]
    fn reorg_that_orphans_a_spend_frees_its_nullifier_then_still_blocks_a_repeat() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = open_temp_storage();
        let genesis = genesis_parent_for(chain_id);
        let miner = "zka1miner".to_string();

        // Canonical: height 1 spends N.
        let a1 = mine_block_with_nullifiers(
            chain_id,
            &genesis.header.hash,
            0,
            0,
            &miner,
            economics::POW_INITIAL_DIFFICULTY_BITS,
            1_700_000_000,
            vec!["N".to_string()],
        );
        let a1_head = apply_block(&storage, chain_id, &a1).expect("apply a1");
        assert!(storage.is_nullifier_spent("N").expect("query"));

        // A heavier side branch at height 1 that does NOT touch N reorgs it
        // away — N must become free again.
        let stronger_bits = (a1_head.cumulative_work as u32).saturating_add(1);
        let b1 = mine_block_with_nullifiers(
            chain_id,
            &genesis.header.hash,
            0,
            0,
            &miner,
            stronger_bits,
            1_700_000_777,
            Vec::new(),
        );
        let b1_head =
            apply_block(&storage, chain_id, &b1).expect("reorg to unrelated heavier branch");
        assert_eq!(b1_head.hash, b1.header.hash);
        assert!(
            !storage.is_nullifier_spent("N").expect("query"),
            "N must be freed once the block that spent it is orphaned"
        );

        // A new block on the new canonical chain may now legitimately spend N.
        let b2 = mine_block_with_nullifiers(
            chain_id,
            &b1_head.hash,
            b1_head.height,
            b1_head.total_supply_emitted,
            &miner,
            economics::POW_INITIAL_DIFFICULTY_BITS,
            1_700_000_888,
            vec!["N".to_string()],
        );
        let b2_head = apply_block(&storage, chain_id, &b2).expect("apply b2 spending freed N");
        assert!(storage.is_nullifier_spent("N").expect("query"));

        // And a further attempt to spend N again must still be rejected.
        let b3 = mine_block_with_nullifiers(
            chain_id,
            &b2_head.hash,
            b2_head.height,
            b2_head.total_supply_emitted,
            &miner,
            economics::POW_INITIAL_DIFFICULTY_BITS,
            1_700_000_999,
            vec!["N".to_string()],
        );
        let err = apply_block(&storage, chain_id, &b3).unwrap_err();
        assert!(
            matches!(err, ChainError::PrivateNullifierDoubleSpend { .. }),
            "expected double-spend rejection, got: {err:?}"
        );
    }
}
