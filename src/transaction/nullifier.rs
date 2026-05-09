//! Nullifiers para prevenir doble gasto
//!
//! Los nullifiers permiten detectar si una nota ha sido gastada sin revelar
//! cuál fue la nota original. Es un componente crítico de privacidad en Zcash.
//!
//! Modelo simplificado:
//! nullifier = hash(note_commitment || spending_key)

use serde::{Deserialize, Serialize};

/// Nullifier: evidencia criptográfica de gasto
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Nullifier([u8; 32]);

impl Nullifier {
    /// Crear nullifier a partir de datos
    pub fn new(data: Vec<u8>) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&data);

        let hash = hasher.finalize();
        let mut nullifier = [0u8; 32];
        nullifier.copy_from_slice(hash.as_bytes());

        Nullifier(nullifier)
    }

    /// Crear nullifier de commitment + spending key
    /// nullifier = hash(commitment || spending_key)
    pub fn from_commitment(commitment: &[u8], spending_key: &[u8]) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(commitment);
        hasher.update(spending_key);

        let hash = hasher.finalize();
        let mut nullifier = [0u8; 32];
        nullifier.copy_from_slice(hash.as_bytes());

        Nullifier(nullifier)
    }

    /// Obtener bytes
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Convertir a hex
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    /// Crear desde hex
    pub fn from_hex(hex_str: &str) -> Result<Self, hex::FromHexError> {
        let bytes = hex::decode(hex_str)?;
        if bytes.len() != 32 {
            return Err(hex::FromHexError::InvalidStringLength);
        }
        let mut nullifier = [0u8; 32];
        nullifier.copy_from_slice(&bytes);
        Ok(Nullifier(nullifier))
    }
}

impl std::fmt::Display for Nullifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(&self.0[..8]))
    }
}

/// Registro de nullifiers gastados (previene doble gasto)
#[derive(Clone, Debug, Default)]
pub struct NullifierSet {
    /// Conjunto de nullifiers ya gastados
    spent: std::collections::HashSet<Nullifier>,
}

impl NullifierSet {
    /// Crear nuevo registro vacío
    pub fn new() -> Self {
        Self {
            spent: std::collections::HashSet::new(),
        }
    }

    /// Marcar nullifier como gastado
    pub fn spend(&mut self, nullifier: Nullifier) -> Result<(), String> {
        if self.spent.contains(&nullifier) {
            return Err("Nullifier ya ha sido gastado (doble gasto detectado)".to_string());
        }

        self.spent.insert(nullifier);
        Ok(())
    }

    /// Verificar si nullifier ya fue gastado
    pub fn is_spent(&self, nullifier: &Nullifier) -> bool {
        self.spent.contains(nullifier)
    }

    /// Obtener número de notas gastadas
    pub fn spent_count(&self) -> usize {
        self.spent.len()
    }

    /// Gastar múltiples nullifiers
    pub fn spend_many(&mut self, nullifiers: Vec<Nullifier>) -> Result<(), String> {
        for nf in nullifiers {
            self.spend(nf)?;
        }
        Ok(())
    }

    /// Verificar que ninguno de estos nullifiers ha sido gastado
    pub fn verify_not_spent(&self, nullifiers: &[Nullifier]) -> Result<(), String> {
        for nf in nullifiers {
            if self.is_spent(nf) {
                return Err(format!("Nullifier {} ya fue gastado", nf));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nullifier_creation() {
        let nf = Nullifier::new(vec![1, 2, 3]);
        assert_eq!(nf.as_bytes().len(), 32);
    }

    #[test]
    fn test_nullifier_deterministic() {
        let data = vec![1, 2, 3];
        let nf1 = Nullifier::new(data.clone());
        let nf2 = Nullifier::new(data);

        assert_eq!(nf1, nf2);
    }

    #[test]
    fn test_nullifier_from_commitment() {
        let commitment = vec![1, 2, 3];
        let spending_key = vec![4, 5, 6];

        let nf1 = Nullifier::from_commitment(&commitment, &spending_key);
        let nf2 = Nullifier::from_commitment(&commitment, &spending_key);

        assert_eq!(nf1, nf2);

        // Diferentes si cambia spending_key
        let nf3 = Nullifier::from_commitment(&commitment, b"different");
        assert_ne!(nf1, nf3);
    }

    #[test]
    fn test_nullifier_hex() {
        let nf1 = Nullifier::new(vec![42; 16]);
        let hex = nf1.to_hex();
        let nf2 = Nullifier::from_hex(&hex).unwrap();

        assert_eq!(nf1, nf2);
    }

    #[test]
    fn test_nullifier_set_spend() {
        let mut set = NullifierSet::new();
        let nf = Nullifier::new(vec![1, 2, 3]);

        // Primer gasto debe ser OK
        assert!(set.spend(nf.clone()).is_ok());

        // Segundo gasto del mismo nullifier debe fallar
        assert!(set.spend(nf).is_err());
    }

    #[test]
    fn test_nullifier_set_is_spent() {
        let mut set = NullifierSet::new();
        let nf1 = Nullifier::new(vec![1, 2, 3]);
        let nf2 = Nullifier::new(vec![4, 5, 6]);

        set.spend(nf1.clone()).ok();

        assert!(set.is_spent(&nf1));
        assert!(!set.is_spent(&nf2));
    }

    #[test]
    fn test_nullifier_set_spending_many() {
        let mut set = NullifierSet::new();

        let nullifiers = vec![
            Nullifier::new(vec![1, 2, 3]),
            Nullifier::new(vec![4, 5, 6]),
            Nullifier::new(vec![7, 8, 9]),
        ];

        assert!(set.spend_many(nullifiers.clone()).is_ok());
        assert_eq!(set.spent_count(), 3);

        // Intentar gastar de nuevo debe fallar
        assert!(set.spend_many(nullifiers).is_err());
    }

    #[test]
    fn test_nullifier_set_verify_not_spent() {
        let mut set = NullifierSet::new();

        let nf1 = Nullifier::new(vec![1, 2, 3]);
        let nf2 = Nullifier::new(vec![4, 5, 6]);

        set.spend(nf1.clone()).ok();

        // Verificar nf1 debe fallar
        assert!(set.verify_not_spent(&[nf1]).is_err());

        // Verificar nf2 debe pasar
        assert!(set.verify_not_spent(&[nf2]).is_ok());
    }
}
