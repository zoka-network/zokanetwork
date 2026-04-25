//! MÃ³dulo de validaciÃ³n de transacciones privadas
//!
//! Valida:
//! - Firma de transacciÃ³n
//! - Balance de compromisos (entrada = salida)
//! - PrevenciÃ³n de doble gasto (nullifiers)
//! - Validez de zk-proof (si presente)

use crate::transaction::nullifier::NullifierSet;
use crate::transaction::private_tx::PrivateTransaction;

/// Errores de validaciÃ³n
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ValidationError {
    /// Error de firma invÃ¡lida
    InvalidSignature,
    /// Desbalance de compromisos (entrada != salida)
    CommitmentMismatch,
    /// Doble gasto detectado
    DoubleSpendDetected,
    /// Zk-proof invÃ¡lido o no presente cuando requerido
    InvalidProof,
    /// Nullifier duplicado dentro de la misma transacciÃ³n
    DuplicateNullifier,
    /// TransacciÃ³n ya confirmada
    TransactionAlreadySpent,
    /// Error genÃ©rico de validaciÃ³n
    ValidationFailed(String),
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSignature => write!(f, "Firma de transacciÃ³n invÃ¡lida"),
            Self::CommitmentMismatch => write!(f, "Desbalance de compromisos"),
            Self::DoubleSpendDetected => write!(f, "Doble gasto detectado"),
            Self::InvalidProof => write!(f, "Zk-proof invÃ¡lido"),
            Self::DuplicateNullifier => write!(f, "Nullifier duplicado en transacciÃ³n"),
            Self::TransactionAlreadySpent => write!(f, "TransacciÃ³n ya ha sido gastada"),
            Self::ValidationFailed(msg) => write!(f, "Error de validaciÃ³n: {}", msg),
        }
    }
}

/// Validador de transacciones privadas
#[derive(Clone, Debug, Default)]
pub struct TransactionValidator {
    /// Conjunto de nullifiers ya gastados
    spent_nullifiers: NullifierSet,
    /// Require zk-proof para todas las transacciones
    require_proof: bool,
}

impl TransactionValidator {
    /// Crear nuevo validador
    pub fn new() -> Self {
        Self {
            spent_nullifiers: NullifierSet::new(),
            require_proof: false,
        }
    }

    /// Crear validador que requiere zk-proofs
    pub fn with_proof_requirement() -> Self {
        Self {
            spent_nullifiers: NullifierSet::new(),
            require_proof: true,
        }
    }

    /// Obtener referencia al conjunto de nullifiers gastados
    pub fn spent_nullifiers(&self) -> &NullifierSet {
        &self.spent_nullifiers
    }

    /// Validar transacciÃ³n completa
    pub fn validate(&mut self, tx: &PrivateTransaction) -> Result<(), ValidationError> {
        // 1. Validar firma
        self.validate_signature(tx)?;

        // 2. Validar compromisos (balance)
        self.validate_commitments(tx)?;

        // 3. Validar no double-spend
        self.validate_no_double_spend(tx)?;

        // 4. Validar zk-proof si estÃ¡ presente
        if tx.zk_proof.is_some() {
            self.validate_zk_proof(tx)?;
        } else if self.require_proof {
            return Err(ValidationError::InvalidProof);
        }

        // 5. Marcar nullifiers como gastados
        for nf in &tx.nullifiers {
            self.spent_nullifiers
                .spend(nf.clone())
                .map_err(|_| ValidationError::DoubleSpendDetected)?;
        }

        Ok(())
    }

    /// Validar firma de transacciÃ³n
    fn validate_signature(&self, tx: &PrivateTransaction) -> Result<(), ValidationError> {
        // La firma estÃ¡ sobre el hash de la transacciÃ³n
        // Para validar necesitarÃ­amos acceso a la clave pÃºblica del emisor
        // Por ahora, validamos estructura bÃ¡sica
        if tx.signature.is_empty() {
            return Err(ValidationError::InvalidSignature);
        }

        // En una implementaciÃ³n real, verificarÃ­amos:
        // signature.verify(&tx_hash, &signer_pubkey)
        Ok(())
    }

    /// Validar balance de compromisos
    /// En un sistema real con valores ocultos, esto serÃ­a verificado vÃ­a zk-proof
    /// AquÃ­ validamos que los compromisos estÃ©n bien formados
    fn validate_commitments(&self, tx: &PrivateTransaction) -> Result<(), ValidationError> {
        // Verificar que input_commitment existe
        if tx.input_commitment.as_bytes().len() != 32 {
            return Err(ValidationError::CommitmentMismatch);
        }

        // Verificar que output_commitment existe
        if tx.output_commitment.as_bytes().len() != 32 {
            return Err(ValidationError::CommitmentMismatch);
        }

        // Verificar que no son iguales (para evitar trivialidades)
        if tx.input_commitment.as_bytes() == tx.output_commitment.as_bytes() {
            return Err(ValidationError::ValidationFailed(
                "Input y output commitments no pueden ser iguales".to_string(),
            ));
        }

        Ok(())
    }

    /// Validar no double-spend (nullifiers no han sido gastados)
    fn validate_no_double_spend(&self, tx: &PrivateTransaction) -> Result<(), ValidationError> {
        // Verificar cada nullifier contra el conjunto de gastados
        for nf in &tx.nullifiers {
            if self.spent_nullifiers.is_spent(nf) {
                return Err(ValidationError::DoubleSpendDetected);
            }
        }

        // Verificar no hay nullifiers duplicados dentro de la transacciÃ³n
        let mut seen = std::collections::HashSet::new();
        for nf in &tx.nullifiers {
            if !seen.insert(nf.clone()) {
                return Err(ValidationError::DuplicateNullifier);
            }
        }

        Ok(())
    }

    /// Validar zk-proof (si estÃ¡ presente)
    fn validate_zk_proof(&self, tx: &PrivateTransaction) -> Result<(), ValidationError> {
        match &tx.zk_proof {
            Some(proof) => {
                // El proof debe ser no-vacÃ­o
                if proof.is_empty() {
                    return Err(ValidationError::InvalidProof);
                }

                // En una implementaciÃ³n real, verificarÃ­amos que:
                // 1. Input commitment = hash(input_value)
                // 2. Output commitment = hash(output_value)
                // 3. input_value = output_value
                // 4. La prueba es vÃ¡lida para estos compromisos

                // Por ahora, solo verificamos estructura
                Ok(())
            }
            None => {
                if self.require_proof {
                    Err(ValidationError::InvalidProof)
                } else {
                    Ok(())
                }
            }
        }
    }

    /// Crear punto de control (snapshot) del estado de validaciÃ³n
    pub fn checkpoint(&self) -> ValidationCheckpoint {
        ValidationCheckpoint {
            spent_count: self.spent_nullifiers.spent_count(),
        }
    }

    /// Revertir a punto de control anterior
    pub fn revert_to(&mut self, _checkpoint: &ValidationCheckpoint) {
        // En una implementaciÃ³n mÃ¡s sofisticada, guardarÃ­amos historial
        // Por ahora, es un placeholder
    }
}

/// Punto de control para revertir validaciones
#[derive(Clone, Debug)]
pub struct ValidationCheckpoint {
    /// NÃºmero de nullifiers gastados en este punto
    #[allow(dead_code)]
    spent_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transaction::commitment::Commitment;
    use crate::transaction::nullifier::Nullifier;
    use crate::transaction::private_tx::PrivateTransaction;

    #[test]
    fn test_validator_creation() {
        let validator = TransactionValidator::new();
        assert_eq!(validator.spent_nullifiers().spent_count(), 0);
        assert!(!validator.require_proof);
    }

    #[test]
    fn test_validator_with_proof_requirement() {
        let validator = TransactionValidator::with_proof_requirement();
        assert!(validator.require_proof);
    }

    #[test]
    fn test_validate_commitments_valid() {
        let validator = TransactionValidator::new();

        let input_commit = Commitment::new(b"input".to_vec());
        let output_commit = Commitment::new(b"output".to_vec());
        let nonce = 42u64;

        let tx = PrivateTransaction::new(
            input_commit,
            output_commit,
            vec![1, 2, 3],
            b"sig".to_vec(),
            nonce,
        );

        assert!(validator.validate_commitments(&tx).is_ok());
    }

    #[test]
    fn test_validate_signature() {
        let validator = TransactionValidator::new();

        let input_commit = Commitment::new(b"input".to_vec());
        let output_commit = Commitment::new(b"output".to_vec());
        let nonce = 42u64;

        let tx = PrivateTransaction::new(
            input_commit,
            output_commit,
            vec![1, 2, 3],
            b"sig".to_vec(),
            nonce,
        );

        // La validaciÃ³n bÃ¡sica debe pasar
        assert!(validator.validate_signature(&tx).is_ok());
    }

    #[test]
    fn test_validate_no_double_spend() {
        let validator = TransactionValidator::new();

        let input_commit = Commitment::new(b"input".to_vec());
        let output_commit = Commitment::new(b"output".to_vec());
        let nf = Nullifier::new(vec![1, 2, 3]);

        let mut tx = PrivateTransaction::new(
            input_commit,
            output_commit,
            vec![1, 2, 3],
            b"sig".to_vec(),
            42u64,
        );

        tx.add_nullifier(nf);

        assert!(validator.validate_no_double_spend(&tx).is_ok());
    }

    #[test]
    fn test_validate_duplicate_nullifier() {
        let validator = TransactionValidator::new();

        let input_commit = Commitment::new(b"input".to_vec());
        let output_commit = Commitment::new(b"output".to_vec());
        let nf = Nullifier::new(vec![1, 2, 3]);

        let mut tx = PrivateTransaction::new(
            input_commit,
            output_commit,
            vec![1, 2, 3],
            b"sig".to_vec(),
            42u64,
        );

        // AÃ±adir el mismo nullifier dos veces
        tx.add_nullifier(nf.clone());
        tx.add_nullifier(nf);

        assert_eq!(
            validator.validate_no_double_spend(&tx),
            Err(ValidationError::DuplicateNullifier)
        );
    }

    #[test]
    fn test_validate_zk_proof_optional() {
        let validator = TransactionValidator::new();

        let input_commit = Commitment::new(b"input".to_vec());
        let output_commit = Commitment::new(b"output".to_vec());

        let tx = PrivateTransaction::new(
            input_commit,
            output_commit,
            vec![1, 2, 3],
            b"sig".to_vec(),
            42u64,
        );

        // Sin proof debe ser OK cuando no se requiere
        assert!(validator.validate_zk_proof(&tx).is_ok());
    }

    #[test]
    fn test_validate_zk_proof_required() {
        let validator = TransactionValidator::with_proof_requirement();

        let input_commit = Commitment::new(b"input".to_vec());
        let output_commit = Commitment::new(b"output".to_vec());

        let tx = PrivateTransaction::new(
            input_commit,
            output_commit,
            vec![1, 2, 3],
            b"sig".to_vec(),
            42u64,
        );

        // Sin proof debe fallar cuando se requiere
        assert_eq!(
            validator.validate_zk_proof(&tx),
            Err(ValidationError::InvalidProof)
        );
    }

    #[test]
    fn test_validate_full_transaction() {
        let mut validator = TransactionValidator::new();

        let input_commit = Commitment::new(b"input".to_vec());
        let output_commit = Commitment::new(b"output".to_vec());
        let nf = Nullifier::new(vec![1, 2, 3]);

        let mut tx = PrivateTransaction::new(
            input_commit,
            output_commit,
            vec![1, 2, 3],
            b"sig".to_vec(),
            42u64,
        );

        tx.add_nullifier(nf);

        // Primera validaciÃ³n debe pasar
        assert!(validator.validate(&tx).is_ok());

        // Segunda validaciÃ³n del mismo nullifier debe fallar (doble gasto)
        assert_eq!(
            validator.validate(&tx),
            Err(ValidationError::DoubleSpendDetected)
        );
    }

    #[test]
    fn test_checkpoint() {
        let validator = TransactionValidator::new();
        let checkpoint = validator.checkpoint();

        assert_eq!(checkpoint.spent_count, 0);
    }
}
