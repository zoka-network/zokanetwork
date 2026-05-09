//! Commitments Pedersen-like para transacciones privadas
//!
//! Los commitments permiten comprometerse con un valor sin revelarlo
//! Propiedades:
//! - Binding: no se puede cambiar el valor comprometido
//! - Hiding: no revela informaciÃ³n del valor

use serde::{Deserialize, Serialize};

/// Commitment a un valor
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Commitment([u8; 32]);

impl Commitment {
    /// Crear commitment a partir de datos
    pub fn new(data: Vec<u8>) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&data);

        let hash = hasher.finalize();
        let mut commitment = [0u8; 32];
        commitment.copy_from_slice(hash.as_bytes());

        Commitment(commitment)
    }

    /// Crear commitment con blindness (valor secreto)
    /// C = hash(data || blindness)
    pub fn new_blinded(data: &[u8], blindness: &[u8]) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(data);
        hasher.update(blindness);

        let hash = hasher.finalize();
        let mut commitment = [0u8; 32];
        commitment.copy_from_slice(hash.as_bytes());

        Commitment(commitment)
    }

    /// Crear commitment con seed aleatorio
    pub fn new_random(data: &[u8]) -> (Self, Vec<u8>) {
        use rand::RngCore;

        let mut blindness = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut blindness);

        (Self::new_blinded(data, &blindness), blindness.to_vec())
    }

    /// Obtener bytes del commitment
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Convertir a hex string
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    /// Crear desde hex
    pub fn from_hex(hex_str: &str) -> Result<Self, hex::FromHexError> {
        let bytes = hex::decode(hex_str)?;
        if bytes.len() != 32 {
            return Err(hex::FromHexError::InvalidStringLength);
        }
        let mut commitment = [0u8; 32];
        commitment.copy_from_slice(&bytes);
        Ok(Commitment(commitment))
    }

    /// Verificar que un dato corresponde a este commitment
    /// open(data, blindness) == commitment
    pub fn verify(&self, data: &[u8], blindness: &[u8]) -> bool {
        Self::new_blinded(data, blindness) == *self
    }
}

impl std::fmt::Display for Commitment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(&self.0[..8]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_commitment_creation() {
        let data = b"test data";
        let commit = Commitment::new(data.to_vec());

        assert_eq!(commit.as_bytes().len(), 32);
    }

    #[test]
    fn test_commitment_deterministic() {
        let data = b"test data";
        let commit1 = Commitment::new(data.to_vec());
        let commit2 = Commitment::new(data.to_vec());

        assert_eq!(commit1, commit2);
    }

    #[test]
    fn test_commitment_different_inputs() {
        let commit1 = Commitment::new(b"data1".to_vec());
        let commit2 = Commitment::new(b"data2".to_vec());

        assert_ne!(commit1, commit2);
    }

    #[test]
    fn test_blinded_commitment() {
        let data = b"secret";
        let blindness = b"blind_factor";

        let commit1 = Commitment::new_blinded(data, blindness);
        let commit2 = Commitment::new_blinded(data, blindness);

        assert_eq!(commit1, commit2);

        // Debe ser diferente sin blindness
        let commit3 = Commitment::new(data.to_vec());
        assert_ne!(commit1, commit3);
    }

    #[test]
    fn test_commitment_verify() {
        let data = b"secret value";
        let blindness = b"blind_factor";

        let commit = Commitment::new_blinded(data, blindness);
        assert!(commit.verify(data, blindness));
        assert!(!commit.verify(data, b"wrong_blind"));
    }

    #[test]
    fn test_commitment_hex_conversion() {
        let commit1 = Commitment::new(b"test".to_vec());
        let hex = commit1.to_hex();
        let commit2 = Commitment::from_hex(&hex).unwrap();

        assert_eq!(commit1, commit2);
    }

    #[test]
    fn test_commitment_random() {
        let data = b"test";
        let (commit1, blind1) = Commitment::new_random(data);
        let (commit2, blind2) = Commitment::new_random(data);

        // Debe ser diferente cada vez (diferente blindness aleatorio)
        assert_ne!(commit1, commit2);
        assert_ne!(blind1, blind2);

        // Pero ambos deben verificar correctamente
        assert!(commit1.verify(data, &blind1));
        assert!(commit2.verify(data, &blind2));
    }
}
