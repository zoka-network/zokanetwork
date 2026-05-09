//! Cryptographic primitives for ZOKA blockchain
//!
//! Provides:
//! - Ed25519 digital signatures
//! - Blake3 & SHA-256 hashing
//! - Key derivation functions (Argon2)

pub mod hashing;
pub mod kdf;
pub mod private_tree;
pub mod signatures;

pub use hashing::{blake3_hash, sha256_hash};
pub use kdf::derive_key;
pub use private_tree::{
    commitment_words as private_tree_commitment_words,
    hash_commitment as private_tree_hash_commitment,
    hash_commitment_words as private_tree_hash_commitment_words,
    hash_pair as private_tree_hash_pair, round_constants as private_tree_round_constants,
    scalar_from_hex as private_tree_scalar_from_hex,
    scalar_from_words as private_tree_scalar_from_words,
    scalar_to_hex as private_tree_scalar_to_hex, scalar_to_words as private_tree_scalar_to_words,
    PRIVATE_TREE_DEPTH, PRIVATE_TREE_SCALAR_WORDS,
};
pub use signatures::{sign, verify, KeyPair, PublicKey, Signature};
