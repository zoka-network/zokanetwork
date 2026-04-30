//! Canonical ZOKA mainnet v1 economic parameters.
//!
//! The current wallet/accounting layer uses whole ZKA units. Do not duplicate
//! these constants in binaries; import this module so emission rules stay
//! consistent across CLI, mining, genesis tooling and future consensus checks.

pub const ECONOMICS_VERSION: &str = "zoka-economics-v1";
pub const TOKEN_SYMBOL: &str = "ZKA";
pub const MAINNET_CHAIN_ID: &str = "zoka-mainnet-v1";

pub const MAX_SUPPLY_ZKA: u64 = 23_000_000;
pub const INITIAL_BLOCK_REWARD_ZKA: u64 = 12;
pub const HALVING_INTERVAL_BLOCKS: u64 = 1_051_200;
pub const TARGET_BLOCK_TIME_SECS: u64 = 120;
pub const COINBASE_MATURITY_BLOCKS: u64 = 60;

pub const POW_MIN_DIFFICULTY_BITS: u32 = 12;
pub const POW_INITIAL_DIFFICULTY_BITS: u32 = 16;
pub const POW_MAX_DIFFICULTY_BITS: u32 = 30;
pub const POW_KEY_EPOCH_BLOCKS: u64 = 2_048;

/// Canonical mainnet PoW algorithm. See `docs/pow-mainnet-decision.md`.
/// Mainnet binaries MUST be built with `--features randomx`; nodes that
/// claim to be mainnet but produce blocks tagged with another algorithm id
/// are invalid and must be rejected by other nodes.
pub const POW_ALGORITHM_MAINNET: &str = "zoka-randomx-v1";

/// CPU fallback used by dev / testnet / CI. Not valid in mainnet.
pub const POW_ALGORITHM_TESTNET: &str = "zoka-cpu-fallback-v1";

/// Returns the canonical PoW algorithm id for a given chain id. Used by
/// `mining::pow::engine_for` and by block validation.
pub fn pow_algorithm_for_chain(chain_id: &str) -> &'static str {
    if chain_id == MAINNET_CHAIN_ID {
        POW_ALGORITHM_MAINNET
    } else {
        POW_ALGORITHM_TESTNET
    }
}

/// Whole-ZKA block subsidy before cap clipping.
pub fn halving_reward_for_height(height: u64) -> u64 {
    let era = height / HALVING_INTERVAL_BLOCKS;
    if era >= u64::BITS as u64 {
        0
    } else {
        INITIAL_BLOCK_REWARD_ZKA >> era
    }
}

/// Final block subsidy after applying the hard cap.
pub fn block_reward_for_height(height: u64, total_supply_emitted: u64) -> u64 {
    let remaining = MAX_SUPPLY_ZKA.saturating_sub(total_supply_emitted);
    if remaining == 0 {
        return 0;
    }
    halving_reward_for_height(height).min(remaining)
}

pub fn halving_blocks_remaining(height: u64) -> u64 {
    let offset = height % HALVING_INTERVAL_BLOCKS;
    if offset == 0 {
        HALVING_INTERVAL_BLOCKS
    } else {
        HALVING_INTERVAL_BLOCKS - offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emission_v1_halving_schedule_is_stable() {
        assert_eq!(halving_reward_for_height(0), 12);
        assert_eq!(halving_reward_for_height(HALVING_INTERVAL_BLOCKS - 1), 12);
        assert_eq!(halving_reward_for_height(HALVING_INTERVAL_BLOCKS), 6);
        assert_eq!(halving_reward_for_height(HALVING_INTERVAL_BLOCKS * 2), 3);
        assert_eq!(halving_reward_for_height(HALVING_INTERVAL_BLOCKS * 3), 1);
        assert_eq!(halving_reward_for_height(HALVING_INTERVAL_BLOCKS * 4), 0);
    }

    #[test]
    fn emission_v1_never_exceeds_hard_cap() {
        assert_eq!(block_reward_for_height(0, 0), 12);
        assert_eq!(block_reward_for_height(0, MAX_SUPPLY_ZKA - 5), 5);
        assert_eq!(block_reward_for_height(0, MAX_SUPPLY_ZKA), 0);
        assert_eq!(block_reward_for_height(0, MAX_SUPPLY_ZKA + 1), 0);
    }

    #[test]
    fn emission_v1_maturity_and_timing_are_frozen() {
        assert_eq!(TOKEN_SYMBOL, "ZKA");
        assert_eq!(MAX_SUPPLY_ZKA, 23_000_000);
        assert_eq!(INITIAL_BLOCK_REWARD_ZKA, 12);
        assert_eq!(HALVING_INTERVAL_BLOCKS, 1_051_200);
        assert_eq!(TARGET_BLOCK_TIME_SECS, 120);
        assert_eq!(COINBASE_MATURITY_BLOCKS, 60);
    }
}
