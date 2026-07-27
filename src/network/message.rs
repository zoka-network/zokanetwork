//! Protocol de Mensajes Privados para ZOKA
//!
//! Define los tipos de mensajes que viajan por la red P2P

use crate::consensus::{PbftMessage, PbftNewView, PbftViewChange};
use crate::private_wallet::{PRIVATE_PROOF_PROFILE_V1, PRIVATE_PROOF_VERSION_V1};
use crate::state::PrivateCommitmentInclusionProof;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

fn default_user_query_found() -> bool {
    true
}

fn default_ring_version() -> u8 {
    1
}

fn default_private_proof_version() -> u8 {
    PRIVATE_PROOF_VERSION_V1
}

fn default_private_proof_profile() -> String {
    PRIVATE_PROOF_PROFILE_V1.to_string()
}

fn default_asset_tag() -> String {
    "ZOKA".to_string()
}

/// Tipos de mensajes en ZOKA
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ZokaMessage {
    /// Transacción encriptada
    Transaction(crate::types::Transaction),
    /// Transacción privada
    PrivateTransaction(PrivateTransactionMessage),
    /// Anuncio liviano de disponibilidad de cuerpo de transaccion privada.
    PrivateTxAnnounce(PrivateTxAnnounceMessage),
    StemPrivateTransaction(StemPrivateTransactionMessage),
    PrivateMixerBatch(MixerBatchMessage),
    /// Solicitud de bloque
    BlockRequest {
        block_height: u64,
        #[serde(default)]
        block_hash: Option<String>,
    },
    /// Respuesta con bloque
    BlockResponse(BlockResponseMessage),
    /// Anuncio liviano de bloque disponible.
    BlockAnnounce(BlockAnnounceMessage),
    /// Estado sincronizado
    SyncState(SyncStateMessage),
    /// Keep-alive / heartbeat
    Ping(PingMessage),
    /// Mensajes de consenso PBFT
    Pbft(PbftNetworkMessage),
    /// Registro de usuario (anuncio minimizado para descubrimiento)
    UserRegistry {
        username: String,
        #[serde(default)]
        record_digest: String,
        #[serde(default)]
        updated_at: u64,
    },
    /// Consulta para encontrar usuario
    UserQuery {
        username: String,
    },
    /// Anuncio de una oferta ZKA → USDT.
    ///
    /// Viaja por su propio tópico gossipsub, no por el general: los nodos
    /// viejos no conocen esta variante y la descartarían con un `warn!` por
    /// cada oferta, llenando sus logs. Con tópico propio ni se suscriben.
    OfferAnnounce(OfferAnnounceMessage),
    /// Cambio de estado de una oferta ya anunciada (tomada, expirada,
    /// cancelada, liberada).
    OfferStatusUpdate(OfferStatusUpdateMessage),
    /// Public evidence of a paid trade the seller did not release.
    OfferPaymentFailure(crate::p2p_exchange::PaymentFailureReport),
    /// Respuesta de consulta (sin exponer metadata sensible)
    UserQueryResponse {
        username: String,
        #[serde(default = "default_user_query_found")]
        found: bool,
        #[serde(default)]
        record_digest: String,
        #[serde(default)]
        updated_at: u64,
    },
}

/// Anuncio de oferta ZKA → USDT.
///
/// El nodo receptor valida `offer` antes de retransmitir. La firma es del
/// vendedor sobre el id de la oferta: sin ella cualquiera podría publicar
/// ofertas a nombre ajeno o cancelar las de otro.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OfferAnnounceMessage {
    pub offer: crate::p2p_exchange::Offer,
    #[serde(default)]
    pub signature: Vec<u8>,
    pub timestamp: u64,
}

/// Cambio de estado de una oferta ya conocida.
///
/// Lleva solo el id y el estado nuevo en vez de la oferta entera: el receptor
/// ya tiene el cuerpo del `OfferAnnounce`, y reenviarlo permitiría a un peer
/// alterar montos o direcciones camuflados en una actualización de estado.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OfferStatusUpdateMessage {
    pub offer_id: String,
    pub seller_address: String,
    pub status: crate::p2p_exchange::OfferStatus,
    #[serde(default)]
    pub claimed_by: Option<String>,
    #[serde(default)]
    pub claim_expires_at: Option<u64>,
    #[serde(default)]
    pub signature: Vec<u8>,
    pub timestamp: u64,
}

/// Mensaje de transacción privada
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PrivateOutputMessage {
    #[serde(default)]
    pub output_id: String,
    #[serde(default = "default_asset_tag")]
    pub asset_tag: String,
    #[serde(default)]
    pub asset_tag_commitment: String,
    #[serde(default)]
    pub recipient_commitment: String,
    #[serde(default)]
    pub stealth_address: String,
    #[serde(default)]
    pub ephemeral_pubkey: Vec<u8>,
    #[serde(default)]
    pub commitment: String,
    #[serde(default)]
    pub amount_commitment: String,
    #[serde(default)]
    pub range_proof: Vec<u8>,
    #[serde(default)]
    pub one_time_public_key: String,
    #[serde(default)]
    pub encrypted_amount: Vec<u8>,
    #[serde(default)]
    pub commitment_blind: Vec<u8>,
    #[serde(default)]
    pub is_change: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AssetBalanceCommitmentMessage {
    #[serde(default = "default_asset_tag")]
    pub asset_tag: String,
    #[serde(default)]
    pub balance_commitment: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PrivateTransactionMessage {
    /// Hash único de la transacción
    pub tx_hash: String,
    /// Commitment (cantidad oculta)
    pub commitment: String,
    #[serde(default = "default_private_proof_version")]
    pub private_proof_version: u8,
    #[serde(default = "default_private_proof_profile")]
    pub private_proof_profile: String,
    #[serde(default)]
    pub stealth_address: String,
    #[serde(default)]
    pub ephemeral_pubkey: Vec<u8>,
    /// Nullifier (previene double-spend)
    pub nullifier: String,
    /// Prueba ZK-SNARK
    pub zk_proof: Option<Vec<u8>>,
    /// Timestamp del envío
    pub timestamp: u64,
    /// Recipient commitment identifier (who can decrypt)
    #[serde(default)]
    pub recipient_commitment: String,
    /// Encrypted amount payload for receiver
    #[serde(default)]
    pub encrypted_amount: Vec<u8>,
    /// Groth16 public input #1
    #[serde(default)]
    pub proof_commitment: u64,
    /// Groth16 public input #2
    #[serde(default)]
    pub proof_nullifier: u64,
    #[serde(default)]
    pub proof_input_sum: u64,
    #[serde(default)]
    pub proof_output_sum: u64,
    #[serde(default)]
    pub proof_fee: u64,
    #[serde(default)]
    pub proof_key_image: u64,
    #[serde(default)]
    pub proof_private_root_words: Vec<u64>,
    #[serde(default)]
    pub fee: u64,
    #[serde(default)]
    pub ring_id: String,
    #[serde(default = "default_ring_version")]
    pub ring_version: u8,
    #[serde(default)]
    pub ring_members: Vec<String>,
    #[serde(default)]
    pub ring_signature: Vec<u8>,
    #[serde(default)]
    pub input_nullifiers: Vec<String>,
    #[serde(default)]
    pub input_commitments: Vec<String>,
    #[serde(default)]
    pub input_asset_tags: Vec<String>,
    #[serde(default)]
    pub input_public_keys: Vec<String>,
    #[serde(default)]
    pub input_key_images: Vec<String>,
    #[serde(default)]
    pub input_amount_commitments: Vec<String>,
    #[serde(default)]
    pub amount_balance_commitment: String,
    #[serde(default = "default_asset_tag")]
    pub fee_asset_tag: String,
    #[serde(default)]
    pub asset_balance_commitments: Vec<AssetBalanceCommitmentMessage>,
    #[serde(default)]
    pub fee_commitment: String,
    #[serde(default)]
    pub fee_range_proof: Vec<u8>,
    #[serde(default)]
    pub private_commitment_root: String,
    #[serde(default)]
    pub input_inclusion_proofs: Vec<PrivateCommitmentInclusionProof>,
    #[serde(default)]
    pub outputs: Vec<PrivateOutputMessage>,
    /// Commitment blindness factor (needed by receiver to verify)
    #[serde(default)]
    pub commitment_blind: Vec<u8>,
    /// Nullifier nonce (needed for reconstruction)
    #[serde(default)]
    pub nullifier_nonce: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PrivateTxAnnounceMessage {
    pub tx_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_size_bytes: Option<u64>,
    #[serde(default = "default_private_proof_profile")]
    pub private_proof_profile: String,
    pub timestamp: u64,
}

impl PrivateTxAnnounceMessage {
    pub fn from_private_tx(tx: &PrivateTransactionMessage, body_size_bytes: Option<u64>) -> Self {
        Self {
            tx_hash: tx.tx_hash.clone(),
            body_size_bytes,
            private_proof_profile: tx.private_proof_profile.clone(),
            timestamp: tx.timestamp,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StemPrivateTransactionMessage {
    pub route_id: String,
    #[serde(default)]
    pub relay_hint: String,
    pub stem_hops: u8,
    #[serde(default)]
    pub padding: Vec<u8>,
    pub private_tx: PrivateTransactionMessage,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MixerBatchMessage {
    pub batch_id: String,
    pub txs: Vec<PrivateTransactionMessage>,
    #[serde(default)]
    pub padding: Vec<u8>,
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
    /// Historical difficulty bits used when this block was accepted.
    ///
    /// Older peer messages may omit this. RPC sync uses it to replay the
    /// already-published chain history exactly, even across rehearsal-era
    /// difficulty changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub difficulty_bits: Option<u32>,
    /// Serialized PrivateTransaction bodies for every `private_tx_hash`
    /// committed in this block that the publisher had locally. Receivers
    /// verify each body's `tx_hash` matches a hash in the block and then
    /// persist it under `priv_tx:<hash>` so future wallet scanners can
    /// recover spend nullifiers and ownership without needing every
    /// original sender to POST to a central archive.
    ///
    /// Backwards-compatible: older peers omit this field and receive
    /// `Vec::new()`. Older receivers ignore it entirely.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub private_tx_bodies: Vec<Vec<u8>>,
    /// Timestamp
    pub timestamp: u64,
}

/// Mensaje de sincronización de estado
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlockAnnounceMessage {
    pub block_height: u64,
    pub block_hash: String,
    #[serde(default)]
    pub cumulative_work: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub difficulty_bits: Option<u32>,
    pub timestamp: u64,
}

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

impl UserDiscoveryRecord {
    /// Deterministic digest for lightweight discovery announcements.
    pub fn digest(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.username.as_bytes());
        hasher.update([0x1f]);
        hasher.update(self.public_address.as_bytes());
        hasher.update([0x1f]);
        hasher.update(self.commitment_key.as_bytes());
        hasher.update([0x1f]);
        hasher.update(self.peer_id.as_bytes());
        hasher.update([0x1f]);
        if let Some(addr) = &self.listen_addr {
            hasher.update(addr.as_bytes());
        }
        hasher.update([0x1f]);
        hasher.update(self.updated_at.to_le_bytes());
        hex::encode(hasher.finalize())
    }
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
            ZokaMessage::PrivateTxAnnounce(msg) => msg.timestamp,
            ZokaMessage::StemPrivateTransaction(msg) => msg.private_tx.timestamp,
            ZokaMessage::PrivateMixerBatch(msg) => msg.timestamp,
            ZokaMessage::BlockRequest { .. } => 0,
            ZokaMessage::BlockResponse(msg) => msg.timestamp,
            ZokaMessage::BlockAnnounce(msg) => msg.timestamp,
            ZokaMessage::SyncState(msg) => msg.timestamp,
            ZokaMessage::Ping(msg) => msg.timestamp,
            ZokaMessage::Pbft(msg) => match msg {
                PbftNetworkMessage::Consensus(inner) => inner.timestamp_ms,
                PbftNetworkMessage::ViewChange(inner) => inner.timestamp_ms,
                PbftNetworkMessage::NewView(inner) => inner.timestamp_ms,
            },
            ZokaMessage::OfferAnnounce(msg) => msg.timestamp,
            ZokaMessage::OfferStatusUpdate(msg) => msg.timestamp,
            ZokaMessage::OfferPaymentFailure(report) => report.timestamp,
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
    use serde_json::json;

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
        };

        let tx_msg = ZokaMessage::Transaction(tx);
        let bytes = tx_msg.to_bytes().unwrap();
        let restored = ZokaMessage::from_bytes(&bytes).unwrap();

        assert!(matches!(restored, ZokaMessage::Transaction(_)));
    }

    #[test]
    fn test_private_transaction_message_roundtrip_with_ring_metadata() {
        let private_tx = PrivateTransactionMessage {
            tx_hash: "0xpriv".to_string(),
            commitment: "commitment".to_string(),
            private_proof_version: PRIVATE_PROOF_VERSION_V1,
            private_proof_profile: PRIVATE_PROOF_PROFILE_V1.to_string(),
            stealth_address: "stlth_abc".to_string(),
            ephemeral_pubkey: vec![1, 2, 3],
            nullifier: "nullifier".to_string(),
            zk_proof: Some(vec![9, 9, 9]),
            timestamp: 123,
            recipient_commitment: "recipient".to_string(),
            encrypted_amount: vec![7, 7, 7, 7, 7, 7, 7, 7],
            proof_commitment: 11,
            proof_nullifier: 22,
            proof_input_sum: 33,
            proof_output_sum: 33,
            proof_fee: 0,
            proof_key_image: 44,
            proof_private_root_words: vec![1, 2, 3, 4],
            fee: 0,
            ring_id: "ring-id".to_string(),
            ring_version: 1,
            ring_members: vec!["member-a".to_string(), "member-b".to_string()],
            ring_signature: vec![4u8; 128],
            input_nullifiers: vec!["nullifier".to_string()],
            input_commitments: vec!["commitment".to_string()],
            input_asset_tags: vec!["ZOKA".to_string()],
            outputs: vec![PrivateOutputMessage {
                output_id: "out-1".to_string(),
                asset_tag: "ZOKA".to_string(),
                asset_tag_commitment: "asset-tag-commitment".to_string(),
                recipient_commitment: "recipient".to_string(),
                stealth_address: "stlth_abc".to_string(),
                ephemeral_pubkey: vec![1, 2, 3],
                commitment: "commitment".to_string(),
                amount_commitment: "amount-commitment".to_string(),
                range_proof: vec![8u8; 32],
                one_time_public_key: "one-time-key".to_string(),
                encrypted_amount: vec![7, 7, 7, 7, 7, 7, 7, 7],
                commitment_blind: vec![5u8; 32],
                is_change: false,
            }],
            input_public_keys: vec!["input-pub-1".to_string()],
            input_key_images: vec!["key-image-1".to_string()],
            input_amount_commitments: vec!["amount-commitment-in".to_string()],
            amount_balance_commitment: "balance-commitment".to_string(),
            fee_asset_tag: "ZOKA".to_string(),
            asset_balance_commitments: vec![AssetBalanceCommitmentMessage {
                asset_tag: "ZOKA".to_string(),
                balance_commitment: "balance-commitment".to_string(),
            }],
            fee_commitment: String::new(),
            fee_range_proof: Vec::new(),
            private_commitment_root: String::new(),
            input_inclusion_proofs: vec![],
            commitment_blind: vec![5u8; 32],
            nullifier_nonce: vec![6u8; 16],
        };

        let msg = ZokaMessage::PrivateTransaction(private_tx);
        let bytes = msg.to_bytes().unwrap();
        let restored = ZokaMessage::from_bytes(&bytes).unwrap();

        match restored {
            ZokaMessage::PrivateTransaction(restored) => {
                assert_eq!(restored.ring_id, "ring-id");
                assert_eq!(restored.private_proof_version, PRIVATE_PROOF_VERSION_V1);
                assert_eq!(restored.private_proof_profile, PRIVATE_PROOF_PROFILE_V1);
                assert_eq!(restored.ring_members.len(), 2);
                assert_eq!(restored.ring_signature.len(), 128);
                assert_eq!(restored.proof_commitment, 11);
                assert_eq!(restored.proof_input_sum, 33);
                assert_eq!(restored.proof_private_root_words.len(), 4);
                assert_eq!(restored.outputs.len(), 1);
            }
            other => panic!("unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn test_stem_private_transaction_message_roundtrip() {
        let stem = StemPrivateTransactionMessage {
            route_id: "route-1".to_string(),
            relay_hint: "peer-a".to_string(),
            stem_hops: 2,
            padding: vec![0u8; 32],
            private_tx: PrivateTransactionMessage {
                tx_hash: "0xpriv".to_string(),
                commitment: "commitment".to_string(),
                private_proof_version: PRIVATE_PROOF_VERSION_V1,
                private_proof_profile: PRIVATE_PROOF_PROFILE_V1.to_string(),
                stealth_address: "stlth_abc".to_string(),
                ephemeral_pubkey: vec![1, 2, 3],
                nullifier: "nullifier".to_string(),
                zk_proof: Some(vec![9, 9, 9]),
                timestamp: 123,
                recipient_commitment: "recipient".to_string(),
                encrypted_amount: vec![7, 7, 7, 7, 7, 7, 7, 7],
                proof_commitment: 11,
                proof_nullifier: 22,
                proof_input_sum: 33,
                proof_output_sum: 33,
                proof_fee: 0,
                proof_key_image: 44,
                proof_private_root_words: vec![1, 2, 3, 4],
                fee: 0,
                ring_id: "ring-id".to_string(),
                ring_version: 1,
                ring_members: vec!["member-a".to_string(), "member-b".to_string()],
                ring_signature: vec![4u8; 128],
                input_nullifiers: vec!["nullifier".to_string()],
                input_commitments: vec!["commitment".to_string()],
                input_asset_tags: vec!["ZOKA".to_string()],
                outputs: vec![PrivateOutputMessage {
                    output_id: "out-1".to_string(),
                    asset_tag: "ZOKA".to_string(),
                    asset_tag_commitment: "asset-tag-commitment".to_string(),
                    recipient_commitment: "recipient".to_string(),
                    stealth_address: "stlth_abc".to_string(),
                    ephemeral_pubkey: vec![1, 2, 3],
                    commitment: "commitment".to_string(),
                    amount_commitment: "amount-commitment".to_string(),
                    range_proof: vec![8u8; 32],
                    one_time_public_key: "one-time-key".to_string(),
                    encrypted_amount: vec![7, 7, 7, 7, 7, 7, 7, 7],
                    commitment_blind: vec![5u8; 32],
                    is_change: false,
                }],
                input_public_keys: vec!["input-pub-1".to_string()],
                input_key_images: vec!["key-image-1".to_string()],
                input_amount_commitments: vec!["amount-commitment-in".to_string()],
                amount_balance_commitment: "balance-commitment".to_string(),
                fee_asset_tag: "ZOKA".to_string(),
                asset_balance_commitments: vec![AssetBalanceCommitmentMessage {
                    asset_tag: "ZOKA".to_string(),
                    balance_commitment: "balance-commitment".to_string(),
                }],
                fee_commitment: String::new(),
                fee_range_proof: Vec::new(),
                private_commitment_root: String::new(),
                input_inclusion_proofs: vec![],
                commitment_blind: vec![5u8; 32],
                nullifier_nonce: vec![6u8; 16],
            },
        };

        let msg = ZokaMessage::StemPrivateTransaction(stem);
        let bytes = msg.to_bytes().unwrap();
        let restored = ZokaMessage::from_bytes(&bytes).unwrap();

        match restored {
            ZokaMessage::StemPrivateTransaction(restored) => {
                assert_eq!(restored.route_id, "route-1");
                assert_eq!(restored.relay_hint, "peer-a");
                assert_eq!(restored.padding.len(), 32);
                assert_eq!(restored.private_tx.tx_hash, "0xpriv");
            }
            other => panic!("unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn test_private_tx_announce_roundtrip() {
        let announce = PrivateTxAnnounceMessage {
            tx_hash: "0xpriv".to_string(),
            body_size_bytes: Some(2048),
            private_proof_profile: PRIVATE_PROOF_PROFILE_V1.to_string(),
            timestamp: 777,
        };
        let msg = ZokaMessage::PrivateTxAnnounce(announce);
        let restored = ZokaMessage::from_bytes(&msg.to_bytes().unwrap()).unwrap();

        match restored {
            ZokaMessage::PrivateTxAnnounce(restored) => {
                assert_eq!(restored.tx_hash, "0xpriv");
                assert_eq!(restored.body_size_bytes, Some(2048));
                assert_eq!(restored.private_proof_profile, PRIVATE_PROOF_PROFILE_V1);
                assert_eq!(restored.timestamp, 777);
            }
            other => panic!("unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn test_private_mixer_batch_message_roundtrip() {
        let batch = MixerBatchMessage {
            batch_id: "batch-1".to_string(),
            txs: vec![PrivateTransactionMessage {
                tx_hash: "0xpriv".to_string(),
                commitment: "commitment".to_string(),
                private_proof_version: PRIVATE_PROOF_VERSION_V1,
                private_proof_profile: PRIVATE_PROOF_PROFILE_V1.to_string(),
                stealth_address: "stlth_abc".to_string(),
                ephemeral_pubkey: vec![1, 2, 3],
                nullifier: "nullifier".to_string(),
                zk_proof: Some(vec![9, 9, 9]),
                timestamp: 123,
                recipient_commitment: "recipient".to_string(),
                encrypted_amount: vec![7, 7, 7, 7, 7, 7, 7, 7],
                proof_commitment: 11,
                proof_nullifier: 22,
                proof_input_sum: 33,
                proof_output_sum: 33,
                proof_fee: 0,
                proof_key_image: 44,
                proof_private_root_words: vec![1, 2, 3, 4],
                fee: 0,
                ring_id: "ring-id".to_string(),
                ring_version: 1,
                ring_members: vec!["member-a".to_string(), "member-b".to_string()],
                ring_signature: vec![4u8; 128],
                input_nullifiers: vec!["nullifier".to_string()],
                input_commitments: vec!["commitment".to_string()],
                input_asset_tags: vec!["ZOKA".to_string()],
                outputs: vec![PrivateOutputMessage {
                    output_id: "out-1".to_string(),
                    asset_tag: "ZOKA".to_string(),
                    asset_tag_commitment: "asset-tag-commitment".to_string(),
                    recipient_commitment: "recipient".to_string(),
                    stealth_address: "stlth_abc".to_string(),
                    ephemeral_pubkey: vec![1, 2, 3],
                    commitment: "commitment".to_string(),
                    amount_commitment: "amount-commitment".to_string(),
                    range_proof: vec![8u8; 32],
                    one_time_public_key: "one-time-key".to_string(),
                    encrypted_amount: vec![7, 7, 7, 7, 7, 7, 7, 7],
                    commitment_blind: vec![5u8; 32],
                    is_change: false,
                }],
                input_public_keys: vec!["input-pub-1".to_string()],
                input_key_images: vec!["key-image-1".to_string()],
                input_amount_commitments: vec!["amount-commitment-in".to_string()],
                amount_balance_commitment: "balance-commitment".to_string(),
                fee_asset_tag: "ZOKA".to_string(),
                asset_balance_commitments: vec![AssetBalanceCommitmentMessage {
                    asset_tag: "ZOKA".to_string(),
                    balance_commitment: "balance-commitment".to_string(),
                }],
                fee_commitment: String::new(),
                fee_range_proof: Vec::new(),
                private_commitment_root: String::new(),
                input_inclusion_proofs: vec![],
                commitment_blind: vec![5u8; 32],
                nullifier_nonce: vec![6u8; 16],
            }],
            padding: vec![0u8; 64],
            timestamp: 555,
        };

        let msg = ZokaMessage::PrivateMixerBatch(batch);
        let bytes = msg.to_bytes().unwrap();
        let restored = ZokaMessage::from_bytes(&bytes).unwrap();

        match restored {
            ZokaMessage::PrivateMixerBatch(restored) => {
                assert_eq!(restored.batch_id, "batch-1");
                assert_eq!(restored.txs.len(), 1);
                assert_eq!(restored.padding.len(), 64);
                assert_eq!(restored.timestamp, 555);
            }
            other => panic!("unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn test_private_transaction_message_missing_ring_version_defaults_to_clsag() {
        let legacy_like = serde_json::json!({
            "PrivateTransaction": {
                "tx_hash": "tx-legacy",
                "commitment": "commitment",
                "nullifier": "nullifier",
                "zk_proof": null,
                "timestamp": 1
            }
        });
        let restored: ZokaMessage =
            serde_json::from_value(legacy_like).expect("deserialize legacy private tx");
        match restored {
            ZokaMessage::PrivateTransaction(tx) => {
                assert_eq!(tx.ring_version, 1);
                assert_eq!(tx.private_proof_version, PRIVATE_PROOF_VERSION_V1);
                assert_eq!(tx.private_proof_profile, PRIVATE_PROOF_PROFILE_V1);
            }
            other => panic!("unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn test_private_transaction_message_missing_output_asset_tag_defaults() {
        let legacy_like = serde_json::json!({
            "PrivateTransaction": {
                "tx_hash": "tx-legacy",
                "commitment": "commitment",
                "nullifier": "nullifier",
                "timestamp": 1,
                "ring_version": 1,
                "ring_members": ["a", "b", "c"],
                "outputs": [{
                    "output_id": "out-1",
                    "recipient_commitment": "recipient",
                    "commitment": "commitment",
                    "amount_commitment": "amount-commitment"
                }]
            }
        });
        let restored: ZokaMessage =
            serde_json::from_value(legacy_like).expect("deserialize legacy private tx");
        match restored {
            ZokaMessage::PrivateTransaction(tx) => {
                assert_eq!(tx.outputs.len(), 1);
                assert_eq!(tx.outputs[0].asset_tag, "ZOKA");
            }
            other => panic!("unexpected variant: {:?}", other),
        }
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
        let block_msg = ZokaMessage::BlockRequest {
            block_height: 100,
            block_hash: None,
        };
        let bytes = block_msg.to_bytes().unwrap();

        assert!(!bytes.is_empty());
    }

    #[test]
    fn test_block_announce_roundtrip() {
        let msg = ZokaMessage::BlockAnnounce(BlockAnnounceMessage {
            block_height: 100,
            block_hash: "hash-100".to_string(),
            cumulative_work: "12345".to_string(),
            difficulty_bits: Some(20),
            timestamp: 123,
        });
        let restored = ZokaMessage::from_bytes(&msg.to_bytes().unwrap()).unwrap();

        match restored {
            ZokaMessage::BlockAnnounce(announce) => {
                assert_eq!(announce.block_height, 100);
                assert_eq!(announce.block_hash, "hash-100");
                assert_eq!(announce.cumulative_work, "12345");
                assert_eq!(announce.timestamp, 123);
            }
            other => panic!("unexpected variant: {:?}", other),
        }
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

    #[test]
    fn test_user_registry_payload_is_minimized() {
        let msg = ZokaMessage::UserRegistry {
            username: "alice".to_string(),
            record_digest: "digest123".to_string(),
            updated_at: 42,
        };

        let value = serde_json::to_value(&msg).expect("serialize user registry");
        let payload = value
            .get("UserRegistry")
            .and_then(|v| v.as_object())
            .expect("UserRegistry payload object");

        assert_eq!(
            payload.get("username").and_then(|v| v.as_str()),
            Some("alice")
        );
        assert_eq!(
            payload.get("record_digest").and_then(|v| v.as_str()),
            Some("digest123")
        );
        assert_eq!(payload.get("updated_at").and_then(|v| v.as_u64()), Some(42));

        assert!(!payload.contains_key("public_address"));
        assert!(!payload.contains_key("commitment_key"));
        assert!(!payload.contains_key("peer_id"));
        assert!(!payload.contains_key("listen_addr"));
    }

    #[test]
    fn test_user_query_response_defaults_found_for_compatibility() {
        let legacy_like = json!({
            "UserQueryResponse": {
                "username": "alice",
                "record_digest": "digest123",
                "updated_at": 7
            }
        });

        let restored: ZokaMessage =
            serde_json::from_value(legacy_like).expect("deserialize user query response");

        match restored {
            ZokaMessage::UserQueryResponse {
                username,
                found,
                record_digest,
                updated_at,
            } => {
                assert_eq!(username, "alice");
                assert!(found);
                assert_eq!(record_digest, "digest123");
                assert_eq!(updated_at, 7);
            }
            other => panic!("unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn test_user_discovery_digest_is_deterministic() {
        let record = UserDiscoveryRecord {
            username: "alice".to_string(),
            public_address: "0x1111".to_string(),
            commitment_key: "cmt".to_string(),
            peer_id: "peer-a".to_string(),
            listen_addr: Some("/ip4/127.0.0.1/tcp/7777".to_string()),
            updated_at: 100,
        };

        let digest_a = record.digest();
        let digest_b = record.digest();
        assert_eq!(digest_a, digest_b);

        let mut changed = record.clone();
        changed.updated_at = 101;
        assert_ne!(digest_a, changed.digest());
    }

    // ── Forward compatibility ────────────────────────────────────────────
    //
    // Before the P2P Exchange work can add an `OfferAnnounce` variant, it has
    // to be settled what today's nodes do when one reaches them. Most of the
    // network still runs older clients, so a variant that made them panic or
    // drop the sender would break the live network the moment a single
    // upgraded node published one.
    //
    // These tests pin the answer: an unknown variant is a plain decode error.
    // Every `from_bytes` call site in p2p.rs treats that the same way — log a
    // warning and continue — so the message is dropped and the peer is kept.
    // Nothing marks the sender incompatible either: that only happens through
    // `mark_peer_protocol_version`, which reads the identify protocol string
    // and never looks at message payloads.

    #[test]
    fn unknown_message_variant_from_a_newer_node_is_a_clean_decode_error() {
        // Shaped like the OfferAnnounce that Etapa 2 will add.
        let future_variant = json!({
            "OfferAnnounce": {
                "id": "offer-1",
                "seller_address": "zka1seller",
                "zka_amount": 1000u64,
                "usdt_amount": "25.00",
                "usdt_receive_address": "TRC20address",
                "created_at": 1_751_000_000u64
            }
        });

        let bytes = serde_json::to_vec(&future_variant).unwrap();
        let decoded = ZokaMessage::from_bytes(&bytes);

        // Err, not panic: serde rejects the unknown variant and the caller
        // discards the message.
        assert!(
            decoded.is_err(),
            "an unknown variant must fail to decode rather than resolve to something else"
        );
    }

    #[test]
    fn a_known_variant_still_decodes_after_an_unknown_one() {
        let unknown = serde_json::to_vec(&json!({ "SomeFutureVariant": { "x": 1 } })).unwrap();
        assert!(ZokaMessage::from_bytes(&unknown).is_err());

        // The decoder is stateless, so a bad message cannot poison the ones
        // that follow it on the same topic.
        let ping = ZokaMessage::Ping(PingMessage {
            nonce: 42,
            timestamp: 1_751_000_001,
        });
        let bytes = ping.to_bytes().unwrap();

        assert!(matches!(
            ZokaMessage::from_bytes(&bytes).unwrap(),
            ZokaMessage::Ping(_)
        ));
    }

    #[test]
    fn offer_messages_survive_a_gossip_round_trip() {
        use crate::p2p_exchange::{Offer, OfferStatus};

        let announce = ZokaMessage::OfferAnnounce(OfferAnnounceMessage {
            offer: Offer {
                id: "offer-1".to_string(),
                seller_address: "zka1seller".to_string(),
                zka_atoms: 100 * crate::economics::ATOMS_PER_ZKA,
                micro_usdt: 25_000_000,
                usdt_receive_address: "TQ5NMqJjW8dWvJf3zJ8xYqL2mNpR4sT6uV".to_string(),
                status: OfferStatus::Open,
                created_at: 1_751_000_000,
                claim_expires_at: None,
                claimed_by: None,
                reserved_note_ids: vec!["note-a".to_string()],
            },
            signature: vec![1, 2, 3],
            timestamp: 1_751_000_000,
        });

        let bytes = announce.to_bytes().unwrap();
        let restored = ZokaMessage::from_bytes(&bytes).unwrap();
        match restored {
            ZokaMessage::OfferAnnounce(msg) => {
                assert_eq!(msg.offer.id, "offer-1");
                assert_eq!(msg.offer.micro_usdt, 25_000_000);
                assert_eq!(msg.offer.status, OfferStatus::Open);
            }
            other => panic!("expected OfferAnnounce, got {other:?}"),
        }

        let update = ZokaMessage::OfferStatusUpdate(OfferStatusUpdateMessage {
            offer_id: "offer-1".to_string(),
            seller_address: "zka1seller".to_string(),
            status: OfferStatus::Claimed,
            claimed_by: Some("zka1buyer".to_string()),
            claim_expires_at: Some(1_751_002_700),
            signature: vec![4, 5, 6],
            timestamp: 1_751_000_100,
        });

        let bytes = update.to_bytes().unwrap();
        match ZokaMessage::from_bytes(&bytes).unwrap() {
            ZokaMessage::OfferStatusUpdate(msg) => {
                assert_eq!(msg.status, OfferStatus::Claimed);
                assert_eq!(msg.claimed_by.as_deref(), Some("zka1buyer"));
            }
            other => panic!("expected OfferStatusUpdate, got {other:?}"),
        }

        let failure = ZokaMessage::OfferPaymentFailure(crate::p2p_exchange::PaymentFailureReport {
            offer_id: "offer-1".to_string(),
            seller_address: "zka1seller".to_string(),
            buyer_address: "zka1buyer".to_string(),
            tron_tx_hash: "a".repeat(64),
            timestamp: 1_751_002_701,
            signature: vec![7, 8, 9],
        });
        let bytes = failure.to_bytes().unwrap();
        match ZokaMessage::from_bytes(&bytes).unwrap() {
            ZokaMessage::OfferPaymentFailure(report) => {
                assert_eq!(report.offer_id, "offer-1");
                assert_eq!(report.tron_tx_hash, "a".repeat(64));
                assert_eq!(report.timestamp, 1_751_002_701);
            }
            other => panic!("expected OfferPaymentFailure, got {other:?}"),
        }
    }

    #[test]
    fn offer_timestamps_are_reported_for_relay_ordering() {
        use crate::p2p_exchange::{Offer, OfferStatus};

        let msg = ZokaMessage::OfferAnnounce(OfferAnnounceMessage {
            offer: Offer {
                id: "offer-2".to_string(),
                seller_address: "zka1seller".to_string(),
                zka_atoms: 10 * crate::economics::ATOMS_PER_ZKA,
                micro_usdt: 5_000_000,
                usdt_receive_address: "TQ5NMqJjW8dWvJf3zJ8xYqL2mNpR4sT6uV".to_string(),
                status: OfferStatus::Open,
                created_at: 1_751_000_000,
                claim_expires_at: None,
                claimed_by: None,
                reserved_note_ids: Vec::new(),
            },
            signature: Vec::new(),
            timestamp: 1_751_000_555,
        });

        // Offers must not fall back to the zero timestamp used by messages
        // that carry none, or relay ordering treats every offer as ancient.
        assert_eq!(msg.timestamp(), 1_751_000_555);
    }

    #[test]
    fn malformed_and_empty_payloads_are_errors_not_panics() {
        assert!(ZokaMessage::from_bytes(b"").is_err());
        assert!(ZokaMessage::from_bytes(b"not json at all").is_err());
        assert!(ZokaMessage::from_bytes(b"{}").is_err());
        assert!(ZokaMessage::from_bytes(b"{\"Ping\":{}}").is_err());
    }
}
