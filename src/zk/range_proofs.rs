use crate::types::BlockError;
use bulletproofs::{BulletproofGens, PedersenGens, RangeProof};
use curve25519_dalek::ristretto::{CompressedRistretto, RistrettoPoint};
use curve25519_dalek::scalar::Scalar;
use merlin::Transcript;

pub const LEGACY_PRIVATE_RANGE_PROOF_BITS: usize = 32;
pub const PRIVATE_RANGE_PROOF_BITS: usize = 64;

fn bulletproof_gens() -> BulletproofGens {
    BulletproofGens::new(64, 1)
}

pub fn pedersen_gens() -> PedersenGens {
    PedersenGens::default()
}

pub fn amount_commitment_point(amount: u64, blind: Scalar) -> RistrettoPoint {
    pedersen_gens().commit(Scalar::from(amount), blind)
}

pub fn amount_commitment_from_hex(commitment_hex: &str) -> Result<CompressedRistretto, BlockError> {
    let bytes = hex::decode(commitment_hex).map_err(|_| {
        BlockError::TransactionValidationFailed("amount commitment is not valid hex".to_string())
    })?;
    if bytes.len() != 32 {
        return Err(BlockError::TransactionValidationFailed(
            "amount commitment has invalid length".to_string(),
        ));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(CompressedRistretto(arr))
}

pub fn prove_output_range(amount: u64, blind: Scalar) -> Result<Vec<u8>, BlockError> {
    let mut transcript = Transcript::new(b"zoka-private-range-proof-v1");
    let (proof, _) = RangeProof::prove_single(
        &bulletproof_gens(),
        &pedersen_gens(),
        &mut transcript,
        amount,
        &blind,
        PRIVATE_RANGE_PROOF_BITS,
    )
    .map_err(|e| {
        BlockError::TransactionValidationFailed(format!(
            "failed to generate Bulletproof range proof: {}",
            e
        ))
    })?;
    Ok(proof.to_bytes())
}

pub fn verify_output_range(commitment_hex: &str, proof_bytes: &[u8]) -> Result<bool, BlockError> {
    let commitment = amount_commitment_from_hex(commitment_hex)?;
    let proof = RangeProof::from_bytes(proof_bytes).map_err(|e| {
        BlockError::TransactionValidationFailed(format!(
            "invalid Bulletproof range proof encoding: {}",
            e
        ))
    })?;
    let mut transcript = Transcript::new(b"zoka-private-range-proof-v1");
    if proof
        .verify_single(
            &bulletproof_gens(),
            &pedersen_gens(),
            &mut transcript,
            &commitment,
            PRIVATE_RANGE_PROOF_BITS,
        )
        .is_ok()
    {
        return Ok(true);
    }

    let mut legacy_transcript = Transcript::new(b"zoka-private-range-proof-v1");
    Ok(proof
        .verify_single(
            &bulletproof_gens(),
            &pedersen_gens(),
            &mut legacy_transcript,
            &commitment,
            LEGACY_PRIVATE_RANGE_PROOF_BITS,
        )
        .is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bulletproof_range_proof_roundtrip() {
        let amount = 42u64;
        let blind = Scalar::from(123456u64);
        let commitment = amount_commitment_point(amount, blind);
        let proof = prove_output_range(amount, blind).expect("range proof");

        assert!(
            verify_output_range(&hex::encode(commitment.compress().to_bytes()), &proof)
                .expect("range verification")
        );
    }

    #[test]
    fn test_bulletproof_range_proof_rejects_wrong_commitment() {
        let proof = prove_output_range(42, Scalar::from(77u64)).expect("range proof");
        let wrong_commitment = amount_commitment_point(41, Scalar::from(77u64));

        assert!(
            !verify_output_range(&hex::encode(wrong_commitment.compress().to_bytes()), &proof)
                .expect("range verification")
        );
    }
}
