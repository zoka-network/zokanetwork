//! Cryptographic primitives for ZOKA blockchain
//!
//! Provides:
//! - Ed25519 digital signatures
//! - Blake3 & SHA-256 hashing
//! - Key derivation functions (Argon2)

pub mod hashing;
pub mod kdf;
pub mod signatures;

pub use hashing::{blake3_hash, sha256_hash};
pub use kdf::derive_key;
pub use signatures::{sign, verify, KeyPair, PublicKey, Signature};
