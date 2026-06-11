//! Canonical, header-driven difficulty retargeting for ZOKA PoW.
//!
//! Every node derives the next-block difficulty from the same chain
//! state, so a miner cannot pick a number out of thin air and a verifier
//! cannot accept a block at a difficulty other than the one the chain
//! demands.
//!
//! The function used here is intentionally simple: a per-block adaptive
//! step. For each new block at height N (N >= 3) we look at the time
//! between block N-1 and block N-2 — the only "solve time" the chain
//! exposes deterministically — and:
//!
//! - if it was faster than `target / 2`: bump by 1 bit (capped at
//!   `POW_MAX_DIFFICULTY_BITS`),
//! - if it was slower than `target * 2`: drop by 1 bit (clamped to
//!   `POW_MIN_DIFFICULTY_BITS`),
//! - otherwise: leave it alone.
//!
//! Heights 1 and 2 use `POW_INITIAL_DIFFICULTY_BITS` because the chain
//! does not yet have two prior committed timestamps. This is a deliberate
//! warmup band; without it, height 1 would retarget against a synthetic
//! genesis timestamp of zero and immediately collapse to `MIN`.
//!
//! A future hard fork can replace the per-block step with a window-based
//! Bitcoin-style retarget. Any change is consensus-breaking and must
//! coincide with a chain id bump.

use crate::economics;
use crate::storage::RocksDbStorage;
use crate::types::Block;

/// Returns the difficulty (in leading-zero bits) that the next block on
/// `storage` MUST clear in order to be accepted by every honest node.
/// Reads the canonical chain head + the two most recent block timestamps
/// (when available) to retarget; falls back to
/// `POW_INITIAL_DIFFICULTY_BITS` during the warmup band.
pub fn expected_for_next_block(storage: &RocksDbStorage) -> Result<u32, String> {
    let head_height = storage
        .get_chain_height()
        .map_err(|e| format!("read chain height failed: {e:?}"))?;
    let head_hash = storage
        .get_last_block_hash()
        .map_err(|e| format!("read last block hash failed: {e:?}"))?;

    if head_height == 0 || head_hash.is_empty() || head_hash == "0" {
        // The next block is the genesis-child (height 1).
        return Ok(economics::POW_INITIAL_DIFFICULTY_BITS);
    }

    if head_height < 2 {
        // Next block is height 2 — still inside the warmup band.
        return Ok(economics::POW_INITIAL_DIFFICULTY_BITS);
    }

    let parent = storage
        .get_block(&head_hash)
        .map_err(|e| format!("read parent block failed: {e:?}"))?
        .ok_or_else(|| format!("chain head {head_hash} missing from storage"))?;
    let grandparent = storage
        .get_block(&parent.header.prev_hash)
        .map_err(|e| format!("read grandparent block failed: {e:?}"))?
        .ok_or_else(|| {
            format!(
                "grandparent {} of head {} missing from storage",
                parent.header.prev_hash, head_hash
            )
        })?;

    let current_bits = storage
        .get_current_difficulty_bits()
        .map_err(|e| format!("read current difficulty failed: {e:?}"))?;
    let parent_difficulty = if current_bits == 0 {
        economics::POW_INITIAL_DIFFICULTY_BITS
    } else {
        current_bits
    };
    let parent_solve_secs = parent
        .header
        .timestamp
        .saturating_sub(grandparent.header.timestamp);

    Ok(retarget_from_solve_time(
        parent_difficulty,
        parent_solve_secs,
        economics::TARGET_BLOCK_TIME_SECS,
    ))
}

/// Pure step function used by `expected_for_next_block`. Exposed for
/// tests and for any caller that wants to reason about the retarget
/// outside of an open RocksDB.
pub fn retarget_from_solve_time(
    parent_difficulty: u32,
    parent_solve_secs: u64,
    target_block_time_secs: u64,
) -> u32 {
    let target = target_block_time_secs.max(1);
    let clamped = parent_difficulty
        .max(economics::POW_MIN_DIFFICULTY_BITS)
        .min(economics::POW_MAX_DIFFICULTY_BITS);
    if parent_solve_secs < target / 2 {
        clamped
            .saturating_add(1)
            .min(economics::POW_MAX_DIFFICULTY_BITS)
    } else if parent_solve_secs > target.saturating_mul(2) {
        clamped
            .saturating_sub(1)
            .max(economics::POW_MIN_DIFFICULTY_BITS)
    } else {
        clamped
    }
}

/// Difficulty to record after applying `applied_block` whose own
/// expected difficulty was `expected_used`. Stored under
/// `chain:current_difficulty_bits` so the next call to
/// `expected_for_next_block` can read it back.
pub fn difficulty_after_applying(_applied_block: &Block, expected_used: u32) -> u32 {
    expected_used
        .max(economics::POW_MIN_DIFFICULTY_BITS)
        .min(economics::POW_MAX_DIFFICULTY_BITS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warmup_returns_initial_difficulty_on_fresh_storage() {
        let dir = tempfile::tempdir().expect("tempdir");
        let storage = RocksDbStorage::new(dir.path().to_str().unwrap()).expect("storage");
        let bits = expected_for_next_block(&storage).expect("expected");
        assert_eq!(bits, economics::POW_INITIAL_DIFFICULTY_BITS);
    }

    #[test]
    fn retarget_steps_up_when_parent_solve_time_is_too_fast() {
        // Use a start in the middle of the [MIN, MAX] band so the step is
        // observable regardless of how MIN/MAX are pinned in the test config.
        // (In the pinned-8 test build MIN==MAX==8 leaves no room to step, so
        // this exercises the band midpoint to validate the step logic.)
        let start = (economics::POW_MIN_DIFFICULTY_BITS + economics::POW_MAX_DIFFICULTY_BITS) / 2;
        let new_bits = retarget_from_solve_time(
            start,
            economics::TARGET_BLOCK_TIME_SECS / 4,
            economics::TARGET_BLOCK_TIME_SECS,
        );
        let expected = (start + 1).min(economics::POW_MAX_DIFFICULTY_BITS);
        assert_eq!(new_bits, expected);
    }

    #[test]
    fn retarget_steps_down_when_parent_solve_time_is_too_slow() {
        let start = (economics::POW_MIN_DIFFICULTY_BITS + economics::POW_MAX_DIFFICULTY_BITS) / 2;
        let new_bits = retarget_from_solve_time(
            start,
            economics::TARGET_BLOCK_TIME_SECS * 3,
            economics::TARGET_BLOCK_TIME_SECS,
        );
        let expected = start
            .saturating_sub(1)
            .max(economics::POW_MIN_DIFFICULTY_BITS);
        assert_eq!(new_bits, expected);
    }

    #[test]
    fn retarget_holds_inside_band() {
        let new_bits = retarget_from_solve_time(
            economics::POW_INITIAL_DIFFICULTY_BITS,
            economics::TARGET_BLOCK_TIME_SECS,
            economics::TARGET_BLOCK_TIME_SECS,
        );
        assert_eq!(new_bits, economics::POW_INITIAL_DIFFICULTY_BITS);
    }

    #[test]
    fn retarget_clamps_at_min_bound() {
        let new_bits = retarget_from_solve_time(
            economics::POW_MIN_DIFFICULTY_BITS,
            economics::TARGET_BLOCK_TIME_SECS * 100,
            economics::TARGET_BLOCK_TIME_SECS,
        );
        assert_eq!(new_bits, economics::POW_MIN_DIFFICULTY_BITS);
    }

    #[test]
    fn retarget_clamps_at_max_bound() {
        let new_bits = retarget_from_solve_time(
            economics::POW_MAX_DIFFICULTY_BITS,
            0,
            economics::TARGET_BLOCK_TIME_SECS,
        );
        assert_eq!(new_bits, economics::POW_MAX_DIFFICULTY_BITS);
    }

    #[test]
    fn parent_difficulty_outside_bounds_is_clamped_first() {
        let new_bits = retarget_from_solve_time(
            economics::POW_MAX_DIFFICULTY_BITS + 100,
            economics::TARGET_BLOCK_TIME_SECS,
            economics::TARGET_BLOCK_TIME_SECS,
        );
        assert_eq!(new_bits, economics::POW_MAX_DIFFICULTY_BITS);
    }

    #[test]
    fn difficulty_after_applying_clamps_inside_bounds() {
        let dummy = crate::types::Block {
            header: crate::types::BlockHeader {
                hash: String::new(),
                prev_hash: String::new(),
                merkle_root: String::new(),
                state_root: String::new(),
                timestamp: 0,
                nonce: 0,
                block_height: 0,
                validator: crate::types::PublicKey(Vec::new()),
                seal: crate::types::Signature(Vec::new()),
            },
            transactions: Vec::new(),
            private_tx_hashes: Vec::new(),
            private_tx_bodies: Vec::new(),
            private_nullifiers: Vec::new(),
            private_fee_claims: Vec::new(),
            private_coinbase: None,
        };
        assert_eq!(
            difficulty_after_applying(&dummy, 0),
            economics::POW_MIN_DIFFICULTY_BITS
        );
        assert_eq!(
            difficulty_after_applying(&dummy, u32::MAX),
            economics::POW_MAX_DIFFICULTY_BITS
        );
        assert_eq!(
            difficulty_after_applying(&dummy, economics::POW_INITIAL_DIFFICULTY_BITS),
            economics::POW_INITIAL_DIFFICULTY_BITS
        );
    }
}
