use bellman::groth16::{
    create_random_proof, generate_random_parameters, prepare_verifying_key, verify_proof,
    Parameters, PreparedVerifyingKey, Proof,
};
use bellman::{Circuit, ConstraintSystem, SynthesisError};
use bls12_381::{Bls12, Scalar};
use rand::rngs::OsRng;
use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::OnceLock;

use crate::crypto::{
    private_tree_hash_commitment_words, private_tree_hash_pair, private_tree_round_constants,
    private_tree_scalar_from_words, PRIVATE_TREE_DEPTH, PRIVATE_TREE_SCALAR_WORDS,
};
use crate::types::BlockError;
use crate::zk::setup_io;

/// Env var pointing at a directory that holds a release trusted-setup bundle.
/// When present, the node boots from that bundle after verifying its digest
/// instead of regenerating the CRS.
pub const TRUSTED_SETUP_ENV: &str = "ZOKA_TRUSTED_SETUP_DIR";

/// Opt-in acknowledgement that the seeded/deterministic fallback CRS is *not*
/// sound. Without this env var set (or explicit code path), we refuse to boot
/// with the seeded fallback so operators cannot run production nodes against
/// a known-tau CRS by accident.
pub const ALLOW_INSECURE_SEEDED_SETUP_ENV: &str = "ZOKA_ALLOW_INSECURE_SEEDED_SETUP";

/// Runtime override of the expected CRS digest. Useful for staging networks
/// and rotating ceremonies without rebuilding the binary.
pub const EXPECTED_SETUP_DIGEST_ENV: &str = "ZOKA_EXPECTED_SETUP_DIGEST";

/// Compile-time binding of the canonical CRS digest. Set at build time with
/// `ZOKA_EXPECTED_SETUP_DIGEST_COMPILED=<hex>` to hard-pin the binary to a
/// specific ceremony output; any operator that tries to point
/// `ZOKA_TRUSTED_SETUP_DIR` at a different bundle gets rejected at boot.
pub const EXPECTED_SETUP_DIGEST_COMPILED: Option<&str> =
    option_env!("ZOKA_EXPECTED_SETUP_DIGEST_COMPILED");

pub const BALANCE_CIRCUIT: &str = "balance_circuit";
pub const MERKLE_CIRCUIT: &str = "merkle_circuit";
pub const NULLIFIER_CIRCUIT: &str = "nullifier_circuit";
pub const PRIVATE_TX_CIRCUIT: &str = "private_tx";
/// Benchmark-only compact profile. It is deliberately excluded from node
/// verification and the canonical trusted-setup bundle until dual-stack
/// activation is implemented and audited.
pub const PRIVATE_TX_V2_I1_LAB_CIRCUIT: &str = "private_tx_v2_i1";

pub const BALANCE_PRIVATE_INPUT_WORDS: usize = 4;
pub const BALANCE_PUBLIC_INPUT_WORDS: usize = 2;
pub const MERKLE_PUBLIC_INPUT_WORDS: usize = 2;
pub const NULLIFIER_PRIVATE_INPUT_WORDS: usize = 3;
pub const NULLIFIER_PUBLIC_INPUT_WORDS: usize = 1;
pub const PRIVATE_TX_MAX_INPUTS: usize = 96;
pub const PRIVATE_TX_MERKLE_MAX_DEPTH: usize = PRIVATE_TREE_DEPTH;
/// Number of private inputs per tx whose Merkle membership is proved
/// *inside* the Groth16 circuit. Must be ≤ `PRIVATE_TX_MAX_INPUTS`.
///
/// We allow large note fan-in for wallets funded by many small outputs,
/// but only the first 8 proven inputs are carried inside the Groth16 circuit.
/// This keeps proving time operational while preserving in-circuit membership
/// checks for a fixed privacy-preserving subset of inputs.
pub const PRIVATE_TX_MERKLE_PROVEN_INPUTS: usize = 8;
pub const PRIVATE_TX_ROOT_WORDS: usize = PRIVATE_TREE_SCALAR_WORDS;
pub const PRIVATE_TX_PRIVATE_INPUT_WORDS: usize = 10
    + (PRIVATE_TX_MAX_INPUTS * 3)
    + (PRIVATE_TX_MERKLE_PROVEN_INPUTS * PRIVATE_TX_MERKLE_MAX_DEPTH * PRIVATE_TX_ROOT_WORDS)
    + (PRIVATE_TX_MERKLE_PROVEN_INPUTS * PRIVATE_TX_MERKLE_MAX_DEPTH); // header + tx asset tags + padded input amounts/key images/asset tags + proven path siblings + proven path directions
pub const PRIVATE_TX_PUBLIC_INPUT_WORDS: usize = 2
    + PRIVATE_TX_MAX_INPUTS
    + PRIVATE_TX_MERKLE_PROVEN_INPUTS
    + (PRIVATE_TX_MERKLE_PROVEN_INPUTS * PRIVATE_TREE_SCALAR_WORDS)
    + PRIVATE_TX_ROOT_WORDS; // header + padded input key images + proven input flags + proven input commitment words + root anchor
pub const PRIVATE_TX_RANGE_BITS: usize = 64;

#[derive(Clone, Copy)]
struct PrivateTxCircuitProfile {
    circuit_name: &'static str,
    max_inputs: usize,
    merkle_proven_inputs: usize,
}

impl PrivateTxCircuitProfile {
    fn private_input_words(self) -> usize {
        10 + (self.max_inputs * 3)
            + (self.merkle_proven_inputs * PRIVATE_TX_MERKLE_MAX_DEPTH * PRIVATE_TX_ROOT_WORDS)
            + (self.merkle_proven_inputs * PRIVATE_TX_MERKLE_MAX_DEPTH)
    }

    fn public_input_words(self) -> usize {
        2 + self.max_inputs
            + self.merkle_proven_inputs
            + (self.merkle_proven_inputs * PRIVATE_TREE_SCALAR_WORDS)
            + PRIVATE_TX_ROOT_WORDS
    }
}

const PRIVATE_TX_V1_PROFILE: PrivateTxCircuitProfile = PrivateTxCircuitProfile {
    circuit_name: PRIVATE_TX_CIRCUIT,
    max_inputs: PRIVATE_TX_MAX_INPUTS,
    merkle_proven_inputs: PRIVATE_TX_MERKLE_PROVEN_INPUTS,
};

const PRIVATE_TX_V2_I1_LAB_PROFILE: PrivateTxCircuitProfile = PrivateTxCircuitProfile {
    circuit_name: PRIVATE_TX_V2_I1_LAB_CIRCUIT,
    max_inputs: 1,
    merkle_proven_inputs: 1,
};

fn private_tx_profile_for_circuit(circuit: &str) -> Result<PrivateTxCircuitProfile, BlockError> {
    match circuit {
        PRIVATE_TX_CIRCUIT => Ok(PRIVATE_TX_V1_PROFILE),
        PRIVATE_TX_V2_I1_LAB_CIRCUIT => Ok(PRIVATE_TX_V2_I1_LAB_PROFILE),
        _ => Err(BlockError::TransactionValidationFailed(format!(
            "unsupported private transaction Groth16 circuit: {}",
            circuit
        ))),
    }
}

const MERKLE_DEPTH: usize = 16;

struct SetupBundle {
    balance_params: Parameters<Bls12>,
    balance_pvk: PreparedVerifyingKey<Bls12>,
    merkle_params: Parameters<Bls12>,
    merkle_pvk: PreparedVerifyingKey<Bls12>,
    nullifier_params: Parameters<Bls12>,
    nullifier_pvk: PreparedVerifyingKey<Bls12>,
    private_tx_params: Parameters<Bls12>,
    private_tx_pvk: PreparedVerifyingKey<Bls12>,
    private_tx_v2_i1_params: Option<Parameters<Bls12>>,
    private_tx_v2_i1_pvk: Option<PreparedVerifyingKey<Bls12>>,
}

/// WARNING: only for development and tests. τ is derivable from the seed, so
/// any proof verified against this CRS can be forged by anyone who has read
/// this file. `seeded_rng` must never feed a production node.
fn seeded_rng(seed_tag: u8) -> ChaCha20Rng {
    let mut seed = [0u8; 32];
    seed[0] = seed_tag;
    seed[31] = seed_tag ^ 0xA5;
    ChaCha20Rng::from_seed(seed)
}

fn build_bundle_from_params(
    balance_params: Parameters<Bls12>,
    merkle_params: Parameters<Bls12>,
    nullifier_params: Parameters<Bls12>,
    private_tx_params: Parameters<Bls12>,
    private_tx_v2_i1_params: Option<Parameters<Bls12>>,
) -> SetupBundle {
    let balance_pvk = prepare_verifying_key(&balance_params.vk);
    let merkle_pvk = prepare_verifying_key(&merkle_params.vk);
    let nullifier_pvk = prepare_verifying_key(&nullifier_params.vk);
    let private_tx_pvk = prepare_verifying_key(&private_tx_params.vk);
    let private_tx_v2_i1_pvk = private_tx_v2_i1_params
        .as_ref()
        .map(|params| prepare_verifying_key(&params.vk));
    SetupBundle {
        balance_params,
        balance_pvk,
        merkle_params,
        merkle_pvk,
        nullifier_params,
        nullifier_pvk,
        private_tx_params,
        private_tx_pvk,
        private_tx_v2_i1_params,
        private_tx_v2_i1_pvk,
    }
}

/// Generate every CRS from OsRng. Suitable for:
///   * tests that want a fresh CRS (not the deterministic one),
///   * release tooling that writes artifacts to disk.
///
/// The resulting secret randomness lives ephemerally inside `OsRng`'s state during this call
/// and is dropped afterwards. That is strictly better than the seeded
/// fallback, but still weaker than a multi-party ceremony: one operator
/// could in principle instrument the RNG. The only cure for that is MPC.
/// Generate Groth16 parameters for a single named circuit using OS entropy.
/// Returns an error string if the circuit name is unrecognised.
pub fn generate_circuit_params_with_osrng(circuit: &str) -> Result<Parameters<Bls12>, String> {
    match circuit {
        BALANCE_CIRCUIT => {
            generate_random_parameters::<Bls12, _, _>(GrothBalanceCircuit::blank(), &mut OsRng)
                .map_err(|e| format!("setup for {circuit}: {e}"))
        }
        MERKLE_CIRCUIT => {
            generate_random_parameters::<Bls12, _, _>(GrothMerkleCircuit::blank(), &mut OsRng)
                .map_err(|e| format!("setup for {circuit}: {e}"))
        }
        NULLIFIER_CIRCUIT => {
            generate_random_parameters::<Bls12, _, _>(GrothNullifierCircuit::blank(), &mut OsRng)
                .map_err(|e| format!("setup for {circuit}: {e}"))
        }
        PRIVATE_TX_CIRCUIT => {
            generate_random_parameters::<Bls12, _, _>(GrothPrivateTxCircuit::blank(), &mut OsRng)
                .map_err(|e| format!("setup for {circuit}: {e}"))
        }
        other => Err(format!("unknown circuit: {other}")),
    }
}

pub fn generate_private_tx_v2_i1_lab_params_with_osrng() -> Result<Parameters<Bls12>, String> {
    generate_random_parameters::<Bls12, _, _>(
        GrothPrivateTxCircuit::blank_for_profile(1, 1),
        &mut OsRng,
    )
    .map_err(|e| format!("setup for {PRIVATE_TX_V2_I1_LAB_CIRCUIT}: {e}"))
}

pub fn generate_setup_bundle_with_osrng() -> (
    Parameters<Bls12>,
    Parameters<Bls12>,
    Parameters<Bls12>,
    Parameters<Bls12>,
) {
    let balance_params =
        generate_random_parameters::<Bls12, _, _>(GrothBalanceCircuit::blank(), &mut OsRng)
            .expect("trusted setup for balance_circuit");
    let merkle_params =
        generate_random_parameters::<Bls12, _, _>(GrothMerkleCircuit::blank(), &mut OsRng)
            .expect("trusted setup for merkle_circuit");
    let nullifier_params =
        generate_random_parameters::<Bls12, _, _>(GrothNullifierCircuit::blank(), &mut OsRng)
            .expect("trusted setup for nullifier_circuit");
    let private_tx_params =
        generate_random_parameters::<Bls12, _, _>(GrothPrivateTxCircuit::blank(), &mut OsRng)
            .expect("trusted setup for private_tx");
    (
        balance_params,
        merkle_params,
        nullifier_params,
        private_tx_params,
    )
}

fn build_setup_bundle_seeded() -> SetupBundle {
    let mut rng_balance = seeded_rng(0x11);
    let balance_params =
        generate_random_parameters::<Bls12, _, _>(GrothBalanceCircuit::blank(), &mut rng_balance)
            .expect("trusted setup for balance_circuit");

    let mut rng_merkle = seeded_rng(0x22);
    let merkle_params =
        generate_random_parameters::<Bls12, _, _>(GrothMerkleCircuit::blank(), &mut rng_merkle)
            .expect("trusted setup for merkle_circuit");

    let mut rng_nullifier = seeded_rng(0x33);
    let nullifier_params = generate_random_parameters::<Bls12, _, _>(
        GrothNullifierCircuit::blank(),
        &mut rng_nullifier,
    )
    .expect("trusted setup for nullifier_circuit");

    let mut rng_private = seeded_rng(0x44);
    let private_tx_params =
        generate_random_parameters::<Bls12, _, _>(GrothPrivateTxCircuit::blank(), &mut rng_private)
            .expect("trusted setup for private_tx");

    build_bundle_from_params(
        balance_params,
        merkle_params,
        nullifier_params,
        private_tx_params,
        None,
    )
}

/// Canonical digest the running binary expects to see in the trusted-setup
/// directory. Runtime env var wins over the compile-time constant; missing
/// both means "trust whatever is in the directory" (dev / staging).
fn expected_setup_digest() -> Option<[u8; 32]> {
    let hex_string = std::env::var(EXPECTED_SETUP_DIGEST_ENV)
        .ok()
        .or_else(|| EXPECTED_SETUP_DIGEST_COMPILED.map(|s| s.to_string()))?;
    let bytes = hex::decode(hex_string.trim()).ok()?;
    if bytes.len() != 32 {
        return None;
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Some(out)
}

fn load_setup_bundle_from_disk(dir: &std::path::Path) -> Result<SetupBundle, String> {
    let actual_digest = setup_io::verify_directory_digest(dir).map_err(|e| e.to_string())?;
    if let Some(expected) = expected_setup_digest() {
        if expected != actual_digest {
            return Err(format!(
                "trusted setup digest does not match expected canonical CRS. \
                 expected = {}, found = {}. Either update {} / rebuild with \
                 ZOKA_EXPECTED_SETUP_DIGEST_COMPILED, or point {} at the correct bundle.",
                hex::encode(expected),
                hex::encode(actual_digest),
                EXPECTED_SETUP_DIGEST_ENV,
                TRUSTED_SETUP_ENV,
            ));
        }
    }
    let balance_params =
        setup_io::read_circuit_params(dir, BALANCE_CIRCUIT).map_err(|e| e.to_string())?;
    let merkle_params =
        setup_io::read_circuit_params(dir, MERKLE_CIRCUIT).map_err(|e| e.to_string())?;
    let nullifier_params =
        setup_io::read_circuit_params(dir, NULLIFIER_CIRCUIT).map_err(|e| e.to_string())?;
    let private_tx_params =
        setup_io::read_circuit_params(dir, PRIVATE_TX_CIRCUIT).map_err(|e| e.to_string())?;
    let private_tx_v2_i1_path = dir
        .join(setup_io::params_filename(PRIVATE_TX_V2_I1_LAB_CIRCUIT).map_err(|e| e.to_string())?);
    let private_tx_v2_i1_params = if private_tx_v2_i1_path.exists() {
        Some(
            setup_io::read_circuit_params(dir, PRIVATE_TX_V2_I1_LAB_CIRCUIT)
                .map_err(|e| e.to_string())?,
        )
    } else {
        None
    };
    Ok(build_bundle_from_params(
        balance_params,
        merkle_params,
        nullifier_params,
        private_tx_params,
        private_tx_v2_i1_params,
    ))
}

/// Directories to probe for a trusted-setup bundle when
/// `ZOKA_TRUSTED_SETUP_DIR` is unset: alongside the executable first (so a
/// shipped folder works from anywhere), then the current working directory.
fn auto_trusted_setup_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            out.push(dir.join("trusted-setup"));
        }
    }
    out.push(PathBuf::from("trusted-setup"));
    out
}

fn build_setup_bundle() -> SetupBundle {
    if let Ok(dir) = std::env::var(TRUSTED_SETUP_ENV) {
        let path = PathBuf::from(&dir);
        match load_setup_bundle_from_disk(&path) {
            Ok(bundle) => {
                tracing::info!(
                    target: "zk::trusted_setup",
                    path = %path.display(),
                    "loaded trusted setup from disk"
                );
                return bundle;
            }
            Err(err) => {
                panic!(
                    "failed to load trusted setup from {}: {}. Replace it with \
                     a valid release trusted-setup bundle or unset {}.",
                    path.display(),
                    err,
                    TRUSTED_SETUP_ENV
                );
            }
        }
    }

    // No explicit env var: auto-discover a release bundle next to the
    // executable or in the current directory, so standalone users can just run
    // the binary with its `trusted-setup/` folder alongside it — no env var and
    // no wrapper script needed. The digest check in load_setup_bundle_from_disk
    // still rejects a tampered or foreign bundle, so this is exactly as safe as
    // pointing the env var at the same directory.
    for candidate in auto_trusted_setup_candidates() {
        if !candidate.is_dir() {
            continue;
        }
        match load_setup_bundle_from_disk(&candidate) {
            Ok(bundle) => {
                tracing::info!(
                    target: "zk::trusted_setup",
                    path = %candidate.display(),
                    "auto-discovered trusted setup next to the binary"
                );
                return bundle;
            }
            Err(err) => {
                tracing::warn!(
                    target: "zk::trusted_setup",
                    path = %candidate.display(),
                    error = %err,
                    "found a trusted-setup directory but it did not load; ignoring it"
                );
            }
        }
    }

    // Debug builds (local dev, `cargo test`) fall back to the seeded CRS with
    // a loud warning. Release builds refuse to boot without either a setup
    // directory or an explicit opt-in env var; otherwise an operator could
    // ship a production node silently verifying against a source-visible CRS.
    let allow_seeded =
        cfg!(debug_assertions) || std::env::var(ALLOW_INSECURE_SEEDED_SETUP_ENV).is_ok();
    if allow_seeded {
        tracing::warn!(
            target: "zk::trusted_setup",
            "using seeded (insecure) trusted setup; proofs verified against this CRS can be forged. \
             Set {} to a directory of ceremony artifacts before running in production.",
            TRUSTED_SETUP_ENV
        );
        return build_setup_bundle_seeded();
    }

    panic!(
        "ZOKA refuses to boot with the seeded in-memory trusted setup in release mode. Either:\n\
         (a) Point {} at a valid release trusted-setup bundle:\n    \
             export {}=./trusted-setup\n\
         (b) Opt into the insecure seeded setup explicitly for dev by setting {}=1.",
        TRUSTED_SETUP_ENV, TRUSTED_SETUP_ENV, ALLOW_INSECURE_SEEDED_SETUP_ENV
    );
}

fn setup_bundle() -> &'static SetupBundle {
    static SETUP: OnceLock<SetupBundle> = OnceLock::new();
    SETUP.get_or_init(build_setup_bundle)
}

fn parameters_for(circuit: &str) -> Result<&'static Parameters<Bls12>, BlockError> {
    let setup = setup_bundle();
    match circuit {
        BALANCE_CIRCUIT => Ok(&setup.balance_params),
        MERKLE_CIRCUIT => Ok(&setup.merkle_params),
        NULLIFIER_CIRCUIT => Ok(&setup.nullifier_params),
        PRIVATE_TX_CIRCUIT => Ok(&setup.private_tx_params),
        PRIVATE_TX_V2_I1_LAB_CIRCUIT => setup.private_tx_v2_i1_params.as_ref().ok_or_else(|| {
            BlockError::TransactionValidationFailed(format!(
                "{} parameters are unavailable in the authenticated trusted setup bundle",
                PRIVATE_TX_V2_I1_LAB_CIRCUIT
            ))
        }),
        _ => Err(BlockError::TransactionValidationFailed(format!(
            "unsupported Groth16 circuit: {}",
            circuit
        ))),
    }
}

fn pvk_for(circuit: &str) -> Result<&'static PreparedVerifyingKey<Bls12>, String> {
    let setup = setup_bundle();
    match circuit {
        BALANCE_CIRCUIT => Ok(&setup.balance_pvk),
        MERKLE_CIRCUIT => Ok(&setup.merkle_pvk),
        NULLIFIER_CIRCUIT => Ok(&setup.nullifier_pvk),
        PRIVATE_TX_CIRCUIT => Ok(&setup.private_tx_pvk),
        PRIVATE_TX_V2_I1_LAB_CIRCUIT => setup.private_tx_v2_i1_pvk.as_ref().ok_or_else(|| {
            format!(
                "{} verification key is unavailable in the authenticated trusted setup bundle",
                PRIVATE_TX_V2_I1_LAB_CIRCUIT
            )
        }),
        _ => Err(format!("unsupported Groth16 circuit: {}", circuit)),
    }
}

fn serialize_proof(proof: &Proof<Bls12>) -> Result<Vec<u8>, BlockError> {
    let mut out = Vec::new();
    proof.write(&mut out).map_err(|e| {
        BlockError::SerializationError(format!("failed to serialize Groth16 proof: {}", e))
    })?;
    Ok(out)
}

fn parse_u64_words(
    bytes: &[u8],
    expected_words: usize,
    context: &str,
) -> Result<Vec<u64>, BlockError> {
    let expected_len = expected_words * 8;
    if bytes.len() != expected_len {
        return Err(BlockError::TransactionValidationFailed(format!(
            "{} must be {} bytes (got {})",
            context,
            expected_len,
            bytes.len()
        )));
    }

    bytes
        .chunks_exact(8)
        .map(|chunk| {
            let arr: [u8; 8] = chunk.try_into().map_err(|_| {
                BlockError::TransactionValidationFailed("invalid u64 encoding".to_string())
            })?;
            Ok(u64::from_le_bytes(arr))
        })
        .collect()
}

fn encode_public_input_words(public_inputs: &[u8]) -> Vec<Vec<u8>> {
    public_inputs
        .chunks_exact(8)
        .map(|chunk| chunk.to_vec())
        .collect()
}

fn scalar_from_public_words(words: &[u64], context: &str) -> Result<Scalar, BlockError> {
    private_tree_scalar_from_words(words).ok_or_else(|| {
        BlockError::TransactionValidationFailed(format!(
            "{} is not a canonical private tree scalar",
            context
        ))
    })
}

fn decode_public_input_words(
    public_inputs: &[Vec<u8>],
    expected: usize,
) -> Result<Vec<u64>, String> {
    if public_inputs.len() != expected {
        return Err(format!(
            "invalid public input count: expected {}, got {}",
            expected,
            public_inputs.len()
        ));
    }

    public_inputs
        .iter()
        .enumerate()
        .map(|(idx, input)| {
            if input.len() != 8 {
                return Err(format!(
                    "public input {} must be 8 bytes (got {})",
                    idx,
                    input.len()
                ));
            }
            let arr: [u8; 8] = input
                .as_slice()
                .try_into()
                .map_err(|_| format!("invalid public input {}", idx))?;
            Ok(u64::from_le_bytes(arr))
        })
        .collect()
}

pub fn is_groth16_circuit(circuit: &str) -> bool {
    matches!(
        circuit,
        BALANCE_CIRCUIT
            | MERKLE_CIRCUIT
            | NULLIFIER_CIRCUIT
            | PRIVATE_TX_CIRCUIT
            | PRIVATE_TX_V2_I1_LAB_CIRCUIT
    )
}

pub fn expected_public_input_count(circuit: &str) -> Option<usize> {
    match circuit {
        BALANCE_CIRCUIT => Some(BALANCE_PUBLIC_INPUT_WORDS),
        MERKLE_CIRCUIT => Some(MERKLE_PUBLIC_INPUT_WORDS),
        NULLIFIER_CIRCUIT => Some(NULLIFIER_PUBLIC_INPUT_WORDS),
        PRIVATE_TX_CIRCUIT => Some(PRIVATE_TX_PUBLIC_INPUT_WORDS),
        PRIVATE_TX_V2_I1_LAB_CIRCUIT => Some(PRIVATE_TX_V2_I1_LAB_PROFILE.public_input_words()),
        _ => None,
    }
}

pub fn ensure_trusted_setup() {
    let _ = setup_bundle();
}

struct PrivateTxV2I1LabSetup {
    trusted_setup_dir: PathBuf,
    params: Parameters<Bls12>,
    pvk: PreparedVerifyingKey<Bls12>,
}

fn private_tx_v2_i1_lab_setup(
    trusted_setup_dir: &std::path::Path,
) -> Result<&'static PrivateTxV2I1LabSetup, String> {
    let requested_dir = trusted_setup_dir
        .canonicalize()
        .map_err(|e| format!("canonicalizing {}: {e}", trusted_setup_dir.display()))?;
    static LAB_SETUP: OnceLock<Result<PrivateTxV2I1LabSetup, String>> = OnceLock::new();
    match LAB_SETUP.get_or_init(|| {
        let params =
            setup_io::read_circuit_params(&requested_dir, PRIVATE_TX_V2_I1_LAB_CIRCUIT)
                .map_err(|e| e.to_string())?;
        let pvk = prepare_verifying_key(&params.vk);
        Ok(PrivateTxV2I1LabSetup {
            trusted_setup_dir: requested_dir.clone(),
            params,
            pvk,
        })
    }) {
        Ok(setup) if setup.trusted_setup_dir == requested_dir => Ok(setup),
        Ok(setup) => Err(format!(
            "{PRIVATE_TX_V2_I1_LAB_CIRCUIT} lab setup is already loaded from {}; refusing to switch to {} in the same process",
            setup.trusted_setup_dir.display(),
            requested_dir.display()
        )),
        Err(err) => Err(err.clone()),
    }
}

pub fn preload_private_tx_v2_i1_lab_params(
    trusted_setup_dir: &std::path::Path,
) -> Result<(), String> {
    let _ = private_tx_v2_i1_lab_setup(trusted_setup_dir)?;
    Ok(())
}

pub struct PrivateTxV2I1LabBenchmark {
    pub params_bytes: u64,
    pub params_load_ms: u64,
    pub prove_ms: u64,
    pub verify_ms: u64,
    pub proof_bytes: usize,
    pub public_input_count: usize,
}

fn encode_u64_words(words: &[u64]) -> Vec<u8> {
    words.iter().flat_map(|word| word.to_le_bytes()).collect()
}

fn synthetic_inactive_private_tx_v2_i1_inputs() -> (Vec<u8>, Vec<u8>) {
    let mut private_words = vec![
        0, // amount
        0, // blind
        0, // secret
        0, // index
        0, // input sum
        0, // output sum
        0, // fee
        0, // primary key image
        1, // input asset tag
        1, // output asset tag
        0, // padded input amount
        0, // padded input key image
        0, // padded input asset tag
    ];
    private_words.extend(vec![0; PRIVATE_TX_MERKLE_MAX_DEPTH * PRIVATE_TX_ROOT_WORDS]);
    private_words.extend(vec![0; PRIVATE_TX_MERKLE_MAX_DEPTH]);
    debug_assert_eq!(
        private_words.len(),
        PRIVATE_TX_V2_I1_LAB_PROFILE.private_input_words()
    );

    let public_words = vec![0; PRIVATE_TX_V2_I1_LAB_PROFILE.public_input_words()];
    (
        encode_u64_words(&private_words),
        encode_u64_words(&public_words),
    )
}

pub fn benchmark_private_tx_v2_i1_lab(
    trusted_setup_dir: &std::path::Path,
) -> Result<PrivateTxV2I1LabBenchmark, String> {
    let params_path = trusted_setup_dir
        .join(setup_io::params_filename(PRIVATE_TX_V2_I1_LAB_CIRCUIT).map_err(|e| e.to_string())?);
    let params_bytes = std::fs::metadata(&params_path)
        .map_err(|e| format!("reading {} metadata: {e}", params_path.display()))?
        .len();

    let load_started = std::time::Instant::now();
    let setup = private_tx_v2_i1_lab_setup(trusted_setup_dir)?;
    let params_load_ms = load_started
        .elapsed()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX);

    let (private_inputs, public_inputs) = synthetic_inactive_private_tx_v2_i1_inputs();
    let prove_started = std::time::Instant::now();
    let (proof_bytes, encoded_public_inputs) = prove_private_tx_for_profile(
        &private_inputs,
        &public_inputs,
        PRIVATE_TX_V2_I1_LAB_PROFILE,
        &setup.params,
    )
    .map_err(|e| e.to_string())?;
    let prove_ms = prove_started
        .elapsed()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX);

    let verify_started = std::time::Instant::now();
    if !verify_private_tx_for_profile(
        &proof_bytes,
        &encoded_public_inputs,
        PRIVATE_TX_V2_I1_LAB_PROFILE,
        &setup.pvk,
    )? {
        return Err(format!(
            "Groth16 verification for {PRIVATE_TX_V2_I1_LAB_CIRCUIT} failed"
        ));
    }
    let verify_ms = verify_started
        .elapsed()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX);

    Ok(PrivateTxV2I1LabBenchmark {
        params_bytes,
        params_load_ms,
        prove_ms,
        verify_ms,
        proof_bytes: proof_bytes.len(),
        public_input_count: encoded_public_inputs.len(),
    })
}

pub fn verification_key_bytes(circuit: &str) -> Result<Vec<u8>, BlockError> {
    let params = parameters_for(circuit)?;
    let mut out = Vec::new();
    params.vk.write(&mut out).map_err(|e| {
        BlockError::SerializationError(format!("failed to serialize verification key: {}", e))
    })?;
    Ok(out)
}

pub fn prove_balance(
    private_inputs: &[u8],
    public_inputs: &[u8],
) -> Result<(Vec<u8>, Vec<Vec<u8>>), BlockError> {
    let private_values = parse_u64_words(
        private_inputs,
        BALANCE_PRIVATE_INPUT_WORDS,
        "balance_circuit private inputs",
    )?;
    let public_values = parse_u64_words(
        public_inputs,
        BALANCE_PUBLIC_INPUT_WORDS,
        "balance_circuit public inputs",
    )?;

    let circuit = GrothBalanceCircuit {
        input_commitment: Some(Scalar::from(public_values[0])),
        output_commitment: Some(Scalar::from(public_values[1])),
        input_value: Some(Scalar::from(private_values[0])),
        output_value: Some(Scalar::from(private_values[1])),
        input_blind: Some(Scalar::from(private_values[2])),
        output_blind: Some(Scalar::from(private_values[3])),
    };

    let params = parameters_for(BALANCE_CIRCUIT)?;
    let proof = create_random_proof(circuit, params, &mut OsRng).map_err(|e| {
        BlockError::TransactionValidationFailed(format!("Groth16 proving failed: {}", e))
    })?;

    Ok((
        serialize_proof(&proof)?,
        encode_public_input_words(public_inputs),
    ))
}

pub fn prove_merkle(
    private_inputs: &[u8],
    public_inputs: &[u8],
) -> Result<(Vec<u8>, Vec<Vec<u8>>), BlockError> {
    let public_values = parse_u64_words(
        public_inputs,
        MERKLE_PUBLIC_INPUT_WORDS,
        "merkle_circuit public inputs",
    )?;

    if private_inputs.is_empty() {
        return Err(BlockError::TransactionValidationFailed(
            "merkle_circuit private inputs too short".to_string(),
        ));
    }

    let depth = private_inputs[0] as usize;
    if depth > MERKLE_DEPTH {
        return Err(BlockError::TransactionValidationFailed(format!(
            "merkle_circuit depth {} exceeds max {}",
            depth, MERKLE_DEPTH
        )));
    }

    let path_start = 1;
    let path_end = path_start + (depth * 8);
    if private_inputs.len() < path_end {
        return Err(BlockError::TransactionValidationFailed(
            "merkle_circuit private inputs incomplete".to_string(),
        ));
    }

    let mut path_values: Vec<u64> = private_inputs[path_start..path_end]
        .chunks_exact(8)
        .map(|chunk| {
            let arr: [u8; 8] = chunk.try_into().unwrap();
            u64::from_le_bytes(arr)
        })
        .collect();
    let mut computed = public_values[0];
    for sibling in &path_values {
        computed = computed.wrapping_add(*sibling);
    }
    if computed != public_values[1] {
        return Err(BlockError::TransactionValidationFailed(
            "merkle_circuit constraints not satisfied".to_string(),
        ));
    }

    let dir_start = path_end;
    let mut directions = Vec::with_capacity(depth);
    if dir_start < private_inputs.len() {
        let dir_bytes = &private_inputs[dir_start..];
        for i in 0..depth {
            let byte = dir_bytes.get(i / 8).copied().unwrap_or(0);
            directions.push(((byte >> (i % 8)) & 1u8) == 1);
        }
    } else {
        directions.resize(depth, false);
    }

    path_values.resize(MERKLE_DEPTH, 0);
    directions.resize(MERKLE_DEPTH, false);

    let circuit = GrothMerkleCircuit {
        leaf: Some(Scalar::from(public_values[0])),
        root: Some(Scalar::from(public_values[1])),
        path: path_values.into_iter().map(Scalar::from).collect(),
        directions,
    };

    let params = parameters_for(MERKLE_CIRCUIT)?;
    let proof = create_random_proof(circuit, params, &mut OsRng).map_err(|e| {
        BlockError::TransactionValidationFailed(format!("Groth16 proving failed: {}", e))
    })?;

    Ok((
        serialize_proof(&proof)?,
        encode_public_input_words(public_inputs),
    ))
}

pub fn prove_nullifier(
    private_inputs: &[u8],
    public_inputs: &[u8],
) -> Result<(Vec<u8>, Vec<Vec<u8>>), BlockError> {
    let private_values = parse_u64_words(
        private_inputs,
        NULLIFIER_PRIVATE_INPUT_WORDS,
        "nullifier_circuit private inputs",
    )?;
    let public_values = parse_u64_words(
        public_inputs,
        NULLIFIER_PUBLIC_INPUT_WORDS,
        "nullifier_circuit public inputs",
    )?;

    let expected = private_values[0]
        .wrapping_mul(private_values[1])
        .wrapping_add(private_values[2]);
    if expected != public_values[0] {
        return Err(BlockError::TransactionValidationFailed(
            "nullifier_circuit constraints not satisfied".to_string(),
        ));
    }

    let circuit = GrothNullifierCircuit {
        nullifier: Some(Scalar::from(public_values[0])),
        secret: Some(Scalar::from(private_values[0])),
        index: Some(Scalar::from(private_values[1])),
        blind: Some(Scalar::from(private_values[2])),
    };

    let params = parameters_for(NULLIFIER_CIRCUIT)?;
    let proof = create_random_proof(circuit, params, &mut OsRng).map_err(|e| {
        BlockError::TransactionValidationFailed(format!("Groth16 proving failed: {}", e))
    })?;

    Ok((
        serialize_proof(&proof)?,
        encode_public_input_words(public_inputs),
    ))
}

pub fn prove_private_tx(
    private_inputs: &[u8],
    public_inputs: &[u8],
) -> Result<(Vec<u8>, Vec<Vec<u8>>), BlockError> {
    prove_private_tx_for_circuit(PRIVATE_TX_CIRCUIT, private_inputs, public_inputs)
}

pub fn prove_private_tx_for_circuit(
    circuit: &str,
    private_inputs: &[u8],
    public_inputs: &[u8],
) -> Result<(Vec<u8>, Vec<Vec<u8>>), BlockError> {
    let profile = private_tx_profile_for_circuit(circuit)?;
    let params = parameters_for(circuit)?;
    prove_private_tx_for_profile(private_inputs, public_inputs, profile, params)
}

fn prove_private_tx_for_profile(
    private_inputs: &[u8],
    public_inputs: &[u8],
    profile: PrivateTxCircuitProfile,
    params: &Parameters<Bls12>,
) -> Result<(Vec<u8>, Vec<Vec<u8>>), BlockError> {
    let private_values = parse_u64_words(
        private_inputs,
        profile.private_input_words(),
        "private_tx private inputs",
    )?;
    let public_values = parse_u64_words(
        public_inputs,
        profile.public_input_words(),
        "private_tx public inputs",
    )?;

    let amount = private_values[0];
    let blind = private_values[1];
    let secret = private_values[2];
    let index = private_values[3];
    let input_sum = private_values[4];
    let output_sum = private_values[5];
    let fee = private_values[6];
    let key_image = private_values[7];
    let input_asset_tag = private_values[8];
    let output_asset_tag = private_values[9];
    let input_amounts = private_values[10..(10 + profile.max_inputs)].to_vec();
    let input_key_images =
        private_values[(10 + profile.max_inputs)..(10 + (profile.max_inputs * 2))].to_vec();
    let input_asset_tags =
        private_values[(10 + (profile.max_inputs * 2))..(10 + (profile.max_inputs * 3))].to_vec();
    let sibling_words_start = 10 + (profile.max_inputs * 3);
    let sibling_words_end = sibling_words_start
        + (profile.merkle_proven_inputs * PRIVATE_TX_MERKLE_MAX_DEPTH * PRIVATE_TX_ROOT_WORDS);
    let path_sibling_words = private_values[sibling_words_start..sibling_words_end].to_vec();
    let path_direction_values = private_values[sibling_words_end..].to_vec();
    let commitment = public_values[0];
    let nullifier = public_values[1];
    let public_input_key_images = public_values[2..(2 + profile.max_inputs)].to_vec();
    let public_membership_flags = public_values
        [(2 + profile.max_inputs)..(2 + profile.max_inputs + profile.merkle_proven_inputs)]
        .to_vec();
    let public_commitment_words_start = 2 + profile.max_inputs + profile.merkle_proven_inputs;
    let public_commitment_words_end =
        public_commitment_words_start + (profile.merkle_proven_inputs * PRIVATE_TREE_SCALAR_WORDS);
    let public_proven_commitment_words =
        public_values[public_commitment_words_start..public_commitment_words_end].to_vec();
    let public_root_words = public_values[public_commitment_words_end..].to_vec();

    let expected_commitment = amount.wrapping_add(blind);
    if expected_commitment != commitment {
        return Err(BlockError::TransactionValidationFailed(
            "private_tx commitment constraint not satisfied".to_string(),
        ));
    }

    let expected_nullifier = secret.wrapping_mul(index).wrapping_add(blind);
    if expected_nullifier != nullifier {
        return Err(BlockError::TransactionValidationFailed(
            "private_tx nullifier constraint not satisfied".to_string(),
        ));
    }

    if input_key_images != public_input_key_images {
        return Err(BlockError::TransactionValidationFailed(
            "private_tx input key image anchors do not match".to_string(),
        ));
    }

    if input_key_images.first().copied().unwrap_or(0) != key_image {
        return Err(BlockError::TransactionValidationFailed(
            "private_tx primary key image anchor not satisfied".to_string(),
        ));
    }
    if input_asset_tag == 0 || output_asset_tag == 0 {
        return Err(BlockError::TransactionValidationFailed(
            "private_tx asset tags must be non-zero".to_string(),
        ));
    }
    if input_asset_tag != output_asset_tag {
        return Err(BlockError::TransactionValidationFailed(
            "private_tx input/output asset tags do not match".to_string(),
        ));
    }
    for idx in 0..profile.max_inputs {
        let amount = input_amounts[idx];
        let asset_tag = input_asset_tags[idx];
        if amount > 0 && asset_tag != input_asset_tag {
            return Err(BlockError::TransactionValidationFailed(format!(
                "private_tx input asset tag mismatch at index {}",
                idx
            )));
        }
        if amount == 0 && asset_tag != 0 {
            return Err(BlockError::TransactionValidationFailed(format!(
                "private_tx inactive input asset tag at index {} must be zero padded",
                idx
            )));
        }
    }

    let witness_input_sum = input_amounts.iter().try_fold(0u64, |acc, amount| {
        acc.checked_add(*amount).ok_or_else(|| {
            BlockError::TransactionValidationFailed(
                "private_tx witness input sum overflow".to_string(),
            )
        })
    })?;
    if witness_input_sum != input_sum {
        return Err(BlockError::TransactionValidationFailed(
            "private_tx witness inputs do not sum to public input sum".to_string(),
        ));
    }

    if output_sum.checked_add(fee).ok_or_else(|| {
        BlockError::TransactionValidationFailed(
            "private_tx balance arithmetic overflow".to_string(),
        )
    })? != input_sum
    {
        return Err(BlockError::TransactionValidationFailed(
            "private_tx value conservation constraint not satisfied".to_string(),
        ));
    }

    if path_direction_values.len() != (profile.merkle_proven_inputs * PRIVATE_TX_MERKLE_MAX_DEPTH) {
        return Err(BlockError::TransactionValidationFailed(
            "private_tx proven path directions are incomplete".to_string(),
        ));
    }
    if path_direction_values.iter().any(|value| *value > 1) {
        return Err(BlockError::TransactionValidationFailed(
            "private_tx proven path directions must be boolean".to_string(),
        ));
    }
    if public_membership_flags.len() != profile.merkle_proven_inputs
        || public_membership_flags.iter().any(|value| *value > 1)
    {
        return Err(BlockError::TransactionValidationFailed(
            "private_tx proven input flags are malformed".to_string(),
        ));
    }
    let public_root_scalar = scalar_from_public_words(&public_root_words, "private_tx root")?;

    let mut proven_membership_flags = Vec::with_capacity(profile.merkle_proven_inputs);
    let mut proven_commitment_words = Vec::with_capacity(profile.merkle_proven_inputs);
    let mut path_siblings = Vec::with_capacity(profile.merkle_proven_inputs);
    let mut path_directions = Vec::with_capacity(profile.merkle_proven_inputs);
    for proven_idx in 0..profile.merkle_proven_inputs {
        let is_active = public_membership_flags[proven_idx] == 1;
        proven_membership_flags.push(is_active);

        let commitment_words_start = proven_idx * PRIVATE_TREE_SCALAR_WORDS;
        let commitment_words_end = commitment_words_start + PRIVATE_TREE_SCALAR_WORDS;
        let commitment_words: [u64; PRIVATE_TREE_SCALAR_WORDS] = public_proven_commitment_words
            [commitment_words_start..commitment_words_end]
            .to_vec()
            .try_into()
            .map_err(|_| {
                BlockError::TransactionValidationFailed(
                    "private_tx proven commitment public anchor is malformed".to_string(),
                )
            })?;

        let input_is_nonzero = input_amounts[proven_idx] > 0;
        if is_active != input_is_nonzero {
            return Err(BlockError::TransactionValidationFailed(format!(
                "private_tx proven input flag {} does not match padded input activity",
                proven_idx
            )));
        }
        if !is_active {
            if input_key_images[proven_idx] != 0 || commitment_words.iter().any(|word| *word != 0) {
                return Err(BlockError::TransactionValidationFailed(format!(
                    "private_tx inactive proven input {} must be zero padded",
                    proven_idx
                )));
            }
        }
        proven_commitment_words.push(commitment_words);

        let path_words_start = proven_idx * PRIVATE_TX_MERKLE_MAX_DEPTH * PRIVATE_TX_ROOT_WORDS;
        let path_words_end =
            path_words_start + (PRIVATE_TX_MERKLE_MAX_DEPTH * PRIVATE_TX_ROOT_WORDS);
        let input_path_sibling_words = path_sibling_words[path_words_start..path_words_end]
            .chunks_exact(PRIVATE_TX_ROOT_WORDS)
            .map(|chunk| {
                scalar_from_public_words(
                    chunk,
                    &format!(
                        "private_tx path sibling {}:{}",
                        proven_idx,
                        path_sibling_words.len()
                    ),
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let path_dirs_start = proven_idx * PRIVATE_TX_MERKLE_MAX_DEPTH;
        let path_dirs_end = path_dirs_start + PRIVATE_TX_MERKLE_MAX_DEPTH;
        let input_path_directions = path_direction_values[path_dirs_start..path_dirs_end]
            .iter()
            .map(|value| *value == 1)
            .collect::<Vec<_>>();

        if !is_active {
            if input_path_sibling_words
                .iter()
                .any(|sibling| *sibling != Scalar::from(0u64))
                || input_path_directions.iter().any(|direction| *direction)
            {
                return Err(BlockError::TransactionValidationFailed(format!(
                    "private_tx inactive proven path {} must be zero padded",
                    proven_idx
                )));
            }
        } else {
            let mut current = private_tree_hash_commitment_words(&commitment_words);
            for (depth, sibling) in input_path_sibling_words.iter().enumerate() {
                current = if input_path_directions[depth] {
                    private_tree_hash_pair(*sibling, current)
                } else {
                    private_tree_hash_pair(current, *sibling)
                };
            }
            if current != public_root_scalar {
                return Err(BlockError::TransactionValidationFailed(format!(
                    "private_tx proven input {} membership does not match private root",
                    proven_idx
                )));
            }
        }

        path_siblings.push(input_path_sibling_words);
        path_directions.push(input_path_directions);
    }

    let mut amount_bits = Vec::with_capacity(PRIVATE_TX_RANGE_BITS);
    let mut input_amount_bits = Vec::with_capacity(profile.max_inputs);
    let mut input_sum_bits = Vec::with_capacity(PRIVATE_TX_RANGE_BITS);
    let mut output_sum_bits = Vec::with_capacity(PRIVATE_TX_RANGE_BITS);
    let mut fee_bits = Vec::with_capacity(PRIVATE_TX_RANGE_BITS);
    for i in 0..PRIVATE_TX_RANGE_BITS {
        amount_bits.push(((amount >> i) & 1u64) == 1);
        input_sum_bits.push(((input_sum >> i) & 1u64) == 1);
        output_sum_bits.push(((output_sum >> i) & 1u64) == 1);
        fee_bits.push(((fee >> i) & 1u64) == 1);
    }
    for input_amount in &input_amounts {
        let mut bits = Vec::with_capacity(PRIVATE_TX_RANGE_BITS);
        for i in 0..PRIVATE_TX_RANGE_BITS {
            bits.push(((input_amount >> i) & 1u64) == 1);
        }
        input_amount_bits.push(bits);
    }

    let circuit = GrothPrivateTxCircuit {
        commitment: Some(Scalar::from(commitment)),
        nullifier: Some(Scalar::from(nullifier)),
        input_sum: Some(Scalar::from(input_sum)),
        output_sum: Some(Scalar::from(output_sum)),
        fee: Some(Scalar::from(fee)),
        key_image: Some(Scalar::from(key_image)),
        amount: Some(Scalar::from(amount)),
        blind: Some(Scalar::from(blind)),
        secret: Some(Scalar::from(secret)),
        index: Some(Scalar::from(index)),
        input_asset_tag: Some(Scalar::from(input_asset_tag)),
        output_asset_tag: Some(Scalar::from(output_asset_tag)),
        input_amounts: input_amounts.into_iter().map(Scalar::from).collect(),
        input_key_images: input_key_images.into_iter().map(Scalar::from).collect(),
        input_asset_tags: input_asset_tags.into_iter().map(Scalar::from).collect(),
        proven_input_flags: proven_membership_flags,
        proven_input_commitment_words: proven_commitment_words
            .iter()
            .map(|words| words.iter().copied().map(Scalar::from).collect())
            .collect(),
        path_sibling_words: path_siblings
            .iter()
            .map(|siblings| {
                let mut flat_words =
                    Vec::with_capacity(PRIVATE_TX_MERKLE_MAX_DEPTH * PRIVATE_TX_ROOT_WORDS);
                for sibling in siblings {
                    let words = sibling.to_bytes();
                    for word_idx in 0..PRIVATE_TX_ROOT_WORDS {
                        let start = word_idx * 8;
                        let mut chunk = [0u8; 8];
                        chunk.copy_from_slice(&words[start..start + 8]);
                        flat_words.push(Scalar::from(u64::from_le_bytes(chunk)));
                    }
                }
                flat_words
            })
            .collect(),
        path_directions,
        private_root_words: public_root_words.into_iter().map(Scalar::from).collect(),
        amount_bits,
        input_amount_bits,
        input_sum_bits,
        output_sum_bits,
        fee_bits,
    };

    let proof = create_random_proof(circuit, params, &mut OsRng).map_err(|e| {
        BlockError::TransactionValidationFailed(format!(
            "Groth16 proving for {} failed: {}",
            profile.circuit_name, e
        ))
    })?;

    Ok((
        serialize_proof(&proof)?,
        encode_public_input_words(public_inputs),
    ))
}

/// Offline-only proving route for the compact `i1` candidate. Callers must
/// provide an explicit lab setup directory; this profile is intentionally not
/// registered with node verification or the canonical setup bundle yet.
pub fn prove_private_tx_v2_i1_candidate(
    trusted_setup_dir: &std::path::Path,
    private_inputs: &[u8],
    public_inputs: &[u8],
) -> Result<(Vec<u8>, Vec<Vec<u8>>), BlockError> {
    let setup = private_tx_v2_i1_lab_setup(trusted_setup_dir).map_err(|e| {
        BlockError::TransactionValidationFailed(format!(
            "{PRIVATE_TX_V2_I1_LAB_CIRCUIT} setup is unavailable: {e}"
        ))
    })?;
    prove_private_tx_for_profile(
        private_inputs,
        public_inputs,
        PRIVATE_TX_V2_I1_LAB_PROFILE,
        &setup.params,
    )
}

/// Offline-only verification route paired with
/// `prove_private_tx_v2_i1_candidate`.
pub fn verify_private_tx_v2_i1_candidate(
    trusted_setup_dir: &std::path::Path,
    proof_bytes: &[u8],
    public_inputs: &[Vec<u8>],
) -> Result<bool, String> {
    let setup = private_tx_v2_i1_lab_setup(trusted_setup_dir)?;
    verify_private_tx_for_profile(
        proof_bytes,
        public_inputs,
        PRIVATE_TX_V2_I1_LAB_PROFILE,
        &setup.pvk,
    )
}

fn verify_private_tx_for_profile(
    proof_bytes: &[u8],
    public_inputs: &[Vec<u8>],
    profile: PrivateTxCircuitProfile,
    pvk: &PreparedVerifyingKey<Bls12>,
) -> Result<bool, String> {
    let public_words = decode_public_input_words(public_inputs, profile.public_input_words())?;
    let public_scalars: Vec<Scalar> = public_words.into_iter().map(Scalar::from).collect();

    let mut cursor = Cursor::new(proof_bytes);
    let proof = Proof::<Bls12>::read(&mut cursor)
        .map_err(|e| format!("failed to deserialize Groth16 proof: {}", e))?;

    match verify_proof(pvk, &proof, &public_scalars) {
        Ok(()) => Ok(true),
        Err(_) => Ok(false),
    }
}

pub fn verify(
    circuit: &str,
    proof_bytes: &[u8],
    public_inputs: &[Vec<u8>],
) -> Result<bool, String> {
    if !is_groth16_circuit(circuit) {
        return Err(format!("unsupported Groth16 circuit: {}", circuit));
    }

    let expected = expected_public_input_count(circuit).unwrap_or(0);
    let public_words = decode_public_input_words(public_inputs, expected)?;
    let public_scalars: Vec<Scalar> = public_words.into_iter().map(Scalar::from).collect();

    let mut cursor = Cursor::new(proof_bytes);
    let proof = Proof::<Bls12>::read(&mut cursor)
        .map_err(|e| format!("failed to deserialize Groth16 proof: {}", e))?;

    let pvk = pvk_for(circuit)?;
    match verify_proof(pvk, &proof, &public_scalars) {
        Ok(()) => Ok(true),
        Err(_) => Ok(false),
    }
}

#[derive(Clone)]
struct GrothBalanceCircuit {
    input_commitment: Option<Scalar>,
    output_commitment: Option<Scalar>,
    input_value: Option<Scalar>,
    output_value: Option<Scalar>,
    input_blind: Option<Scalar>,
    output_blind: Option<Scalar>,
}

impl GrothBalanceCircuit {
    fn blank() -> Self {
        Self {
            input_commitment: Some(Scalar::from(0u64)),
            output_commitment: Some(Scalar::from(0u64)),
            input_value: Some(Scalar::from(0u64)),
            output_value: Some(Scalar::from(0u64)),
            input_blind: Some(Scalar::from(0u64)),
            output_blind: Some(Scalar::from(0u64)),
        }
    }
}

impl Circuit<Scalar> for GrothBalanceCircuit {
    fn synthesize<CS: ConstraintSystem<Scalar>>(self, cs: &mut CS) -> Result<(), SynthesisError> {
        let input_value = {
            let val = self.input_value.ok_or(SynthesisError::AssignmentMissing)?;
            cs.alloc(|| "input_value", || Ok(val))?
        };
        let output_value = {
            let val = self.output_value.ok_or(SynthesisError::AssignmentMissing)?;
            cs.alloc(|| "output_value", || Ok(val))?
        };
        let input_blind = {
            let val = self.input_blind.ok_or(SynthesisError::AssignmentMissing)?;
            cs.alloc(|| "input_blind", || Ok(val))?
        };
        let output_blind = {
            let val = self.output_blind.ok_or(SynthesisError::AssignmentMissing)?;
            cs.alloc(|| "output_blind", || Ok(val))?
        };
        let input_com = {
            let val = self
                .input_commitment
                .ok_or(SynthesisError::AssignmentMissing)?;
            cs.alloc_input(|| "input_commitment", || Ok(val))?
        };
        let output_com = {
            let val = self
                .output_commitment
                .ok_or(SynthesisError::AssignmentMissing)?;
            cs.alloc_input(|| "output_commitment", || Ok(val))?
        };

        cs.enforce(
            || "input_commitment_consistency",
            |lc| lc + input_value + input_blind,
            |lc| lc + CS::one(),
            |lc| lc + input_com,
        );
        cs.enforce(
            || "output_commitment_consistency",
            |lc| lc + output_value + output_blind,
            |lc| lc + CS::one(),
            |lc| lc + output_com,
        );
        cs.enforce(
            || "balance_preservation",
            |lc| lc + input_value,
            |lc| lc + CS::one(),
            |lc| lc + output_value,
        );
        Ok(())
    }
}

#[derive(Clone)]
struct GrothMerkleCircuit {
    leaf: Option<Scalar>,
    root: Option<Scalar>,
    path: Vec<Scalar>,
    directions: Vec<bool>,
}

impl GrothMerkleCircuit {
    fn blank() -> Self {
        Self {
            leaf: Some(Scalar::from(0u64)),
            root: Some(Scalar::from(0u64)),
            path: vec![Scalar::from(0u64); MERKLE_DEPTH],
            directions: vec![false; MERKLE_DEPTH],
        }
    }
}

impl Circuit<Scalar> for GrothMerkleCircuit {
    fn synthesize<CS: ConstraintSystem<Scalar>>(self, cs: &mut CS) -> Result<(), SynthesisError> {
        let leaf_val = self.leaf.ok_or(SynthesisError::AssignmentMissing)?;
        let root_val = self.root.ok_or(SynthesisError::AssignmentMissing)?;

        let leaf = cs.alloc_input(|| "leaf", || Ok(leaf_val))?;
        let root = cs.alloc_input(|| "root", || Ok(root_val))?;

        let mut current_val = leaf_val;
        let mut current = cs.alloc(|| "current_0", || Ok(current_val))?;
        cs.enforce(
            || "current_starts_at_leaf",
            |lc| lc + current,
            |lc| lc + CS::one(),
            |lc| lc + leaf,
        );

        for i in 0..MERKLE_DEPTH {
            let sibling_val = *self.path.get(i).unwrap_or(&Scalar::from(0u64));
            let sibling = cs.alloc(|| format!("path_{}", i), || Ok(sibling_val))?;

            let dir_bit = self.directions.get(i).copied().unwrap_or(false);
            let dir_val = if dir_bit {
                Scalar::from(1u64)
            } else {
                Scalar::from(0u64)
            };
            let dir = cs.alloc(|| format!("dir_{}", i), || Ok(dir_val))?;
            cs.enforce(
                || format!("dir_boolean_{}", i),
                |lc| lc + dir,
                |lc| lc + CS::one() - dir,
                |lc| lc,
            );

            let next_val = current_val + sibling_val;
            let next = cs.alloc(|| format!("current_{}", i + 1), || Ok(next_val))?;
            cs.enforce(
                || format!("merkle_step_{}", i),
                |lc| lc + current + sibling,
                |lc| lc + CS::one(),
                |lc| lc + next,
            );

            current = next;
            current_val = next_val;
        }

        cs.enforce(
            || "merkle_root_match",
            |lc| lc + current,
            |lc| lc + CS::one(),
            |lc| lc + root,
        );

        Ok(())
    }
}

#[derive(Clone)]
struct GrothNullifierCircuit {
    nullifier: Option<Scalar>,
    secret: Option<Scalar>,
    index: Option<Scalar>,
    blind: Option<Scalar>,
}

impl GrothNullifierCircuit {
    fn blank() -> Self {
        Self {
            nullifier: Some(Scalar::from(0u64)),
            secret: Some(Scalar::from(0u64)),
            index: Some(Scalar::from(0u64)),
            blind: Some(Scalar::from(0u64)),
        }
    }
}

impl Circuit<Scalar> for GrothNullifierCircuit {
    fn synthesize<CS: ConstraintSystem<Scalar>>(self, cs: &mut CS) -> Result<(), SynthesisError> {
        let secret_val = self.secret.ok_or(SynthesisError::AssignmentMissing)?;
        let index_val = self.index.ok_or(SynthesisError::AssignmentMissing)?;
        let blind_val = self.blind.ok_or(SynthesisError::AssignmentMissing)?;
        let nullifier_val = self.nullifier.ok_or(SynthesisError::AssignmentMissing)?;

        let secret = cs.alloc(|| "secret", || Ok(secret_val))?;
        let index = cs.alloc(|| "index", || Ok(index_val))?;
        let blind = cs.alloc(|| "blind", || Ok(blind_val))?;
        let nullifier = cs.alloc_input(|| "nullifier", || Ok(nullifier_val))?;

        let nullifier_pre = cs.alloc(|| "nullifier_pre", || Ok(secret_val * index_val))?;
        cs.enforce(
            || "secret_index_multiply",
            |lc| lc + secret,
            |lc| lc + index,
            |lc| lc + nullifier_pre,
        );
        cs.enforce(
            || "nullifier_with_blind",
            |lc| lc + nullifier_pre + blind,
            |lc| lc + CS::one(),
            |lc| lc + nullifier,
        );

        Ok(())
    }
}

#[derive(Clone)]
struct GrothPrivateTxCircuit {
    commitment: Option<Scalar>,
    nullifier: Option<Scalar>,
    input_sum: Option<Scalar>,
    output_sum: Option<Scalar>,
    fee: Option<Scalar>,
    key_image: Option<Scalar>,
    amount: Option<Scalar>,
    blind: Option<Scalar>,
    secret: Option<Scalar>,
    index: Option<Scalar>,
    input_asset_tag: Option<Scalar>,
    output_asset_tag: Option<Scalar>,
    input_amounts: Vec<Scalar>,
    input_key_images: Vec<Scalar>,
    input_asset_tags: Vec<Scalar>,
    proven_input_flags: Vec<bool>,
    proven_input_commitment_words: Vec<Vec<Scalar>>,
    path_sibling_words: Vec<Vec<Scalar>>,
    path_directions: Vec<Vec<bool>>,
    private_root_words: Vec<Scalar>,
    amount_bits: Vec<bool>,
    input_amount_bits: Vec<Vec<bool>>,
    input_sum_bits: Vec<bool>,
    output_sum_bits: Vec<bool>,
    fee_bits: Vec<bool>,
}

impl GrothPrivateTxCircuit {
    fn blank() -> Self {
        Self::blank_for_profile(PRIVATE_TX_MAX_INPUTS, PRIVATE_TX_MERKLE_PROVEN_INPUTS)
    }

    fn blank_for_profile(max_inputs: usize, merkle_proven_inputs: usize) -> Self {
        assert!(max_inputs > 0);
        assert!(merkle_proven_inputs > 0);
        assert!(merkle_proven_inputs <= max_inputs);
        Self {
            commitment: Some(Scalar::from(0u64)),
            nullifier: Some(Scalar::from(0u64)),
            input_sum: Some(Scalar::from(0u64)),
            output_sum: Some(Scalar::from(0u64)),
            fee: Some(Scalar::from(0u64)),
            key_image: Some(Scalar::from(0u64)),
            amount: Some(Scalar::from(0u64)),
            blind: Some(Scalar::from(0u64)),
            secret: Some(Scalar::from(0u64)),
            index: Some(Scalar::from(0u64)),
            input_asset_tag: Some(Scalar::from(0u64)),
            output_asset_tag: Some(Scalar::from(0u64)),
            input_amounts: vec![Scalar::from(0u64); max_inputs],
            input_key_images: vec![Scalar::from(0u64); max_inputs],
            input_asset_tags: vec![Scalar::from(0u64); max_inputs],
            proven_input_flags: vec![false; merkle_proven_inputs],
            proven_input_commitment_words: vec![
                vec![Scalar::from(0u64); PRIVATE_TREE_SCALAR_WORDS];
                merkle_proven_inputs
            ],
            path_sibling_words: vec![
                vec![
                    Scalar::from(0u64);
                    PRIVATE_TX_MERKLE_MAX_DEPTH * PRIVATE_TX_ROOT_WORDS
                ];
                merkle_proven_inputs
            ],
            path_directions: vec![vec![false; PRIVATE_TX_MERKLE_MAX_DEPTH]; merkle_proven_inputs],
            private_root_words: vec![Scalar::from(0u64); PRIVATE_TX_ROOT_WORDS],
            amount_bits: vec![false; PRIVATE_TX_RANGE_BITS],
            input_amount_bits: vec![vec![false; PRIVATE_TX_RANGE_BITS]; max_inputs],
            input_sum_bits: vec![false; PRIVATE_TX_RANGE_BITS],
            output_sum_bits: vec![false; PRIVATE_TX_RANGE_BITS],
            fee_bits: vec![false; PRIVATE_TX_RANGE_BITS],
        }
    }
}

impl Circuit<Scalar> for GrothPrivateTxCircuit {
    fn synthesize<CS: ConstraintSystem<Scalar>>(self, cs: &mut CS) -> Result<(), SynthesisError> {
        let amount_val = self.amount.ok_or(SynthesisError::AssignmentMissing)?;
        let blind_val = self.blind.ok_or(SynthesisError::AssignmentMissing)?;
        let secret_val = self.secret.ok_or(SynthesisError::AssignmentMissing)?;
        let index_val = self.index.ok_or(SynthesisError::AssignmentMissing)?;
        let input_asset_tag_val = self
            .input_asset_tag
            .ok_or(SynthesisError::AssignmentMissing)?;
        let output_asset_tag_val = self
            .output_asset_tag
            .ok_or(SynthesisError::AssignmentMissing)?;
        let commitment_val = self.commitment.ok_or(SynthesisError::AssignmentMissing)?;
        let nullifier_val = self.nullifier.ok_or(SynthesisError::AssignmentMissing)?;
        let input_sum_val = self.input_sum.ok_or(SynthesisError::AssignmentMissing)?;
        let output_sum_val = self.output_sum.ok_or(SynthesisError::AssignmentMissing)?;
        let fee_val = self.fee.ok_or(SynthesisError::AssignmentMissing)?;
        let key_image_val = self.key_image.ok_or(SynthesisError::AssignmentMissing)?;

        let amount = cs.alloc(|| "amount", || Ok(amount_val))?;
        let blind = cs.alloc(|| "blind", || Ok(blind_val))?;
        let secret = cs.alloc(|| "secret", || Ok(secret_val))?;
        let index = cs.alloc(|| "index", || Ok(index_val))?;
        let input_asset_tag = cs.alloc(|| "input_asset_tag", || Ok(input_asset_tag_val))?;
        let output_asset_tag = cs.alloc(|| "output_asset_tag", || Ok(output_asset_tag_val))?;
        let input_sum = cs.alloc(|| "input_sum", || Ok(input_sum_val))?;
        let output_sum = cs.alloc(|| "output_sum", || Ok(output_sum_val))?;
        let fee = cs.alloc(|| "fee", || Ok(fee_val))?;

        let commitment = cs.alloc_input(|| "commitment", || Ok(commitment_val))?;
        let nullifier = cs.alloc_input(|| "nullifier", || Ok(nullifier_val))?;
        let key_image = cs.alloc(|| "key_image_private", || Ok(key_image_val))?;

        let input_amount_vars = self
            .input_amounts
            .iter()
            .enumerate()
            .map(|(idx, value)| cs.alloc(|| format!("input_amount_{}", idx), || Ok(*value)))
            .collect::<Result<Vec<_>, _>>()?;
        let input_key_image_vars = self
            .input_key_images
            .iter()
            .enumerate()
            .map(|(idx, value)| cs.alloc(|| format!("input_key_image_{}", idx), || Ok(*value)))
            .collect::<Result<Vec<_>, _>>()?;
        let input_asset_tag_vars = self
            .input_asset_tags
            .iter()
            .enumerate()
            .map(|(idx, value)| cs.alloc(|| format!("input_asset_tag_{}", idx), || Ok(*value)))
            .collect::<Result<Vec<_>, _>>()?;
        let input_key_image_public_vars = self
            .input_key_images
            .iter()
            .enumerate()
            .map(|(idx, value)| {
                cs.alloc_input(|| format!("input_key_image_public_{}", idx), || Ok(*value))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let proven_input_flag_public_vars = self
            .proven_input_flags
            .iter()
            .enumerate()
            .map(|(idx, flag)| {
                cs.alloc_input(
                    || format!("proven_input_flag_public_{}", idx),
                    || {
                        Ok(if *flag {
                            Scalar::from(1u64)
                        } else {
                            Scalar::from(0u64)
                        })
                    },
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let proven_input_commitment_public_vars = self
            .proven_input_commitment_words
            .iter()
            .enumerate()
            .map(|(proven_idx, words)| {
                words
                    .iter()
                    .enumerate()
                    .map(|(word_idx, value)| {
                        cs.alloc_input(
                            || {
                                format!(
                                    "proven_input_commitment_public_{}_word_{}",
                                    proven_idx, word_idx
                                )
                            },
                            || Ok(*value),
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .collect::<Result<Vec<_>, _>>()?;
        let private_root_public_vars = self
            .private_root_words
            .iter()
            .enumerate()
            .map(|(idx, value)| {
                cs.alloc_input(
                    || format!("private_root_public_word_{}", idx),
                    || Ok(*value),
                )
            })
            .collect::<Result<Vec<_>, _>>()?;

        cs.enforce(
            || "commitment_consistency",
            |lc| lc + amount + blind,
            |lc| lc + CS::one(),
            |lc| lc + commitment,
        );

        let nullifier_pre = cs.alloc(|| "nullifier_pre", || Ok(secret_val * index_val))?;
        cs.enforce(
            || "secret_index_multiply",
            |lc| lc + secret,
            |lc| lc + index,
            |lc| lc + nullifier_pre,
        );
        cs.enforce(
            || "nullifier_consistency",
            |lc| lc + nullifier_pre + blind,
            |lc| lc + CS::one(),
            |lc| lc + nullifier,
        );

        cs.enforce(
            || "value_conservation",
            |lc| lc + output_sum + fee,
            |lc| lc + CS::one(),
            |lc| lc + input_sum,
        );
        cs.enforce(
            || "asset_tag_consistency",
            |lc| lc + input_asset_tag,
            |lc| lc + CS::one(),
            |lc| lc + output_asset_tag,
        );
        cs.enforce(
            || "primary_key_image_matches_first_input",
            |lc| lc + key_image,
            |lc| lc + CS::one(),
            |lc| lc + input_key_image_vars[0],
        );
        cs.enforce(
            || "input_amount_sum_matches_input_sum",
            |lc| {
                let mut acc = lc;
                for input_amount in &input_amount_vars {
                    acc = acc + *input_amount;
                }
                acc
            },
            |lc| lc + CS::one(),
            |lc| lc + input_sum,
        );
        for idx in 0..self.input_amounts.len() {
            cs.enforce(
                || format!("input_key_image_public_anchor_{}", idx),
                |lc| lc + input_key_image_vars[idx],
                |lc| lc + CS::one(),
                |lc| lc + input_key_image_public_vars[idx],
            );
            let asset_delta = cs.alloc(
                || format!("input_asset_tag_delta_{}", idx),
                || Ok(self.input_asset_tags[idx] - input_asset_tag_val),
            )?;
            cs.enforce(
                || format!("input_asset_tag_delta_anchor_{}", idx),
                |lc| lc + CS::one(),
                |lc| lc + asset_delta,
                |lc| lc + input_asset_tag_vars[idx] - input_asset_tag,
            );
            cs.enforce(
                || format!("input_asset_tag_matches_anchor_{}", idx),
                |lc| lc + asset_delta,
                |lc| lc + input_amount_vars[idx],
                |lc| lc,
            );
        }

        fn allocate_range_bits<CS: ConstraintSystem<Scalar>>(
            cs: &mut CS,
            label: &str,
            bits: &[bool],
        ) -> Result<Vec<bellman::Variable>, SynthesisError> {
            let mut bit_vars = Vec::with_capacity(PRIVATE_TX_RANGE_BITS);
            for i in 0..PRIVATE_TX_RANGE_BITS {
                let bit_val = if bits.get(i).copied().unwrap_or(false) {
                    Scalar::from(1u64)
                } else {
                    Scalar::from(0u64)
                };
                let bit = cs.alloc(|| format!("{}_bit_{}", label, i), || Ok(bit_val))?;
                cs.enforce(
                    || format!("{}_bit_boolean_{}", label, i),
                    |lc| lc + bit,
                    |lc| lc + CS::one() - bit,
                    |lc| lc,
                );
                bit_vars.push(bit);
            }
            Ok(bit_vars)
        }

        fn enforce_recomposition<CS: ConstraintSystem<Scalar>>(
            cs: &mut CS,
            label: &str,
            value_var: bellman::Variable,
            bit_vars: &[bellman::Variable],
        ) {
            cs.enforce(
                || format!("{}_range_recompose", label),
                |lc| lc + CS::one(),
                |lc| lc + value_var,
                |lc| {
                    let mut acc = lc;
                    for (i, bit) in bit_vars.iter().enumerate() {
                        let coeff = Scalar::from(1u64 << i);
                        acc = acc + (coeff, *bit);
                    }
                    acc
                },
            );
        }

        let amount_bits = allocate_range_bits(cs, "amount", &self.amount_bits)?;
        let input_sum_bits = allocate_range_bits(cs, "input_sum", &self.input_sum_bits)?;
        let output_sum_bits = allocate_range_bits(cs, "output_sum", &self.output_sum_bits)?;
        let fee_bits = allocate_range_bits(cs, "fee", &self.fee_bits)?;
        let input_amount_bits = self
            .input_amount_bits
            .iter()
            .enumerate()
            .map(|(idx, bits)| allocate_range_bits(cs, &format!("input_amount_{}", idx), bits))
            .collect::<Result<Vec<_>, _>>()?;

        enforce_recomposition(cs, "amount", amount, &amount_bits);
        for (idx, bits) in input_amount_bits.iter().enumerate() {
            enforce_recomposition(
                cs,
                &format!("input_amount_{}", idx),
                input_amount_vars[idx],
                bits,
            );
        }
        enforce_recomposition(cs, "input_sum", input_sum, &input_sum_bits);
        enforce_recomposition(cs, "output_sum", output_sum, &output_sum_bits);
        enforce_recomposition(cs, "fee", fee, &fee_bits);

        fn scalar_from_word_vars<CS: ConstraintSystem<Scalar>>(
            cs: &mut CS,
            label: &str,
            word_vars: &[bellman::Variable],
            scalar_value: Scalar,
        ) -> Result<bellman::Variable, SynthesisError> {
            fn limb_coefficient(idx: usize) -> Scalar {
                let mut coeff = Scalar::from(1u64);
                for _ in 0..(idx * 64) {
                    coeff = coeff + coeff;
                }
                coeff
            }

            let scalar_var = cs.alloc(|| format!("{}_scalar", label), || Ok(scalar_value))?;
            cs.enforce(
                || format!("{}_scalar_recompose", label),
                |lc| lc + CS::one(),
                |lc| lc + scalar_var,
                |lc| {
                    let mut acc = lc;
                    for (idx, word_var) in word_vars.iter().enumerate() {
                        acc = acc + (limb_coefficient(idx), *word_var);
                    }
                    acc
                },
            );
            Ok(scalar_var)
        }

        fn enforce_zero_when_inactive<CS: ConstraintSystem<Scalar>>(
            cs: &mut CS,
            label: &str,
            value_var: bellman::Variable,
            inactive_var: bellman::Variable,
        ) {
            cs.enforce(
                || format!("{}_zero_when_inactive", label),
                |lc| lc + value_var,
                |lc| lc + inactive_var,
                |lc| lc,
            );
        }

        fn hash_pair_gadget<CS: ConstraintSystem<Scalar>>(
            cs: &mut CS,
            label: &str,
            left_var: bellman::Variable,
            left_value: Scalar,
            right_var: bellman::Variable,
            right_value: Scalar,
        ) -> Result<(bellman::Variable, Scalar), SynthesisError> {
            let mut state_value = left_value + Scalar::from(0xA5A5_A5A5u64);
            let key_value = right_value + Scalar::from(0x5A5A_5A5Au64);
            let mut state = cs.alloc(|| format!("{}_state_init", label), || Ok(state_value))?;
            cs.enforce(
                || format!("{}_state_init_anchor", label),
                |lc| lc + CS::one(),
                |lc| lc + state,
                |lc| lc + left_var + (Scalar::from(0xA5A5_A5A5u64), CS::one()),
            );

            for (round, constant) in private_tree_round_constants().iter().enumerate() {
                let t_value = state_value + key_value + constant;
                let t = cs.alloc(|| format!("{}_t_{}", label, round), || Ok(t_value))?;
                cs.enforce(
                    || format!("{}_t_anchor_{}", label, round),
                    |lc| lc + CS::one(),
                    |lc| lc + t,
                    |lc| {
                        lc + state
                            + right_var
                            + (*constant + Scalar::from(0x5A5A_5A5Au64), CS::one())
                    },
                );

                let square_value = t_value * t_value;
                let square = cs.alloc(
                    || format!("{}_square_{}", label, round),
                    || Ok(square_value),
                )?;
                cs.enforce(
                    || format!("{}_square_constraint_{}", label, round),
                    |lc| lc + t,
                    |lc| lc + t,
                    |lc| lc + square,
                );

                let next_value = square_value + key_value;
                let next = cs.alloc(
                    || format!("{}_state_{}", label, round + 1),
                    || Ok(next_value),
                )?;
                cs.enforce(
                    || format!("{}_next_state_{}", label, round),
                    |lc| lc + CS::one(),
                    |lc| lc + next,
                    |lc| lc + square + right_var + (Scalar::from(0x5A5A_5A5Au64), CS::one()),
                );

                state = next;
                state_value = next_value;
            }

            let output_value = state_value + left_value;
            let output = cs.alloc(|| format!("{}_output", label), || Ok(output_value))?;
            cs.enforce(
                || format!("{}_output_anchor", label),
                |lc| lc + CS::one(),
                |lc| lc + output,
                |lc| lc + state + left_var,
            );
            Ok((output, output_value))
        }

        let reconstructed_root_value = private_tree_scalar_from_words(
            &self
                .private_root_words
                .iter()
                .map(|value| {
                    let bytes = value.to_bytes();
                    let mut chunk = [0u8; 8];
                    chunk.copy_from_slice(&bytes[..8]);
                    u64::from_le_bytes(chunk)
                })
                .collect::<Vec<_>>(),
        )
        .ok_or(SynthesisError::AssignmentMissing)?;
        let reconstructed_root = scalar_from_word_vars(
            cs,
            "private_root",
            &private_root_public_vars,
            reconstructed_root_value,
        )?;

        for proven_idx in 0..self.proven_input_flags.len() {
            let flag_value = if self.proven_input_flags[proven_idx] {
                Scalar::from(1u64)
            } else {
                Scalar::from(0u64)
            };
            let flag = cs.alloc(
                || format!("proven_input_flag_{}", proven_idx),
                || Ok(flag_value),
            )?;
            cs.enforce(
                || format!("proven_input_flag_boolean_{}", proven_idx),
                |lc| lc + flag,
                |lc| lc + CS::one() - flag,
                |lc| lc,
            );
            cs.enforce(
                || format!("proven_input_flag_public_anchor_{}", proven_idx),
                |lc| lc + flag,
                |lc| lc + CS::one(),
                |lc| lc + proven_input_flag_public_vars[proven_idx],
            );

            let inactive = cs.alloc(
                || format!("proven_input_inactive_{}", proven_idx),
                || Ok(Scalar::from(1u64) - flag_value),
            )?;
            cs.enforce(
                || format!("proven_input_inactive_anchor_{}", proven_idx),
                |lc| lc + CS::one(),
                |lc| lc + inactive,
                |lc| lc + CS::one() - flag,
            );

            enforce_zero_when_inactive(
                cs,
                &format!("proven_input_amount_{}", proven_idx),
                input_amount_vars[proven_idx],
                inactive,
            );
            enforce_zero_when_inactive(
                cs,
                &format!("proven_input_key_image_{}", proven_idx),
                input_key_image_vars[proven_idx],
                inactive,
            );

            let (mut current_var, mut current_value) = {
                let mut state_value = Scalar::from(0xC0DE_CAFEu64);
                let state = cs.alloc(
                    || format!("proven_input_leaf_state_{}_0", proven_idx),
                    || Ok(state_value),
                )?;
                cs.enforce(
                    || format!("proven_input_leaf_state_{}_0_anchor", proven_idx),
                    |lc| lc + CS::one(),
                    |lc| lc + state,
                    |lc| lc + (Scalar::from(0xC0DE_CAFEu64), CS::one()),
                );

                let mut current_state = state;
                for word_idx in 0..PRIVATE_TREE_SCALAR_WORDS {
                    let word_value = self.proven_input_commitment_words[proven_idx][word_idx];
                    let word_var = cs.alloc(
                        || format!("proven_input_commitment_{}_word_{}", proven_idx, word_idx),
                        || Ok(word_value),
                    )?;
                    cs.enforce(
                        || {
                            format!(
                                "proven_input_commitment_public_anchor_{}_word_{}",
                                proven_idx, word_idx
                            )
                        },
                        |lc| lc + word_var,
                        |lc| lc + CS::one(),
                        |lc| lc + proven_input_commitment_public_vars[proven_idx][word_idx],
                    );
                    enforce_zero_when_inactive(
                        cs,
                        &format!("proven_input_commitment_{}_word_{}", proven_idx, word_idx),
                        word_var,
                        inactive,
                    );

                    let left_value = state_value + Scalar::from((word_idx as u64) + 1);
                    let left = cs.alloc(
                        || format!("proven_input_leaf_left_{}_{}", proven_idx, word_idx),
                        || Ok(left_value),
                    )?;
                    cs.enforce(
                        || format!("proven_input_leaf_left_anchor_{}_{}", proven_idx, word_idx),
                        |lc| lc + CS::one(),
                        |lc| lc + left,
                        |lc| lc + current_state + (Scalar::from((word_idx as u64) + 1), CS::one()),
                    );

                    let (next, next_value) = hash_pair_gadget(
                        cs,
                        &format!("proven_input_leaf_hash_{}_{}", proven_idx, word_idx),
                        left,
                        left_value,
                        word_var,
                        word_value,
                    )?;
                    current_state = next;
                    state_value = next_value;
                }
                (current_state, state_value)
            };

            for depth in 0..self.path_directions[proven_idx].len() {
                let sibling_word_vars = (0..PRIVATE_TX_ROOT_WORDS)
                    .map(|word_idx| {
                        let flat_idx = depth * PRIVATE_TX_ROOT_WORDS + word_idx;
                        let value = self.path_sibling_words[proven_idx][flat_idx];
                        cs.alloc(
                            || {
                                format!(
                                    "proven_input_path_sibling_{}_{}_word_{}",
                                    proven_idx, depth, word_idx
                                )
                            },
                            || Ok(value),
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                for (word_idx, sibling_word_var) in sibling_word_vars.iter().enumerate() {
                    enforce_zero_when_inactive(
                        cs,
                        &format!(
                            "proven_input_path_sibling_{}_{}_word_{}",
                            proven_idx, depth, word_idx
                        ),
                        *sibling_word_var,
                        inactive,
                    );
                }

                let sibling_words_u64 = (0..PRIVATE_TX_ROOT_WORDS)
                    .map(|word_idx| {
                        let flat_idx = depth * PRIVATE_TX_ROOT_WORDS + word_idx;
                        let bytes = self.path_sibling_words[proven_idx][flat_idx].to_bytes();
                        let mut chunk = [0u8; 8];
                        chunk.copy_from_slice(&bytes[..8]);
                        u64::from_le_bytes(chunk)
                    })
                    .collect::<Vec<_>>();
                let sibling_value = private_tree_scalar_from_words(&sibling_words_u64)
                    .ok_or(SynthesisError::AssignmentMissing)?;
                let sibling_var = scalar_from_word_vars(
                    cs,
                    &format!("proven_input_path_sibling_{}_{}", proven_idx, depth),
                    &sibling_word_vars,
                    sibling_value,
                )?;

                let dir_value = if self.path_directions[proven_idx][depth] {
                    Scalar::from(1u64)
                } else {
                    Scalar::from(0u64)
                };
                let dir = cs.alloc(
                    || format!("proven_input_path_dir_{}_{}", proven_idx, depth),
                    || Ok(dir_value),
                )?;
                cs.enforce(
                    || format!("proven_input_path_dir_boolean_{}_{}", proven_idx, depth),
                    |lc| lc + dir,
                    |lc| lc + CS::one() - dir,
                    |lc| lc,
                );
                enforce_zero_when_inactive(
                    cs,
                    &format!("proven_input_path_dir_{}_{}", proven_idx, depth),
                    dir,
                    inactive,
                );

                let left_selected_value = if self.path_directions[proven_idx][depth] {
                    sibling_value
                } else {
                    current_value
                };
                let right_selected_value = if self.path_directions[proven_idx][depth] {
                    current_value
                } else {
                    sibling_value
                };

                let current_minus_sibling_value = current_value - sibling_value;
                let current_minus_sibling = cs.alloc(
                    || format!("proven_input_path_delta_{}_{}", proven_idx, depth),
                    || Ok(current_minus_sibling_value),
                )?;
                cs.enforce(
                    || format!("proven_input_path_delta_anchor_{}_{}", proven_idx, depth),
                    |lc| lc + CS::one(),
                    |lc| lc + current_minus_sibling,
                    |lc| lc + current_var - sibling_var,
                );

                let dir_times_delta_value = dir_value * current_minus_sibling_value;
                let dir_times_delta = cs.alloc(
                    || format!("proven_input_path_dir_times_delta_{}_{}", proven_idx, depth),
                    || Ok(dir_times_delta_value),
                )?;
                cs.enforce(
                    || {
                        format!(
                            "proven_input_path_dir_times_delta_constraint_{}_{}",
                            proven_idx, depth
                        )
                    },
                    |lc| lc + dir,
                    |lc| lc + current_minus_sibling,
                    |lc| lc + dir_times_delta,
                );

                let left_selected = cs.alloc(
                    || format!("proven_input_path_left_{}_{}", proven_idx, depth),
                    || Ok(left_selected_value),
                )?;
                cs.enforce(
                    || format!("proven_input_path_left_anchor_{}_{}", proven_idx, depth),
                    |lc| lc + CS::one(),
                    |lc| lc + left_selected,
                    |lc| lc + current_var - dir_times_delta,
                );

                let right_selected = cs.alloc(
                    || format!("proven_input_path_right_{}_{}", proven_idx, depth),
                    || Ok(right_selected_value),
                )?;
                cs.enforce(
                    || format!("proven_input_path_right_anchor_{}_{}", proven_idx, depth),
                    |lc| lc + CS::one(),
                    |lc| lc + right_selected,
                    |lc| lc + sibling_var + dir_times_delta,
                );

                let (next_var, next_value) = hash_pair_gadget(
                    cs,
                    &format!("proven_input_path_hash_{}_{}", proven_idx, depth),
                    left_selected,
                    left_selected_value,
                    right_selected,
                    right_selected_value,
                )?;
                current_var = next_var;
                current_value = next_value;
            }

            cs.enforce(
                || format!("proven_input_membership_root_match_{}", proven_idx),
                |lc| lc + flag,
                |lc| lc + current_var - reconstructed_root,
                |lc| lc,
            );
        }

        Ok(())
    }
}

#[cfg(test)]
mod private_tx_v2_i1_lab_tests {
    use super::*;
    use bellman::gadgets::test::TestConstraintSystem;

    const LAB_SETUP_DIR_ENV: &str = "ZOKA_PRIVATE_TX_V2_I1_LAB_DIR";

    #[test]
    fn v1_shape_remains_96_inputs_and_8_merkle_paths() {
        let circuit = GrothPrivateTxCircuit::blank();
        assert_eq!(circuit.input_amounts.len(), PRIVATE_TX_MAX_INPUTS);
        assert_eq!(
            circuit.proven_input_flags.len(),
            PRIVATE_TX_MERKLE_PROVEN_INPUTS
        );
        assert_eq!(
            PRIVATE_TX_V1_PROFILE.private_input_words(),
            PRIVATE_TX_PRIVATE_INPUT_WORDS
        );
        assert_eq!(
            PRIVATE_TX_V1_PROFILE.public_input_words(),
            PRIVATE_TX_PUBLIC_INPUT_WORDS
        );

        let mut cs = TestConstraintSystem::new();
        circuit.synthesize(&mut cs).expect("v1 synthesis");
        assert!(
            cs.is_satisfied(),
            "v1 blank circuit must remain satisfiable"
        );
    }

    #[test]
    fn v2_i1_lab_shape_uses_one_input_and_one_merkle_path() {
        let circuit = GrothPrivateTxCircuit::blank_for_profile(1, 1);
        assert_eq!(circuit.input_amounts.len(), 1);
        assert_eq!(circuit.proven_input_flags.len(), 1);

        let mut cs = TestConstraintSystem::new();
        circuit.synthesize(&mut cs).expect("v2 i1 synthesis");
        assert!(cs.is_satisfied(), "v2 i1 blank circuit must be satisfiable");
    }

    #[test]
    fn v2_i1_candidate_rejects_tampering_and_malformed_public_inputs() {
        let params = generate_private_tx_v2_i1_lab_params_with_osrng().expect("generate params");
        let pvk = prepare_verifying_key(&params.vk);
        let (private_inputs, public_inputs) = synthetic_inactive_private_tx_v2_i1_inputs();
        let (proof_bytes, encoded_public_inputs) = prove_private_tx_for_profile(
            &private_inputs,
            &public_inputs,
            PRIVATE_TX_V2_I1_LAB_PROFILE,
            &params,
        )
        .expect("prove satisfiable i1 candidate");

        assert_eq!(proof_bytes.len(), 192);
        assert!(verify_private_tx_for_profile(
            &proof_bytes,
            &encoded_public_inputs,
            PRIVATE_TX_V2_I1_LAB_PROFILE,
            &pvk,
        )
        .expect("verify satisfiable i1 candidate"));

        let mut tampered_public_inputs = encoded_public_inputs.clone();
        tampered_public_inputs[0][0] ^= 1;
        assert!(!verify_private_tx_for_profile(
            &proof_bytes,
            &tampered_public_inputs,
            PRIVATE_TX_V2_I1_LAB_PROFILE,
            &pvk,
        )
        .expect("tampered public inputs must parse"));

        let malformed_public_inputs =
            &encoded_public_inputs[..encoded_public_inputs.len().saturating_sub(1)];
        let err = verify_private_tx_for_profile(
            &proof_bytes,
            malformed_public_inputs,
            PRIVATE_TX_V2_I1_LAB_PROFILE,
            &pvk,
        )
        .expect_err("missing public input must be rejected");
        assert!(err.contains("invalid public input count"));

        let mut tampered_proof = proof_bytes;
        tampered_proof[0] ^= 1;
        assert!(!matches!(
            verify_private_tx_for_profile(
                &tampered_proof,
                &encoded_public_inputs,
                PRIVATE_TX_V2_I1_LAB_PROFILE,
                &pvk,
            ),
            Ok(true)
        ));
    }

    #[test]
    #[ignore = "manual generation of temporary benchmark-only Groth16 parameters"]
    fn generate_v2_i1_lab_params() {
        let out_dir = std::path::PathBuf::from(
            std::env::var(LAB_SETUP_DIR_ENV).expect("set ZOKA_PRIVATE_TX_V2_I1_LAB_DIR"),
        );
        std::fs::create_dir_all(&out_dir).expect("create lab setup directory");

        let started = std::time::Instant::now();
        let params = generate_private_tx_v2_i1_lab_params_with_osrng().expect("generate params");
        setup_io::write_circuit_params(&out_dir, PRIVATE_TX_V2_I1_LAB_CIRCUIT, &params)
            .expect("write params");
        let path = out_dir.join(
            setup_io::params_filename(PRIVATE_TX_V2_I1_LAB_CIRCUIT).expect("params filename"),
        );
        println!(
            "{}",
            serde_json::json!({
                "profile": PRIVATE_TX_V2_I1_LAB_CIRCUIT,
                "params_path": path,
                "params_bytes": std::fs::metadata(path).expect("params metadata").len(),
                "generate_ms": started.elapsed().as_millis(),
            })
        );
    }

    #[test]
    #[ignore = "manual host benchmark for temporary v2 i1 parameters"]
    fn benchmark_v2_i1_lab_profile_on_host() {
        let setup_dir = std::path::PathBuf::from(
            std::env::var(LAB_SETUP_DIR_ENV).expect("set ZOKA_PRIVATE_TX_V2_I1_LAB_DIR"),
        );
        let result = benchmark_private_tx_v2_i1_lab(&setup_dir).expect("benchmark");
        println!(
            "{}",
            serde_json::json!({
                "profile": PRIVATE_TX_V2_I1_LAB_CIRCUIT,
                "params_bytes": result.params_bytes,
                "params_load_ms": result.params_load_ms,
                "prove_ms": result.prove_ms,
                "verify_ms": result.verify_ms,
                "proof_bytes": result.proof_bytes,
                "public_input_count": result.public_input_count,
            })
        );
    }

    #[test]
    #[ignore = "manual runtime wiring check with temporary v2 i1 parameters"]
    fn runtime_v2_i1_authenticated_bundle_route() {
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let release_setup_dir = manifest_dir.join("trusted-setup");
        let lab_setup_dir = std::path::PathBuf::from(
            std::env::var(LAB_SETUP_DIR_ENV).expect("set ZOKA_PRIVATE_TX_V2_I1_LAB_DIR"),
        );
        let runtime_bundle = tempfile::TempDir::new().expect("runtime bundle tmpdir");

        for circuit in setup_io::REQUIRED_CIRCUITS {
            let filename = setup_io::params_filename(circuit).expect("required params filename");
            std::fs::copy(
                release_setup_dir.join(filename),
                runtime_bundle.path().join(filename),
            )
            .expect("copy required release params");
        }
        let i1_filename =
            setup_io::params_filename(PRIVATE_TX_V2_I1_LAB_CIRCUIT).expect("i1 params filename");
        std::fs::copy(
            lab_setup_dir.join(i1_filename),
            runtime_bundle.path().join(i1_filename),
        )
        .expect("copy temporary i1 params");
        let digest =
            setup_io::compute_directory_digest(runtime_bundle.path()).expect("runtime digest");
        setup_io::write_digest(runtime_bundle.path(), &digest).expect("write runtime digest");

        std::env::set_var(TRUSTED_SETUP_ENV, runtime_bundle.path());
        let (private_inputs, public_inputs) = synthetic_inactive_private_tx_v2_i1_inputs();
        let (proof_bytes, encoded_public_inputs) = prove_private_tx_for_circuit(
            PRIVATE_TX_V2_I1_LAB_CIRCUIT,
            &private_inputs,
            &public_inputs,
        )
        .expect("runtime i1 proving route");
        assert!(verify(
            PRIVATE_TX_V2_I1_LAB_CIRCUIT,
            &proof_bytes,
            &encoded_public_inputs
        )
        .expect("runtime i1 verification route"));
        println!(
            "{}",
            serde_json::json!({
                "profile": PRIVATE_TX_V2_I1_LAB_CIRCUIT,
                "digest": hex::encode(digest),
                "proof_bytes": proof_bytes.len(),
                "public_input_count": encoded_public_inputs.len(),
            })
        );
    }
}

#[cfg(test)]
mod digest_pin_tests {
    //! Runtime digest-pin behavior. Uses a dedicated lock because the tests
    //! mutate a process-wide env var; running them in parallel would race.
    use super::*;
    use std::sync::Mutex;

    static ENV_GUARD: Mutex<()> = Mutex::new(());

    #[test]
    fn expected_digest_none_when_unset() {
        let _guard = ENV_GUARD.lock().unwrap();
        std::env::remove_var(EXPECTED_SETUP_DIGEST_ENV);
        assert!(expected_setup_digest().is_none() || EXPECTED_SETUP_DIGEST_COMPILED.is_some());
    }

    #[test]
    fn expected_digest_parses_hex() {
        let _guard = ENV_GUARD.lock().unwrap();
        let bytes = [0xAB; 32];
        std::env::set_var(EXPECTED_SETUP_DIGEST_ENV, hex::encode(bytes));
        let parsed = expected_setup_digest().expect("env var set");
        assert_eq!(parsed, bytes);
        std::env::remove_var(EXPECTED_SETUP_DIGEST_ENV);
    }

    #[test]
    fn expected_digest_rejects_wrong_length() {
        let _guard = ENV_GUARD.lock().unwrap();
        std::env::set_var(EXPECTED_SETUP_DIGEST_ENV, "deadbeef");
        let parsed = expected_setup_digest();
        std::env::remove_var(EXPECTED_SETUP_DIGEST_ENV);
        // If no compile-time override is present, short hex must be rejected.
        if EXPECTED_SETUP_DIGEST_COMPILED.is_none() {
            assert!(parsed.is_none());
        }
    }

    #[test]
    fn load_from_disk_rejects_mismatched_digest() {
        // Build a tempdir with a valid seeded CRS, but override expected digest
        // to a random value so the pin check fails.
        let dir = tempfile::TempDir::new().expect("tmpdir");
        let (balance, merkle, nullifier, private_tx) = {
            // Use seeded params — much faster than OsRng for a unit test — but
            // still exercises the same serialize/read path as real CRS.
            let mut rng_balance = seeded_rng(0x11);
            let balance = generate_random_parameters::<Bls12, _, _>(
                GrothBalanceCircuit::blank(),
                &mut rng_balance,
            )
            .unwrap();
            let mut rng_merkle = seeded_rng(0x22);
            let merkle = generate_random_parameters::<Bls12, _, _>(
                GrothMerkleCircuit::blank(),
                &mut rng_merkle,
            )
            .unwrap();
            let mut rng_nullifier = seeded_rng(0x33);
            let nullifier = generate_random_parameters::<Bls12, _, _>(
                GrothNullifierCircuit::blank(),
                &mut rng_nullifier,
            )
            .unwrap();
            let mut rng_private = seeded_rng(0x44);
            let private_tx = generate_random_parameters::<Bls12, _, _>(
                GrothPrivateTxCircuit::blank(),
                &mut rng_private,
            )
            .unwrap();
            (balance, merkle, nullifier, private_tx)
        };
        setup_io::write_circuit_params(dir.path(), BALANCE_CIRCUIT, &balance).unwrap();
        setup_io::write_circuit_params(dir.path(), MERKLE_CIRCUIT, &merkle).unwrap();
        setup_io::write_circuit_params(dir.path(), NULLIFIER_CIRCUIT, &nullifier).unwrap();
        setup_io::write_circuit_params(dir.path(), PRIVATE_TX_CIRCUIT, &private_tx).unwrap();
        let digest = setup_io::compute_directory_digest(dir.path()).unwrap();
        setup_io::write_digest(dir.path(), &digest).unwrap();

        let _guard = ENV_GUARD.lock().unwrap();
        let wrong = [0xFFu8; 32];
        std::env::set_var(EXPECTED_SETUP_DIGEST_ENV, hex::encode(wrong));
        let err = match load_setup_bundle_from_disk(dir.path()) {
            Ok(_) => panic!("expected digest mismatch error"),
            Err(msg) => msg,
        };
        std::env::remove_var(EXPECTED_SETUP_DIGEST_ENV);
        assert!(err.contains("does not match expected"), "got: {}", err);

        // And that removing the pin lets it load cleanly (assuming no
        // compile-time override is set in this test binary).
        if EXPECTED_SETUP_DIGEST_COMPILED.is_none() {
            load_setup_bundle_from_disk(dir.path())
                .map(|_| ())
                .expect("loads without pin");
        }
    }
}
