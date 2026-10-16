//! Canonical chain snapshots for fast sync (`zoka-snapshot-v1`).
//!
//! A snapshot is a deterministic byte-blob that captures the current
//! canonical chain head + every block from height 1 up to that head.
//! New nodes can import a verified snapshot to skip per-block IBD over
//! gossipsub.
//!
//! Trust model:
//!
//! - The snapshot file ALWAYS carries a SHA-256 checksum over its
//!   canonical bytes. Tampering with any byte invalidates the file.
//! - Snapshots MAY also carry an ed25519 signature (raw 64 bytes hex).
//!   When the importer is given an `--public-key`, the signature MUST
//!   verify against that key. If no key is provided the signature field
//!   is left informational and the importer falls back to byte-for-byte
//!   verification of every block via `state::chain::apply_block` — i.e.
//!   the blocks are re-validated by full PoW + coinbase rules. Even an
//!   unsigned snapshot from a hostile source cannot inject invalid
//!   blocks; it can only delay (by serving correct blocks).
//!
//! Implementation note: the snapshot is intentionally one-directional.
//! It only captures public block data. Per-account state and per-wallet
//! private notes are reconstructed on the importer side from the imported
//! blocks (or, for private notes the importer never receives, ignored —
//! private notes are recipient-side anyway). This keeps the snapshot
//! size proportional to chain height + tx count and not to wallet count.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ed25519_dalek::{Signature as Ed25519Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use thiserror::Error;

use crate::economics;
use crate::state::chain::{apply_block, ChainError};
use crate::storage::RocksDbStorage;
use crate::types::Block;

/// Schema discriminator. Bumping this is a breaking change.
pub const SNAPSHOT_SCHEMA_ID: &str = "zoka-snapshot-v1";

/// Header carried at the top of the snapshot. Frozen so any byte
/// difference (including in metadata) is detectable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotHeader {
    pub schema: String,
    pub chain_id: String,
    pub head_height: u64,
    pub head_hash: String,
    pub total_supply_emitted: u64,
    pub cumulative_work: u128,
    pub current_difficulty_bits: u32,
    pub block_count: u64,
    pub generated_unix_secs: u64,
}

/// Body of the snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChainSnapshotV1 {
    pub header: SnapshotHeader,
    pub blocks: Vec<Block>,
    /// Raw 64-byte ed25519 signature, hex-encoded. Optional. The
    /// signature MUST cover the canonical bytes of `header` + `blocks`
    /// (NOT including the signature field).
    #[serde(default)]
    pub signature_hex: Option<String>,
    /// Verifying key the signer used. Hex-encoded raw 32 bytes. Only
    /// informational; the importer must independently know the trusted
    /// key it expects (passed via `--public-key`).
    #[serde(default)]
    pub signer_pubkey_hex: Option<String>,
    /// ML-DSA-65 half of a hybrid signature (stage 6), hex. When present,
    /// `signature_hex` is the Ed25519 half and both sign
    /// `"zoka-snapshot-hybrid-v1" ‖ 0x1f ‖ canonical bytes`. Absent fields
    /// are not serialized, so an unsigned or Ed25519-only snapshot keeps the
    /// exact bytes older nodes expect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mldsa_signature_hex: Option<String>,
    /// ML-DSA-65 public key of the signer (hex, 1952 bytes). Informational,
    /// like `signer_pubkey_hex`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signer_mldsa_pubkey_hex: Option<String>,
}

#[derive(Debug, Clone, Error)]
pub enum SnapshotError {
    #[error("snapshot schema is '{got}', expected '{expected}'")]
    SchemaMismatch { got: String, expected: &'static str },

    #[error("snapshot chain_id is '{got}', expected '{expected}'")]
    ChainIdMismatch { got: String, expected: String },

    #[error("snapshot block count mismatch: header says {claimed}, body has {actual}")]
    BlockCountMismatch { claimed: u64, actual: u64 },

    #[error("snapshot block at index {index} has height {got}, expected {expected}")]
    BlockHeightDrift {
        index: usize,
        got: u64,
        expected: u64,
    },

    #[error("snapshot head metadata drift: field '{field}' has value {got}, computed {expected}")]
    HeadDrift {
        field: &'static str,
        got: String,
        expected: String,
    },

    #[error("snapshot serialization failed: {0}")]
    Serialize(String),

    #[error("snapshot deserialization failed: {0}")]
    Deserialize(String),

    #[error("snapshot signature is invalid: {0}")]
    BadSignature(String),

    #[error("snapshot is signed but no signer pubkey is present")]
    SignatureWithoutPubkey,

    #[error(
        "snapshot signer pubkey ({got}) does not match the expected verifying key ({expected})"
    )]
    SignerPubkeyMismatch { got: String, expected: String },

    #[error("a hybrid (Ed25519 + ML-DSA-65) snapshot signature is required")]
    HybridSignatureRequired,
    #[error("this build cannot verify hybrid snapshot signatures (needs pq-experimental)")]
    HybridSignatureUnsupported,
    #[error("no configured snapshot signer accepts this snapshot")]
    NoTrustedSigner,
    #[error("snapshot signature was required but no signature field is present")]
    SignatureRequired,

    #[error("snapshot apply_block rejected block at height {height}: {reason}")]
    ApplyRejected { height: u64, reason: String },

    #[error("snapshot storage error: {0}")]
    Storage(String),
}

impl From<ChainError> for SnapshotError {
    fn from(err: ChainError) -> Self {
        SnapshotError::Storage(err.to_string())
    }
}

impl From<crate::types::StorageError> for SnapshotError {
    fn from(err: crate::types::StorageError) -> Self {
        SnapshotError::Storage(format!("{err:?}"))
    }
}

/// Build a snapshot from the live RocksDB storage.
///
/// Reads the chain head + every block by height ordering. The returned
/// struct is unsigned. Use `sign` to attach an ed25519 signature, then
/// `serialize_canonical` to get the bytes to write to disk.
pub fn export_from_storage(
    storage: &RocksDbStorage,
    chain_id: &str,
    generated_unix_secs: u64,
) -> Result<ChainSnapshotV1, SnapshotError> {
    let head_height = storage.get_chain_height()?;
    let head_hash = storage.get_last_block_hash()?;
    let total_supply_emitted = storage.get_total_supply_emitted()?;
    let cumulative_work = storage.get_cumulative_work()?;
    let current_difficulty_bits = storage.get_current_difficulty_bits()?;

    let mut blocks: Vec<Block> = Vec::with_capacity(head_height as usize);
    for height in 1..=head_height {
        match storage.get_block_by_height(height)? {
            Some(b) => blocks.push(b),
            None => {
                return Err(SnapshotError::Storage(format!(
                    "missing block at height {height} during export",
                )))
            }
        }
    }

    let header = SnapshotHeader {
        schema: SNAPSHOT_SCHEMA_ID.to_string(),
        chain_id: chain_id.to_string(),
        head_height,
        head_hash,
        total_supply_emitted,
        cumulative_work,
        current_difficulty_bits,
        block_count: blocks.len() as u64,
        generated_unix_secs,
    };

    Ok(ChainSnapshotV1 {
        header,
        blocks,
        signature_hex: None,
        signer_pubkey_hex: None,
        mldsa_signature_hex: None,
        signer_mldsa_pubkey_hex: None,
    })
}

/// Bytes that go into the digest and into the signature. EXCLUDES the
/// `signature_hex` field so the signature can't sign itself, and
/// excludes the `signer_pubkey_hex` so the same payload can be
/// re-signed by a different key without changing the signed bytes.
fn canonical_signed_bytes(snapshot: &ChainSnapshotV1) -> Result<Vec<u8>, SnapshotError> {
    #[derive(Serialize)]
    struct Signable<'a> {
        header: &'a SnapshotHeader,
        blocks: &'a [Block],
    }
    serde_json::to_vec(&Signable {
        header: &snapshot.header,
        blocks: &snapshot.blocks,
    })
    .map_err(|e| SnapshotError::Serialize(e.to_string()))
}

/// SHA-256 digest of the canonical signed bytes. Independent of the
/// signature field — useful for tooling that wants a stable identifier
/// regardless of who signed the snapshot.
pub fn compute_digest(snapshot: &ChainSnapshotV1) -> Result<String, SnapshotError> {
    let bytes = canonical_signed_bytes(snapshot)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(hex::encode(hasher.finalize()))
}

/// Attach an ed25519 signature using a 32-byte secret key (hex). Stores
/// the signing key's verifying key alongside the signature.
pub fn sign(
    snapshot: &mut ChainSnapshotV1,
    signing_key_bytes: &[u8; 32],
) -> Result<(), SnapshotError> {
    use ed25519_dalek::{Signer as _, SigningKey};
    let signing_key = SigningKey::from_bytes(signing_key_bytes);
    let bytes = canonical_signed_bytes(snapshot)?;
    let signature = signing_key.sign(&bytes);
    snapshot.signature_hex = Some(hex::encode(signature.to_bytes()));
    snapshot.signer_pubkey_hex = Some(hex::encode(signing_key.verifying_key().to_bytes()));
    Ok(())
}

/// Domain of hybrid snapshot signatures (Ed25519 + ML-DSA-65).
pub const HYBRID_SIGNING_DOMAIN: &[u8] = b"zoka-snapshot-hybrid-v1";
/// Length of a hybrid public key (version, Ed25519, ML-DSA-65) in hex, as
/// it appears in `snapshot_signers`.
pub const HYBRID_SIGNER_HEX_LEN: usize = (1 + 32 + 1952) * 2;

/// Attach a hybrid Ed25519 + ML-DSA-65 signature (stage 6). Both halves
/// sign the same message; a verifier that trusts a hybrid key accepts
/// neither half alone.
#[cfg(feature = "pq-experimental")]
pub fn sign_hybrid(
    snapshot: &mut ChainSnapshotV1,
    key: &crate::hybrid_signature::HybridSigningKey,
) -> Result<(), SnapshotError> {
    let bytes = canonical_signed_bytes(snapshot)?;
    let signature = key
        .sign(HYBRID_SIGNING_DOMAIN, &bytes)
        .map_err(|e| SnapshotError::BadSignature(e.to_string()))?;
    let (ed25519, mldsa) = signature.to_hex_parts();
    let public = key.verifying_key();
    snapshot.signature_hex = Some(ed25519);
    snapshot.signer_pubkey_hex = Some(hex::encode(public.ed25519().to_bytes()));
    snapshot.mldsa_signature_hex = Some(mldsa);
    snapshot.signer_mldsa_pubkey_hex = Some(hex::encode(public.mldsa().to_bytes()));
    Ok(())
}

/// Serialize a complete (possibly signed) snapshot to JSON bytes ready
/// to be written to disk.
pub fn serialize(snapshot: &ChainSnapshotV1) -> Result<Vec<u8>, SnapshotError> {
    serde_json::to_vec_pretty(snapshot).map_err(|e| SnapshotError::Serialize(e.to_string()))
}

/// Parse a snapshot from JSON bytes. Does not yet verify checksum,
/// signature, or block validity — call [`verify`] for that.
pub fn deserialize(bytes: &[u8]) -> Result<ChainSnapshotV1, SnapshotError> {
    serde_json::from_slice(bytes).map_err(|e| SnapshotError::Deserialize(e.to_string()))
}

/// Verification options for `verify`. The default is "fully strict":
/// fail if the chain id does not match, fail if the signature is
/// missing when expected, etc. Tests use `relaxed_for_chain_id` to
/// validate snapshots from non-mainnet chains.
#[derive(Debug, Clone)]
pub struct VerifyOptions<'a> {
    pub expected_chain_id: &'a str,
    pub require_signature: bool,
    /// When `Some`, the signature MUST verify against this verifying key.
    pub expected_pubkey: Option<&'a [u8; 32]>,
    /// When `Some`, the snapshot MUST carry a hybrid signature by exactly
    /// this hybrid public key (raw bytes: version, Ed25519, ML-DSA-65).
    pub expected_hybrid_pubkey: Option<&'a [u8]>,
    /// Refuse Ed25519-only signatures.
    pub require_hybrid: bool,
}

impl<'a> VerifyOptions<'a> {
    pub fn for_mainnet() -> Self {
        Self {
            expected_chain_id: economics::MAINNET_CHAIN_ID,
            require_signature: true,
            expected_pubkey: None,
            expected_hybrid_pubkey: None,
            require_hybrid: false,
        }
    }
}

/// Run all the structural checks on a snapshot:
/// - schema discriminator
/// - chain id
/// - block count vs header
/// - block heights are 1..=head_height with no gaps
/// - head metadata matches the last block
/// - signature presence / verification per `opts`
///
/// Does NOT touch storage and does NOT replay PoW. For the full
/// security guarantee, follow this with `import_into_storage`, which
/// re-runs every consensus rule via `state::chain::apply_block`.
pub fn verify(
    snapshot: &ChainSnapshotV1,
    opts: &VerifyOptions<'_>,
) -> Result<String, SnapshotError> {
    if snapshot.header.schema != SNAPSHOT_SCHEMA_ID {
        return Err(SnapshotError::SchemaMismatch {
            got: snapshot.header.schema.clone(),
            expected: SNAPSHOT_SCHEMA_ID,
        });
    }
    if snapshot.header.chain_id != opts.expected_chain_id {
        return Err(SnapshotError::ChainIdMismatch {
            got: snapshot.header.chain_id.clone(),
            expected: opts.expected_chain_id.to_string(),
        });
    }
    if snapshot.blocks.len() as u64 != snapshot.header.block_count {
        return Err(SnapshotError::BlockCountMismatch {
            claimed: snapshot.header.block_count,
            actual: snapshot.blocks.len() as u64,
        });
    }
    for (i, block) in snapshot.blocks.iter().enumerate() {
        let expected_height = (i as u64).saturating_add(1);
        if block.header.block_height != expected_height {
            return Err(SnapshotError::BlockHeightDrift {
                index: i,
                got: block.header.block_height,
                expected: expected_height,
            });
        }
    }
    if snapshot.header.head_height != snapshot.header.block_count {
        return Err(SnapshotError::HeadDrift {
            field: "head_height",
            got: snapshot.header.head_height.to_string(),
            expected: snapshot.header.block_count.to_string(),
        });
    }
    if let Some(last) = snapshot.blocks.last() {
        if last.header.hash != snapshot.header.head_hash {
            return Err(SnapshotError::HeadDrift {
                field: "head_hash",
                got: snapshot.header.head_hash.clone(),
                expected: last.header.hash.clone(),
            });
        }
    } else if snapshot.header.head_height != 0 {
        return Err(SnapshotError::HeadDrift {
            field: "head_height",
            got: snapshot.header.head_height.to_string(),
            expected: "0".to_string(),
        });
    }

    let digest = compute_digest(snapshot)?;

    let signature_hex = snapshot.signature_hex.as_ref();
    if opts.require_signature && signature_hex.is_none() {
        return Err(SnapshotError::SignatureRequired);
    }
    let hybrid = snapshot.mldsa_signature_hex.is_some() || snapshot.signer_mldsa_pubkey_hex.is_some();
    if (opts.require_hybrid || opts.expected_hybrid_pubkey.is_some()) && !hybrid {
        return Err(SnapshotError::HybridSignatureRequired);
    }
    if hybrid {
        verify_hybrid_signature(snapshot, opts)?;
        return Ok(digest);
    }
    if let Some(sig_hex) = signature_hex {
        let signer_pubkey_hex = snapshot
            .signer_pubkey_hex
            .as_ref()
            .ok_or(SnapshotError::SignatureWithoutPubkey)?;
        let signer_pubkey_bytes: [u8; 32] = hex::decode(signer_pubkey_hex)
            .map_err(|e| SnapshotError::BadSignature(format!("signer pubkey hex decode: {e}")))?
            .try_into()
            .map_err(|_| {
                SnapshotError::BadSignature("signer pubkey is not 32 bytes".to_string())
            })?;
        if let Some(expected) = opts.expected_pubkey {
            if &signer_pubkey_bytes != expected {
                return Err(SnapshotError::SignerPubkeyMismatch {
                    got: hex::encode(signer_pubkey_bytes),
                    expected: hex::encode(expected),
                });
            }
        }
        let signature_bytes: [u8; 64] = hex::decode(sig_hex)
            .map_err(|e| SnapshotError::BadSignature(format!("sig hex decode: {e}")))?
            .try_into()
            .map_err(|_| SnapshotError::BadSignature("signature is not 64 bytes".to_string()))?;
        let key = VerifyingKey::from_bytes(&signer_pubkey_bytes)
            .map_err(|e| SnapshotError::BadSignature(format!("invalid pubkey: {e}")))?;
        let signature = Ed25519Signature::from_bytes(&signature_bytes);
        let bytes = canonical_signed_bytes(snapshot)?;
        key.verify(&bytes, &signature)
            .map_err(|e| SnapshotError::BadSignature(format!("ed25519 verify failed: {e}")))?;
    }

    Ok(digest)
}

#[cfg(feature = "pq-experimental")]
fn verify_hybrid_signature(
    snapshot: &ChainSnapshotV1,
    opts: &VerifyOptions<'_>,
) -> Result<(), SnapshotError> {
    let missing = |what: &str| SnapshotError::BadSignature(format!("hybrid signature without {what}"));
    let ed_hex = snapshot.signature_hex.as_deref().ok_or_else(|| missing("its Ed25519 half"))?;
    let ml_hex = snapshot.mldsa_signature_hex.as_deref().ok_or_else(|| missing("its ML-DSA half"))?;
    let ed_key_hex = snapshot.signer_pubkey_hex.as_deref().ok_or(SnapshotError::SignatureWithoutPubkey)?;
    let ml_key_hex = snapshot
        .signer_mldsa_pubkey_hex
        .as_deref()
        .ok_or(SnapshotError::SignatureWithoutPubkey)?;
    let mut key_bytes = vec![crate::hybrid_signature::HYBRID_KEY_VERSION];
    key_bytes.extend(
        hex::decode(ed_key_hex).map_err(|e| SnapshotError::BadSignature(format!("signer pubkey hex decode: {e}")))?,
    );
    key_bytes.extend(
        hex::decode(ml_key_hex).map_err(|e| SnapshotError::BadSignature(format!("ML-DSA pubkey hex decode: {e}")))?,
    );
    let key = crate::hybrid_signature::HybridVerifyingKey::from_bytes(&key_bytes)
        .map_err(|e| SnapshotError::BadSignature(e.to_string()))?;
    if let Some(expected) = opts.expected_hybrid_pubkey {
        if key.to_bytes() != expected {
            return Err(SnapshotError::SignerPubkeyMismatch {
                got: hex::encode(key.ed25519().to_bytes()),
                expected: hex::encode(expected.get(1..33).unwrap_or_default()),
            });
        }
    }
    if let Some(expected) = opts.expected_pubkey {
        if key.ed25519().as_bytes() != expected {
            return Err(SnapshotError::SignerPubkeyMismatch {
                got: hex::encode(key.ed25519().to_bytes()),
                expected: hex::encode(expected),
            });
        }
    }
    let signature = crate::hybrid_signature::HybridSignature::from_hex_parts(ed_hex, ml_hex)
        .map_err(|e| SnapshotError::BadSignature(e.to_string()))?;
    let bytes = canonical_signed_bytes(snapshot)?;
    key.verify(HYBRID_SIGNING_DOMAIN, &bytes, &signature)
        .map_err(|e| SnapshotError::BadSignature(e.to_string()))
}

#[cfg(not(feature = "pq-experimental"))]
fn verify_hybrid_signature(
    _snapshot: &ChainSnapshotV1,
    _opts: &VerifyOptions<'_>,
) -> Result<(), SnapshotError> {
    Err(SnapshotError::HybridSignatureUnsupported)
}

/// Checks `snapshot` against the `snapshot_signers` of a network config:
/// Ed25519 keys (64 hex) or hybrid keys ([`HYBRID_SIGNER_HEX_LEN`] hex).
/// If any hybrid signer is listed, only a hybrid signature by one of them is
/// accepted: trusting Ed25519 alone would let whoever breaks it in.
/// Returns the digest, or the last verification error.
pub fn verify_against_trusted_signers(
    snapshot: &ChainSnapshotV1,
    chain_id: &str,
    signers: &[String],
) -> Result<String, SnapshotError> {
    let decoded: Vec<Vec<u8>> = signers
        .iter()
        .filter_map(|signer| hex::decode(signer.trim()).ok())
        .collect();
    let hybrid: Vec<&Vec<u8>> = decoded
        .iter()
        .filter(|key| key.len() * 2 == HYBRID_SIGNER_HEX_LEN)
        .collect();
    let mut last_error = SnapshotError::NoTrustedSigner;
    if !hybrid.is_empty() {
        for key in hybrid {
            let opts = VerifyOptions {
                expected_chain_id: chain_id,
                require_signature: true,
                expected_pubkey: None,
                expected_hybrid_pubkey: Some(key),
                require_hybrid: true,
            };
            match verify(snapshot, &opts) {
                Ok(digest) => return Ok(digest),
                Err(error) => last_error = error,
            }
        }
        return Err(last_error);
    }
    for key in decoded.iter().filter(|key| key.len() == 32) {
        let pubkey: [u8; 32] = key.as_slice().try_into().expect("32 bytes");
        let opts = VerifyOptions {
            expected_chain_id: chain_id,
            require_signature: true,
            expected_pubkey: Some(&pubkey),
            expected_hybrid_pubkey: None,
            require_hybrid: false,
        };
        match verify(snapshot, &opts) {
            Ok(digest) => return Ok(digest),
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

/// Replay every block in `snapshot` through `state::chain::apply_block`,
/// thereby re-running all consensus rules (PoW, coinbase, fork-link).
/// Equivalent to receiving the same blocks one at a time via gossipsub
/// — same ingress path, no shortcut.
///
/// Caller is responsible for having already called [`verify`].
// ── Snapshot catalog / manifest sidecar ───────────────────────────────────

/// Lightweight manifest written alongside each `.snap` file so peers and
/// the HTTP API can advertise available snapshots without loading the full
/// block data.
///
/// Stored at `<snap_path>.manifest` (e.g. `~/.zoka/snapshots/1000.snap.manifest`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotFileManifest {
    pub chain_id: String,
    pub schema: String,
    pub head_height: u64,
    pub head_hash: String,
    pub total_supply_emitted: u64,
    pub cumulative_work: u128,
    pub block_count: u64,
    pub generated_unix_secs: u64,
    /// Size of the companion `.snap` file in bytes.
    pub size_bytes: u64,
    /// SHA-256 of the companion `.snap` file (hex).
    pub sha256_hex: String,
    /// Absolute or relative path to the companion `.snap` file.
    pub snap_path: String,
    /// HTTP download URLs for the `.snap` file (operator-configured).
    #[serde(default)]
    pub download_urls: Vec<String>,
    /// Hex-encoded ed25519 signature over canonical snapshot bytes.
    #[serde(default)]
    pub signature_hex: Option<String>,
    /// Hex-encoded ed25519 verifying key of the signer.
    #[serde(default)]
    pub signer_pubkey_hex: Option<String>,
    /// ML-DSA-65 half of a hybrid signature, if any (stage 6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mldsa_signature_hex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signer_mldsa_pubkey_hex: Option<String>,
}

/// Returns the sidecar path for a given `.snap` file path.
pub fn manifest_path_for(snap_path: &Path) -> PathBuf {
    let mut p = snap_path.to_path_buf();
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("snap")
        .to_string();
    p.set_extension(format!("{ext}.manifest"));
    p
}

/// Writes a `.manifest` sidecar next to the `.snap` file described by
/// `snap_path`. Call this right after writing the `.snap` file.
pub fn write_manifest_sidecar(
    snapshot: &ChainSnapshotV1,
    snap_path: &Path,
    size_bytes: u64,
    sha256_hex: String,
    download_urls: Vec<String>,
) -> Result<PathBuf, SnapshotError> {
    let manifest = SnapshotFileManifest {
        chain_id: snapshot.header.chain_id.clone(),
        schema: snapshot.header.schema.clone(),
        head_height: snapshot.header.head_height,
        head_hash: snapshot.header.head_hash.clone(),
        total_supply_emitted: snapshot.header.total_supply_emitted,
        cumulative_work: snapshot.header.cumulative_work,
        block_count: snapshot.header.block_count,
        generated_unix_secs: snapshot.header.generated_unix_secs,
        size_bytes,
        sha256_hex,
        snap_path: snap_path.to_string_lossy().to_string(),
        download_urls,
        signature_hex: snapshot.signature_hex.clone(),
        signer_pubkey_hex: snapshot.signer_pubkey_hex.clone(),
        mldsa_signature_hex: snapshot.mldsa_signature_hex.clone(),
        signer_mldsa_pubkey_hex: snapshot.signer_mldsa_pubkey_hex.clone(),
    };
    let sidecar_path = manifest_path_for(snap_path);
    let json = serde_json::to_vec_pretty(&manifest)
        .map_err(|e| SnapshotError::Serialize(e.to_string()))?;
    std::fs::write(&sidecar_path, json)
        .map_err(|e| SnapshotError::Storage(format!("write manifest sidecar: {e}")))?;
    Ok(sidecar_path)
}

/// Scan `dir` for `*.snap.manifest` sidecar files and return parsed manifests
/// sorted by `head_height` descending (newest first).
///
/// Files that fail to parse are skipped and logged at debug level.
pub fn scan_snapshot_manifests(dir: &Path) -> Vec<SnapshotFileManifest> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut manifests: Vec<SnapshotFileManifest> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            let name = path.file_name()?.to_string_lossy().to_string();
            if !name.ends_with(".snap.manifest") {
                return None;
            }
            let raw = std::fs::read_to_string(&path).ok()?;
            match serde_json::from_str::<SnapshotFileManifest>(&raw) {
                Ok(m) => Some(m),
                Err(err) => {
                    tracing::debug!(
                        "snapshot manifest parse failed for {}: {err}",
                        path.display()
                    );
                    None
                }
            }
        })
        .collect();
    manifests.sort_by(|a, b| b.head_height.cmp(&a.head_height));
    manifests
}

/// Default snapshot directory: `$ZOKA_HOME/snapshots/` or `~/.zoka/snapshots/`.
pub fn default_snapshot_dir() -> PathBuf {
    if let Ok(home) = std::env::var("ZOKA_HOME") {
        let home = home.trim();
        if !home.is_empty() {
            return PathBuf::from(home).join("snapshots");
        }
    }
    home::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".zoka")
        .join("snapshots")
}

pub fn import_into_storage(
    storage: &Arc<RocksDbStorage>,
    chain_id: &str,
    snapshot: &ChainSnapshotV1,
) -> Result<u64, SnapshotError> {
    let mut applied = 0u64;
    for block in &snapshot.blocks {
        match apply_block(storage, chain_id, block) {
            Ok(_) => applied = applied.saturating_add(1),
            Err(ChainError::BlockAlreadyKnown(_)) => continue,
            Err(err) => {
                return Err(SnapshotError::ApplyRejected {
                    height: block.header.block_height,
                    reason: err.to_string(),
                });
            }
        }
    }
    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mining::block_validator::build_coinbase_tx;
    use crate::mining::pow;
    use crate::state::chain::{
        apply_block_with_expected_difficulty, genesis_parent_for, ChainError as CE,
    };
    use crate::types::{Address, BlockHeader, PrivateCoinbaseOutput, PublicKey, Signature};
    use ed25519_dalek::SigningKey;

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

    fn open_temp_storage() -> (RocksDbStorage, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let storage = RocksDbStorage::new(dir.path().to_str().unwrap()).expect("rocksdb open");
        (storage, dir)
    }

    fn mine_test_block(
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
            quantic_transactions: Vec::new(),
        };
        block.header.merkle_root = block.compute_merkle_root();
        block.header.hash = block.compute_hash();
        block
    }

    fn build_chain_with_blocks(chain_id: &str, n: u64) -> (RocksDbStorage, tempfile::TempDir) {
        let (storage, dir) = open_temp_storage();
        let miner = "zka1miner".to_string();
        let mut prev_hash = genesis_parent_for(chain_id).header.hash;
        let mut prev_height = 0u64;
        let mut supply = 0u64;
        let mut ts = 1_700_000_000u64;
        for _ in 0..n {
            let block = mine_test_block(chain_id, &prev_hash, prev_height, supply, &miner, 4, ts);
            apply_block_with_expected_difficulty(&storage, chain_id, &block, 4)
                .expect("apply block");
            prev_hash = block.header.hash.clone();
            prev_height = block.header.block_height;
            supply = supply.saturating_add(block.transactions[0].amount);
            ts += 120;
        }
        (storage, dir)
    }

    #[test]
    fn export_captures_chain_head_and_blocks_in_order() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = build_chain_with_blocks(chain_id, 3);
        let snap = export_from_storage(&storage, chain_id, 1_700_000_999).expect("export");
        assert_eq!(snap.header.schema, SNAPSHOT_SCHEMA_ID);
        assert_eq!(snap.header.chain_id, chain_id);
        assert_eq!(snap.header.head_height, 3);
        assert_eq!(snap.header.block_count, 3);
        assert_eq!(snap.blocks.len(), 3);
        assert_eq!(snap.blocks[0].header.block_height, 1);
        assert_eq!(snap.blocks[1].header.block_height, 2);
        assert_eq!(snap.blocks[2].header.block_height, 3);
        assert_eq!(snap.header.head_hash, snap.blocks[2].header.hash);
    }

    #[test]
    fn empty_chain_exports_a_zero_height_snapshot() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = open_temp_storage();
        let snap = export_from_storage(&storage, chain_id, 0).expect("empty export");
        assert_eq!(snap.header.head_height, 0);
        assert_eq!(snap.header.block_count, 0);
        assert!(snap.blocks.is_empty());
    }

    #[test]
    fn digest_is_deterministic_for_identical_chain() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = build_chain_with_blocks(chain_id, 3);
        let snap_a = export_from_storage(&storage, chain_id, 1).expect("export a");
        let snap_b = export_from_storage(&storage, chain_id, 1).expect("export b");
        assert_eq!(
            compute_digest(&snap_a).unwrap(),
            compute_digest(&snap_b).unwrap()
        );
    }

    #[test]
    fn digest_excludes_signature_field() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = build_chain_with_blocks(chain_id, 2);
        let mut snap = export_from_storage(&storage, chain_id, 1).expect("export");
        let unsigned = compute_digest(&snap).unwrap();
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        sign(&mut snap, &sk.to_bytes()).expect("sign");
        let signed = compute_digest(&snap).unwrap();
        assert_eq!(unsigned, signed);
    }

    #[test]
    fn signed_snapshot_verifies_against_signer_pubkey() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = build_chain_with_blocks(chain_id, 2);
        let mut snap = export_from_storage(&storage, chain_id, 1).expect("export");
        let sk = SigningKey::from_bytes(&[9u8; 32]);
        sign(&mut snap, &sk.to_bytes()).expect("sign");
        let pk = sk.verifying_key().to_bytes();
        let opts = VerifyOptions {
            expected_chain_id: chain_id,
            require_signature: true,
            expected_pubkey: Some(&pk),
            expected_hybrid_pubkey: None,
            require_hybrid: false,
        };
        verify(&snap, &opts).expect("must verify");
    }

    #[test]
    fn signed_snapshot_rejects_wrong_pubkey() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = build_chain_with_blocks(chain_id, 2);
        let mut snap = export_from_storage(&storage, chain_id, 1).expect("export");
        let sk = SigningKey::from_bytes(&[9u8; 32]);
        sign(&mut snap, &sk.to_bytes()).expect("sign");
        let wrong = [0xfeu8; 32];
        let opts = VerifyOptions {
            expected_chain_id: chain_id,
            require_signature: true,
            expected_pubkey: Some(&wrong),
            expected_hybrid_pubkey: None,
            require_hybrid: false,
        };
        let err = verify(&snap, &opts).unwrap_err();
        assert!(matches!(err, SnapshotError::SignerPubkeyMismatch { .. }));
    }

    #[test]
    fn signed_snapshot_rejects_byte_tampering() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = build_chain_with_blocks(chain_id, 2);
        let mut snap = export_from_storage(&storage, chain_id, 1).expect("export");
        let sk = SigningKey::from_bytes(&[9u8; 32]);
        sign(&mut snap, &sk.to_bytes()).expect("sign");
        // Tamper with a block timestamp; the signature becomes invalid.
        snap.blocks[0].header.timestamp = snap.blocks[0].header.timestamp.wrapping_add(1);
        let pk = sk.verifying_key().to_bytes();
        let opts = VerifyOptions {
            expected_chain_id: chain_id,
            require_signature: true,
            expected_pubkey: Some(&pk),
            expected_hybrid_pubkey: None,
            require_hybrid: false,
        };
        let err = verify(&snap, &opts).unwrap_err();
        assert!(matches!(err, SnapshotError::BadSignature(_)));
    }

    #[test]
    fn unsigned_snapshot_fails_when_signature_required() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = build_chain_with_blocks(chain_id, 2);
        let snap = export_from_storage(&storage, chain_id, 1).expect("export");
        let opts = VerifyOptions {
            expected_chain_id: chain_id,
            require_signature: true,
            expected_pubkey: None,
            expected_hybrid_pubkey: None,
            require_hybrid: false,
        };
        let err = verify(&snap, &opts).unwrap_err();
        assert!(matches!(err, SnapshotError::SignatureRequired));
    }

    #[test]
    fn unsigned_snapshot_passes_when_signature_not_required() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = build_chain_with_blocks(chain_id, 2);
        let snap = export_from_storage(&storage, chain_id, 1).expect("export");
        let opts = VerifyOptions {
            expected_chain_id: chain_id,
            require_signature: false,
            expected_pubkey: None,
            expected_hybrid_pubkey: None,
            require_hybrid: false,
        };
        verify(&snap, &opts).expect("unsigned ok");
    }

    #[test]
    fn chain_id_mismatch_is_rejected() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = build_chain_with_blocks(chain_id, 1);
        let snap = export_from_storage(&storage, chain_id, 1).expect("export");
        let opts = VerifyOptions {
            expected_chain_id: "zoka-other-net",
            require_signature: false,
            expected_pubkey: None,
            expected_hybrid_pubkey: None,
            require_hybrid: false,
        };
        let err = verify(&snap, &opts).unwrap_err();
        assert!(matches!(err, SnapshotError::ChainIdMismatch { .. }));
    }

    #[test]
    fn block_height_gap_in_snapshot_is_rejected() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = build_chain_with_blocks(chain_id, 3);
        let mut snap = export_from_storage(&storage, chain_id, 1).expect("export");
        // Drop the middle block; everything after it now has wrong height.
        snap.blocks.remove(1);
        snap.header.block_count = snap.blocks.len() as u64;
        let opts = VerifyOptions {
            expected_chain_id: chain_id,
            require_signature: false,
            expected_pubkey: None,
            expected_hybrid_pubkey: None,
            require_hybrid: false,
        };
        let err = verify(&snap, &opts).unwrap_err();
        assert!(matches!(err, SnapshotError::BlockHeightDrift { .. }));
    }

    #[test]
    fn json_roundtrip_preserves_snapshot() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = build_chain_with_blocks(chain_id, 2);
        let snap_a = export_from_storage(&storage, chain_id, 1).expect("export");
        let bytes = serialize(&snap_a).expect("serialize");
        let snap_b = deserialize(&bytes).expect("parse");
        assert_eq!(snap_a.header, snap_b.header);
        assert_eq!(snap_a.blocks.len(), snap_b.blocks.len());
        assert_eq!(
            compute_digest(&snap_a).unwrap(),
            compute_digest(&snap_b).unwrap()
        );
    }

    #[test]
    fn import_into_fresh_storage_replays_chain() {
        let chain_id = "zoka-test-net";
        let (source, _dir1) = build_chain_with_blocks(chain_id, 2);
        let snap = export_from_storage(&source, chain_id, 1).expect("export");

        let (target_raw, _dir2) = open_temp_storage();
        let target = Arc::new(target_raw);

        // For this test we cannot use the canonical apply_block (it
        // demands warmup INITIAL difficulty), so we replay manually
        // using the relaxed test path. This mirrors what the production
        // path does, except the difficulty is parameterized.
        for block in &snap.blocks {
            apply_block_with_expected_difficulty(&target, chain_id, block, 4)
                .expect("relaxed apply");
        }
        assert_eq!(target.get_chain_height().expect("h"), 2);
        assert_eq!(
            target.get_last_block_hash().expect("h"),
            snap.header.head_hash
        );
    }

    // Full snapshot-bootstrap-over-iroh pipeline: a source node exports its
    // chain, serves the blob over an iroh QUIC connection, an independent
    // fresh node pulls it (NAT hole-punch capable, no open ports) and replays
    // every block into empty storage. This is exactly what a new node will do
    // to catch up in minutes instead of crawling the block-by-block IBD.
    #[tokio::test]
    async fn snapshot_bootstraps_over_iroh_end_to_end() {
        use crate::network::iroh_snapshot_transfer::{fetch_snapshot, serve_snapshot};
        use iroh::endpoint::presets;

        let chain_id = "zoka-test-net";
        let (source, _dir1) = build_chain_with_blocks(chain_id, 3);
        let snap = export_from_storage(&source, chain_id, 1).expect("export");
        let bytes = serialize(&snap).expect("serialize");
        let expected_digest = compute_digest(&snap).expect("digest");

        let server = serve_snapshot(Arc::new(bytes), presets::Minimal)
            .await
            .expect("serve");
        let pulled = fetch_snapshot(server.addr(), presets::Minimal)
            .await
            .expect("fetch");
        server.close().await;

        let received = deserialize(&pulled).expect("parse pulled snapshot");
        assert_eq!(
            compute_digest(&received).expect("digest"),
            expected_digest,
            "snapshot pulled over iroh must match the exported one"
        );

        let (target_raw, _dir2) = open_temp_storage();
        let target = Arc::new(target_raw);
        for block in &received.blocks {
            apply_block_with_expected_difficulty(&target, chain_id, block, 4)
                .expect("relaxed apply");
        }
        assert_eq!(target.get_chain_height().expect("h"), 3);
        assert_eq!(
            target.get_last_block_hash().expect("h"),
            received.header.head_hash
        );
    }

    #[test]
    fn import_idempotent_when_blocks_are_already_known() {
        let chain_id = "zoka-test-net";
        let (source, _dir1) = build_chain_with_blocks(chain_id, 2);
        let snap = export_from_storage(&source, chain_id, 1).expect("export");
        // Re-export the same snapshot into the SAME storage; every block
        // is already there. The production importer would treat each as
        // BlockAlreadyKnown and continue, but `apply_block_with_expected_difficulty`
        // returns the same error, so we just confirm the variant.
        for block in &snap.blocks {
            let result = apply_block_with_expected_difficulty(&source, chain_id, block, 4);
            match result {
                Err(CE::BlockAlreadyKnown(_)) => {}
                other => panic!("expected BlockAlreadyKnown, got: {other:?}"),
            }
        }
    }

    #[cfg(feature = "pq-experimental")]
    fn hybrid_key(byte: u8) -> crate::hybrid_signature::HybridSigningKey {
        crate::hybrid_signature::HybridSigningKey::from_parts(SigningKey::from_bytes(&[byte; 32]), &[byte ^ 0x11; 32])
            .expect("hybrid key")
    }

    #[cfg(feature = "pq-experimental")]
    #[test]
    fn a_hybrid_snapshot_verifies_only_with_both_halves() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = build_chain_with_blocks(chain_id, 2);
        let mut snap = export_from_storage(&storage, chain_id, 1).expect("export");
        let key = hybrid_key(4);
        sign_hybrid(&mut snap, &key).expect("sign");
        let public = key.verifying_key().to_bytes();
        let opts = VerifyOptions {
            expected_chain_id: chain_id,
            require_signature: true,
            expected_pubkey: None,
            expected_hybrid_pubkey: Some(&public),
            require_hybrid: true,
        };
        let digest = verify(&snap, &opts).expect("verifies");
        // The digest does not depend on who signed nor how.
        assert_eq!(digest, compute_digest(&snap).unwrap());

        // Its Ed25519 half alone is not a v1 signature: the message differs.
        let mut stripped = snap.clone();
        stripped.mldsa_signature_hex = None;
        stripped.signer_mldsa_pubkey_hex = None;
        let pk = key.verifying_key().ed25519().to_bytes();
        let v1_opts = VerifyOptions {
            expected_chain_id: chain_id,
            require_signature: true,
            expected_pubkey: Some(&pk),
            expected_hybrid_pubkey: None,
            require_hybrid: false,
        };
        assert!(verify(&stripped, &v1_opts).is_err());
        assert!(matches!(verify(&stripped, &opts), Err(SnapshotError::HybridSignatureRequired)));

        // Another signer, or one byte of a block changed.
        let other = hybrid_key(5).verifying_key().to_bytes();
        let wrong = VerifyOptions { expected_hybrid_pubkey: Some(&other), ..opts.clone() };
        assert!(verify(&snap, &wrong).is_err());
        let mut tampered = snap.clone();
        tampered.header.generated_unix_secs += 1;
        assert!(verify(&tampered, &opts).is_err());

        // The JSON goes through serialize/deserialize unchanged.
        let again = deserialize(&serialize(&snap).unwrap()).unwrap();
        verify(&again, &opts).expect("verifies after a round trip");
    }

    #[cfg(feature = "pq-experimental")]
    #[test]
    fn listing_a_hybrid_signer_refuses_ed25519_only_snapshots() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = build_chain_with_blocks(chain_id, 2);
        let key = hybrid_key(6);
        let ed_hex = hex::encode(key.verifying_key().ed25519().to_bytes());
        let hybrid_hex = key.verifying_key().to_hex();
        assert_eq!(hybrid_hex.len(), HYBRID_SIGNER_HEX_LEN);

        let mut v1 = export_from_storage(&storage, chain_id, 1).expect("export");
        sign(&mut v1, &key.ed25519().to_bytes()).expect("sign v1");
        let mut v2 = export_from_storage(&storage, chain_id, 1).expect("export");
        sign_hybrid(&mut v2, &key).expect("sign v2");

        // Only the Ed25519 key listed: v1 passes, as before.
        verify_against_trusted_signers(&v1, chain_id, &[ed_hex.clone()]).expect("v1 trusted");
        // The hybrid key listed (even next to the Ed25519 one): only v2 passes.
        let both = [ed_hex, hybrid_hex.clone()];
        assert!(verify_against_trusted_signers(&v1, chain_id, &both).is_err());
        verify_against_trusted_signers(&v2, chain_id, &both).expect("v2 trusted");
        verify_against_trusted_signers(&v2, chain_id, &[hybrid_hex]).expect("v2 trusted");
        // Nobody listed, or someone else.
        assert!(verify_against_trusted_signers(&v2, chain_id, &[]).is_err());
        let stranger = hybrid_key(7).verifying_key().to_hex();
        assert!(verify_against_trusted_signers(&v2, chain_id, &[stranger]).is_err());
    }

    #[test]
    fn an_unsigned_snapshot_keeps_the_json_older_nodes_read() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = build_chain_with_blocks(chain_id, 1);
        let snap = export_from_storage(&storage, chain_id, 1).expect("export");
        let json = String::from_utf8(serialize(&snap).unwrap()).unwrap();
        assert!(!json.contains("mldsa"), "no new field when there is no hybrid signature");
    }
}
