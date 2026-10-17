//! Message handler for private network messages.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

use crate::network::message::{PbftNetworkMessage, ZokaMessage};

/// Trait for message processing.
#[async_trait]
pub trait MessageHandler: Send + Sync {
    async fn handle_message(&self, message: ZokaMessage) -> Result<(), String>;
}

/// Default message handler implementation.
#[derive(Clone)]
pub struct DefaultMessageHandler {
    /// Number of processed messages.
    processed_count: Arc<RwLock<u64>>,
    /// Message counters by category.
    stats: Arc<RwLock<MessageStats>>,
}

impl DefaultMessageHandler {
    /// Create a new handler.
    pub fn new() -> Self {
        Self {
            processed_count: Arc::new(RwLock::new(0)),
            stats: Arc::new(RwLock::new(MessageStats::default())),
        }
    }

    /// Return processed message count.
    pub async fn message_count(&self) -> u64 {
        *self.processed_count.read().await
    }

    /// Return a snapshot of message stats.
    pub async fn stats(&self) -> MessageStats {
        self.stats.read().await.clone()
    }
}

#[async_trait]
impl MessageHandler for DefaultMessageHandler {
    async fn handle_message(&self, message: ZokaMessage) -> Result<(), String> {
        match message {
            ZokaMessage::Transaction(_tx) => {
                info!("Processing transaction");
                let mut stats = self.stats.write().await;
                stats.transactions += 1;
            }
            ZokaMessage::PrivateTransaction(private_tx) => {
                info!("Processing private transaction: {}", private_tx.tx_hash);
                let mut stats = self.stats.write().await;
                stats.transactions += 1;
            }
            ZokaMessage::PrivateTxAnnounce(announce) => {
                info!("Private tx announce received: {}", announce.tx_hash);
                let mut stats = self.stats.write().await;
                stats.private_tx_announces += 1;
            }
            ZokaMessage::StemPrivateTransaction(private_tx) => {
                info!(
                    "Processing stem private transaction: route={}, tx={}",
                    private_tx.route_id, private_tx.private_tx.tx_hash
                );
                let mut stats = self.stats.write().await;
                stats.transactions += 1;
            }
            ZokaMessage::PrivateMixerBatch(batch) => {
                info!(
                    "Processing private mixer batch: id={}, size={}",
                    batch.batch_id,
                    batch.txs.len()
                );
                let mut stats = self.stats.write().await;
                stats.transactions += batch.txs.len() as u64;
            }
            ZokaMessage::BlockRequest { block_height, .. } => {
                info!("Block request received: height={}", block_height);
                let mut stats = self.stats.write().await;
                stats.block_requests += 1;
            }
            ZokaMessage::BlockResponse(resp) => {
                info!("Block response received: height={}", resp.block_height);
                let mut stats = self.stats.write().await;
                stats.block_responses += 1;
            }
            ZokaMessage::BlockAnnounce(announce) => {
                info!(
                    "Block announce received: height={}, hash={}",
                    announce.block_height, announce.block_hash
                );
                let mut stats = self.stats.write().await;
                stats.block_announces += 1;
            }
            ZokaMessage::SyncState(sync) => {
                info!("State sync received: remote_height={}", sync.last_height);
                let mut stats = self.stats.write().await;
                stats.sync_messages += 1;
            }
            ZokaMessage::Ping(ping) => {
                info!("Ping received: nonce={}", ping.nonce);
                let mut stats = self.stats.write().await;
                stats.pings += 1;
            }
            ZokaMessage::Pbft(pbft_msg) => match pbft_msg {
                PbftNetworkMessage::Consensus(msg) => {
                    info!(
                        "PBFT {:?}: view={}, seq={}, sender={}",
                        msg.msg_type, msg.view, msg.sequence, msg.sender
                    );
                    let mut stats = self.stats.write().await;
                    stats.pbft_messages += 1;
                }
                PbftNetworkMessage::ViewChange(msg) => {
                    info!(
                        "PBFT ViewChange: new_view={}, sender={}",
                        msg.new_view, msg.sender
                    );
                    let mut stats = self.stats.write().await;
                    stats.pbft_messages += 1;
                }
                PbftNetworkMessage::NewView(msg) => {
                    info!(
                        "PBFT NewView: new_view={}, sender={}",
                        msg.new_view, msg.sender
                    );
                    let mut stats = self.stats.write().await;
                    stats.pbft_messages += 1;
                }
            },
            ZokaMessage::UserRegistry { username, .. } => {
                info!("User registry: {}", username);
                let mut stats = self.stats.write().await;
                stats.transactions += 1;
            }
            ZokaMessage::UserQuery { username } => {
                info!("User query: {}", username);
                let mut stats = self.stats.write().await;
                stats.transactions += 1;
            }
            ZokaMessage::UserQueryResponse { username, .. } => {
                info!("User query response: {}", username);
                let mut stats = self.stats.write().await;
                stats.transactions += 1;
            }
            ZokaMessage::TeamAdvertise(advertisement) => {
                info!(
                    "Mining team advertised: {} ({} members)",
                    advertisement.team_id, advertisement.member_count
                );
            }
            ZokaMessage::TeamJoin(membership) => {
                info!(
                    "Mining team membership request: {} {:?}",
                    membership.team_id, membership.action
                );
            }
            ZokaMessage::ShareSubmit(share) => {
                info!(
                    "Mining share submitted: {} by {}",
                    share.template_id, share.miner_address
                );
            }
            ZokaMessage::TurnUpdate(update) => {
                info!(
                    "Mining team turn update: {} epoch {}",
                    update.state.team_id, update.state.epoch
                );
            }
            ZokaMessage::SignedCheckpoint(checkpoint) => {
                info!(
                    "Signed checkpoint received: {} at {}",
                    checkpoint.chain_id, checkpoint.height
                );
            }
            ZokaMessage::QuanticTransaction { tx_hex } => {
                info!("Quantic transaction received: {} hex chars", tx_hex.len());
                let mut stats = self.stats.write().await;
                stats.transactions += 1;
            }
        }

        let mut count = self.processed_count.write().await;
        *count += 1;

        let mut stats = self.stats.write().await;
        stats.total_processed = *count;
        Ok(())
    }
}

impl Default for DefaultMessageHandler {
    fn default() -> Self {
        Self::new()
    }
}

/// Message statistics summary.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MessageStats {
    /// Total processed messages.
    pub total_processed: u64,
    /// Transaction messages.
    pub transactions: u64,
    /// Block requests.
    pub block_requests: u64,
    /// Block response messages.
    pub block_responses: u64,
    /// Block announce messages.
    pub block_announces: u64,
    /// Private transaction availability announces.
    pub private_tx_announces: u64,
    /// State sync messages.
    pub sync_messages: u64,
    /// Ping messages.
    pub pings: u64,
    /// PBFT messages.
    pub pbft_messages: u64,
}

impl Default for MessageStats {
    fn default() -> Self {
        Self {
            total_processed: 0,
            transactions: 0,
            block_requests: 0,
            block_responses: 0,
            block_announces: 0,
            private_tx_announces: 0,
            sync_messages: 0,
            pings: 0,
            pbft_messages: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::{PbftMessage, PbftMessageType};
    use crate::network::message::{PbftNetworkMessage, PingMessage};

    #[tokio::test]
    async fn test_handler_creation() {
        let handler = DefaultMessageHandler::new();
        assert_eq!(handler.message_count().await, 0);
    }

    #[tokio::test]
    async fn test_handle_ping() {
        let handler = DefaultMessageHandler::new();
        let ping = PingMessage {
            nonce: 42,
            timestamp: 1234567890,
        };

        let msg = ZokaMessage::Ping(ping);
        let result = handler.handle_message(msg).await;

        assert!(result.is_ok());
        assert_eq!(handler.message_count().await, 1);
    }

    #[tokio::test]
    async fn test_handle_pbft_consensus_message() {
        let handler = DefaultMessageHandler::new();
        let pbft = PbftMessage::new(
            PbftMessageType::Prepare,
            0,
            1,
            "hash".to_string(),
            "n1".to_string(),
        );
        let msg = ZokaMessage::Pbft(PbftNetworkMessage::Consensus(pbft));

        let result = handler.handle_message(msg).await;
        assert!(result.is_ok());
        assert_eq!(handler.message_count().await, 1);
        let stats = handler.stats().await;
        assert_eq!(stats.pbft_messages, 1);
        assert_eq!(stats.total_processed, 1);
    }

    #[tokio::test]
    async fn test_multiple_messages() {
        let handler = DefaultMessageHandler::new();

        for i in 0..5 {
            let ping = PingMessage {
                nonce: i,
                timestamp: 1234567890 + i as u64,
            };
            let msg = ZokaMessage::Ping(ping);
            handler.handle_message(msg).await.ok();
        }

        assert_eq!(handler.message_count().await, 5);
        let stats = handler.stats().await;
        assert_eq!(stats.pings, 5);
        assert_eq!(stats.total_processed, 5);
    }
}
