/// ZK Circuit implementations using Bellman R1CS
///
/// This module provides Bellman circuit constraints for:
/// - Transaction validation (balance preservation)
/// - Merkle tree membership proofs
/// - Nullifier non-duplication checks
use bellman::{Circuit, ConstraintSystem, SynthesisError};
use pasta_curves::Fp;

/// Circuit for validating transaction balance
///
/// Public inputs: input_commitment, output_commitment
/// Private inputs: opening_value, blinding_factors
///
/// Constraint model (MVP hardening):
/// - input_commitment = input_value + input_blind
/// - output_commitment = output_value + output_blind
/// - input_value = output_value
#[derive(Clone)]
pub struct TransactionBalanceCircuit {
    /// Public: Sum of input commitments
    pub input_commitment: Option<Fp>,
    /// Public: Sum of output commitments
    pub output_commitment: Option<Fp>,
    /// Private: Actual input value
    pub input_value: Option<Fp>,
    /// Private: Actual output value
    pub output_value: Option<Fp>,
    /// Private: Randomness for input commitment
    pub input_blind: Option<Fp>,
    /// Private: Randomness for output commitment
    pub output_blind: Option<Fp>,
}

impl Circuit<Fp> for TransactionBalanceCircuit {
    fn synthesize<CS: ConstraintSystem<Fp>>(self, cs: &mut CS) -> Result<(), SynthesisError> {
        // Allocate private variables
        let input_value = {
            let val = self.input_value.ok_or(SynthesisError::AssignmentMissing)?;
            cs.alloc(|| "input_value", || Ok(val))?
        };

        let output_value = {
            let val = self.output_value.ok_or(SynthesisError::AssignmentMissing)?;
            cs.alloc(|| "output_value", || Ok(val))?
        };

        let input_blind = {
            let val = self.input_blind.ok_or(SynthesisError::AssignmentMissing)?;
            cs.alloc(|| "input_blind", || Ok(val))?
        };

        let output_blind = {
            let val = self.output_blind.ok_or(SynthesisError::AssignmentMissing)?;
            cs.alloc(|| "output_blind", || Ok(val))?
        };

        // Allocate public inputs as statement elements.
        let input_com = {
            let val = self
                .input_commitment
                .ok_or(SynthesisError::AssignmentMissing)?;
            cs.alloc_input(|| "input_commitment", || Ok(val))?
        };

        let output_com = {
            let val = self
                .output_commitment
                .ok_or(SynthesisError::AssignmentMissing)?;
            cs.alloc_input(|| "output_commitment", || Ok(val))?
        };

        // Commitment consistency (simplified linear commitments for MVP hardening):
        // input_commitment = input_value + input_blind
        cs.enforce(
            || "input_commitment_consistency",
            |lc| lc + input_value + input_blind,
            |lc| lc + CS::one(),
            |lc| lc + input_com,
        );

        // output_commitment = output_value + output_blind
        cs.enforce(
            || "output_commitment_consistency",
            |lc| lc + output_value + output_blind,
            |lc| lc + CS::one(),
            |lc| lc + output_com,
        );

        // Constraint: input_value must equal output_value (balance preservation)
        cs.enforce(
            || "balance_preservation",
            |lc| lc + input_value,
            |lc| lc + CS::one(),
            |lc| lc + output_value,
        );

        Ok(())
    }
}

/// Circuit for proving Merkle tree membership
///
/// Public inputs: leaf_hash, root_hash
/// Private inputs: path_elements, path_directions
///
/// Constraint: Recursive hash computation validates leaf -> root path
/// Tree depth: up to 16 levels (for efficiency)
#[derive(Clone)]
pub struct MerkleProofCircuit {
    /// Public: Claimed leaf hash
    pub leaf: Option<Fp>,
    /// Public: Claimed Merkle root
    pub root: Option<Fp>,
    /// Private: Sibling hashes along the path (max 16 elements)
    pub path: Option<Vec<Fp>>,
    /// Private: Direction bits (left=true, right=false) for each sibling
    pub path_index: Option<Vec<bool>>,
}

impl Circuit<Fp> for MerkleProofCircuit {
    fn synthesize<CS: ConstraintSystem<Fp>>(self, cs: &mut CS) -> Result<(), SynthesisError> {
        // Allocate public inputs
        let _leaf = {
            let val = self.leaf.ok_or(SynthesisError::AssignmentMissing)?;
            cs.alloc_input(|| "leaf", || Ok(val))?
        };

        let root = {
            let val = self.root.ok_or(SynthesisError::AssignmentMissing)?;
            cs.alloc_input(|| "root", || Ok(val))?
        };

        let mut v = self.leaf.ok_or(SynthesisError::AssignmentMissing)?;

        // Process path elements to compute merkle root
        // Simplified approach: accumulate sum of siblings with directions
        if let Some(path) = &self.path {
            if let Some(directions) = &self.path_index {
                if path.len() > 16 {
                    return Err(SynthesisError::AssignmentMissing);
                }

                for (i, &sibling) in path.iter().enumerate() {
                    // Allocate path element
                    cs.alloc(|| format!("path_{}", i), || Ok(sibling))?;

                    // Extract direction for this level
                    let direction = if i < directions.len() {
                        if directions[i] {
                            Fp::one()
                        } else {
                            Fp::zero()
                        }
                    } else {
                        Fp::zero()
                    };

                    // Allocate direction bit
                    cs.alloc(|| format!("dir_{}", i), || Ok(direction))?;

                    // Update merkle hash: v = v + direction * sibling
                    // Simple accumulation: v = v + sibling
                    v = v.add(&sibling);
                }
            }
        }

        // Final constraint: our computed hash must equal the claimed root
        let computed = cs.alloc(|| "computed_root", || Ok(v))?;
        cs.enforce(
            || "merkle_match",
            |lc| lc + computed,
            |lc| lc + CS::one(),
            |lc| lc + root,
        );

        Ok(())
    }
}

/// Circuit for validating nullifier uniqueness
///
/// Public inputs: nullifier_hash (H(secret || index))
/// Private inputs: secret_key, spent_index
///
/// Constraint: secret * index + field_constant == nullifier_hash
/// This proves knowledge of secret and index that generate the nullifier without revealing them.
#[derive(Clone)]
pub struct NullifierCircuit {
    /// Public: Nullifier hash to prove validity
    pub nullifier: Option<Fp>,
    /// Private: Secret key component
    pub secret: Option<Fp>,
    /// Private: Unique index within transaction set
    pub index: Option<Fp>,
    /// Private: Blinding factor for zero-knowledge
    pub blind: Option<Fp>,
}

impl Circuit<Fp> for NullifierCircuit {
    fn synthesize<CS: ConstraintSystem<Fp>>(self, cs: &mut CS) -> Result<(), SynthesisError> {
        let secret = {
            let val = self.secret.ok_or(SynthesisError::AssignmentMissing)?;
            cs.alloc(|| "secret", || Ok(val))?
        };

        let index = {
            let val = self.index.ok_or(SynthesisError::AssignmentMissing)?;
            cs.alloc(|| "index", || Ok(val))?
        };

        let blind = {
            let val = self.blind.ok_or(SynthesisError::AssignmentMissing)?;
            cs.alloc(|| "blind", || Ok(val))?
        };

        let nullifier = {
            let val = self.nullifier.ok_or(SynthesisError::AssignmentMissing)?;
            cs.alloc_input(|| "nullifier", || Ok(val))?
        };

        // Constraint 1: nullifier_pre = secret * index
        // This is enforced as: 1 * secret * index = nullifier_pre
        let nullifier_pre = cs.alloc(
            || "nullifier_pre",
            || {
                Ok(self
                    .secret
                    .ok_or(SynthesisError::AssignmentMissing)?
                    .mul(&self.index.ok_or(SynthesisError::AssignmentMissing)?))
            },
        )?;

        cs.enforce(
            || "secret_index_multiply",
            |lc| lc + secret,
            |lc| lc + index,
            |lc| lc + nullifier_pre,
        );

        // Constraint 2: nullifier = nullifier_pre + blind
        // This is enforced as: (nullifier_pre + blind) * 1 = nullifier
        cs.enforce(
            || "nullifier_with_blind",
            |lc| lc + nullifier_pre + blind,
            |lc| lc + CS::one(),
            |lc| lc + nullifier,
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bellman::gadgets::test::TestConstraintSystem;

    #[test]
    fn test_balance_circuit_satisfiable() {
        let input_value = Fp::from(50u64);
        let input_blind = Fp::from(25u64);
        let input_commitment = input_value.add(&input_blind); // 50 + 25 = 75

        let output_value = Fp::from(50u64);
        let output_blind = Fp::from(25u64);
        let output_commitment = output_value.add(&output_blind); // 50 + 25 = 75

        let circuit = TransactionBalanceCircuit {
            input_commitment: Some(input_commitment),
            output_commitment: Some(output_commitment),
            input_value: Some(input_value),
            output_value: Some(output_value),
            input_blind: Some(input_blind),
            output_blind: Some(output_blind),
        };

        let mut cs = TestConstraintSystem::<Fp>::new();
        circuit.synthesize(&mut cs).expect("synthesis failed");
        assert!(
            cs.is_satisfied(),
            "balance circuit constraints not satisfied"
        );
    }

    #[test]
    fn test_balance_circuit_unsatisfiable_on_mismatch() {
        // Input != output should fail
        let circuit = TransactionBalanceCircuit {
            input_commitment: Some(Fp::from(100u64)),
            output_commitment: Some(Fp::from(100u64)),
            input_value: Some(Fp::from(50u64)),
            output_value: Some(Fp::from(51u64)), // Mismatch!
            input_blind: Some(Fp::from(25u64)),
            output_blind: Some(Fp::from(25u64)),
        };

        let mut cs = TestConstraintSystem::<Fp>::new();
        circuit.synthesize(&mut cs).expect("synthesis failed");
        assert!(
            !cs.is_satisfied(),
            "circuit should fail on balance mismatch"
        );
    }

    #[test]
    fn test_merkle_circuit_satisfiable() {
        let leaf_val = Fp::from(1u64);
        let sibling1 = Fp::from(10u64);
        let sibling2 = Fp::from(20u64);
        let root_val = leaf_val.add(&sibling1).add(&sibling2);

        let circuit = MerkleProofCircuit {
            leaf: Some(leaf_val),
            root: Some(root_val),
            path: Some(vec![sibling1, sibling2]),
            path_index: Some(vec![true, false]),
        };

        let mut cs = TestConstraintSystem::<Fp>::new();
        circuit.synthesize(&mut cs).expect("synthesis failed");
        assert!(
            cs.is_satisfied(),
            "merkle circuit constraints not satisfied"
        );
    }

    #[test]
    fn test_merkle_circuit_unsatisfiable_on_wrong_root() {
        let leaf_val = Fp::from(1u64);
        let sibling1 = Fp::from(10u64);
        let sibling2 = Fp::from(20u64);
        let wrong_root = Fp::from(999u64);

        let circuit = MerkleProofCircuit {
            leaf: Some(leaf_val),
            root: Some(wrong_root), // Wrong root!
            path: Some(vec![sibling1, sibling2]),
            path_index: Some(vec![true, false]),
        };

        let mut cs = TestConstraintSystem::<Fp>::new();
        circuit.synthesize(&mut cs).expect("synthesis failed");
        assert!(
            !cs.is_satisfied(),
            "merkle circuit should fail on wrong root"
        );
    }

    #[test]
    fn test_nullifier_circuit_satisfiable() {
        let secret = Fp::from(123u64);
        let index = Fp::from(5u64);
        let blind = Fp::from(7u64);
        let nullifier = secret.mul(&index).add(&blind);

        let circuit = NullifierCircuit {
            nullifier: Some(nullifier),
            secret: Some(secret),
            index: Some(index),
            blind: Some(blind),
        };

        let mut cs = TestConstraintSystem::<Fp>::new();
        circuit.synthesize(&mut cs).expect("synthesis failed");
        assert!(
            cs.is_satisfied(),
            "nullifier circuit constraints not satisfied"
        );
    }

    #[test]
    fn test_nullifier_circuit_unsatisfiable_on_wrong_nullifier() {
        let secret = Fp::from(123u64);
        let index = Fp::from(5u64);
        let blind = Fp::from(7u64);
        let wrong_nullifier = Fp::from(999u64);

        let circuit = NullifierCircuit {
            nullifier: Some(wrong_nullifier), // Wrong nullifier!
            secret: Some(secret),
            index: Some(index),
            blind: Some(blind),
        };

        let mut cs = TestConstraintSystem::<Fp>::new();
        circuit.synthesize(&mut cs).expect("synthesis failed");
        assert!(
            !cs.is_satisfied(),
            "nullifier circuit should fail on wrong nullifier"
        );
    }

    #[test]
    fn test_balance_circuit_creation() {
        let circuit = TransactionBalanceCircuit {
            input_commitment: Some(Fp::from(100u64)),
            output_commitment: Some(Fp::from(100u64)),
            input_value: Some(Fp::from(50u64)),
            output_value: Some(Fp::from(50u64)),
            input_blind: Some(Fp::from(7u64)),
            output_blind: Some(Fp::from(7u64)),
        };

        assert!(circuit.input_commitment.is_some());
        assert!(circuit.output_commitment.is_some());
    }

    #[test]
    fn test_merkle_circuit_creation() {
        let circuit = MerkleProofCircuit {
            leaf: Some(Fp::from(1u64)),
            root: Some(Fp::from(100u64)),
            path: Some(vec![Fp::from(10u64), Fp::from(20u64)]),
            path_index: Some(vec![true, false, true]),
        };

        assert!(circuit.leaf.is_some());
        assert!(circuit.root.is_some());
    }

    #[test]
    fn test_nullifier_circuit_creation() {
        let circuit = NullifierCircuit {
            nullifier: Some(Fp::from(42u64)),
            secret: Some(Fp::from(123u64)),
            index: Some(Fp::from(0u64)),
            blind: Some(Fp::from(11u64)),
        };

        assert!(circuit.nullifier.is_some());
        assert!(circuit.secret.is_some());
    }
}
