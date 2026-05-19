/// ZK Prover: Generate zero-knowledge proofs
///
/// This module handles proof generation using Bellman R1CS checks for MVP hardening.
/// It keeps placeholder proof bytes for non-hardened circuits.
use bellman::gadgets::test::TestConstraintSystem;
use bellman::Circuit;
use pasta_curves::Fp;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::types::BlockError;

use super::circuits::{MerkleProofCircuit, NullifierCircuit, TransactionBalanceCircuit};
use super::groth16_backend::{
    self as g16, BALANCE_CIRCUIT, BALANCE_PRIVATE_INPUT_WORDS, BALANCE_PUBLIC_INPUT_WORDS,
    MERKLE_CIRCUIT, MERKLE_PUBLIC_INPUT_WORDS, NULLIFIER_CIRCUIT, NULLIFIER_PRIVATE_INPUT_WORDS,
    NULLIFIER_PUBLIC_INPUT_WORDS, PRIVATE_TX_CIRCUIT,
};

const MERKLE_MAX_DEPTH: usize = 16;

/// Serializable ZK proof with metadata
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ZKProof {
    /// Groth16-like proof bytes (placeholder serialization in MVP)
    pub proof_bytes: Vec<u8>,
    /// Circuit type identifier
    pub circuit_type: String,
    /// Public inputs
    pub public_inputs: Vec<Vec<u8>>,
    /// Proof generation timestamp
    pub timestamp: u64,
}

impl ZKProof {
    /// Create new proof with metadata
    pub fn new(
        proof_bytes: Vec<u8>,
        circuit_type: String,
        public_inputs: Vec<Vec<u8>>,
        timestamp: u64,
    ) -> Self {
        ZKProof {
            proof_bytes,
            circuit_type,
            public_inputs,
            timestamp,
        }
    }

    /// Get proof size in bytes
    pub fn size(&self) -> usize {
        self.proof_bytes.len()
    }

    /// Check if proof has expected structure (basic validation)
    pub fn is_valid_structure(&self) -> bool {
        // Groth16 proofs are typically ~192 bytes.
        !self.proof_bytes.is_empty()
            && self.proof_bytes.len() >= 48
            && !self.circuit_type.is_empty()
    }
}

/// Prover state machine
pub struct Prover {
    /// Circuit name
    circuit_name: String,
    /// Cached proving keys (in production, loaded from disk)
    proving_keys_cache: std::collections::HashMap<String, Vec<u8>>,
}

impl Prover {
    /// Create new prover instance
    pub fn new(circuit_name: String) -> Self {
        if g16::is_groth16_circuit(&circuit_name) {
            g16::ensure_trusted_setup();
        }
        Prover {
            circuit_name,
            proving_keys_cache: std::collections::HashMap::new(),
        }
    }

    /// Generate a proof.
    ///
    /// For `balance_circuit`, `merkle_circuit`, and `nullifier_circuit`,
    /// this runs real Bellman R1CS satisfiability checks.
    /// Other circuits keep deterministic placeholder behavior.
    pub fn prove(
        &self,
        private_inputs: &[u8],
        public_inputs: &[u8],
    ) -> Result<ZKProof, BlockError> {
        match self.circuit_name.as_str() {
            BALANCE_CIRCUIT => self.prove_balance_circuit(private_inputs, public_inputs),
            MERKLE_CIRCUIT => self.prove_merkle_circuit(private_inputs, public_inputs),
            NULLIFIER_CIRCUIT => self.prove_nullifier_circuit(private_inputs, public_inputs),
            PRIVATE_TX_CIRCUIT => self.prove_private_tx_circuit(private_inputs, public_inputs),
            _ => self.prove_placeholder(private_inputs, public_inputs),
        }
    }

    /// Batch prove multiple statements
    pub fn prove_batch(
        &self,
        statements: &[(Vec<u8>, Vec<u8>)],
    ) -> Result<Vec<ZKProof>, BlockError> {
        statements
            .iter()
            .map(|(private, public)| self.prove(private, public))
            .collect()
    }

    /// Load proving key from file (stub for future implementation)
    pub fn load_proving_key(&mut self, _path: &str) -> Result<(), BlockError> {
        // In production: load from trusted setup file
        if g16::is_groth16_circuit(&self.circuit_name) {
            self.proving_keys_cache
                .insert(self.circuit_name.clone(), vec![1u8]);
        }
        Ok(())
    }

    /// Check if proving key is loaded
    pub fn has_proving_key(&self) -> bool {
        g16::is_groth16_circuit(&self.circuit_name) || !self.proving_keys_cache.is_empty()
    }

    /// Get circuit name
    pub fn circuit_name(&self) -> &str {
        &self.circuit_name
    }

    fn prove_placeholder(
        &self,
        private_inputs: &[u8],
        public_inputs: &[u8],
    ) -> Result<ZKProof, BlockError> {
        // Deterministic placeholder for non-hardened circuits.
        let mut hasher = Sha256::new();
        hasher.update(private_inputs);
        hasher.update(public_inputs);
        let proof_digest = hasher.finalize();

        let mut proof_bytes = vec![0u8; 192];
        proof_bytes[..32].copy_from_slice(&proof_digest);

        let public_inputs_serialized = vec![public_inputs.to_vec()];

        Ok(ZKProof::new(
            proof_bytes,
            self.circuit_name.clone(),
            public_inputs_serialized,
            current_unix_timestamp(),
        ))
    }

    fn prove_balance_circuit(
        &self,
        private_inputs: &[u8],
        public_inputs: &[u8],
    ) -> Result<ZKProof, BlockError> {
        let private_values = parse_u64_words(
            private_inputs,
            BALANCE_PRIVATE_INPUT_WORDS,
            "balance_circuit private inputs",
        )?;
        let public_values = parse_u64_words(
            public_inputs,
            BALANCE_PUBLIC_INPUT_WORDS,
            "balance_circuit public inputs",
        )?;

        let circuit = TransactionBalanceCircuit {
            input_commitment: Some(Fp::from(public_values[0])),
            output_commitment: Some(Fp::from(public_values[1])),
            input_value: Some(Fp::from(private_values[0])),
            output_value: Some(Fp::from(private_values[1])),
            input_blind: Some(Fp::from(private_values[2])),
            output_blind: Some(Fp::from(private_values[3])),
        };

        let mut cs = TestConstraintSystem::<Fp>::new();
        circuit.synthesize(&mut cs).map_err(|_| {
            BlockError::TransactionValidationFailed("balance_circuit synthesis failed".to_string())
        })?;

        if !cs.is_satisfied() {
            return Err(BlockError::TransactionValidationFailed(
                "balance_circuit constraints not satisfied".to_string(),
            ));
        }

        let (proof_bytes, public_inputs_serialized) =
            g16::prove_balance(private_inputs, public_inputs)?;

        Ok(ZKProof::new(
            proof_bytes,
            self.circuit_name.clone(),
            public_inputs_serialized,
            current_unix_timestamp(),
        ))
    }

    fn prove_merkle_circuit(
        &self,
        private_inputs: &[u8],
        public_inputs: &[u8],
    ) -> Result<ZKProof, BlockError> {
        // Parse public inputs: leaf (u64), root (u64)
        let public_values = parse_u64_words(
            public_inputs,
            MERKLE_PUBLIC_INPUT_WORDS,
            "merkle_circuit public inputs",
        )?;

        // Parse private inputs: tree depth (u8), then path elements (each 8 bytes), then directions (bits packed)
        if private_inputs.len() < 1 {
            return Err(BlockError::TransactionValidationFailed(
                "merkle_circuit private inputs too short".to_string(),
            ));
        }

        let tree_depth = private_inputs[0] as usize;
        if tree_depth > MERKLE_MAX_DEPTH {
            return Err(BlockError::TransactionValidationFailed(format!(
                "merkle_circuit tree depth {} exceeds max {}",
                tree_depth, MERKLE_MAX_DEPTH
            )));
        }

        let path_elements_start = 1;
        let path_elements_end = path_elements_start + (tree_depth * 8);
        if private_inputs.len() < path_elements_end {
            return Err(BlockError::TransactionValidationFailed(
                "merkle_circuit private inputs incomplete".to_string(),
            ));
        }

        let path_values: Vec<u64> = private_inputs[path_elements_start..path_elements_end]
            .chunks_exact(8)
            .map(|chunk| {
                let arr: [u8; 8] = chunk.try_into().unwrap();
                u64::from_le_bytes(arr)
            })
            .collect();

        // Extract direction bits: each bit indicates left (1) or right (0)
        let directions_start = path_elements_end;
        let directions_bits = if directions_start < private_inputs.len() {
            let bytes_needed = (tree_depth + 7) / 8;
            let slice = &private_inputs
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
        circuit.synthesize(&mut cs).map_err(|_| {
            BlockError::TransactionValidationFailed("merkle_circuit synthesis failed".to_string())
        })?;

        if !cs.is_satisfied() {
            return Err(BlockError::TransactionValidationFailed(
                "merkle_circuit constraints not satisfied".to_string(),
            ));
        }

        let (proof_bytes, public_inputs_serialized) =
            g16::prove_merkle(private_inputs, public_inputs)?;

        Ok(ZKProof::new(
            proof_bytes,
            self.circuit_name.clone(),
            public_inputs_serialized,
            current_unix_timestamp(),
        ))
    }

    fn prove_nullifier_circuit(
        &self,
        private_inputs: &[u8],
        public_inputs: &[u8],
    ) -> Result<ZKProof, BlockError> {
        let private_values = parse_u64_words(
            private_inputs,
            NULLIFIER_PRIVATE_INPUT_WORDS,
            "nullifier_circuit private inputs",
        )?;
        let public_values = parse_u64_words(
            public_inputs,
            NULLIFIER_PUBLIC_INPUT_WORDS,
            "nullifier_circuit public inputs",
        )?;

        let circuit = NullifierCircuit {
            nullifier: Some(Fp::from(public_values[0])),
            secret: Some(Fp::from(private_values[0])),
            index: Some(Fp::from(private_values[1])),
            blind: Some(Fp::from(private_values[2])),
        };

        let mut cs = TestConstraintSystem::<Fp>::new();
        circuit.synthesize(&mut cs).map_err(|_| {
            BlockError::TransactionValidationFailed(
                "nullifier_circuit synthesis failed".to_string(),
            )
        })?;

        if !cs.is_satisfied() {
            return Err(BlockError::TransactionValidationFailed(
                "nullifier_circuit constraints not satisfied".to_string(),
            ));
        }

        let (proof_bytes, public_inputs_serialized) =
            g16::prove_nullifier(private_inputs, public_inputs)?;

        Ok(ZKProof::new(
            proof_bytes,
            self.circuit_name.clone(),
            public_inputs_serialized,
            current_unix_timestamp(),
        ))
    }

    fn prove_private_tx_circuit(
        &self,
        private_inputs: &[u8],
        public_inputs: &[u8],
    ) -> Result<ZKProof, BlockError> {
        let (proof_bytes, public_inputs_serialized) =
            g16::prove_private_tx(private_inputs, public_inputs)?;
        Ok(ZKProof::new(
            proof_bytes,
            self.circuit_name.clone(),
            public_inputs_serialized,
            current_unix_timestamp(),
        ))
    }
}

fn parse_u64_words(
    bytes: &[u8],
    expected_words: usize,
    context: &str,
) -> Result<Vec<u64>, BlockError> {
    let expected_len = expected_words * 8;
    if bytes.len() != expected_len {
        return Err(BlockError::TransactionValidationFailed(format!(
            "{} must be {} bytes (got {})",
            context,
            expected_len,
            bytes.len()
        )));
    }

    bytes
        .chunks_exact(8)
        .map(|chunk| {
            let arr: [u8; 8] = chunk.try_into().map_err(|_| {
                BlockError::TransactionValidationFailed("invalid u64 encoding".to_string())
            })?;
            Ok(u64::from_le_bytes(arr))
        })
        .collect()
}

fn current_unix_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Batch prover for efficient multi-proof generation
pub struct BatchProver {
    provers: std::collections::HashMap<String, Prover>,
}

impl BatchProver {
    /// Create new batch prover
    pub fn new() -> Self {
        BatchProver {
            provers: std::collections::HashMap::new(),
        }
    }

    /// Register a circuit type
    pub fn register(&mut self, circuit_name: String) {
        self.provers
            .insert(circuit_name.clone(), Prover::new(circuit_name));
    }

    /// Prove statement with specified circuit
    pub fn prove(
        &self,
        circuit_name: &str,
        private_inputs: &[u8],
        public_inputs: &[u8],
    ) -> Result<ZKProof, BlockError> {
        self.provers
            .get(circuit_name)
            .ok_or(BlockError::InvalidCommitment)?
            .prove(private_inputs, public_inputs)
    }

    /// Get number of registered circuits
    pub fn circuit_count(&self) -> usize {
        self.provers.len()
    }
}

impl Default for BatchProver {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::PRIVATE_TREE_DEPTH;
    use crate::state::build_private_commitment_inclusion_proofs;

    fn encode_u64_words(values: &[u64]) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(values.len() * 8);
        for value in values {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes
    }

    fn encode_private_tx_case(
        amount: u64,
        blind: u64,
        secret: u64,
        index: u64,
        input_amounts: &[u64],
        output_sum: u64,
        fee: u64,
        key_images: &[u64],
    ) -> (Vec<u8>, Vec<u8>) {
        let input_sum = input_amounts.iter().sum::<u64>();
        let input_commitments = input_amounts
            .iter()
            .enumerate()
            .map(|(idx, amount)| {
                let words = [
                    11u64 + idx as u64,
                    22u64 + *amount,
                    33u64 + idx as u64,
                    44u64 + *amount,
                ];
                words
                    .into_iter()
                    .flat_map(|word| word.to_le_bytes())
                    .map(|byte| format!("{:02x}", byte))
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        let (root_hex, proofs) =
            build_private_commitment_inclusion_proofs(&input_commitments, &input_commitments)
                .expect("proofs must exist");
        let root_words = {
            let bytes = hex::decode(root_hex).unwrap();
            let mut words = [0u64; g16::PRIVATE_TX_ROOT_WORDS];
            for (idx, word) in words.iter_mut().enumerate() {
                let start = idx * 8;
                let mut chunk = [0u8; 8];
                chunk.copy_from_slice(&bytes[start..start + 8]);
                *word = u64::from_le_bytes(chunk);
            }
            words
        };
        let mut padded_flags = vec![0u64; g16::PRIVATE_TX_MERKLE_PROVEN_INPUTS];
        let mut padded_commitment_words = Vec::new();
        let mut padded_sibling_words = Vec::new();
        let mut padded_directions = Vec::new();
        for proven_idx in 0..g16::PRIVATE_TX_MERKLE_PROVEN_INPUTS {
            if proven_idx < input_commitments.len() {
                padded_flags[proven_idx] = 1;
                let commitment_bytes = hex::decode(&input_commitments[proven_idx]).unwrap();
                for word_idx in 0..g16::PRIVATE_TX_ROOT_WORDS {
                    let start = word_idx * 8;
                    let mut chunk = [0u8; 8];
                    chunk.copy_from_slice(&commitment_bytes[start..start + 8]);
                    padded_commitment_words.push(u64::from_le_bytes(chunk));
                }
                for step in &proofs[proven_idx].steps {
                    let sibling_bytes = hex::decode(&step.sibling_hash).unwrap();
                    for word_idx in 0..g16::PRIVATE_TX_ROOT_WORDS {
                        let start = word_idx * 8;
                        let mut chunk = [0u8; 8];
                        chunk.copy_from_slice(&sibling_bytes[start..start + 8]);
                        padded_sibling_words.push(u64::from_le_bytes(chunk));
                    }
                    padded_directions.push(if step.sibling_on_left { 1u64 } else { 0u64 });
                }
            } else {
                padded_commitment_words.extend_from_slice(&[0u64; g16::PRIVATE_TX_ROOT_WORDS]);
                for _ in 0..PRIVATE_TREE_DEPTH {
                    padded_sibling_words.extend_from_slice(&[0u64; g16::PRIVATE_TX_ROOT_WORDS]);
                    padded_directions.push(0u64);
                }
            }
        }

        let mut private_words = vec![
            amount,
            blind,
            secret,
            index,
            input_sum,
            output_sum,
            fee,
            key_images.first().copied().unwrap_or(0),
            1,
            1,
        ];
        let mut padded_amounts = input_amounts.to_vec();
        padded_amounts.resize(g16::PRIVATE_TX_MAX_INPUTS, 0);
        let mut padded_key_images = key_images.to_vec();
        padded_key_images.resize(g16::PRIVATE_TX_MAX_INPUTS, 0);
        let mut padded_asset_tags = vec![0u64; g16::PRIVATE_TX_MAX_INPUTS];
        for idx in 0..input_amounts.len().min(g16::PRIVATE_TX_MAX_INPUTS) {
            padded_asset_tags[idx] = 1;
        }
        private_words.extend_from_slice(&padded_amounts);
        private_words.extend_from_slice(&padded_key_images);
        private_words.extend_from_slice(&padded_asset_tags);
        private_words.extend_from_slice(&padded_sibling_words);
        private_words.extend_from_slice(&padded_directions);

        let mut public_words = vec![
            amount.checked_add(blind).unwrap(),
            secret
                .checked_mul(index)
                .unwrap()
                .checked_add(blind)
                .unwrap(),
        ];
        public_words.extend_from_slice(&padded_key_images);
        public_words.extend_from_slice(&padded_flags);
        public_words.extend_from_slice(&padded_commitment_words);
        public_words.extend_from_slice(&root_words);
        (
            encode_u64_words(&private_words),
            encode_u64_words(&public_words),
        )
    }

    #[test]
    fn test_zkproof_creation() {
        let proof = ZKProof::new(
            vec![1, 2, 3, 4],
            "test_circuit".to_string(),
            vec![vec![5, 6, 7]],
            1234567890,
        );

        assert_eq!(proof.circuit_type, "test_circuit");
        assert_eq!(proof.size(), 4);
        assert!(!proof.is_valid_structure()); // Too small
    }

    #[test]
    fn test_prover_creation() {
        let prover = Prover::new("balance_circuit".to_string());
        assert_eq!(prover.circuit_name(), "balance_circuit");
        assert!(prover.has_proving_key());
    }

    #[test]
    fn test_prover_prove_placeholder() {
        let prover = Prover::new("test_circuit".to_string());
        let private = b"secret_data";
        let public = b"public_statement";

        let result = prover.prove(private, public);
        assert!(result.is_ok());

        let proof = result.expect("proof");
        assert_eq!(proof.circuit_type, "test_circuit");
        assert!(proof.is_valid_structure());
        assert_eq!(proof.public_inputs.len(), 1);
    }

    #[test]
    fn test_prover_prove_balance_circuit_valid() {
        let prover = Prover::new("balance_circuit".to_string());

        // input_value=50, output_value=50, input_blind=7, output_blind=3
        // input_commitment=57, output_commitment=53
        let private = encode_u64_words(&[50, 50, 7, 3]);
        let public = encode_u64_words(&[57, 53]);

        let proof = prover.prove(&private, &public).expect("balance proof");
        assert_eq!(proof.circuit_type, "balance_circuit");
        assert_eq!(proof.public_inputs.len(), 2);
        assert!(proof.is_valid_structure());
    }

    #[test]
    fn test_prover_prove_balance_circuit_invalid() {
        let prover = Prover::new("balance_circuit".to_string());

        // Invalid statement: output_commitment should be 53, but we pass 99.
        let private = encode_u64_words(&[50, 50, 7, 3]);
        let public = encode_u64_words(&[57, 99]);

        let result = prover.prove(&private, &public);
        assert!(result.is_err());
    }

    #[test]
    fn test_prover_batch_prove() {
        let prover = Prover::new("batch_circuit".to_string());
        let statements = vec![
            (vec![1, 2], vec![3, 4]),
            (vec![5, 6], vec![7, 8]),
            (vec![9, 10], vec![11, 12]),
        ];

        let results = prover.prove_batch(&statements);
        assert!(results.is_ok());

        let proofs = results.expect("proofs");
        assert_eq!(proofs.len(), 3);
        assert!(proofs.iter().all(|p| p.is_valid_structure()));
    }

    #[test]
    fn test_batch_prover() {
        let mut batch = BatchProver::new();
        batch.register("circuit_a".to_string());
        batch.register("circuit_b".to_string());

        assert_eq!(batch.circuit_count(), 2);

        let proof_a = batch.prove("circuit_a", &[1, 2], &[3, 4]);
        assert!(proof_a.is_ok());
        assert_eq!(proof_a.expect("proof").circuit_type, "circuit_a");

        let proof_unknown = batch.prove("circuit_c", &[1], &[2]);
        assert!(proof_unknown.is_err());
    }

    #[test]
    fn test_zkproof_size() {
        let proof = ZKProof::new(
            vec![0u8; 192], // Groth16-like size
            "merkle_circuit".to_string(),
            vec![],
            0,
        );

        assert_eq!(proof.size(), 192);
    }

    #[test]
    fn test_prover_prove_merkle_circuit_valid() {
        let prover = Prover::new("merkle_circuit".to_string());

        // Build merkle proof inputs
        // Tree depth = 2, leaf = 1, siblings = [10, 20], directions = [true, false]
        let mut private = vec![2u8]; // depth
        private.extend_from_slice(&10u64.to_le_bytes());
        private.extend_from_slice(&20u64.to_le_bytes());
        private.push(0b01); // direction bits: bit 0 = 1 (true), bit 1 = 0 (false)

        let public = encode_u64_words(&[1, 31]); // leaf=1, root=1+10+20=31

        let proof = prover.prove(&private, &public).expect("merkle proof");
        assert_eq!(proof.circuit_type, "merkle_circuit");
        assert_eq!(proof.public_inputs.len(), 2);
        assert!(proof.is_valid_structure());
    }

    #[test]
    fn test_prover_prove_merkle_circuit_invalid_root() {
        let prover = Prover::new("merkle_circuit".to_string());

        let mut private = vec![2u8]; // depth
        private.extend_from_slice(&10u64.to_le_bytes());
        private.extend_from_slice(&20u64.to_le_bytes());
        private.push(0b01);

        let public = encode_u64_words(&[1, 999]); // Wrong root!

        let result = prover.prove(&private, &public);
        assert!(result.is_err(), "should fail on wrong merkle root");
    }

    #[test]
    fn test_prover_prove_nullifier_circuit_valid() {
        let prover = Prover::new("nullifier_circuit".to_string());

        // secret = 123, index = 5, blind = 7
        // nullifier = 123 * 5 + 7 = 622
        let secret = 123u64;
        let index = 5u64;
        let blind = 7u64;
        let nullifier = secret.wrapping_mul(index).wrapping_add(blind);

        let private = encode_u64_words(&[secret, index, blind]);
        let public = encode_u64_words(&[nullifier]);

        let proof = prover.prove(&private, &public).expect("nullifier proof");
        assert_eq!(proof.circuit_type, "nullifier_circuit");
        assert_eq!(proof.public_inputs.len(), 1);
        assert!(proof.is_valid_structure());
    }

    #[test]
    fn test_prover_prove_nullifier_circuit_invalid() {
        let prover = Prover::new("nullifier_circuit".to_string());

        let secret = 123u64;
        let index = 5u64;
        let blind = 7u64;
        let wrong_nullifier = 999u64; // Should be 622

        let private = encode_u64_words(&[secret, index, blind]);
        let public = encode_u64_words(&[wrong_nullifier]);

        let result = prover.prove(&private, &public);
        assert!(result.is_err(), "should fail on wrong nullifier");
    }

    #[test]
    fn test_batch_prover_multiple_circuits() {
        let mut batch = BatchProver::new();
        batch.register("balance_circuit".to_string());
        batch.register("merkle_circuit".to_string());
        batch.register("nullifier_circuit".to_string());

        assert_eq!(batch.circuit_count(), 3);

        // Balance
        let private_balance = encode_u64_words(&[50, 50, 7, 3]);
        let public_balance = encode_u64_words(&[57, 53]);
        let proof_balance = batch.prove("balance_circuit", &private_balance, &public_balance);
        assert!(proof_balance.is_ok());

        // Merkle
        let mut private_merkle = vec![1u8];
        private_merkle.extend_from_slice(&5u64.to_le_bytes());
        let public_merkle = encode_u64_words(&[10, 15]); // leaf=10, root=15
        let proof_merkle = batch.prove("merkle_circuit", &private_merkle, &public_merkle);
        assert!(proof_merkle.is_ok());

        // Nullifier
        let private_nullifier = encode_u64_words(&[2, 3, 1]); // secret=2, index=3, blind=1
        let public_nullifier = encode_u64_words(&[7]); // nullifier=2*3+1=7
        let proof_nullifier =
            batch.prove("nullifier_circuit", &private_nullifier, &public_nullifier);
        assert!(proof_nullifier.is_ok());
    }

    #[test]
    fn test_prover_prove_private_tx_circuit_valid() {
        let prover = Prover::new("private_tx".to_string());
        let (private, public) = encode_private_tx_case(100, 7, 20, 1, &[40, 60], 100, 0, &[55, 66]);

        let proof = prover.prove(&private, &public).expect("private_tx proof");
        assert_eq!(proof.circuit_type, "private_tx");
        assert_eq!(
            proof.public_inputs.len(),
            g16::PRIVATE_TX_PUBLIC_INPUT_WORDS
        );
        assert!(proof.is_valid_structure());
    }

    #[test]
    fn test_prover_prove_private_tx_circuit_accepts_above_32_bit_amount() {
        let prover = Prover::new("private_tx".to_string());
        let (private, public) = encode_private_tx_case(
            (u32::MAX as u64) + 1,
            7,
            20,
            1,
            &[(u32::MAX as u64) + 1],
            (u32::MAX as u64) + 1,
            0,
            &[55],
        );

        let result = prover.prove(&private, &public);
        assert!(
            result.is_ok(),
            "64-bit range proof should accept amounts above the old 32-bit bound"
        );
    }

    #[test]
    fn test_prover_prove_private_tx_rejects_asset_tag_mismatch() {
        let prover = Prover::new("private_tx".to_string());
        let (mut private, public) =
            encode_private_tx_case(100, 7, 20, 1, &[40, 60], 100, 0, &[55, 66]);

        // private header word #9 = output_asset_tag; make it mismatch input_asset_tag.
        private[(9 * 8)..((9 * 8) + 8)].copy_from_slice(&2u64.to_le_bytes());

        let result = prover.prove(&private, &public);
        assert!(
            result.is_err(),
            "private_tx proof should reject input/output asset tag mismatch"
        );
    }
}
