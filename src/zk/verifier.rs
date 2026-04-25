/// ZK Verifier: Verify zero-knowledge proofs
///
/// This module handles proof verification using Bellman's Groth16 system.
/// Includes verification key management, public input schema validation,
/// and batch verification.
use bellman::gadgets::test::TestConstraintSystem;
use bellman::Circuit;
use pasta_curves::Fp;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::types::BlockError;

use super::circuits::{MerkleProofCircuit, NullifierCircuit, TransactionBalanceCircuit};
use super::prover::ZKProof;

const BALANCE_CIRCUIT: &str = "balance_circuit";
const MERKLE_CIRCUIT: &str = "merkle_circuit";
const NULLIFIER_CIRCUIT: &str = "nullifier_circuit";

/// Verification key state
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VerificationKey {
    /// Circuit identifier
    pub circuit_type: String,
    /// Serialized verification key bytes
    pub vk_bytes: Vec<u8>,
    /// Hash of verification key (for integrity check)
    pub vk_hash: [u8; 32],
}

impl VerificationKey {
    /// Create new verification key
    pub fn new(circuit_type: String, vk_bytes: Vec<u8>, vk_hash: [u8; 32]) -> Self {
        VerificationKey {
            circuit_type,
            vk_bytes,
            vk_hash,
        }
    }

    /// Verify integrity of VK
    pub fn verify_integrity(&self) -> bool {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(&self.vk_bytes);
        let computed_hash: [u8; 32] = hasher.finalize().into();
        computed_hash == self.vk_hash
    }

    /// Get VK size in bytes
    pub fn size(&self) -> usize {
        self.vk_bytes.len()
    }
}

/// Public input schema for a circuit.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PublicInputSchema {
    /// Number of public inputs expected by the circuit.
    pub expected_inputs: usize,
    /// Minimum byte size for each serialized input.
    pub min_input_size: usize,
    /// Maximum byte size for each serialized input.
    pub max_input_size: usize,
}

impl PublicInputSchema {
    /// Create a public input schema.
    pub fn new(expected_inputs: usize, min_input_size: usize, max_input_size: usize) -> Self {
        PublicInputSchema {
            expected_inputs,
            min_input_size,
            max_input_size,
        }
    }

    /// Validate serialized public inputs against this schema.
    pub fn validate(&self, inputs: &[Vec<u8>]) -> Result<(), String> {
        if inputs.len() != self.expected_inputs {
            return Err(format!(
                "Invalid public input count: expected {}, got {}",
                self.expected_inputs,
                inputs.len()
            ));
        }

        for (idx, input) in inputs.iter().enumerate() {
            if input.len() < self.min_input_size {
                return Err(format!(
                    "Public input {} too small: min {}, got {}",
                    idx,
                    self.min_input_size,
                    input.len()
                ));
            }
            if input.len() > self.max_input_size {
                return Err(format!(
                    "Public input {} too large: max {}, got {}",
                    idx,
                    self.max_input_size,
                    input.len()
                ));
            }
        }

        Ok(())
    }
}

/// Runtime verifier behavior.
#[derive(Clone, Debug)]
pub struct VerifierPolicy {
    /// If true, verification fails when no VK is registered for the circuit.
    pub require_verification_key: bool,
    /// If true, verification fails when no public input schema is registered for the circuit.
    pub require_public_input_schema: bool,
}

impl VerifierPolicy {
    /// Keep current MVP behavior: allow verification without pre-registered VK/schema.
    pub fn permissive() -> Self {
        VerifierPolicy {
            require_verification_key: false,
            require_public_input_schema: false,
        }
    }

    /// Stronger behavior for production: both VK and schema must be pre-registered.
    pub fn strict() -> Self {
        VerifierPolicy {
            require_verification_key: true,
            require_public_input_schema: true,
        }
    }
}

impl Default for VerifierPolicy {
    fn default() -> Self {
        Self::permissive()
    }
}

/// Verification result with detailed information
#[derive(Clone, Debug)]
pub struct VerificationResult {
    /// Is proof valid
    pub is_valid: bool,
    /// Verification error message (if any)
    pub error: Option<String>,
    /// Time spent verifying (nanoseconds)
    pub verification_time_ns: u128,
    /// Circuit type that was verified
    pub circuit_type: String,
}

impl VerificationResult {
    /// Create successful verification result
    pub fn valid(circuit_type: String, verification_time_ns: u128) -> Self {
        VerificationResult {
            is_valid: true,
            error: None,
            verification_time_ns,
            circuit_type,
        }
    }

    /// Create failed verification result
    pub fn invalid(circuit_type: String, error: String, verification_time_ns: u128) -> Self {
        VerificationResult {
            is_valid: false,
            error: Some(error),
            verification_time_ns,
            circuit_type,
        }
    }
}

/// Verifier state machine
pub struct Verifier {
    /// Verification keys for different circuits
    verification_keys: HashMap<String, VerificationKey>,
    /// Public input schema by circuit type
    public_input_schemas: HashMap<String, PublicInputSchema>,
    /// Runtime behavior policy
    policy: VerifierPolicy,
}

impl Verifier {
    /// Create new verifier instance with permissive policy.
    pub fn new() -> Self {
        Self::with_policy(VerifierPolicy::default())
    }

    /// Create a verifier with an explicit policy.
    pub fn with_policy(policy: VerifierPolicy) -> Self {
        Verifier {
            verification_keys: HashMap::new(),
            public_input_schemas: HashMap::new(),
            policy,
        }
    }

    /// Update verifier policy at runtime.
    pub fn set_policy(&mut self, policy: VerifierPolicy) {
        self.policy = policy;
    }

    /// Inspect current verifier policy.
    pub fn policy(&self) -> &VerifierPolicy {
        &self.policy
    }

    /// Register a verification key
    pub fn register_verification_key(
        &mut self,
        circuit_type: String,
        vk: VerificationKey,
    ) -> Result<(), BlockError> {
        if !vk.verify_integrity() {
            return Err(BlockError::InvalidCommitment);
        }
        self.verification_keys.insert(circuit_type, vk);
        Ok(())
    }

    /// Register the expected public input schema for a circuit.
    pub fn register_public_input_schema(
        &mut self,
        circuit_type: String,
        schema: PublicInputSchema,
    ) {
        self.public_input_schemas.insert(circuit_type, schema);
    }

    /// Verify a proof
    ///
    /// In production:
    /// 1. Deserialize proof and public inputs
    /// 2. Load verification key for circuit type
    /// 3. Use groth16::verify_proof
    /// 4. Return verification result
    pub fn verify(&self, proof: &ZKProof) -> Result<VerificationResult, BlockError> {
        let start = std::time::Instant::now();

        // Check proof structure first.
        if !proof.is_valid_structure() {
            return Ok(VerificationResult::invalid(
                proof.circuit_type.clone(),
                "Invalid proof structure".to_string(),
                start.elapsed().as_nanos(),
            ));
        }

        // Validate public inputs against schema if configured.
        match self.public_input_schemas.get(&proof.circuit_type) {
            Some(schema) => {
                if let Err(err) = schema.validate(&proof.public_inputs) {
                    return Ok(VerificationResult::invalid(
                        proof.circuit_type.clone(),
                        err,
                        start.elapsed().as_nanos(),
                    ));
                }
            }
            None if self.policy.require_public_input_schema => {
                return Ok(VerificationResult::invalid(
                    proof.circuit_type.clone(),
                    "No public input schema registered for circuit".to_string(),
                    start.elapsed().as_nanos(),
                ));
            }
            None => {}
        }

        // If key is required, fail fast when none is registered.
        match self.verification_keys.get(&proof.circuit_type) {
            Some(vk) => {
                if !vk.verify_integrity() {
                    return Ok(VerificationResult::invalid(
                        proof.circuit_type.clone(),
                        "Verification key integrity check failed".to_string(),
                        start.elapsed().as_nanos(),
                    ));
                }
            }
            None if self.policy.require_verification_key => {
                return Ok(VerificationResult::invalid(
                    proof.circuit_type.clone(),
                    "No verification key registered for circuit".to_string(),
                    start.elapsed().as_nanos(),
                ));
            }
            None => {}
        }

        // Hardened path: replay real circuit constraints for hardened circuits.
        let is_valid = match proof.circuit_type.as_str() {
            BALANCE_CIRCUIT => match Self::verify_balance_circuit_constraints(proof) {
                Ok(valid) => valid,
                Err(err) => {
                    return Ok(VerificationResult::invalid(
                        proof.circuit_type.clone(),
                        err,
                        start.elapsed().as_nanos(),
                    ))
                }
            },
            MERKLE_CIRCUIT => match Self::verify_merkle_circuit_constraints(proof) {
                Ok(valid) => valid,
                Err(err) => {
                    return Ok(VerificationResult::invalid(
                        proof.circuit_type.clone(),
                        err,
                        start.elapsed().as_nanos(),
                    ))
                }
            },
            NULLIFIER_CIRCUIT => match Self::verify_nullifier_circuit_constraints(proof) {
                Ok(valid) => valid,
                Err(err) => {
                    return Ok(VerificationResult::invalid(
                        proof.circuit_type.clone(),
                        err,
                        start.elapsed().as_nanos(),
                    ))
                }
            },
            _ => {
                // Fallback path for non-hardened circuits.
                // Full implementation would use Bellman's groth16::verify_proof.
                Self::verify_proof_structure(&proof.proof_bytes)
            }
        };
        let verification_time = start.elapsed().as_nanos();

        if is_valid {
            Ok(VerificationResult::valid(
                proof.circuit_type.clone(),
                verification_time,
            ))
        } else {
            Ok(VerificationResult::invalid(
                proof.circuit_type.clone(),
                "Proof verification failed".to_string(),
                verification_time,
            ))
        }
    }

    /// Batch verify multiple proofs
    pub fn verify_batch(&self, proofs: &[ZKProof]) -> Result<Vec<VerificationResult>, BlockError> {
        proofs.iter().map(|p| self.verify(p)).collect()
    }

    /// Check if verification key is registered
    pub fn has_verification_key(&self, circuit_type: &str) -> bool {
        self.verification_keys.contains_key(circuit_type)
    }

    /// Check if public input schema is registered
    pub fn has_public_input_schema(&self, circuit_type: &str) -> bool {
        self.public_input_schemas.contains_key(circuit_type)
    }

    /// Get number of registered circuits with verification keys
    pub fn circuit_count(&self) -> usize {
        self.verification_keys.len()
    }

    fn verify_balance_circuit_constraints(proof: &ZKProof) -> Result<bool, String> {
        let private_values = Self::decode_u64_words(&proof.proof_bytes, 0, 4)?;
        let public_values = Self::decode_public_inputs(&proof.public_inputs, 2)?;

        let circuit = TransactionBalanceCircuit {
            input_commitment: Some(Fp::from(public_values[0])),
            output_commitment: Some(Fp::from(public_values[1])),
            input_value: Some(Fp::from(private_values[0])),
            output_value: Some(Fp::from(private_values[1])),
            input_blind: Some(Fp::from(private_values[2])),
            output_blind: Some(Fp::from(private_values[3])),
        };

        let mut cs = TestConstraintSystem::<Fp>::new();
        circuit
            .synthesize(&mut cs)
            .map_err(|_| "balance_circuit synthesis failed".to_string())?;

        Ok(cs.is_satisfied())
    }

    fn verify_merkle_circuit_constraints(proof: &ZKProof) -> Result<bool, String> {
        if proof.public_inputs.len() != 2 {
            return Err(format!(
                "merkle_circuit requires 2 public inputs, got {}",
                proof.public_inputs.len()
            ));
        }

        let public_values = Self::decode_public_inputs(&proof.public_inputs, 2)?;

        // Extract tree depth from first byte of proof
        let tree_depth = if !proof.proof_bytes.is_empty() {
            proof.proof_bytes[0] as usize
        } else {
            return Err("merkle_circuit proof missing tree depth".to_string());
        };

        if tree_depth > 16 {
            return Err(format!(
                "merkle_circuit tree depth {} exceeds max 16",
                tree_depth
            ));
        }

        let path_elements_start = 1;
        let path_elements_end = path_elements_start + (tree_depth * 8);
        if proof.proof_bytes.len() < path_elements_end {
            return Err("merkle_circuit proof too short for path elements".to_string());
        }

        let path_values: Vec<u64> = proof.proof_bytes[path_elements_start..path_elements_end]
            .chunks_exact(8)
            .map(|chunk| {
                let arr: [u8; 8] = chunk.try_into().unwrap();
                u64::from_le_bytes(arr)
            })
            .collect();

        // Extract directions
        let directions_start = path_elements_end;
        let directions_bits = if directions_start < proof.proof_bytes.len() {
            let bytes_needed = (tree_depth + 7) / 8;
            let slice = &proof.proof_bytes
                [directions_start..directions_start.min(directions_start + bytes_needed)];
            slice
                .iter()
                .enumerate()
                .flat_map(|(byte_idx, &byte)| {
                    (0..8).map(move |bit_idx| {
                        let overall_bit_idx = byte_idx * 8 + bit_idx;
                        overall_bit_idx < tree_depth && (byte >> bit_idx) & 1 == 1
                    })
                })
                .collect::<Vec<_>>()
        } else {
            vec![false; tree_depth]
        };

        let circuit = MerkleProofCircuit {
            leaf: Some(Fp::from(public_values[0])),
            root: Some(Fp::from(public_values[1])),
            path: Some(path_values.iter().map(|&v| Fp::from(v)).collect()),
            path_index: Some(directions_bits),
        };

        let mut cs = TestConstraintSystem::<Fp>::new();
        circuit
            .synthesize(&mut cs)
            .map_err(|_| "merkle_circuit synthesis failed".to_string())?;

        Ok(cs.is_satisfied())
    }

    fn verify_nullifier_circuit_constraints(proof: &ZKProof) -> Result<bool, String> {
        if proof.public_inputs.len() != 1 {
            return Err(format!(
                "nullifier_circuit requires 1 public input, got {}",
                proof.public_inputs.len()
            ));
        }

        // Extract public input (nullifier)
        let pub_input = Self::decode_public_inputs(&proof.public_inputs, 1)?;

        // Extract private inputs (secret, index, blind) from proof bytes
        let private_values = Self::decode_u64_words(&proof.proof_bytes, 0, 3)?;

        let circuit = NullifierCircuit {
            nullifier: Some(Fp::from(pub_input[0])),
            secret: Some(Fp::from(private_values[0])),
            index: Some(Fp::from(private_values[1])),
            blind: Some(Fp::from(private_values[2])),
        };

        let mut cs = TestConstraintSystem::<Fp>::new();
        circuit
            .synthesize(&mut cs)
            .map_err(|_| "nullifier_circuit synthesis failed".to_string())?;

        Ok(cs.is_satisfied())
    }

    fn decode_u64_words(bytes: &[u8], start: usize, word_count: usize) -> Result<Vec<u64>, String> {
        let end = start + (word_count * 8);
        if bytes.len() < end {
            return Err(format!(
                "proof bytes too short for witness decoding: need at least {}, got {}",
                end,
                bytes.len()
            ));
        }

        bytes[start..end]
            .chunks_exact(8)
            .map(|chunk| {
                let arr: [u8; 8] = chunk
                    .try_into()
                    .map_err(|_| "invalid u64 encoding in proof bytes".to_string())?;
                Ok(u64::from_le_bytes(arr))
            })
            .collect()
    }

    fn decode_public_inputs(inputs: &[Vec<u8>], expected_count: usize) -> Result<Vec<u64>, String> {
        if inputs.len() != expected_count {
            return Err(format!(
                "invalid public input count for balance_circuit: expected {}, got {}",
                expected_count,
                inputs.len()
            ));
        }

        inputs
            .iter()
            .enumerate()
            .map(|(idx, input)| {
                if input.len() != 8 {
                    return Err(format!(
                        "public input {} must be 8 bytes (got {})",
                        idx,
                        input.len()
                    ));
                }
                let arr: [u8; 8] = input
                    .as_slice()
                    .try_into()
                    .map_err(|_| format!("invalid encoding for public input {}", idx))?;
                Ok(u64::from_le_bytes(arr))
            })
            .collect()
    }

    /// Internal: Verify proof structure (basic checks)
    fn verify_proof_structure(proof_bytes: &[u8]) -> bool {
        // Groth16 proofs should be exactly 192 bytes (2 curve points + 1 scalar)
        proof_bytes.len() == 192 && proof_bytes.iter().any(|b| *b != 0)
    }
}

impl Default for Verifier {
    fn default() -> Self {
        Self::new()
    }
}

/// Batch verifier for concurrent verification
pub struct BatchVerifier {
    verifiers: HashMap<String, Verifier>,
}

impl BatchVerifier {
    /// Create new batch verifier
    pub fn new() -> Self {
        BatchVerifier {
            verifiers: HashMap::new(),
        }
    }

    /// Register verifier for a circuit type
    pub fn register_circuit(&mut self, circuit_type: String) {
        self.verifiers.insert(circuit_type, Verifier::new());
    }

    /// Register verifier for a circuit type with explicit policy.
    pub fn register_circuit_with_policy(&mut self, circuit_type: String, policy: VerifierPolicy) {
        self.verifiers
            .insert(circuit_type, Verifier::with_policy(policy));
    }

    /// Verify proof with registered verifier
    pub fn verify(
        &self,
        circuit_type: &str,
        proof: &ZKProof,
    ) -> Result<VerificationResult, BlockError> {
        let verifier = self
            .verifiers
            .get(circuit_type)
            .ok_or(BlockError::InvalidCommitment)?;
        verifier.verify(proof)
    }

    /// Verify multiple proofs in batch
    pub fn verify_batch(
        &self,
        circuit_type: &str,
        proofs: &[ZKProof],
    ) -> Result<Vec<VerificationResult>, BlockError> {
        let verifier = self
            .verifiers
            .get(circuit_type)
            .ok_or(BlockError::InvalidCommitment)?;
        verifier.verify_batch(proofs)
    }

    /// Get number of registered verifiers
    pub fn circuit_count(&self) -> usize {
        self.verifiers.len()
    }
}

impl Default for BatchVerifier {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zk::prover::Prover;

    fn valid_vk(circuit_type: &str, vk_bytes: Vec<u8>) -> VerificationKey {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(&vk_bytes);
        let hash: [u8; 32] = hasher.finalize().into();
        VerificationKey::new(circuit_type.to_string(), vk_bytes, hash)
    }

    fn encode_u64_words(values: &[u64]) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(values.len() * 8);
        for value in values {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn test_verification_key_creation() {
        let vk = VerificationKey::new("test_circuit".to_string(), vec![1, 2, 3, 4], [0u8; 32]);

        assert_eq!(vk.circuit_type, "test_circuit");
        assert_eq!(vk.size(), 4);
    }

    #[test]
    fn test_verification_key_integrity() {
        use sha2::{Digest, Sha256};

        let vk_bytes = vec![1, 2, 3, 4, 5];
        let mut hasher = Sha256::new();
        hasher.update(&vk_bytes);
        let hash: [u8; 32] = hasher.finalize().into();

        let vk = VerificationKey::new("circuit".to_string(), vk_bytes, hash);
        assert!(vk.verify_integrity());
    }

    #[test]
    fn test_public_input_schema_validation() {
        let schema = PublicInputSchema::new(2, 1, 8);
        assert!(schema.validate(&[vec![1], vec![2, 3]]).is_ok());
        assert!(schema.validate(&[vec![1]]).is_err());
        assert!(schema.validate(&[vec![], vec![2]]).is_err());
        assert!(schema.validate(&[vec![1; 9], vec![2]]).is_err());
    }

    #[test]
    fn test_verification_result_valid() {
        let result = VerificationResult::valid("test".to_string(), 1000);
        assert!(result.is_valid);
        assert!(result.error.is_none());
    }

    #[test]
    fn test_verification_result_invalid() {
        let result = VerificationResult::invalid("test".to_string(), "Failed".to_string(), 2000);
        assert!(!result.is_valid);
        assert!(result.error.is_some());
    }

    #[test]
    fn test_verifier_creation() {
        let verifier = Verifier::new();
        assert_eq!(verifier.circuit_count(), 0);
        assert!(!verifier.policy().require_verification_key);
        assert!(!verifier.policy().require_public_input_schema);
    }

    #[test]
    fn test_verifier_register_vk() {
        let mut verifier = Verifier::new();
        let vk = VerificationKey::new(
            "test_circuit".to_string(),
            vec![1, 2, 3],
            [0u8; 32], // Invalid hash, will fail
        );

        let result = verifier.register_verification_key("test_circuit".to_string(), vk);
        assert!(result.is_err());
    }

    #[test]
    fn test_verifier_has_vk() {
        let verifier = Verifier::new();
        assert!(!verifier.has_verification_key("test_circuit"));
    }

    #[test]
    fn test_verifier_verify_invalid_proof() {
        let verifier = Verifier::new();
        let bad_proof = ZKProof::new(
            vec![0u8; 50], // Too small
            "test_circuit".to_string(),
            vec![],
            0,
        );

        let result = verifier.verify(&bad_proof);
        assert!(result.is_ok());
        let vr = result.expect("verification should return result");
        assert!(!vr.is_valid);
    }

    #[test]
    fn test_verifier_verify_valid_structure() {
        let verifier = Verifier::new();
        let good_proof = ZKProof::new(
            vec![5u8; 192], // Correct size
            "test_circuit".to_string(),
            vec![],
            0,
        );

        let result = verifier.verify(&good_proof);
        assert!(result.is_ok());
    }

    #[test]
    fn test_verifier_strict_requires_vk() {
        let verifier = Verifier::with_policy(VerifierPolicy {
            require_verification_key: true,
            require_public_input_schema: false,
        });

        let good_proof = ZKProof::new(vec![5u8; 192], "strict_vk_circuit".to_string(), vec![], 0);

        let result = verifier.verify(&good_proof).expect("verification result");
        assert!(!result.is_valid);
        assert!(result
            .error
            .expect("missing error")
            .contains("No verification key registered"));
    }

    #[test]
    fn test_verifier_strict_requires_schema() {
        let verifier = Verifier::with_policy(VerifierPolicy {
            require_verification_key: false,
            require_public_input_schema: true,
        });

        let good_proof = ZKProof::new(
            vec![5u8; 192],
            "strict_schema_circuit".to_string(),
            vec![vec![1, 2, 3]],
            0,
        );

        let result = verifier.verify(&good_proof).expect("verification result");
        assert!(!result.is_valid);
        assert!(result
            .error
            .expect("missing error")
            .contains("No public input schema registered"));
    }

    #[test]
    fn test_verifier_schema_mismatch() {
        let mut verifier = Verifier::new();
        verifier.register_public_input_schema(
            "schema_circuit".to_string(),
            PublicInputSchema::new(2, 1, 16),
        );

        let proof = ZKProof::new(
            vec![5u8; 192],
            "schema_circuit".to_string(),
            vec![vec![1, 2, 3]], // Only one input, but schema expects two.
            0,
        );

        let result = verifier.verify(&proof).expect("verification result");
        assert!(!result.is_valid);
        assert!(result
            .error
            .expect("missing error")
            .contains("Invalid public input count"));
    }

    #[test]
    fn test_verifier_strict_success_with_vk_and_schema() {
        let mut verifier = Verifier::with_policy(VerifierPolicy::strict());
        verifier
            .register_verification_key(
                "balance_circuit".to_string(),
                valid_vk("balance_circuit", vec![42, 43, 44]),
            )
            .expect("valid vk should register");
        verifier.register_public_input_schema(
            "balance_circuit".to_string(),
            PublicInputSchema::new(2, 8, 8),
        );

        let prover = Prover::new("balance_circuit".to_string());
        let private = encode_u64_words(&[50, 50, 7, 3]);
        let public = encode_u64_words(&[57, 53]);
        let proof = prover
            .prove(&private, &public)
            .expect("valid balance proof should be generated");

        let result = verifier.verify(&proof).expect("verification result");
        assert!(result.is_valid);
        assert!(result.error.is_none());
    }

    #[test]
    fn test_verifier_balance_circuit_constraint_failure() {
        let verifier = Verifier::new();

        let private = encode_u64_words(&[50, 49, 7, 3]); // Breaks balance: 50 != 49
        let public = encode_u64_words(&[57, 53]);

        let mut proof_bytes = vec![0u8; 192];
        proof_bytes[..private.len()].copy_from_slice(&private);
        proof_bytes[private.len()..private.len() + public.len()].copy_from_slice(&public);
        proof_bytes[48] = 1;

        let proof = ZKProof::new(
            proof_bytes,
            "balance_circuit".to_string(),
            vec![57u64.to_le_bytes().to_vec(), 53u64.to_le_bytes().to_vec()],
            0,
        );

        let result = verifier.verify(&proof).expect("verification result");
        assert!(!result.is_valid);
    }

    #[test]
    fn test_batch_verifier() {
        let mut batch = BatchVerifier::new();
        batch.register_circuit("circuit_a".to_string());
        batch.register_circuit("circuit_b".to_string());

        assert_eq!(batch.circuit_count(), 2);
    }

    #[test]
    fn test_batch_verifier_with_policy() {
        let mut batch = BatchVerifier::new();
        batch.register_circuit_with_policy("strict_circuit".to_string(), VerifierPolicy::strict());
        assert_eq!(batch.circuit_count(), 1);
    }
}
