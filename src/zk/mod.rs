//! ZK Module: Zero-Knowledge Proof Infrastructure (NIVEL 4)
//!
//! Provides complete ZK-SNARK support using Bellman + Pasta Curves for:
//! - Transaction validation proofs (balance preservation)
//! - Merkle tree membership proofs
//! - Nullifier uniqueness proofs
//!
//! # Architecture
//!
//! - **circuits.rs**: Bellman R1CS circuit definitions
//! - **prover.rs**: Proof generation using Groth16
//! - **verifier.rs**: Proof verification infrastructure
//!
//! # Example
//!
//! ```ignore
//! use zoka::zk::prover::{Prover, ZKProof};
//! use zoka::zk::verifier::Verifier;
//!
//! let prover = Prover::new("balance_circuit".to_string());
//! let proof = prover.prove(b"private_data", b"public_statement")?;
//!
//! let mut verifier = Verifier::new();
//! let result = verifier.verify(&proof)?;
//! assert!(result.is_valid);
//! ```

pub mod circuits;
pub mod groth16_backend;
pub mod prover;
pub mod range_proofs;
pub mod setup_io;
pub mod verifier;

pub use prover::{Prover, ZKProof};
pub use verifier::{
    PublicInputSchema, VerificationKey, VerificationResult, Verifier, VerifierPolicy,
};

use crate::types::BlockError;

/// Legacy compatibility function - Generate a ZK proof
/// Deprecated: Use `Prover::prove()` instead
#[deprecated(since = "0.2.0", note = "Use Prover::prove() instead")]
pub fn generate_zk_proof(data: &[u8]) -> Result<ZKProof, BlockError> {
    let prover = Prover::new("legacy".to_string());
    prover.prove(data, &[])
}

/// Legacy compatibility function - Verify a ZK proof
/// Deprecated: Use `Verifier::verify()` instead
#[deprecated(since = "0.2.0", note = "Use Verifier::verify() instead")]
pub fn verify_zk_proof(proof: &ZKProof) -> Result<bool, BlockError> {
    let verifier = Verifier::new();
    match verifier.verify(proof) {
        Ok(result) => Ok(result.is_valid),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prover_and_verifier_flow() {
        let data = b"test data";
        let prover = Prover::new("legacy".to_string());
        let proof = prover.prove(data, &[]).expect("proof generation");
        assert!(!proof.proof_bytes.is_empty());
        let verifier = Verifier::new();
        let result = verifier.verify(&proof).expect("proof verification");
        assert!(result.is_valid);
    }
}
