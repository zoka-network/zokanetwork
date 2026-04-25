//! Pruebas de integraciÃ³n para NIVEL 3: Transacciones Privadas
//!
//! Demuestra que:
//! - Commitments se crean y verifican correctamente
//! - Nullifiers previenen doble gasto
//! - Transacciones privadas se pueden validar

#[cfg(test)]
mod nivel3_integration_tests {
    use zoka::transaction::{Commitment, Nullifier, PrivateTransaction, TransactionValidator};

    #[test]
    fn test_commitment_creation_and_verification() {
        // Crear commitments
        let input_data = b"secret_input_value";
        let output_data = b"secret_output_value";

        let input_commit = Commitment::new(input_data.to_vec());
        let output_commit = Commitment::new(output_data.to_vec());

        // Verificar que son diferentes
        assert_ne!(
            input_commit.as_bytes(),
            output_commit.as_bytes(),
            "Commitments de diferentes valores deben ser diferentes"
        );
    }

    #[test]
    fn test_blinded_commitment_hiding() {
        let data = b"secret_amount_100";
        let (commitment, blindness) = Commitment::new_random(data);

        // Verificar que el commitment se puede abrir con el blindness
        assert!(
            commitment.verify(data, &blindness),
            "Commitment debe verificarse correctamente"
        );

        // Verificar que falla sin el blindness correcto
        let wrong_blindness = vec![0u8; 32];
        assert!(
            !commitment.verify(data, &wrong_blindness),
            "Commitment no debe verificarse con blindness incorrecto"
        );
    }

    #[test]
    fn test_nullifier_prevents_double_spend() {
        let mut nullifier_set = zoka::transaction::nullifier::NullifierSet::new();
        let nf = Nullifier::new(vec![1, 2, 3, 4, 5]);

        // Primer gasto debe ser exitoso
        assert!(
            nullifier_set.spend(nf.clone()).is_ok(),
            "El primer gasto debe ser exitoso"
        );

        // Segundo gasto del mismo nullifier debe fallar
        assert!(
            nullifier_set.spend(nf).is_err(),
            "El segundo gasto debe fallar (doble gasto)"
        );
    }

    #[test]
    fn test_private_transaction_creation() {
        let input_commit = Commitment::new(b"input_value".to_vec());
        let output_commit = Commitment::new(b"output_value".to_vec());
        let encrypted_data = vec![1, 2, 3, 4, 5];
        let signature = b"signature_bytes".to_vec();
        let nonce = 42u64;

        let tx = PrivateTransaction::new(
            input_commit,
            output_commit,
            encrypted_data,
            signature,
            nonce,
        );

        // Verificaciones bÃ¡sicas
        assert_eq!(tx.nonce, nonce, "Nonce debe coincidir");
        assert_eq!(tx.nullifiers.len(), 0, "Sin nullifiers al crear");
    }

    #[test]
    fn test_transaction_with_nullifiers() {
        let input_commit = Commitment::new(b"input".to_vec());
        let output_commit = Commitment::new(b"output".to_vec());

        let mut tx = PrivateTransaction::new(
            input_commit,
            output_commit,
            vec![1, 2, 3],
            b"sig".to_vec(),
            1u64,
        );

        // Agregar nullifiers
        let nf1 = Nullifier::new(vec![1, 1, 1]);
        let nf2 = Nullifier::new(vec![2, 2, 2]);

        tx.add_nullifier(nf1);
        tx.add_nullifier(nf2);

        assert_eq!(tx.nullifiers.len(), 2, "Debe haber dos nullifiers");
    }

    #[test]
    fn test_transaction_validation_full_cycle() {
        let mut validator = TransactionValidator::new();

        let input_commit = Commitment::new(b"input".to_vec());
        let output_commit = Commitment::new(b"output".to_vec());

        let mut tx = PrivateTransaction::new(
            input_commit,
            output_commit,
            vec![1, 2, 3],
            b"sig".to_vec(),
            1u64,
        );

        // Agregar nullifier
        let nf = Nullifier::new(vec![1, 1, 1]);
        tx.add_nullifier(nf);

        // Primera validaciÃ³n debe pasar
        assert!(
            validator.validate(&tx).is_ok(),
            "Primera validaciÃ³n debe pasar"
        );

        // Segunda validaciÃ³n con el mismo nullifier debe fallar
        assert!(
            validator.validate(&tx).is_err(),
            "Segunda validaciÃ³n debe fallar (doble gasto)"
        );
    }

    #[test]
    fn test_commitment_hex_serialization() {
        let commit = Commitment::new(b"test_data".to_vec());
        let hex = commit.to_hex();

        assert_eq!(hex.len(), 64, "Hex debe ser 64 caracteres (32 bytes)");

        // Verificar que se puede parsear de vuelta
        let commit2 = Commitment::from_hex(&hex).expect("Debe parsear desde hex");
        assert_eq!(commit, commit2, "Debe recuperar el commitment original");
    }

    #[test]
    fn test_multiple_transactions_different_nullifiers() {
        let mut validator = TransactionValidator::new();

        // TransacciÃ³n 1
        let tx1_input = Commitment::new(b"tx1_input".to_vec());
        let tx1_output = Commitment::new(b"tx1_output".to_vec());

        let mut tx1 =
            PrivateTransaction::new(tx1_input, tx1_output, vec![1, 2, 3], b"sig1".to_vec(), 1u64);
        tx1.add_nullifier(Nullifier::new(vec![1, 1, 1]));

        // TransacciÃ³n 2 con diferente nullifier
        let tx2_input = Commitment::new(b"tx2_input".to_vec());
        let tx2_output = Commitment::new(b"tx2_output".to_vec());

        let mut tx2 =
            PrivateTransaction::new(tx2_input, tx2_output, vec![4, 5, 6], b"sig2".to_vec(), 2u64);
        tx2.add_nullifier(Nullifier::new(vec![2, 2, 2]));

        // Ambas deben validarse correctamente
        assert!(validator.validate(&tx1).is_ok(), "TX1 debe validarse");
        assert!(validator.validate(&tx2).is_ok(), "TX2 debe validarse");
    }
}
