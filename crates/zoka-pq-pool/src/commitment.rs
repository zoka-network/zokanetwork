//! What a STARK block header commits to, computed as a node computes it (the
//! node's `quantic::stark::state_root` and `Block::compute_hash`, which a test
//! there keeps equal to these). A light wallet rebuilds the ledger from pages
//! of outputs and nullifiers; with these it checks every page against its
//! block's header, and the chain of headers against a checkpoint the team
//! signs with ML-DSA-65. A server that drops or forges an output then breaks
//! the note-tree root the header commits to.
use crate::crypto::Digest;
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use std::collections::BTreeSet;

/// Prefix of a v2 block's `state_root`.
pub const STATE_PREFIX: &str = "quantic-v2:";

/// The ledger state a block header commits to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Committed {
    pub network: u8,
    pub height: u32,
    pub issued: u64,
    pub notes: u64,
    pub root: Digest,
    /// [`accumulate`] over every block's nullifiers, from `[0; 32]`.
    pub nullifiers: [u8; 32],
}

/// The header's `state_root` for `state`.
pub fn state_root(state: &Committed) -> String {
    let bytes = postcard::to_allocvec(state).expect("a committed state always encodes");
    let mut hash = blake3::Hasher::new_derive_key("ZOKA QUANTIC STARK state v3");
    hash.update(&bytes);
    let mut digest = [0; 64];
    hash.finalize_xof().fill(&mut digest);
    let mut text = String::with_capacity(STATE_PREFIX.len() + 128);
    text.push_str(STATE_PREFIX);
    for byte in digest {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

/// `H(previous ‖ this block's nullifiers, sorted)`: binds the whole spent set.
pub fn accumulate(previous: &[u8; 32], spent: &BTreeSet<Digest>) -> [u8; 32] {
    let mut hash = blake3::Hasher::new_derive_key("ZOKA QUANTIC STARK nullifiers v1");
    hash.update(previous);
    for nullifier in spent {
        for word in nullifier.0 {
            hash.update(&word.to_le_bytes());
        }
    }
    *hash.finalize().as_bytes()
}

/// A block's hash from its header fields: SHA-256 of
/// `prev:merkle_root:state_root:timestamp:nonce:height`, in hex.
pub fn block_hash(
    prev_hash: &str,
    merkle_root: &str,
    state_root: &str,
    timestamp: u64,
    nonce: u64,
    height: u64,
) -> String {
    let data = format!("{prev_hash}:{merkle_root}:{state_root}:{timestamp}:{nonce}:{height}");
    Sha256::digest(data.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
