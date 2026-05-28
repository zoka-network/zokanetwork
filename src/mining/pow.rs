//! Pluggable proof-of-work backend for ZOKA.
//!
//! This module isolates the consensus-critical PoW choice behind a single
//! trait. The mainnet algorithm is RandomX (`zoka-randomx-v1`); the
//! development/testnet algorithm is a SHA3-based CPU fallback
//! (`zoka-cpu-fallback-v1`). The choice is anchored to the chain id, never
//! configurable per-node; see `docs/protocol-overview.md` for the
//! rationale and the binding implications.
//!
//! All consensus-relevant constants (algorithm IDs, epoch length, difficulty
//! bounds, target block time) live in `crate::economics`. Do NOT redefine
//! them here; this module only consumes them.
//!
//! Build matrix:
//! - default features: only `cpu-fallback` is available. Suitable for dev,
//!   CI and testnet. Mainnet builds will refuse to mine.
//! - `--features randomx`: the RandomX backend is compiled in. This is the
//!   only valid configuration for a mainnet node.

use sha2::{Digest as _, Sha256};
use sha3::Sha3_256;

use crate::economics;

/// Identifier persisted in `UserMiningState::algorithm` and checked against
/// `economics::POW_ALGORITHM_*` to detect mismatched / unsafe configurations.
pub type AlgorithmId = &'static str;

/// Errors a backend can surface while preparing or running.
#[derive(Debug, thiserror::Error)]
pub enum PowError {
    #[error("pow input cannot be empty")]
    EmptyInput,

    #[error(
        "pow backend '{0}' is not compiled into this binary; rebuild with the matching feature flag"
    )]
    BackendNotAvailable(AlgorithmId),

    #[error(
        "pow backend '{0}' refused chain id '{1}': mainnet requires '{}'",
        crate::economics::POW_ALGORITHM_MAINNET
    )]
    AlgorithmChainMismatch(AlgorithmId, String),

    #[error("pow runtime error: {0}")]
    Runtime(String),
}

/// Stateless contract every PoW backend has to implement.
///
/// `calculate_hash` MUST be deterministic for a given engine instance. Two
/// nodes that build the engine with the same `(algorithm_id, epoch_seed)` and
/// hash the same input MUST get byte-identical output. This is what makes
/// PoW verifiable on the network.
pub trait PowEngine: Send + Sync {
    fn algorithm_id(&self) -> AlgorithmId;
    fn calculate_hash(&self, input: &[u8]) -> Result<Vec<u8>, PowError>;
}

/// Hash the canonical PoW seed for a given (chain_id, height) pair. The
/// seed rotates every `economics::POW_KEY_EPOCH_BLOCKS` blocks — that is the
/// "epoch" in RandomX language. Both backends consume the same seed so we
/// can swap them without changing the rest of the pipeline.
pub fn epoch_seed(chain_id: &str, height: u64) -> Vec<u8> {
    let epoch = height / economics::POW_KEY_EPOCH_BLOCKS;
    let mut hasher = Sha256::new();
    hasher.update(b"zoka-pow-epoch-seed-v1");
    hasher.update([0x1f]);
    hasher.update(chain_id.as_bytes());
    hasher.update([0x1f]);
    hasher.update(epoch.to_le_bytes());
    hasher.finalize().to_vec()
}

/// Number of leading zero bits in `hash`. Used by the mining loop to compare
/// against the difficulty target. Lives here so backends share a single
/// definition of "weight".
pub fn leading_zero_bits(hash: &[u8]) -> u32 {
    let mut bits = 0u32;
    for byte in hash {
        let lz = byte.leading_zeros();
        bits = bits.saturating_add(lz);
        if lz < 8 {
            break;
        }
    }
    bits
}

/// True when `hash` clears the difficulty target measured in leading zero
/// bits. Kept symmetric with `leading_zero_bits` so any backend's output is
/// scored the same way.
pub fn hash_meets_difficulty(hash: &[u8], difficulty_bits: u32) -> bool {
    leading_zero_bits(hash) >= difficulty_bits
}

/// Build the canonical PoW preimage. Every byte that goes into the hash
/// commits to a single field of the would-be block header. Changing this
/// layout is a hard fork.
pub fn build_pow_input(
    chain_id: &str,
    prev_hash: &str,
    wallet_address: &str,
    height: u64,
    timestamp: u64,
    nonce: u64,
) -> Vec<u8> {
    let mut input = Vec::with_capacity(192);
    input.extend_from_slice(chain_id.as_bytes());
    input.push(0x1f);
    input.extend_from_slice(prev_hash.as_bytes());
    input.push(0x1f);
    input.extend_from_slice(wallet_address.as_bytes());
    input.push(0x1f);
    input.extend_from_slice(&height.to_le_bytes());
    input.extend_from_slice(&timestamp.to_le_bytes());
    input.extend_from_slice(&nonce.to_le_bytes());
    input
}

/// Builds a backend for `(chain_id, height)`, picking the algorithm declared
/// canonical for the chain id. Returns `BackendNotAvailable` when the binary
/// was built without the required feature.
pub fn engine_for(chain_id: &str, height: u64) -> Result<Box<dyn PowEngine>, PowError> {
    let algorithm = canonical_algorithm_for(chain_id);
    let seed = epoch_seed(chain_id, height);
    build_engine(algorithm, &seed)
}

pub fn engine_pool_for(
    chain_id: &str,
    height: u64,
    lanes: usize,
) -> Result<Vec<Box<dyn PowEngine>>, PowError> {
    let algorithm = canonical_algorithm_for(chain_id);
    let seed = epoch_seed(chain_id, height);
    build_engine_pool(algorithm, &seed, lanes)
}

/// Returns the canonical algorithm for the given chain id. Today only
/// mainnet and testnet are recognized; anything else falls back to testnet
/// rules, which is intentional — unknown networks are dev/test, never
/// allowed to claim the mainnet algorithm.
pub fn canonical_algorithm_for(chain_id: &str) -> AlgorithmId {
    if chain_id == economics::MAINNET_CHAIN_ID || chain_id == economics::TESTNET_CHAIN_ID {
        economics::POW_ALGORITHM_MAINNET
    } else {
        economics::POW_ALGORITHM_TESTNET
    }
}

/// Lower-level builder: pick a backend by explicit algorithm id rather than
/// chain id. Useful in tests and benchmarks.
pub fn build_engine(algorithm: AlgorithmId, seed: &[u8]) -> Result<Box<dyn PowEngine>, PowError> {
    match algorithm {
        id if id == economics::POW_ALGORITHM_TESTNET => {
            Ok(Box::new(CpuFallbackPow::new(seed.to_vec())))
        }
        id if id == economics::POW_ALGORITHM_MAINNET => {
            #[cfg(feature = "randomx")]
            {
                Ok(Box::new(self::randomx_backend::RandomXPow::new(
                    seed.to_vec(),
                )?))
            }
            #[cfg(not(feature = "randomx"))]
            {
                Err(PowError::BackendNotAvailable(
                    economics::POW_ALGORITHM_MAINNET,
                ))
            }
        }
        other => Err(PowError::Runtime(format!(
            "unknown PoW algorithm id: {}",
            other
        ))),
    }
}

pub fn build_engine_pool(
    algorithm: AlgorithmId,
    seed: &[u8],
    lanes: usize,
) -> Result<Vec<Box<dyn PowEngine>>, PowError> {
    let lanes = lanes.max(1);
    match algorithm {
        id if id == economics::POW_ALGORITHM_TESTNET => Ok((0..lanes)
            .map(|_| Box::new(CpuFallbackPow::new(seed.to_vec())) as Box<dyn PowEngine>)
            .collect()),
        id if id == economics::POW_ALGORITHM_MAINNET => {
            #[cfg(feature = "randomx")]
            {
                self::randomx_backend::RandomXPow::new_pool(seed.to_vec(), lanes).map(|pool| {
                    pool.into_iter()
                        .map(|engine| Box::new(engine) as Box<dyn PowEngine>)
                        .collect()
                })
            }
            #[cfg(not(feature = "randomx"))]
            {
                Err(PowError::BackendNotAvailable(
                    economics::POW_ALGORITHM_MAINNET,
                ))
            }
        }
        other => Err(PowError::Runtime(format!(
            "unknown PoW algorithm id: {}",
            other
        ))),
    }
}

/// SHA3-based CPU fallback. **Not** a mainnet algorithm. Kept for dev,
/// testnet and CI where building the C++ RandomX library is impractical.
pub struct CpuFallbackPow {
    seed: Vec<u8>,
}

impl CpuFallbackPow {
    pub fn new(seed: Vec<u8>) -> Self {
        Self { seed }
    }
}

impl PowEngine for CpuFallbackPow {
    fn algorithm_id(&self) -> AlgorithmId {
        economics::POW_ALGORITHM_TESTNET
    }

    fn calculate_hash(&self, input: &[u8]) -> Result<Vec<u8>, PowError> {
        if input.is_empty() {
            return Err(PowError::EmptyInput);
        }
        let mut hasher = Sha3_256::new();
        hasher.update(b"zoka-pow-cpu-fallback-v1");
        hasher.update([0x1f]);
        hasher.update(&self.seed);
        hasher.update([0x1f]);
        hasher.update(input);
        Ok(hasher.finalize().to_vec())
    }
}

/// Mainnet PoW. Wraps the official RandomX C++ implementation via the
/// `randomx-rs` crate. The cache is rebuilt every `POW_KEY_EPOCH_BLOCKS`
/// blocks from the canonical epoch seed so any node can recompute the same
/// VM state from the chain.
#[cfg(feature = "randomx")]
mod randomx_backend {
    use super::*;
    use randomx_rs::{RandomXCache, RandomXDataset, RandomXFlag, RandomXVM};
    use std::sync::{Mutex, OnceLock};

    static DATASET_CACHE: OnceLock<Mutex<Option<CachedDataset>>> = OnceLock::new();

    #[derive(Clone)]
    struct CachedDataset {
        seed: Vec<u8>,
        flags: u32,
        dataset: RandomXDataset,
    }

    // `RandomXDataset` is internally reference-counted by `randomx-rs`, but the
    // crate does not mark its raw C handle as Send/Sync. We only store it behind
    // a process-wide mutex and clone it into VMs after lookup.
    unsafe impl Send for CachedDataset {}
    unsafe impl Sync for CachedDataset {}

    pub struct RandomXPow {
        vm: Mutex<RandomXVM>,
    }

    // `randomx-rs` wraps raw C pointers and does not declare Send/Sync.
    // ZOKA never exposes the VM directly: all RandomX calls go through this
    // mutex, so cross-thread access is serialized and the VM/cache outlive the
    // engine.
    unsafe impl Send for RandomXPow {}
    unsafe impl Sync for RandomXPow {}

    impl RandomXPow {
        pub fn new(seed: Vec<u8>) -> Result<Self, PowError> {
            let mut pool = Self::new_pool(seed, 1)?;
            pool.pop()
                .ok_or_else(|| PowError::Runtime("randomx pool init returned no VMs".to_string()))
        }

        pub fn new_pool(seed: Vec<u8>, lanes: usize) -> Result<Vec<Self>, PowError> {
            let (flags, dataset) = dataset_for_seed_best_effort(&seed)?;
            let mut pool = Vec::with_capacity(lanes.max(1));
            for _ in 0..lanes.max(1) {
                let vm = RandomXVM::new(flags, None, Some(dataset.clone()))
                    .map_err(|e| PowError::Runtime(format!("randomx vm init failed: {e:?}")))?;
                pool.push(Self { vm: Mutex::new(vm) });
            }
            Ok(pool)
        }
    }

    /// Returns the flags to use, choosing full-memory mode (fast, 2 GB dataset)
    /// only when the system has at least 2.5 GB of free RAM.  Otherwise uses
    /// light mode (256 MB cache).  Both produce identical RandomX hashes.
    fn select_randomx_flags() -> RandomXFlag {
        let base = RandomXFlag::get_recommended_flags();
        if free_ram_bytes() >= 2_500 * 1024 * 1024 {
            base | RandomXFlag::FLAG_FULL_MEM
        } else {
            base
        }
    }

    /// Best-effort estimate of free physical RAM in bytes.  Falls back to 0
    /// (forces light mode) if /proc/meminfo is unavailable (non-Linux).
    fn free_ram_bytes() -> u64 {
        let Ok(content) = std::fs::read_to_string("/proc/meminfo") else {
            return 0;
        };
        for line in content.lines() {
            if line.starts_with("MemAvailable:") {
                if let Some(kb) = line
                    .split_whitespace()
                    .nth(1)
                    .and_then(|s| s.parse::<u64>().ok())
                {
                    return kb * 1024;
                }
            }
        }
        0
    }

    fn dataset_for_seed_best_effort(
        seed: &[u8],
    ) -> Result<(RandomXFlag, RandomXDataset), PowError> {
        let flags = select_randomx_flags();
        let ds = dataset_for_seed(flags, seed)?;
        Ok((flags, ds))
    }

    fn dataset_for_seed(flags: RandomXFlag, seed: &[u8]) -> Result<RandomXDataset, PowError> {
        let cache = DATASET_CACHE.get_or_init(|| Mutex::new(None));
        let mut guard = cache
            .lock()
            .map_err(|e| PowError::Runtime(format!("randomx dataset cache lock poisoned: {e}")))?;

        if let Some(cached) = guard.as_ref() {
            if cached.seed == seed && cached.flags == flags.bits() {
                return Ok(cached.dataset.clone());
            }
        }

        let rx_cache = RandomXCache::new(flags, seed)
            .map_err(|e| PowError::Runtime(format!("randomx cache init failed: {e:?}")))?;
        let dataset = RandomXDataset::new(flags, rx_cache, 0)
            .map_err(|e| PowError::Runtime(format!("randomx dataset init failed: {e:?}")))?;
        *guard = Some(CachedDataset {
            seed: seed.to_vec(),
            flags: flags.bits(),
            dataset: dataset.clone(),
        });
        Ok(dataset)
    }

    impl PowEngine for RandomXPow {
        fn algorithm_id(&self) -> AlgorithmId {
            crate::economics::POW_ALGORITHM_MAINNET
        }

        fn calculate_hash(&self, input: &[u8]) -> Result<Vec<u8>, PowError> {
            if input.is_empty() {
                return Err(PowError::EmptyInput);
            }
            let guard = self
                .vm
                .lock()
                .map_err(|e| PowError::Runtime(format!("randomx vm lock poisoned: {e}")))?;
            let hash = guard
                .calculate_hash(input)
                .map_err(|e| PowError::Runtime(format!("randomx calculate_hash failed: {e:?}")))?;
            Ok(hash.to_vec())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_seed_rotates_every_epoch() {
        let chain_id = "zoka-test-net";
        let same_a = epoch_seed(chain_id, 0);
        let same_b = epoch_seed(chain_id, economics::POW_KEY_EPOCH_BLOCKS - 1);
        assert_eq!(same_a, same_b, "seed must be stable inside an epoch");

        let next = epoch_seed(chain_id, economics::POW_KEY_EPOCH_BLOCKS);
        assert_ne!(same_a, next, "seed must rotate at the epoch boundary");
    }

    #[test]
    fn cpu_fallback_is_deterministic_for_same_seed() {
        let seed = b"deterministic-test-seed".to_vec();
        let engine_a = CpuFallbackPow::new(seed.clone());
        let engine_b = CpuFallbackPow::new(seed);
        let input = b"zoka-input";
        assert_eq!(
            engine_a.calculate_hash(input).expect("hash a"),
            engine_b.calculate_hash(input).expect("hash b"),
            "two engines built from the same seed must produce the same hash"
        );
    }

    #[test]
    fn cpu_fallback_rejects_empty_input() {
        let engine = CpuFallbackPow::new(b"seed".to_vec());
        assert!(matches!(
            engine.calculate_hash(&[]),
            Err(PowError::EmptyInput)
        ));
    }

    #[test]
    fn cpu_fallback_seed_change_changes_output() {
        let input = b"same-input";
        let a = CpuFallbackPow::new(b"seed-a".to_vec())
            .calculate_hash(input)
            .expect("hash a");
        let b = CpuFallbackPow::new(b"seed-b".to_vec())
            .calculate_hash(input)
            .expect("hash b");
        assert_ne!(a, b);
    }

    #[test]
    fn build_pow_input_layout_commits_to_each_field() {
        // Every field must contribute to the preimage. If any of these
        // assertions starts failing it means the layout was loosened —
        // that is a consensus-breaking change and must go through a
        // hard fork, not a silent edit.
        let baseline = build_pow_input("zoka-net-x", "prev", "zka1addr", 1, 100, 7);

        assert_ne!(
            baseline,
            build_pow_input("zoka-net-y", "prev", "zka1addr", 1, 100, 7),
            "chain_id must affect the preimage"
        );
        assert_ne!(
            baseline,
            build_pow_input("zoka-net-x", "PREV", "zka1addr", 1, 100, 7),
            "prev_hash must affect the preimage"
        );
        assert_ne!(
            baseline,
            build_pow_input("zoka-net-x", "prev", "zka1other", 1, 100, 7),
            "wallet_address must affect the preimage"
        );
        assert_ne!(
            baseline,
            build_pow_input("zoka-net-x", "prev", "zka1addr", 2, 100, 7),
            "height must affect the preimage"
        );
        assert_ne!(
            baseline,
            build_pow_input("zoka-net-x", "prev", "zka1addr", 1, 101, 7),
            "timestamp must affect the preimage"
        );
        assert_ne!(
            baseline,
            build_pow_input("zoka-net-x", "prev", "zka1addr", 1, 100, 8),
            "nonce must affect the preimage"
        );
    }

    #[test]
    fn build_pow_input_is_deterministic() {
        let a = build_pow_input("zoka-net", "prev", "zka1addr", 5, 50, 9);
        let b = build_pow_input("zoka-net", "prev", "zka1addr", 5, 50, 9);
        assert_eq!(a, b);
    }

    #[test]
    fn hash_meets_difficulty_matches_leading_zero_bits() {
        // Hash whose first byte is 0x0F => 4 leading zero bits.
        let hash = vec![0x0F, 0xFF, 0xFF];
        assert_eq!(leading_zero_bits(&hash), 4);
        assert!(hash_meets_difficulty(&hash, 4));
        assert!(hash_meets_difficulty(&hash, 3));
        assert!(!hash_meets_difficulty(&hash, 5));
    }

    #[test]
    fn engine_for_mainnet_without_randomx_feature_returns_clear_error() {
        let result = engine_for(economics::MAINNET_CHAIN_ID, 0);
        #[cfg(not(feature = "randomx"))]
        {
            match result {
                Err(PowError::BackendNotAvailable(id)) => {
                    assert_eq!(id, economics::POW_ALGORITHM_MAINNET);
                }
                other => panic!(
                    "expected BackendNotAvailable for mainnet without --features randomx, got: {:?}",
                    other.map(|_| "engine")
                ),
            }
        }
        #[cfg(feature = "randomx")]
        {
            assert!(
                result.is_ok(),
                "with --features randomx, mainnet engine must build"
            );
        }
    }

    #[test]
    fn engine_for_unknown_chain_id_picks_testnet_algorithm() {
        let engine = engine_for("zoka-some-devnet", 0).expect("testnet engine builds by default");
        assert_eq!(engine.algorithm_id(), economics::POW_ALGORITHM_TESTNET);
    }

    #[test]
    fn canonical_algorithm_per_chain_is_locked_in() {
        assert_eq!(
            canonical_algorithm_for(economics::MAINNET_CHAIN_ID),
            economics::POW_ALGORITHM_MAINNET
        );
        assert_eq!(
            canonical_algorithm_for(economics::TESTNET_CHAIN_ID),
            economics::POW_ALGORITHM_MAINNET
        );
        assert_eq!(
            canonical_algorithm_for("zoka-something-else"),
            economics::POW_ALGORITHM_TESTNET
        );
    }
}
