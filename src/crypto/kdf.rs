//! Key Derivation Functions (KDF)
//!
//! Provides secure key derivation using:
//! - Argon2: Memory-hard password hashing
//! - HKDF-like functionality: For expanding secrets

use argon2::{password_hash::SaltString, Algorithm, Argon2, Params, PasswordHasher, Version};
use rand::rngs::OsRng;

const ARGON2_OUTPUT_LEN: u32 = 32; // 256 bits

/// Derive a key from a password using Argon2id
///
/// Argon2id is resistant to both GPU and side-channel attacks.
/// Uses reasonable defaults: memory=64MiB (65536), iterations=2, parallelism=1
pub fn derive_key_from_password(password: &[u8]) -> Result<[u8; 32], Box<dyn std::error::Error>> {
    let salt = SaltString::generate(&mut OsRng);

    // Argon2 parameters
    let params = Params::new(
        65536, // m_cost: 64 MiB of memory
        2,     // t_cost: 2 iterations
        1,     // p_cost: 1 degree of parallelism
        Some(ARGON2_OUTPUT_LEN as usize),
    )
    .map_err(|e| format!("Params error: {}", e))?;

    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);

    let password_hash = argon2
        .hash_password(password, &salt)
        .map_err(|e| format!("Hash error: {}", e))?;
    let hash_str = password_hash.to_string();

    // Extract raw bytes from the hash
    let base64_part = hash_str.split('$').last().ok_or("Invalid hash format")?;

    let decoded = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD_NO_PAD,
        base64_part,
    )
    .map_err(|e| format!("Base64 decode error: {}", e))?;

    if decoded.len() < 32 {
        return Err("Decoded hash too short".into());
    }

    let mut key = [0u8; 32];
    key.copy_from_slice(&decoded[..32]);
    Ok(key)
}

/// Simpler KDF using Blake3 for expanding a seed
///
/// This is useful for deriving multiple keys from a single master secret
pub fn derive_keys_from_seed(seed: &[u8], count: usize) -> Vec<[u8; 32]> {
    let mut keys = Vec::new();

    for i in 0..count {
        let mut hasher = blake3::Hasher::new();
        hasher.update(seed);
        hasher.update(&i.to_le_bytes());

        let hash = hasher.finalize();
        let mut key = [0u8; 32];
        key.copy_from_slice(hash.as_bytes());
        keys.push(key);
    }

    keys
}

/// Derive a single key from a seed and index
pub fn derive_key(seed: &[u8], index: u64) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(seed);
    hasher.update(&index.to_le_bytes());

    let hash = hasher.finalize();
    let mut key = [0u8; 32];
    key.copy_from_slice(hash.as_bytes());
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derive_key_from_password() {
        let password = b"my_secure_password";
        let key1 = derive_key_from_password(password).unwrap();
        let key2 = derive_key_from_password(password).unwrap();

        // Different salt -> different key
        assert_ne!(key1, key2);

        // Correct length
        assert_eq!(key1.len(), 32);
    }

    #[test]
    fn test_derive_keys_from_seed() {
        let seed = b"master_seed";
        let keys = derive_keys_from_seed(seed, 5);

        assert_eq!(keys.len(), 5);

        // All different
        for i in 0..keys.len() {
            for j in (i + 1)..keys.len() {
                assert_ne!(keys[i], keys[j]);
            }
        }
    }

    #[test]
    fn test_derive_key_deterministic() {
        let seed = b"test_seed";
        let index = 42u64;

        let key1 = derive_key(seed, index);
        let key2 = derive_key(seed, index);

        assert_eq!(key1, key2); // Deterministic

        let key3 = derive_key(seed, index + 1);
        assert_ne!(key1, key3); // Different index -> different key
    }

    #[test]
    fn test_key_length() {
        let seed = b"seed";
        let key = derive_key(seed, 0);
        assert_eq!(key.len(), 32);
    }
}
