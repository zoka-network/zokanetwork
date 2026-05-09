//! Integration tests for NIVEL 3 cryptographic primitives.
//!
//! These are the low-level building blocks used by `private_wallet::PrivateTransaction`:
//! - Pedersen commitments (hide + bind)
//! - Nullifiers (prevent double-spend)
//!
//! The legacy `transaction::PrivateTransaction` and `TransactionValidator`
//! that used to live alongside these tests were removed together with
//! `zk_proof` on the public `Transaction` type. End-to-end private tx
//! behaviour is exercised by `private_wallet` unit tests and by the ZK
//! property tests in `zk::verifier::tests`.

#[cfg(test)]
mod nivel3_integration_tests {
    use zoka::transaction::{Commitment, Nullifier};

    #[test]
    fn test_commitment_creation_and_verification() {
        let input_data = b"secret_input_value";
        let output_data = b"secret_output_value";

        let input_commit = Commitment::new(input_data.to_vec());
        let output_commit = Commitment::new(output_data.to_vec());

        assert_ne!(
            input_commit.as_bytes(),
            output_commit.as_bytes(),
            "Commitments for different values must differ"
        );
    }

    #[test]
    fn test_blinded_commitment_hiding() {
        let data = b"secret_amount_100";
        let (commitment, blindness) = Commitment::new_random(data);

        assert!(
            commitment.verify(data, &blindness),
            "commitment must open with the correct blindness"
        );

        let wrong_blindness = vec![0u8; 32];
        assert!(
            !commitment.verify(data, &wrong_blindness),
            "commitment must NOT open with a different blindness"
        );
    }

    #[test]
    fn test_nullifier_prevents_double_spend() {
        let mut nullifier_set = zoka::transaction::nullifier::NullifierSet::new();
        let nf = Nullifier::new(vec![1, 2, 3, 4, 5]);

        assert!(
            nullifier_set.spend(nf.clone()).is_ok(),
            "first spend must succeed"
        );
        assert!(
            nullifier_set.spend(nf).is_err(),
            "second spend of the same nullifier must fail (double-spend)"
        );
    }

    #[test]
    fn test_commitment_hex_serialization() {
        let commit = Commitment::new(b"test_data".to_vec());
        let hex = commit.to_hex();

        assert_eq!(hex.len(), 64, "hex encoding must be 64 chars (32 bytes)");

        let commit2 = Commitment::from_hex(&hex).expect("must parse from hex");
        assert_eq!(
            commit, commit2,
            "round-trip must reproduce the original commitment"
        );
    }

    #[test]
    fn test_multiple_nullifiers_are_distinguished() {
        let mut nullifier_set = zoka::transaction::nullifier::NullifierSet::new();
        let nf1 = Nullifier::new(vec![1, 1, 1]);
        let nf2 = Nullifier::new(vec![2, 2, 2]);

        assert!(nullifier_set.spend(nf1).is_ok());
        assert!(
            nullifier_set.spend(nf2).is_ok(),
            "a distinct nullifier must not conflict with a previously spent one"
        );
    }
}
