//! Transacción Privada Encriptada

use super::{Commitment, Nullifier, TransactionStatus};
use crate::network::message::TransactionMessage;
use serde::{Deserialize, Serialize};

/// Transacción privada completamente encriptada
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PrivateTransaction {
    /// Identificador único (hash)
    pub id: String,

    /// Commitments: suma oculta de inputs
    pub input_commitment: Commitment,

    /// Commitments: suma oculta de outputs
    pub output_commitment: Commitment,

    /// Nullifiers para prevenir doble gasto
    pub nullifiers: Vec<Nullifier>,

    /// Datos de transacción encriptados (AES-256-GCM)
    pub encrypted_data: Vec<u8>,

    /// Prueba zk de validez (opcional pero recomendada)
    pub zk_proof: Option<Vec<u8>>,

    /// Firma del remitente (verificación de autorización)
    pub signature: Vec<u8>,

    /// Timestamp de creación
    pub timestamp: u64,

    /// Nonce para prevenirtransmisión replay
    pub nonce: u64,

    /// Estado de la transacción
    pub status: TransactionStatus,

    /// Altura de bloque donde fue confirmada (0 si no está confirmada)
    pub block_height: u64,
}

impl PrivateTransaction {
    /// Crear nueva transacción privada
    pub fn new(
        input_commitment: Commitment,
        output_commitment: Commitment,
        encrypted_data: Vec<u8>,
        signature: Vec<u8>,
        nonce: u64,
    ) -> Self {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Hash = blake3(inputs || outputs || timestamp || nonce)
        let mut hasher = blake3::Hasher::new();
        hasher.update(input_commitment.as_bytes());
        hasher.update(output_commitment.as_bytes());
        hasher.update(&timestamp.to_le_bytes());
        hasher.update(&nonce.to_le_bytes());

        let id = hex::encode(hasher.finalize().as_bytes());

        Self {
            id,
            input_commitment,
            output_commitment,
            nullifiers: Vec::new(),
            encrypted_data,
            zk_proof: None,
            signature,
            timestamp,
            nonce,
            status: TransactionStatus::Pending,
            block_height: 0,
        }
    }

    /// Agregar nullifier (prevenir doble gasto)
    pub fn add_nullifier(&mut self, nullifier: Nullifier) {
        self.nullifiers.push(nullifier);
    }

    /// Agregar varios nullifiers
    pub fn add_nullifiers(&mut self, nullifiers: Vec<Nullifier>) {
        self.nullifiers.extend(nullifiers);
    }

    /// Agregar prueba zk
    pub fn set_zk_proof(&mut self, proof: Vec<u8>) {
        self.zk_proof = Some(proof);
    }

    /// Obtener tamaño estimado en bytes
    pub fn estimated_size(&self) -> usize {
        self.id.len()
            + self.input_commitment.as_bytes().len()
            + self.output_commitment.as_bytes().len()
            + self.nullifiers.len() * 32  // ~32 bytes per nullifier
            + self.encrypted_data.len()
            + self.zk_proof.as_ref().map(|p| p.len()).unwrap_or(0)
            + self.signature.len()
            + 128 // overhead
    }

    /// Convertir a mensaje de red
    pub fn to_network_message(&self) -> TransactionMessage {
        TransactionMessage {
            tx_hash: self.id.clone(),
            encrypted_data: self.encrypted_data.clone(),
            zk_proof: self.zk_proof.clone(),
            timestamp: self.timestamp,
            signature: self.signature.clone(),
        }
    }

    /// Marcar como validada
    pub fn mark_validated(&mut self) {
        self.status = TransactionStatus::Validated;
    }

    /// Marcar como confirmada
    pub fn mark_confirmed(&mut self, block_height: u64) {
        self.status = TransactionStatus::Confirmed;
        self.block_height = block_height;
    }

    /// Marcar como rechazada
    pub fn mark_rejected(&mut self) {
        self.status = TransactionStatus::Rejected;
    }

    /// Obtener serialización compacta
    pub fn to_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    /// Deserializar desde bytes
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transaction::Commitment;

    #[test]
    fn test_create_private_transaction() {
        let input_commit = Commitment::new(vec![1, 2, 3]);
        let output_commit = Commitment::new(vec![4, 5, 6]);

        let tx = PrivateTransaction::new(
            input_commit,
            output_commit,
            vec![10, 11, 12],
            vec![20, 21, 22],
            42,
        );

        assert_eq!(tx.status, TransactionStatus::Pending);
        assert_eq!(tx.nonce, 42);
        assert_eq!(tx.block_height, 0);
        assert!(!tx.id.is_empty());
    }

    #[test]
    fn test_transaction_status_changes() {
        let input_commit = Commitment::new(vec![1, 2, 3]);
        let output_commit = Commitment::new(vec![4, 5, 6]);

        let mut tx = PrivateTransaction::new(input_commit, output_commit, vec![], vec![], 0);

        assert_eq!(tx.status, TransactionStatus::Pending);

        tx.mark_validated();
        assert_eq!(tx.status, TransactionStatus::Validated);

        tx.mark_confirmed(100);
        assert_eq!(tx.status, TransactionStatus::Confirmed);
        assert_eq!(tx.block_height, 100);
    }

    #[test]
    fn test_transaction_serialization() {
        let input_commit = Commitment::new(vec![1, 2, 3]);
        let output_commit = Commitment::new(vec![4, 5, 6]);

        let tx =
            PrivateTransaction::new(input_commit, output_commit, vec![10, 20], vec![30, 40], 99);

        let bytes = tx.to_bytes().unwrap();
        let restored = PrivateTransaction::from_bytes(&bytes).unwrap();

        assert_eq!(tx.id, restored.id);
        assert_eq!(tx.nonce, restored.nonce);
    }

    #[test]
    fn test_nullifier_management() {
        let input_commit = Commitment::new(vec![1, 2, 3]);
        let output_commit = Commitment::new(vec![4, 5, 6]);

        let mut tx = PrivateTransaction::new(input_commit, output_commit, vec![], vec![], 0);

        let nullifier1 = Nullifier::new(vec![1, 2, 3]);
        let nullifier2 = Nullifier::new(vec![4, 5, 6]);

        tx.add_nullifier(nullifier1);
        tx.add_nullifier(nullifier2);

        assert_eq!(tx.nullifiers.len(), 2);
    }
}
