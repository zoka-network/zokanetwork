//! Canonical ZOKA mainnet v1 economic parameters.
//!
//! The wallet/accounting layer uses atomic units internally. User-facing CLI
//! and genesis fields keep speaking in ZKA and convert at the boundary.

pub const ECONOMICS_VERSION: &str = "zoka-economics-v2-atoms";
pub const TOKEN_SYMBOL: &str = "ZKA";
// Bumped for the Zenith relaunch genesis reset: the synthetic genesis hash
// (genesis_pow_hash_for_chain) is derived directly from this id, so a stale
// pre-relaunch chain is cryptographically incompatible with the new one —
// not just rejected by protocol version, but unable to even produce a
// matching block-1 parent hash. Defense in depth alongside PROTOCOL_FREEZE_ID.
pub const MAINNET_CHAIN_ID: &str = "zoka-mainnet-zenith-v1";
pub const TESTNET_CHAIN_ID: &str = "zoka-testnet-v1";

/// Emergency mainnet checkpoint used by the 1.4.x hotfix rollout. Nodes that
/// have this checkpoint must not reorganize onto a branch that does not contain
/// this exact block.
pub const MAINNET_EMERGENCY_CHECKPOINT_HEIGHT: u64 = 296;
pub const MAINNET_EMERGENCY_CHECKPOINT_HASH: &str =
    "d8c4b8d5945469c46fb32c002d34c08cb736ef470578f2464c3054ac190ac95c";
/// Canonical blocks mined during the isolated 1.5.0 recovery. These close the
/// post-296 fork window: upgraded nodes reject any branch above 296 that does
/// not contain this exact recovery chain.
pub const MAINNET_RECOVERY_CHECKPOINT_297_HEIGHT: u64 = 297;
pub const MAINNET_RECOVERY_CHECKPOINT_297_HASH: &str =
    "a086b823ec51052473860211327bc7e5bad9262cb68df264ecafc05ee070e22f";
pub const MAINNET_RECOVERY_CHECKPOINT_298_HEIGHT: u64 = 298;
pub const MAINNET_RECOVERY_CHECKPOINT_298_HASH: &str =
    "bee49728c38951fc82dfda00d2b4000980cb7a3b07046d6f6c636bd1f2d98227";
pub const MAINNET_EMERGENCY_CHECKPOINTS: &[(u64, &str)] = &[
    (
        MAINNET_EMERGENCY_CHECKPOINT_HEIGHT,
        MAINNET_EMERGENCY_CHECKPOINT_HASH,
    ),
    (
        MAINNET_RECOVERY_CHECKPOINT_297_HEIGHT,
        MAINNET_RECOVERY_CHECKPOINT_297_HASH,
    ),
    (
        MAINNET_RECOVERY_CHECKPOINT_298_HEIGHT,
        MAINNET_RECOVERY_CHECKPOINT_298_HASH,
    ),
];
/// Temporary one-miner recovery window after the 1.4.x checkpoint. The cap is
/// only applied when the local canonical chain contains the checkpoint above.
pub const MAINNET_EMERGENCY_DIFFICULTY_CAP_FROM_HEIGHT: u64 =
    MAINNET_EMERGENCY_CHECKPOINT_HEIGHT + 1;
pub const MAINNET_EMERGENCY_DIFFICULTY_CAP_UNTIL_HEIGHT: u64 = 360;
pub const MAINNET_EMERGENCY_DIFFICULTY_MAX_BITS: u32 = 14;

pub const ZKA_DECIMALS: u32 = 6;
pub const ATOMS_PER_ZKA: u64 = 1_000_000;
pub const MAX_SUPPLY_ZKA: u64 = 23_000_000;
pub const INITIAL_BLOCK_REWARD_ZKA: u64 = 12;
pub const MAX_SUPPLY_ATOMS: u64 = MAX_SUPPLY_ZKA * ATOMS_PER_ZKA;
pub const INITIAL_BLOCK_REWARD_ATOMS: u64 = INITIAL_BLOCK_REWARD_ZKA * ATOMS_PER_ZKA;
pub const HALVING_INTERVAL_BLOCKS: u64 = 1_051_200;
pub const TARGET_BLOCK_TIME_SECS: u64 = 120;
/// Coinbase outputs require 60 confirmations before they can be spent.
/// At 120s/block this is ~2 hours, protecting the network against short reorgs
/// invalidating reward payments.
pub const COINBASE_MATURITY_BLOCKS: u64 = 60;

/// Consensus cap for private transaction hash commitments in one block.
/// Wallets can split large sends across multiple private transactions, but a
/// block must keep a bounded commitment list so validation remains predictable.
pub const PRIVATE_TX_HASHES_PER_BLOCK_LIMIT: usize = 128;
/// Canonical private bodies remain disabled until a reviewed fork height is
/// fixed in release binaries. `None` keeps the historical hash-only format.
///
/// The on-device prover build (`mobile` feature) activates from genesis so the
/// staging app can exercise the compact v2-i1 profile against the test nodes
/// (which already run with these activations enabled). This ONLY affects the
/// phone's local gate — node/server binaries never enable `mobile`, so their
/// consensus default stays `None`/env-controlled and is untouched.
#[cfg(not(feature = "mobile"))]
pub const PRIVATE_TX_CANONICAL_BUNDLE_ACTIVATION_HEIGHT: Option<u64> = None;
#[cfg(feature = "mobile")]
pub const PRIVATE_TX_CANONICAL_BUNDLE_ACTIVATION_HEIGHT: Option<u64> = Some(0);
/// Compact mobile Groth16 profile remains disabled until the final i1 CRS,
/// adversarial vectors and coordinated rollout height are fixed.
#[cfg(not(feature = "mobile"))]
pub const PRIVATE_TX_V2_I1_ACTIVATION_HEIGHT: Option<u64> = None;
#[cfg(feature = "mobile")]
pub const PRIVATE_TX_V2_I1_ACTIVATION_HEIGHT: Option<u64> = Some(0);
/// Defense-in-depth limits for the future canonical private body bundle.
pub const PRIVATE_TX_CANONICAL_BODY_BYTES_LIMIT: usize = 1024 * 1024;
pub const PRIVATE_TX_CANONICAL_BUNDLE_BYTES_LIMIT: usize = 8 * 1024 * 1024;

pub fn private_tx_canonical_bundle_activation_height() -> Option<u64> {
    activation_height_from_env(
        "ZOKA_PRIVATE_TX_CANONICAL_BUNDLE_ACTIVATION_HEIGHT",
        PRIVATE_TX_CANONICAL_BUNDLE_ACTIVATION_HEIGHT,
    )
}

pub fn private_tx_v2_i1_activation_height() -> Option<u64> {
    activation_height_from_env(
        "ZOKA_PRIVATE_TX_V2_I1_ACTIVATION_HEIGHT",
        PRIVATE_TX_V2_I1_ACTIVATION_HEIGHT,
    )
}

fn activation_height_from_env(var: &str, default: Option<u64>) -> Option<u64> {
    let Ok(raw) = std::env::var(var) else {
        return default;
    };
    let raw = raw.trim();
    if raw.is_empty()
        || raw.eq_ignore_ascii_case("none")
        || raw.eq_ignore_ascii_case("null")
        || raw.eq_ignore_ascii_case("off")
    {
        return None;
    }
    Some(
        raw.parse::<u64>()
            .unwrap_or_else(|_| panic!("{var} must be an unsigned block height or 'none'")),
    )
}

pub fn private_tx_canonical_bundle_required(height: u64) -> bool {
    private_tx_canonical_bundle_required_at(height, private_tx_canonical_bundle_activation_height())
}

pub fn private_tx_canonical_bundle_required_at(
    height: u64,
    activation_height: Option<u64>,
) -> bool {
    activation_height.is_some_and(|activation| height >= activation)
}

pub fn private_tx_v2_i1_active(height: u64) -> bool {
    private_tx_v2_i1_active_at(
        height,
        private_tx_v2_i1_activation_height(),
        private_tx_canonical_bundle_activation_height(),
    )
}

pub fn private_tx_v2_i1_active_at(
    height: u64,
    activation_height: Option<u64>,
    canonical_bundle_activation_height: Option<u64>,
) -> bool {
    activation_height.is_some_and(|activation| height >= activation)
        && private_tx_canonical_bundle_required_at(height, canonical_bundle_activation_height)
}

/// Minimum difficulty the per-block retarget can fall to. Keeps the chain
/// viable even with a single low-end miner.
pub const POW_MIN_DIFFICULTY_BITS: u32 = 12;
/// Starting difficulty for the first two blocks (warmup band before two
/// committed timestamps are available for retarget).
pub const POW_INITIAL_DIFFICULTY_BITS: u32 = 12;
/// Hard ceiling for the per-block retarget. 30 bits at RandomX is
/// comfortably achievable by GPU/ASIC-resistant CPUs without freezing
/// the chain.
pub const POW_MAX_DIFFICULTY_BITS: u32 = 30;
pub const POW_KEY_EPOCH_BLOCKS: u64 = 2_048;

/// Canonical public-network PoW algorithm. See `docs/protocol-overview.md`.
/// Mainnet binaries MUST be built with `--features randomx`; nodes that
/// claim to be mainnet but produce blocks tagged with another algorithm id
/// are invalid and must be rejected by other nodes.
pub const POW_ALGORITHM_MAINNET: &str = "zoka-randomx-v1";

/// CPU fallback used by dev / testnet / CI. Not valid in mainnet.
pub const POW_ALGORITHM_TESTNET: &str = "zoka-cpu-fallback-v1";

/// Returns the canonical PoW algorithm id for a given chain id. Used by
/// `mining::pow::engine_for` and by block validation.
pub fn pow_algorithm_for_chain(chain_id: &str) -> &'static str {
    if chain_id == MAINNET_CHAIN_ID || chain_id == TESTNET_CHAIN_ID {
        POW_ALGORITHM_MAINNET
    } else {
        POW_ALGORITHM_TESTNET
    }
}

pub fn zka_to_atoms(zka: u64) -> Option<u64> {
    zka.checked_mul(ATOMS_PER_ZKA)
}

pub fn parse_zka_amount(raw: &str) -> Result<u64, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("amount cannot be empty".to_string());
    }
    if raw.starts_with('-') {
        return Err("amount cannot be negative".to_string());
    }
    let parts = raw.split('.').collect::<Vec<_>>();
    if parts.len() > 2 {
        return Err("amount must contain at most one decimal point".to_string());
    }
    let whole = if parts[0].is_empty() {
        0
    } else {
        parts[0]
            .parse::<u64>()
            .map_err(|_| format!("invalid whole-ZKA amount: {}", parts[0]))?
    };
    let whole_atoms = zka_to_atoms(whole).ok_or_else(|| "amount overflows u64".to_string())?;
    let fractional_atoms = if parts.len() == 2 {
        let frac = parts[1];
        if frac.len() > ZKA_DECIMALS as usize {
            return Err(format!(
                "amount has more than {ZKA_DECIMALS} decimal places"
            ));
        }
        let mut padded = frac.to_string();
        while padded.len() < ZKA_DECIMALS as usize {
            padded.push('0');
        }
        if padded.is_empty() {
            0
        } else {
            padded
                .parse::<u64>()
                .map_err(|_| format!("invalid fractional-ZKA amount: {frac}"))?
        }
    } else {
        0
    };
    whole_atoms
        .checked_add(fractional_atoms)
        .ok_or_else(|| "amount overflows u64".to_string())
}

pub fn format_zka_amount(atoms: u64) -> String {
    let whole = atoms / ATOMS_PER_ZKA;
    let frac = atoms % ATOMS_PER_ZKA;
    if frac == 0 {
        return whole.to_string();
    }
    let mut frac_text = format!("{frac:0width$}", width = ZKA_DECIMALS as usize);
    while frac_text.ends_with('0') {
        frac_text.pop();
    }
    format!("{whole}.{frac_text}")
}

/// Atomic-unit block subsidy before cap clipping.
pub fn halving_reward_for_height(height: u64) -> u64 {
    let era = height / HALVING_INTERVAL_BLOCKS;
    if era >= u64::BITS as u64 {
        0
    } else {
        (INITIAL_BLOCK_REWARD_ZKA >> era).saturating_mul(ATOMS_PER_ZKA)
    }
}

/// Final block subsidy after applying the hard cap.
pub fn block_reward_for_height(height: u64, total_supply_emitted: u64) -> u64 {
    let remaining = MAX_SUPPLY_ATOMS.saturating_sub(total_supply_emitted);
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
        assert_eq!(halving_reward_for_height(0), 12 * ATOMS_PER_ZKA);
        assert_eq!(
            halving_reward_for_height(HALVING_INTERVAL_BLOCKS - 1),
            12 * ATOMS_PER_ZKA
        );
        assert_eq!(
            halving_reward_for_height(HALVING_INTERVAL_BLOCKS),
            6 * ATOMS_PER_ZKA
        );
        assert_eq!(
            halving_reward_for_height(HALVING_INTERVAL_BLOCKS * 2),
            3 * ATOMS_PER_ZKA
        );
        assert_eq!(
            halving_reward_for_height(HALVING_INTERVAL_BLOCKS * 3),
            ATOMS_PER_ZKA
        );
        assert_eq!(halving_reward_for_height(HALVING_INTERVAL_BLOCKS * 4), 0);
    }

    #[test]
    fn emission_v1_never_exceeds_hard_cap() {
        assert_eq!(block_reward_for_height(0, 0), 12 * ATOMS_PER_ZKA);
        assert_eq!(block_reward_for_height(0, MAX_SUPPLY_ATOMS - 5), 5);
        assert_eq!(block_reward_for_height(0, MAX_SUPPLY_ATOMS), 0);
        assert_eq!(block_reward_for_height(0, MAX_SUPPLY_ATOMS + 1), 0);
    }

    #[test]
    fn emission_v1_maturity_and_timing_are_frozen() {
        assert_eq!(TOKEN_SYMBOL, "ZKA");
        assert_eq!(ATOMS_PER_ZKA, 1_000_000);
        assert_eq!(MAX_SUPPLY_ZKA, 23_000_000);
        assert_eq!(MAX_SUPPLY_ATOMS, 23_000_000_000_000);
        assert_eq!(INITIAL_BLOCK_REWARD_ZKA, 12);
        assert_eq!(INITIAL_BLOCK_REWARD_ATOMS, 12_000_000);
        assert_eq!(HALVING_INTERVAL_BLOCKS, 1_051_200);
        assert_eq!(TARGET_BLOCK_TIME_SECS, 120);
        assert_eq!(COINBASE_MATURITY_BLOCKS, 60);
    }

    #[test]
    fn canonical_private_bundle_stays_dormant_until_height_is_fixed() {
        assert_eq!(PRIVATE_TX_CANONICAL_BUNDLE_ACTIVATION_HEIGHT, None);
        assert!(!private_tx_canonical_bundle_required(0));
        assert!(!private_tx_canonical_bundle_required(u64::MAX));
        assert!(!private_tx_canonical_bundle_required_at(9, Some(10)));
        assert!(private_tx_canonical_bundle_required_at(10, Some(10)));
        assert!(private_tx_canonical_bundle_required_at(11, Some(10)));
    }

    #[test]
    fn compact_private_tx_profile_stays_dormant_until_height_is_fixed() {
        assert_eq!(PRIVATE_TX_V2_I1_ACTIVATION_HEIGHT, None);
        assert!(!private_tx_v2_i1_active(0));
        assert!(!private_tx_v2_i1_active(u64::MAX));
        assert!(!private_tx_v2_i1_active_at(9, Some(10), Some(10)));
        assert!(private_tx_v2_i1_active_at(10, Some(10), Some(10)));
        assert!(private_tx_v2_i1_active_at(11, Some(10), Some(10)));
        assert!(!private_tx_v2_i1_active_at(10, Some(10), None));
    }

    #[test]
    fn activation_height_env_override_is_explicit() {
        const VAR: &str = "ZOKA_TEST_ONLY_PRIVATE_TX_ACTIVATION_HEIGHT";
        std::env::remove_var(VAR);
        assert_eq!(activation_height_from_env(VAR, None), None);
        assert_eq!(activation_height_from_env(VAR, Some(12)), Some(12));

        std::env::set_var(VAR, "42");
        assert_eq!(activation_height_from_env(VAR, None), Some(42));

        std::env::set_var(VAR, "none");
        assert_eq!(activation_height_from_env(VAR, Some(42)), None);

        std::env::remove_var(VAR);
    }

    #[test]
    fn zka_amount_parser_accepts_fractional_atoms() {
        assert_eq!(parse_zka_amount("0").unwrap(), 0);
        assert_eq!(parse_zka_amount("1").unwrap(), ATOMS_PER_ZKA);
        assert_eq!(parse_zka_amount("1.25").unwrap(), 1_250_000);
        assert_eq!(parse_zka_amount("0.000001").unwrap(), 1);
        assert!(parse_zka_amount("0.0000001").is_err());
        assert_eq!(format_zka_amount(1_250_000), "1.25");
        assert_eq!(format_zka_amount(1), "0.000001");
    }

    #[test]
    fn testnet_economics_artifact_matches_code_constants() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("docs")
            .join("economics.testnet.v1.json");
        let raw = std::fs::read_to_string(path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();

        assert_eq!(json["version"], ECONOMICS_VERSION);
        assert_eq!(json["chain_id"], TESTNET_CHAIN_ID);
        assert_eq!(json["token"]["symbol"], TOKEN_SYMBOL);
        assert_eq!(json["token"]["decimals"], ZKA_DECIMALS);
        assert_eq!(json["token"]["atoms_per_zka"], ATOMS_PER_ZKA);
        assert_eq!(json["supply"]["max_supply_zka"], MAX_SUPPLY_ZKA);
        assert_eq!(json["supply"]["max_supply_atoms"], MAX_SUPPLY_ATOMS);
        assert_eq!(
            json["emission"]["initial_block_reward_zka"],
            INITIAL_BLOCK_REWARD_ZKA
        );
        assert_eq!(
            json["emission"]["initial_block_reward_atoms"],
            INITIAL_BLOCK_REWARD_ATOMS
        );
        assert_eq!(
            json["emission"]["halving_interval_blocks"],
            HALVING_INTERVAL_BLOCKS
        );
        assert_eq!(
            json["emission"]["target_block_time_seconds"],
            TARGET_BLOCK_TIME_SECS
        );
        assert_eq!(
            json["emission"]["coinbase_maturity_blocks"],
            COINBASE_MATURITY_BLOCKS
        );
        assert_eq!(json["pow"]["algorithm_v1"], POW_ALGORITHM_MAINNET);
    }

    #[test]
    fn testnet_v1_mirrors_mainnet_pow_and_economics() {
        assert_eq!(
            pow_algorithm_for_chain(TESTNET_CHAIN_ID),
            POW_ALGORITHM_MAINNET
        );
        assert_eq!(MAX_SUPPLY_ZKA, 23_000_000);
        assert_eq!(INITIAL_BLOCK_REWARD_ATOMS, 12_000_000);
        assert_eq!(COINBASE_MATURITY_BLOCKS, 60);
    }
}
