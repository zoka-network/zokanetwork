//! Cryptographic Hash Functions
//!
//! Provides Blake3 and SHA-256 hashing for:
//! - Blake3: Fast hashing and merkle trees
//! - SHA-256: Compatibility and merkle tree proofs

use serde::{Deserialize, Serialize};
use std::fmt;

/// Blake3 Hash Output (32 bytes)
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Blake3Hash([u8; 32]);

/// SHA-256 Hash Output (32 bytes)
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Sha256Hash([u8; 32]);

impl fmt::Display for Blake3Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

impl fmt::Debug for Blake3Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Blake3({}...)", hex::encode(&self.0[..8]))
    }
}

impl fmt::Display for Sha256Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

impl fmt::Debug for Sha256Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Sha256({}...)", hex::encode(&self.0[..8]))
    }
}

/// Compute Blake3 hash of data
///
/// Blake3 is faster and more suitable for merkle trees and incremental hashing
pub fn blake3_hash(data: &[u8]) -> Blake3Hash {
    let hash = blake3::hash(data);
    Blake3Hash(*hash.as_bytes())
}

/// Compute SHA-256 hash of data
///
/// SHA-256 is standard and provides compatibility
pub fn sha256_hash(data: &[u8]) -> Sha256Hash {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();

    let mut hash = [0u8; 32];
    hash.copy_from_slice(&result);
    Sha256Hash(hash)
}

impl Blake3Hash {
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn to_bytes(&self) -> [u8; 32] {
        self.0
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Blake3Hash(bytes)
    }

    pub fn from_hex(hex_str: &str) -> Result<Self, hex::FromHexError> {
        let bytes = hex::decode(hex_str)?;
        if bytes.len() != 32 {
            return Err(hex::FromHexError::InvalidStringLength);
        }
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&bytes);
        Ok(Blake3Hash(hash))
    }
}

impl Sha256Hash {
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn to_bytes(&self) -> [u8; 32] {
        self.0
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Sha256Hash(bytes)
    }

    pub fn from_hex(hex_str: &str) -> Result<Self, hex::FromHexError> {
        let bytes = hex::decode(hex_str)?;
        if bytes.len() != 32 {
            return Err(hex::FromHexError::InvalidStringLength);
        }
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&bytes);
        Ok(Sha256Hash(hash))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blake3_hash() {
        let data = b"Hello, ZOKA!";
        let hash1 = blake3_hash(data);
        let hash2 = blake3_hash(data);

        assert_eq!(hash1, hash2);
        assert_eq!(hash1.0.len(), 32);
    }

    #[test]
    fn test_sha256_hash() {
        let data = b"Hello, ZOKA!";
        let hash1 = sha256_hash(data);
        let hash2 = sha256_hash(data);

        assert_eq!(hash1, hash2);
        assert_eq!(hash1.0.len(), 32);
    }

    #[test]
    fn test_different_inputs_different_hashes() {
        let data1 = b"Message 1";
        let data2 = b"Message 2";

        assert_ne!(blake3_hash(data1), blake3_hash(data2));
        assert_ne!(sha256_hash(data1), sha256_hash(data2));
    }

    #[test]
    fn test_hash_serialization() {
        let hash = blake3_hash(b"test");
        let json = serde_json::to_string(&hash).unwrap();
        let deserialized: Blake3Hash = serde_json::from_str(&json).unwrap();

        assert_eq!(hash, deserialized);
    }

    #[test]
    fn test_blake3_known_vector() {
        // Known test vector for Blake3
        let data = b"";
        let hash = blake3_hash(data);
        let expected = "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262";
        assert_eq!(hash.to_string(), expected);
    }
}
