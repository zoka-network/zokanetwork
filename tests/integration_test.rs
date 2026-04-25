// Integration tests for ZOKA blockchain
#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;
    use zoka::consensus::PoAConsensus;
    use zoka::identity;
    use zoka::mempool::Mempool;
    use zoka::state::StateManager;
    use zoka::storage::RocksDbStorage;
    use zoka::types::*;

    #[tokio::test]
    async fn test_full_blockchain_lifecycle() {
        let test_dir = ".test_blockchain_full";
        let _ = fs::remove_dir_all(test_dir);

        // 1. Initialize storage
        let storage = Arc::new(RocksDbStorage::new(test_dir).expect("Failed to init storage"));

        // 2. Initialize state manager
        let state_mgr = Arc::new(StateManager::new(storage.clone()));
        state_mgr
            .load_from_storage()
            .await
            .expect("Failed to load state");

        // 3. Create genesis accounts
        state_mgr
            .create_account("alice".to_string(), 5000)
            .await
            .unwrap();
        state_mgr
            .create_account("bob".to_string(), 0)
            .await
            .unwrap();

        // 4. Initialize mempool
        let mempool = Arc::new(Mempool::new(1000));

        // 5. Create a transaction
        let tx = Transaction::new(
            "alice".to_string(),
            "bob".to_string(),
            100,
            1,
            1000,
            Signature(vec![]),
        );

        let state = state_mgr.get_state().await;
        let result = mempool.add_transaction(tx.clone(), &state).await;
        assert!(result.is_ok(), "Failed to add transaction to mempool");

        // 6. Verify transaction in mempool
        assert_eq!(mempool.len().await, 1, "Transaction not in mempool");

        // 7. Create a block with the transaction
        let draft_block = Block::new(
            "0".to_string(),
            1000,
            0,
            1,
            vec![tx],
            PublicKey(vec![1, 2, 3]),
            Signature(vec![]),
        );
        let expected_state_root = state_mgr
            .preview_state_root_for_block(&draft_block)
            .await
            .expect("Failed to preview state root");
        let block = draft_block.with_state_root(expected_state_root.clone());

        // 8. Validate block
        let consensus = PoAConsensus::new();
        let validation_result = consensus.validate_block(&block);
        assert!(validation_result.is_ok(), "Block validation failed");

        // 9. Apply block to state
        let apply_result = state_mgr.apply_block(&block).await;
        assert!(apply_result.is_ok(), "Failed to apply block");

        // 10. Verify state changes
        let alice_state = state_mgr.get_account("alice").await.unwrap();
        let bob_state = state_mgr.get_account("bob").await.unwrap();

        assert_eq!(alice_state.balance, 4900, "Alice balance incorrect");
        assert_eq!(bob_state.balance, 100, "Bob balance incorrect");
        assert_eq!(
            state_mgr.get_state_root().await,
            expected_state_root,
            "State root mismatch after block apply"
        );

        println!("âœ“ Full blockchain lifecycle test passed!");

        let _ = fs::remove_dir_all(test_dir);
    }

    #[tokio::test]
    async fn test_transaction_validation() {
        let test_dir = ".test_tx_validation";
        let _ = fs::remove_dir_all(test_dir);

        let storage = Arc::new(RocksDbStorage::new(test_dir).unwrap());
        let state_mgr = Arc::new(StateManager::new(storage));
        state_mgr
            .create_account("alice".to_string(), 1000)
            .await
            .unwrap();

        let mempool = Mempool::new(100);
        let state = state_mgr.get_state().await;

        // Test 1: Valid transaction
        let valid_tx = Transaction {
            hash: "valid".to_string(),
            sender: "alice".to_string(),
            receiver: "bob".to_string(),
            amount: 100,
            nonce: 1,
            timestamp: 0,
            signature: Signature(vec![]),
            zk_proof: None,
        };

        assert!(mempool.add_transaction(valid_tx, &state).await.is_ok());

        // Test 2: Invalid nonce (too low)
        let state = state_mgr.get_state().await;
        let invalid_nonce_tx = Transaction {
            hash: "invalid_nonce".to_string(),
            sender: "alice".to_string(),
            receiver: "bob".to_string(),
            amount: 100,
            nonce: 0, // Should be > 0
            timestamp: 0,
            signature: Signature(vec![]),
            zk_proof: None,
        };

        let result = mempool.add_transaction(invalid_nonce_tx, &state).await;
        assert!(result.is_err(), "Invalid nonce should be rejected");

        println!("âœ“ Transaction validation test passed!");

        let _ = fs::remove_dir_all(test_dir);
    }

    #[test]
    fn test_block_validation_rules() {
        let consensus = PoAConsensus::new();

        // Test 1: Valid block
        let valid_block = Block::new(
            "prev".to_string(),
            1000,
            0,
            1,
            vec![],
            PublicKey(vec![1]),
            Signature(vec![]),
        );

        assert!(consensus.validate_block(&valid_block).is_ok());

        // Test 2: Empty hash (invalid)
        let mut invalid_block = Block::new(
            "prev".to_string(),
            1000,
            0,
            1,
            vec![],
            PublicKey(vec![1]),
            Signature(vec![]),
        );
        invalid_block.header.hash = String::new();

        assert!(consensus.validate_block(&invalid_block).is_err());

        println!("âœ“ Block validation test passed!");
    }

    #[test]
    fn test_identity_operations() {
        let (pub_key1, priv_key1) = identity::generate_keypair().unwrap();
        let (pub_key2, _) = identity::generate_keypair().unwrap();

        assert_ne!(pub_key1, pub_key2, "Keys should be different");

        // Test signing
        let message = b"test message";
        let signature = identity::sign_message(message, &priv_key1).unwrap();
        assert!(!signature.0.is_empty(), "Signature should not be empty");

        // Test verification
        let verified = identity::verify_signature(&signature, message, &pub_key1);
        assert!(verified, "Signature verification failed");

        println!("âœ“ Identity operations test passed!");
    }

    #[tokio::test]
    async fn test_storage_persistence() {
        let test_dir = ".test_storage_persist";
        let _ = fs::remove_dir_all(test_dir);

        // Write data
        {
            let storage = RocksDbStorage::new(test_dir).unwrap();
            let account = AccountState {
                address: "alice".to_string(),
                balance: 1000,
                nonce: 5,
                last_updated_height: 10,
            };

            storage.save_account_state("alice", &account).unwrap();
            storage.set_chain_height(42).unwrap();
            storage.set_state_root("state-root-42").unwrap();
        }

        // Read data back
        {
            let storage = RocksDbStorage::new(test_dir).unwrap();
            let account = storage.get_account_state("alice").unwrap().unwrap();
            assert_eq!(account.balance, 1000);
            assert_eq!(account.nonce, 5);

            let height = storage.get_chain_height().unwrap();
            assert_eq!(height, 42);

            let state_root = storage.get_state_root().unwrap();
            assert_eq!(state_root, "state-root-42");
        }

        println!("âœ“ Storage persistence test passed!");

        let _ = fs::remove_dir_all(test_dir);
    }

    #[test]
    fn test_types_serde() {
        let tx = Transaction {
            hash: "abc123".to_string(),
            sender: "alice".to_string(),
            receiver: "bob".to_string(),
            amount: 100,
            nonce: 1,
            timestamp: 0,
            signature: Signature(vec![1, 2, 3]),
            zk_proof: None,
        };

        // Serialize
        let json = serde_json::to_string(&tx).unwrap();
        assert!(!json.is_empty());

        // Deserialize
        let deserialized: Transaction = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.hash, tx.hash);
        assert_eq!(deserialized.amount, tx.amount);

        println!("âœ“ Types serialization test passed!");
    }
}
