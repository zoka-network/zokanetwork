//! Persistent trusted-setup artifacts.
//!
//! Serializes and deserializes Groth16 `Parameters<Bls12>` to/from disk so the
//! CRS can be generated once by a dedicated ceremony (or a local operator) and
//! then consumed by every node at startup, instead of being regenerated in
//! memory on each process boot.
//!
//! Layout inside a setup directory:
//!
//! ```text
//! <dir>/
//!   balance.params        (Parameters<Bls12> for balance_circuit)
//!   merkle.params         (Parameters<Bls12> for merkle_circuit)
//!   nullifier.params      (Parameters<Bls12> for nullifier_circuit)
//!   private_tx.params     (Parameters<Bls12> for private_tx)
//!   DIGEST                (hex-encoded SHA-256 over the four files, newline-terminated)
//! ```
//!
//! `DIGEST` lets a node be bound at compile time (or via a constant) to a
//! specific CRS; loading anything whose digest does not match refuses to boot.

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use bellman::groth16::Parameters;
use bls12_381::Bls12;
use sha2::{Digest, Sha256};

use super::groth16_backend::{
    BALANCE_CIRCUIT, MERKLE_CIRCUIT, NULLIFIER_CIRCUIT, PRIVATE_TX_CIRCUIT,
};

pub const DIGEST_FILENAME: &str = "DIGEST";

/// Ordered list of circuits whose params must live in the setup directory.
pub const REQUIRED_CIRCUITS: [&str; 4] = [
    BALANCE_CIRCUIT,
    MERKLE_CIRCUIT,
    NULLIFIER_CIRCUIT,
    PRIVATE_TX_CIRCUIT,
];

#[derive(Debug)]
pub enum SetupIoError {
    Io(String),
    Serialization(String),
    DigestMismatch { expected: String, got: String },
    MissingFile(PathBuf),
    InvalidDigestFormat(String),
}

impl std::fmt::Display for SetupIoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SetupIoError::Io(msg) => write!(f, "setup I/O error: {}", msg),
            SetupIoError::Serialization(msg) => write!(f, "setup serialization error: {}", msg),
            SetupIoError::DigestMismatch { expected, got } => write!(
                f,
                "trusted setup digest mismatch: expected {}, got {}",
                expected, got
            ),
            SetupIoError::MissingFile(path) => {
                write!(f, "trusted setup file missing: {}", path.display())
            }
            SetupIoError::InvalidDigestFormat(msg) => {
                write!(f, "invalid DIGEST file: {}", msg)
            }
        }
    }
}

impl std::error::Error for SetupIoError {}

pub fn params_filename(circuit: &str) -> Result<&'static str, SetupIoError> {
    match circuit {
        BALANCE_CIRCUIT => Ok("balance.params"),
        MERKLE_CIRCUIT => Ok("merkle.params"),
        NULLIFIER_CIRCUIT => Ok("nullifier.params"),
        PRIVATE_TX_CIRCUIT => Ok("private_tx.params"),
        other => Err(SetupIoError::Serialization(format!(
            "unknown circuit for setup IO: {}",
            other
        ))),
    }
}

fn params_path(dir: &Path, circuit: &str) -> Result<PathBuf, SetupIoError> {
    Ok(dir.join(params_filename(circuit)?))
}

pub fn digest_path(dir: &Path) -> PathBuf {
    dir.join(DIGEST_FILENAME)
}

/// Serialize a single circuit's parameters into `<dir>/<circuit>.params`.
pub fn write_circuit_params(
    dir: &Path,
    circuit: &str,
    params: &Parameters<Bls12>,
) -> Result<(), SetupIoError> {
    if !dir.exists() {
        std::fs::create_dir_all(dir)
            .map_err(|e| SetupIoError::Io(format!("creating {}: {}", dir.display(), e)))?;
    }
    let path = params_path(dir, circuit)?;
    let file = File::create(&path)
        .map_err(|e| SetupIoError::Io(format!("opening {} for write: {}", path.display(), e)))?;
    let mut writer = BufWriter::new(file);
    params
        .write(&mut writer)
        .map_err(|e| SetupIoError::Serialization(format!("writing {}: {}", circuit, e)))?;
    writer
        .flush()
        .map_err(|e| SetupIoError::Io(format!("flushing {}: {}", path.display(), e)))?;
    Ok(())
}

/// Read a circuit's parameters from `<dir>/<circuit>.params`.
pub fn read_circuit_params(dir: &Path, circuit: &str) -> Result<Parameters<Bls12>, SetupIoError> {
    let path = params_path(dir, circuit)?;
    if !path.exists() {
        return Err(SetupIoError::MissingFile(path));
    }
    let file = File::open(&path)
        .map_err(|e| SetupIoError::Io(format!("opening {}: {}", path.display(), e)))?;
    let mut reader = BufReader::new(file);
    // `Parameters::read` with `checked = true` validates subgroup membership; slow but
    // exactly the work you want on a piece of infrastructure you must fully trust.
    Parameters::<Bls12>::read(&mut reader, true)
        .map_err(|e| SetupIoError::Serialization(format!("reading {}: {}", circuit, e)))
}

fn file_sha256(path: &Path) -> Result<[u8; 32], SetupIoError> {
    let mut file = File::open(path)
        .map_err(|e| SetupIoError::Io(format!("opening {} for hashing: {}", path.display(), e)))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| SetupIoError::Io(format!("reading {}: {}", path.display(), e)))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().into())
}

/// Compute a SHA-256 digest over every required circuit file, in canonical order
/// (`balance`, `merkle`, `nullifier`, `private_tx`).
///
/// The digest binds every node to a single, specific CRS snapshot.
pub fn compute_directory_digest(dir: &Path) -> Result<[u8; 32], SetupIoError> {
    let mut hasher = Sha256::new();
    for circuit in REQUIRED_CIRCUITS {
        let path = params_path(dir, circuit)?;
        if !path.exists() {
            return Err(SetupIoError::MissingFile(path));
        }
        let file_hash = file_sha256(&path)?;
        hasher.update(circuit.as_bytes());
        hasher.update([0x1f]);
        hasher.update(file_hash);
        hasher.update([0x1e]);
    }
    Ok(hasher.finalize().into())
}

pub fn write_digest(dir: &Path, digest: &[u8; 32]) -> Result<(), SetupIoError> {
    let path = digest_path(dir);
    let mut file = File::create(&path)
        .map_err(|e| SetupIoError::Io(format!("opening {}: {}", path.display(), e)))?;
    writeln!(file, "{}", hex::encode(digest))
        .map_err(|e| SetupIoError::Io(format!("writing DIGEST: {}", e)))?;
    Ok(())
}

pub fn read_digest(dir: &Path) -> Result<[u8; 32], SetupIoError> {
    let path = digest_path(dir);
    if !path.exists() {
        return Err(SetupIoError::MissingFile(path));
    }
    let mut file = File::open(&path)
        .map_err(|e| SetupIoError::Io(format!("opening {}: {}", path.display(), e)))?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .map_err(|e| SetupIoError::Io(format!("reading DIGEST: {}", e)))?;
    let trimmed = contents.trim();
    let bytes = hex::decode(trimmed)
        .map_err(|e| SetupIoError::InvalidDigestFormat(format!("hex decode: {}", e)))?;
    if bytes.len() != 32 {
        return Err(SetupIoError::InvalidDigestFormat(format!(
            "expected 32 bytes, got {}",
            bytes.len()
        )));
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

/// Verify that `<dir>/DIGEST` matches the actual SHA-256 over the current
/// param files. Returns the digest on success.
pub fn verify_directory_digest(dir: &Path) -> Result<[u8; 32], SetupIoError> {
    let stored = read_digest(dir)?;
    let actual = compute_directory_digest(dir)?;
    if stored != actual {
        return Err(SetupIoError::DigestMismatch {
            expected: hex::encode(stored),
            got: hex::encode(actual),
        });
    }
    Ok(actual)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bellman::groth16::generate_random_parameters;
    use bellman::{Circuit, ConstraintSystem, SynthesisError};
    use bls12_381::Scalar;
    use rand::rngs::OsRng;
    use tempfile::TempDir;

    #[derive(Clone)]
    struct Trivial;
    impl Circuit<Scalar> for Trivial {
        fn synthesize<CS: ConstraintSystem<Scalar>>(
            self,
            cs: &mut CS,
        ) -> Result<(), SynthesisError> {
            let x = cs.alloc_input(|| "x", || Ok(Scalar::from(1u64)))?;
            cs.enforce(|| "id", |lc| lc + x, |lc| lc + CS::one(), |lc| lc + x);
            Ok(())
        }
    }

    #[test]
    fn round_trip_single_circuit() {
        let dir = TempDir::new().expect("tmpdir");
        let params = generate_random_parameters::<Bls12, _, _>(Trivial, &mut OsRng).unwrap();
        write_circuit_params(dir.path(), BALANCE_CIRCUIT, &params).expect("write");
        let loaded = read_circuit_params(dir.path(), BALANCE_CIRCUIT).expect("read");
        let mut a = Vec::new();
        params.write(&mut a).unwrap();
        let mut b = Vec::new();
        loaded.write(&mut b).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn digest_detects_tampering() {
        let dir = TempDir::new().expect("tmpdir");
        for circuit in REQUIRED_CIRCUITS {
            let params = generate_random_parameters::<Bls12, _, _>(Trivial, &mut OsRng).unwrap();
            write_circuit_params(dir.path(), circuit, &params).expect("write");
        }
        let digest = compute_directory_digest(dir.path()).expect("digest");
        write_digest(dir.path(), &digest).expect("write digest");
        verify_directory_digest(dir.path()).expect("matches before tamper");

        // Tamper one of the files.
        let tampered = params_path(dir.path(), PRIVATE_TX_CIRCUIT).unwrap();
        let new_params = generate_random_parameters::<Bls12, _, _>(Trivial, &mut OsRng).unwrap();
        let mut tampered_file = File::create(&tampered).unwrap();
        new_params.write(&mut tampered_file).unwrap();

        match verify_directory_digest(dir.path()) {
            Err(SetupIoError::DigestMismatch { .. }) => {}
            other => panic!("expected digest mismatch, got {:?}", other),
        }
    }
}
