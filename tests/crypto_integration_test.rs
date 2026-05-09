// Integration tests for crypto module
#[cfg(test)]
mod crypto_tests {
    use zoka::crypto::{hashing, kdf, signatures};

    #[test]
    fn test_full_crypto_workflow() {
        // 1. Generate keypair
        let keypair = signatures::generate_keypair();
        println!("Generated keypair: {}", keypair.public);

        // 2. Sign a transaction
        let tx = b"Transfer 100 ZK from Alice to Bob";
        let signature = signatures::sign(&keypair.secret, tx);
        println!("Signature created: {}", signature);

        // 3. Verify signature
        assert!(signatures::verify(&keypair.public, tx, &signature).is_ok());

        // 4. Verify with wrong message fails
        let wrong_tx = b"Transfer 1000 ZK from Alice to Bob";
        assert!(signatures::verify(&keypair.public, wrong_tx, &signature).is_err());
    }

    #[test]
    fn test_blake3_hashing() {
        let data = b"ZOKA blockchain transaction";
        let hash = hashing::blake3_hash(data);

        // Deterministic
        let hash2 = hashing::blake3_hash(data);
        assert_eq!(hash, hash2);

        println!("Blake3 hash: {}", hash);
    }

    #[test]
    fn test_sha256_hashing() {
        let data = b"ZOKA blockchain block";
        let hash = hashing::sha256_hash(data);

        // Deterministic
        let hash2 = hashing::sha256_hash(data);
        assert_eq!(hash, hash2);

        println!("SHA256 hash: {}", hash);
    }

    #[test]
    fn test_key_derivation() {
        let seed = b"master_secret";

        // Derive multiple keys
        let keys = kdf::derive_keys_from_seed(seed, 5);
        assert_eq!(keys.len(), 5);

        // All different
        for i in 0..keys.len() {
            for j in (i + 1)..keys.len() {
                assert_ne!(keys[i], keys[j]);
            }
        }

        // Deterministic
        let key1 = kdf::derive_key(seed, 0);
        let key2 = kdf::derive_key(seed, 0);
        assert_eq!(key1, key2);

        println!("Key derivation successful: {} unique keys", keys.len());
    }

    #[test]
    fn test_digital_signature_chain() {
        // Simulate signing multiple messages in a block
        let keypair = signatures::generate_keypair();

        let messages = vec![
            b"Transaction 1".to_vec(),
            b"Transaction 2".to_vec(),
            b"Transaction 3".to_vec(),
        ];

        let mut signatures = Vec::new();
        for msg in &messages {
            let sig = signatures::sign(&keypair.secret, msg);
            signatures.push(sig);
        }

        // Verify all signatures
        for (msg, sig) in messages.iter().zip(signatures.iter()) {
            assert!(signatures::verify(&keypair.public, msg, sig).is_ok());
        }

        println!("Signed and verified {} transactions", messages.len());
    }
}
