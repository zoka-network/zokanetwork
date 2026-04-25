//! Protocol de Mensajes Privados para ZOKA
//!
//! Define los tipos de mensajes que viajan por la red P2P

use crate::consensus::{PbftMessage, PbftNewView, PbftViewChange};
use serde::{Deserialize, Serialize};

/// Tipos de mensajes en ZOKA
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ZokaMessage {
    /// Transacción encriptada
    Transaction(crate::types::Transaction),
    /// Transacción privada
    PrivateTransaction(PrivateTransactionMessage),
    /// Solicitud de bloque
    BlockRequest { block_height: u64 },
    /// Respuesta con bloque
    BlockResponse(BlockResponseMessage),
    /// Estado sincronizado
    SyncState(SyncStateMessage),
    /// Keep-alive / heartbeat
    Ping(PingMessage),
    /// Mensajes de consenso PBFT
    Pbft(PbftNetworkMessage),
    /// Registro de usuario (para descubrimiento)
    UserRegistry {
        username: String,
        public_address: String,
        commitment_key: String,
        peer_id: String,
        #[serde(default)]
        listen_addr: Option<String>,
    },
    /// Consulta para encontrar usuario
    UserQuery { username: String },
    /// Respuesta de consulta de usuario
    UserQueryResponse {
        username: String,
        public_address: String,
        commitment_key: String,
        peer_id: String,
        #[serde(default)]
        listen_addr: Option<String>,
    },
}

/// Mensaje de transacción privada
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PrivateTransactionMessage {
    /// Hash único de la transacción
    pub tx_hash: String,
    /// Commitment (cantidad oculta)
    pub commitment: String,
    /// Nullifier (previene double-spend)
    pub nullifier: String,
    /// Prueba ZK-SNARK
    pub zk_proof: Option<Vec<u8>>,
    /// Timestamp del envío
    pub timestamp: u64,
}

/// Mensaje de transacción privada
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TransactionMessage {
    /// Hash único de la transacción
    pub tx_hash: String,
    /// Datos de transacción encriptados
    pub encrypted_data: Vec<u8>,
    /// Prueba criptográfica zk (opcional)
    pub zk_proof: Option<Vec<u8>>,
    /// Timestamp del envío
    pub timestamp: u64,
    /// Firma del remitente
    pub signature: Vec<u8>,
}

/// Solicitud de bloque privada
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlockRequestMessage {
    /// Altura del bloque solicitado
    pub block_height: u64,
    /// Hash del bloque (si se conoce)
    pub block_hash: Option<String>,
    /// Timestamp
    pub timestamp: u64,
}

/// Respuesta con bloque
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlockResponseMessage {
    /// Altura del bloque
    pub block_height: u64,
    /// Datos del bloque encriptados
    pub encrypted_block: Vec<u8>,
    /// Hash de verificación
    pub block_hash: String,
    /// Timestamp
    pub timestamp: u64,
}

/// Mensaje de sincronización de estado
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncStateMessage {
    /// Última altura conocida
    pub last_height: u64,
    /// Raíz de estado
    pub state_root: String,
    /// Timestamp
    pub timestamp: u64,
}

/// Keep-alive ping
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PingMessage {
    /// Nonce para evitar replay attacks
    pub nonce: u64,
    /// Timestamp
    pub timestamp: u64,
}

/// User discovery record persisted in DHT.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserDiscoveryRecord {
    pub username: String,
    pub public_address: String,
    pub commitment_key: String,
    pub peer_id: String,
    pub listen_addr: Option<String>,
    pub updated_at: u64,
}

/// Mensajes de consenso PBFT sobre la red
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum PbftNetworkMessage {
    /// Flujo principal PBFT (pre-prepare/prepare/commit)
    Consensus(PbftMessage),
    /// Solicitud de cambio de vista por timeout/fallo
    ViewChange(PbftViewChange),
    /// Anuncio de nueva vista por el nuevo primario
    NewView(PbftNewView),
}

impl ZokaMessage {
    /// Obtener timestamp del mensaje
    pub fn timestamp(&self) -> u64 {
        match self {
            ZokaMessage::Transaction(_msg) => 0, // Would get from tx
            ZokaMessage::PrivateTransaction(msg) => msg.timestamp,
            ZokaMessage::BlockRequest { .. } => 0,
            ZokaMessage::BlockResponse(msg) => msg.timestamp,
            ZokaMessage::SyncState(msg) => msg.timestamp,
            ZokaMessage::Ping(msg) => msg.timestamp,
            ZokaMessage::Pbft(msg) => match msg {
                PbftNetworkMessage::Consensus(inner) => inner.timestamp_ms,
                PbftNetworkMessage::ViewChange(inner) => inner.timestamp_ms,
                PbftNetworkMessage::NewView(inner) => inner.timestamp_ms,
            },
            ZokaMessage::UserRegistry { .. } => 0,
            ZokaMessage::UserQuery { .. } => 0,
            ZokaMessage::UserQueryResponse { .. } => 0,
        }
    }

    /// Serializar mensaje completo
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
    use crate::types::{Signature, Transaction};

    #[test]
    fn test_transaction_message() {
        let tx = Transaction {
            hash: "0xabc123".to_string(),
            sender: "sender_addr".to_string(),
            receiver: "receiver_addr".to_string(),
            amount: 1000,
            nonce: 0,
            timestamp: 1234567890,
            signature: Signature(vec![10, 11, 12]),
            zk_proof: Some(vec![5, 6, 7]),
        };

        let tx_msg = ZokaMessage::Transaction(tx);
        let bytes = tx_msg.to_bytes().unwrap();
        let restored = ZokaMessage::from_bytes(&bytes).unwrap();

        assert!(matches!(restored, ZokaMessage::Transaction(_)));
    }

    #[test]
    fn test_message_timestamp() {
        let ping = PingMessage {
            nonce: 42,
            timestamp: 999,
        };

        let msg = ZokaMessage::Ping(ping);
        assert_eq!(msg.timestamp(), 999);
    }

    #[test]
    fn test_block_request() {
        let block_msg = ZokaMessage::BlockRequest { block_height: 100 };
        let bytes = block_msg.to_bytes().unwrap();

        assert!(!bytes.is_empty());
    }

    #[test]
    fn test_sync_state_message() {
        let msg = SyncStateMessage {
            last_height: 500,
            state_root: "0xstate123".to_string(),
            timestamp: 1234567890,
        };

        let sync_msg = ZokaMessage::SyncState(msg);
        assert_eq!(sync_msg.timestamp(), 1234567890);
    }

    #[test]
    fn test_pbft_message_roundtrip() {
        use crate::consensus::PbftMessageType;

        let pbft = PbftMessage::with_timestamp(
            PbftMessageType::Prepare,
            2,
            10,
            "0xpbft".to_string(),
            "node-1".to_string(),
            777,
        );
        let msg = ZokaMessage::Pbft(PbftNetworkMessage::Consensus(pbft));
        let bytes = msg.to_bytes().unwrap();
        let restored = ZokaMessage::from_bytes(&bytes).unwrap();

        assert_eq!(restored.timestamp(), 777);
        assert!(matches!(restored, ZokaMessage::Pbft(_)));
    }
}
