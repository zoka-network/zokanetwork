//! P2P node implementation backed by libp2p.
//!
//! This module now includes gossipsub transport for `ZokaMessage`,
//! including a dedicated PBFT topic.

use futures::StreamExt;
use libp2p::{
    core::multiaddr::Multiaddr,
    gossipsub, identify, identity,
    kad::{self, store::MemoryStore},
    noise,
    swarm::SwarmEvent,
    tcp, yamux, PeerId, Swarm, SwarmBuilder,
};
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::io;
use std::time::Duration;
use tracing::{info, warn};

use crate::network::handler::MessageHandler;
use crate::network::message::{
    BlockResponseMessage, PbftNetworkMessage, PingMessage, SyncStateMessage, UserDiscoveryRecord,
    ZokaMessage,
};

pub const GENERAL_TOPIC_NAME: &str = "zoka/messages/v1";
pub const PBFT_TOPIC_NAME: &str = "zoka/pbft/v1";

/// ZOKA P2P node.
pub struct Node {
    /// Unique peer id.
    pub peer_id: PeerId,
    /// Swarm handles P2P I/O and behaviours.
    swarm: Swarm<NodeBehaviour>,
    /// General message topic.
    general_topic: gossipsub::IdentTopic,
    /// PBFT-only topic.
    pbft_topic: gossipsub::IdentTopic,
}

/// Global peer manager for gossip protocol
pub struct PeerManager {
    pub known_peers: HashSet<PeerId>,
    pub peer_scores: HashMap<PeerId, i32>,
    pub message_cache: HashMap<String, u64>,
    pub max_peers: usize,
}

impl Default for PeerManager {
    fn default() -> Self {
        PeerManager {
            known_peers: HashSet::new(),
            peer_scores: HashMap::new(),
            message_cache: HashMap::new(),
            max_peers: 1000,
        }
    }
}

impl PeerManager {
    pub fn new(max_peers: usize) -> Self {
        PeerManager {
            known_peers: HashSet::new(),
            peer_scores: HashMap::new(),
            message_cache: HashMap::new(),
            max_peers,
        }
    }

    /// Add a peer to known peers
    pub fn add_peer(&mut self, peer_id: PeerId) -> bool {
        if self.known_peers.len() >= self.max_peers {
            return false;
        }
        self.known_peers.insert(peer_id);
        self.peer_scores.insert(peer_id, 0);
        true
    }

    /// Update peer score (reputation system)
    pub fn update_peer_score(&mut self, peer_id: PeerId, delta: i32) {
        self.peer_scores
            .entry(peer_id)
            .and_modify(|score| *score += delta);
    }

    /// Get peer score
    pub fn get_peer_score(&self, peer_id: PeerId) -> i32 {
        self.peer_scores.get(&peer_id).copied().unwrap_or(0)
    }

    /// Remove low-score peers
    pub fn prune_low_score_peers(&mut self, threshold: i32) {
        self.known_peers
            .retain(|peer_id| self.peer_scores.get(peer_id).copied().unwrap_or(0) >= threshold);
    }

    /// Cache message by ID
    pub fn cache_message(&mut self, msg_id: String, timestamp: u64) {
        self.message_cache.insert(msg_id, timestamp);
    }

    /// Check if message was seen before (duplicate detection)
    pub fn has_seen_message(&self, msg_id: &str) -> bool {
        self.message_cache.contains_key(msg_id)
    }

    /// Get peer count
    pub fn peer_count(&self) -> usize {
        self.known_peers.len()
    }

    /// List top peers by score
    pub fn get_top_peers(&self, count: usize) -> Vec<(PeerId, i32)> {
        let mut peers: Vec<_> = self.peer_scores.iter().map(|(p, s)| (*p, *s)).collect();
        peers.sort_by(|a, b| b.1.cmp(&a.1));
        peers.into_iter().take(count).collect()
    }
}

/// Behaviour set for the node.
#[derive(libp2p::swarm::NetworkBehaviour)]
pub struct NodeBehaviour {
    /// Peer identification.
    identify: identify::Behaviour,
    /// DHT peer discovery.
    kademlia: kad::Behaviour<kad::store::MemoryStore>,
    /// Gossip transport for protocol messages.
    gossipsub: gossipsub::Behaviour,
}

impl Node {
    fn dht_user_key(username: &str) -> kad::RecordKey {
        let normalized = username.trim().to_ascii_lowercase();
        let key = format!("zoka:user:{normalized}");
        kad::RecordKey::new(&key)
    }

    /// Create a new node with gossipsub + identify + kademlia.
    pub async fn new(_listen_port: u16) -> Result<Self, Box<dyn Error>> {
        let local_key = identity::Keypair::generate_ed25519();
        let peer_id = PeerId::from(local_key.public());
        info!("Local PeerId: {}", peer_id);

        let general_topic = gossipsub::IdentTopic::new(GENERAL_TOPIC_NAME);
        let pbft_topic = gossipsub::IdentTopic::new(PBFT_TOPIC_NAME);
        let general_topic_for_behaviour = general_topic.clone();
        let pbft_topic_for_behaviour = pbft_topic.clone();

        let swarm = SwarmBuilder::with_existing_identity(local_key)
            .with_tokio()
            .with_tcp(
                tcp::Config::default(),
                noise::Config::new,
                yamux::Config::default,
            )?
            .with_behaviour(move |key| {
                let local_peer_id = PeerId::from(key.public());

                let kademlia = kad::Behaviour::with_config(
                    local_peer_id,
                    MemoryStore::new(local_peer_id),
                    kad::Config::default(),
                );

                let gossipsub_config = gossipsub::ConfigBuilder::default()
                    .heartbeat_interval(Duration::from_secs(1))
                    .validation_mode(gossipsub::ValidationMode::Strict)
                    .flood_publish(true)
                    .build()
                    .map_err(|e| io::Error::other(format!("gossipsub config error: {e}")))?;

                let mut gossipsub = gossipsub::Behaviour::new(
                    gossipsub::MessageAuthenticity::Signed(key.clone()),
                    gossipsub_config,
                )?;

                gossipsub.subscribe(&general_topic_for_behaviour)?;
                gossipsub.subscribe(&pbft_topic_for_behaviour)?;

                Ok(NodeBehaviour {
                    identify: identify::Behaviour::new(identify::Config::new(
                        "/zoka/1.0.0".to_string(),
                        key.public(),
                    )),
                    kademlia,
                    gossipsub,
                })
            })?
            .build();

        Ok(Self {
            peer_id,
            swarm,
            general_topic,
            pbft_topic,
        })
    }

    /// Listen on a multiaddr string.
    pub async fn listen(&mut self, addr: &str) -> Result<(), Box<dyn Error>> {
        let addr: Multiaddr = addr.parse()?;
        self.swarm.listen_on(addr.clone())?;
        info!("Node listening on {}", addr);
        Ok(())
    }

    /// Connect to a peer by multiaddr.
    pub async fn connect_to_peer(&mut self, peer_addr: &str) -> Result<(), Box<dyn Error>> {
        let addr: Multiaddr = peer_addr.parse()?;
        self.swarm.dial(addr)?;
        info!("Dialing peer {}", peer_addr);
        Ok(())
    }

    /// Wait for the next announced listen address.
    pub async fn wait_for_listen_addr(&mut self) -> Result<Multiaddr, Box<dyn Error>> {
        loop {
            match self.swarm.select_next_some().await {
                SwarmEvent::NewListenAddr { address, .. } => {
                    info!("Listening address announced: {}", address);
                    return Ok(address);
                }
                SwarmEvent::Behaviour(NodeBehaviourEvent::Gossipsub(
                    gossipsub::Event::Message { message, .. },
                )) => {
                    if ZokaMessage::from_bytes(&message.data).is_err() {
                        warn!("discarding malformed message while waiting for listen address");
                    }
                }
                SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                    info!("Peer connected while waiting listen addr: {}", peer_id);
                }
                _ => {}
            }
        }
    }

    /// Wait until an expected peer connection is established.
    pub async fn wait_for_peer_connection(
        &mut self,
        expected_peer: PeerId,
    ) -> Result<(), Box<dyn Error>> {
        loop {
            match self.swarm.select_next_some().await {
                SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                    info!("Peer connected: {}", peer_id);
                    if peer_id == expected_peer {
                        return Ok(());
                    }
                }
                SwarmEvent::NewListenAddr { address, .. } => {
                    info!("Listening address announced: {}", address);
                }
                SwarmEvent::Behaviour(NodeBehaviourEvent::Gossipsub(
                    gossipsub::Event::Message { message, .. },
                )) => {
                    if ZokaMessage::from_bytes(&message.data).is_err() {
                        warn!("discarding malformed message while waiting for peer connection");
                    }
                }
                _ => {}
            }
        }
    }

    /// Wait for the first established peer connection.
    pub async fn wait_for_any_connection(&mut self) -> Result<PeerId, Box<dyn Error>> {
        loop {
            match self.swarm.select_next_some().await {
                SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                    info!("Peer connected: {}", peer_id);
                    return Ok(peer_id);
                }
                SwarmEvent::NewListenAddr { address, .. } => {
                    info!("Listening address announced: {}", address);
                }
                SwarmEvent::Behaviour(NodeBehaviourEvent::Gossipsub(
                    gossipsub::Event::Message { message, .. },
                )) => {
                    if ZokaMessage::from_bytes(&message.data).is_err() {
                        warn!("discarding malformed message while waiting for any connection");
                    }
                }
                _ => {}
            }
        }
    }

    /// Pump swarm events for a bounded duration.
    pub async fn pump_for(&mut self, duration: Duration) -> Result<(), Box<dyn Error>> {
        let deadline = tokio::time::Instant::now() + duration;
        loop {
            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => break,
                event = self.swarm.select_next_some() => {
                    match event {
                        SwarmEvent::Behaviour(NodeBehaviourEvent::Gossipsub(
                            gossipsub::Event::Message { message, .. },
                        )) => {
                            if ZokaMessage::from_bytes(&message.data).is_err() {
                                warn!("discarding malformed message while pumping swarm");
                            }
                        }
                        SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                            info!("Peer connected while pumping: {}", peer_id);
                        }
                        SwarmEvent::NewListenAddr { address, .. } => {
                            info!("Listening address announced while pumping: {}", address);
                        }
                        _ => {}
                    }
                }
            }
        }
        Ok(())
    }

    /// Publish a protocol message on the appropriate gossipsub topic.
    pub fn publish_message(&mut self, message: &ZokaMessage) -> Result<(), Box<dyn Error>> {
        let payload = message.to_bytes()?;
        let topic = match message {
            ZokaMessage::Pbft(_) => self.pbft_topic.clone(),
            _ => self.general_topic.clone(),
        };
        self.swarm
            .behaviour_mut()
            .gossipsub
            .publish(topic, payload)
            .map(|_| ())
            .map_err(|e| -> Box<dyn Error> { Box::new(e) })
    }

    /// Publish a protocol message with bounded retries and background pumping.
    pub async fn publish_message_with_retry(
        &mut self,
        message: &ZokaMessage,
        attempts: u32,
        pump_interval: Duration,
    ) -> Result<(), Box<dyn Error>> {
        let max_attempts = attempts.max(1);
        let mut last_error = String::new();
        for _ in 0..max_attempts {
            match self.publish_message(message) {
                Ok(()) => return Ok(()),
                Err(err) => {
                    last_error = err.to_string();
                }
            }
            self.pump_for(pump_interval).await?;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Err(Box::new(io::Error::other(format!(
            "publish retries exhausted: {}",
            last_error
        ))))
    }

    /// Publish PBFT message on dedicated topic.
    pub fn publish_pbft(&mut self, pbft: PbftNetworkMessage) -> Result<(), Box<dyn Error>> {
        self.publish_message(&ZokaMessage::Pbft(pbft))
    }

    /// Publish SyncState message on general topic.
    pub fn publish_sync_state(
        &mut self,
        last_height: u64,
        state_root: String,
    ) -> Result<(), Box<dyn Error>> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.publish_message(&ZokaMessage::SyncState(SyncStateMessage {
            last_height,
            state_root,
            timestamp: now,
        }))
    }

    /// Publish Ping message on general topic.
    pub fn publish_ping(&mut self, nonce: u64) -> Result<(), Box<dyn Error>> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.publish_message(&ZokaMessage::Ping(PingMessage {
            nonce,
            timestamp: now,
        }))
    }

    /// Publish BlockRequest message on general topic.
    pub fn publish_block_request(
        &mut self,
        block_height: u64,
        _block_hash: Option<String>,
    ) -> Result<(), Box<dyn Error>> {
        self.publish_message(&ZokaMessage::BlockRequest { block_height })
    }

    /// Publish BlockResponse message on general topic.
    pub fn publish_block_response(
        &mut self,
        block_height: u64,
        block_hash: String,
        encrypted_block: Vec<u8>,
    ) -> Result<(), Box<dyn Error>> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.publish_message(&ZokaMessage::BlockResponse(BlockResponseMessage {
            block_height,
            encrypted_block,
            block_hash,
            timestamp: now,
        }))
    }

    /// Request a block and wait for a matching response from the network.
    pub async fn request_block_with_retry(
        &mut self,
        block_height: u64,
        expected_hash: Option<String>,
        publish_attempts: u32,
        max_wait: Duration,
    ) -> Result<BlockResponseMessage, Box<dyn Error>> {
        let request = ZokaMessage::BlockRequest { block_height };

        self.publish_message_with_retry(&request, publish_attempts, Duration::from_millis(120))
            .await?;

        let deadline = tokio::time::Instant::now() + max_wait;
        let mut last_error = String::new();
        loop {
            if tokio::time::Instant::now() >= deadline {
                let detail = if last_error.is_empty() {
                    "no matching BlockResponse received".to_string()
                } else {
                    last_error
                };
                return Err(Box::new(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("timed out waiting for block response: {detail}"),
                )));
            }

            let wait_budget = deadline
                .saturating_duration_since(tokio::time::Instant::now())
                .min(Duration::from_millis(800));
            match tokio::time::timeout(wait_budget, self.next_message()).await {
                Ok(Ok(ZokaMessage::BlockResponse(resp))) => {
                    if resp.block_height != block_height {
                        continue;
                    }
                    if let Some(hash) = expected_hash.as_ref() {
                        if &resp.block_hash != hash {
                            continue;
                        }
                    }
                    return Ok(resp);
                }
                Ok(Ok(_)) => {}
                Ok(Err(err)) => {
                    last_error = err.to_string();
                }
                Err(_) => {
                    if let Err(err) = self.publish_message(&request) {
                        last_error = err.to_string();
                    }
                }
            }

            self.pump_for(Duration::from_millis(80)).await?;
        }
    }

    /// Receive next protocol message from gossipsub.
    pub async fn next_message(&mut self) -> Result<ZokaMessage, Box<dyn Error>> {
        loop {
            match self.swarm.select_next_some().await {
                SwarmEvent::Behaviour(NodeBehaviourEvent::Gossipsub(
                    gossipsub::Event::Message { message, .. },
                )) => match ZokaMessage::from_bytes(&message.data) {
                    Ok(msg) => return Ok(msg),
                    Err(err) => warn!("discarding malformed message: {}", err),
                },
                SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                    info!("Peer connected: {}", peer_id);
                }
                SwarmEvent::NewListenAddr { address, .. } => {
                    info!("Listening address announced: {}", address);
                }
                _ => {}
            }
        }
    }

    /// Pull the next message from transport and dispatch it to a handler.
    pub async fn dispatch_next_message<H: MessageHandler>(
        &mut self,
        handler: &H,
    ) -> Result<(), Box<dyn Error>> {
        let message = self.next_message().await?;
        handler
            .handle_message(message)
            .await
            .map_err(|e| io::Error::other(format!("handler error: {e}")))?;
        Ok(())
    }

    /// Return the topic name used for a message kind.
    pub fn topic_name_for_message(message: &ZokaMessage) -> &'static str {
        match message {
            ZokaMessage::Pbft(_) => PBFT_TOPIC_NAME,
            _ => GENERAL_TOPIC_NAME,
        }
    }

    /// Return local peer id.
    pub fn peer_id(&self) -> PeerId {
        self.peer_id
    }

    /// Return current number of connected peers.
    pub fn connected_peer_count(&self) -> usize {
        self.swarm.connected_peers().count()
    }

    /// Wait until the node reaches at least `min_peers` active connections.
    pub async fn wait_for_min_connected_peers(
        &mut self,
        min_peers: usize,
        max_wait: Duration,
    ) -> Result<(), Box<dyn Error>> {
        let deadline = tokio::time::Instant::now() + max_wait;
        loop {
            if self.connected_peer_count() >= min_peers {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Box::new(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "timeout waiting for minimum connected peers",
                )));
            }
            self.pump_for(Duration::from_millis(120)).await?;
            tokio::time::sleep(Duration::from_millis(60)).await;
        }
    }

    /// Count known peers currently subscribed to the PBFT topic.
    pub fn pbft_subscribed_peer_count(&self) -> usize {
        let pbft_hash = self.pbft_topic.hash();
        self.swarm
            .behaviour()
            .gossipsub
            .all_peers()
            .filter(|(_, topics)| topics.iter().any(|topic| *topic == &pbft_hash))
            .count()
    }

    /// Store a user discovery record in Kademlia DHT.
    pub fn put_user_record(
        &mut self,
        record: &UserDiscoveryRecord,
    ) -> Result<kad::QueryId, Box<dyn Error>> {
        let value = serde_json::to_vec(record)?;
        let dht_record = kad::Record::new(Self::dht_user_key(&record.username), value);
        self.swarm
            .behaviour_mut()
            .kademlia
            .put_record(dht_record, kad::Quorum::One)
            .map_err(|e| -> Box<dyn Error> { Box::new(e) })
    }

    /// Query a user discovery record from Kademlia DHT with timeout.
    pub async fn get_user_record(
        &mut self,
        username: &str,
        max_wait: Duration,
    ) -> Result<Option<UserDiscoveryRecord>, Box<dyn Error>> {
        let query_id = self
            .swarm
            .behaviour_mut()
            .kademlia
            .get_record(Self::dht_user_key(username));
        let deadline = tokio::time::Instant::now() + max_wait;

        loop {
            if tokio::time::Instant::now() >= deadline {
                return Err(Box::new(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "timeout waiting for DHT user record",
                )));
            }

            let budget = deadline
                .saturating_duration_since(tokio::time::Instant::now())
                .min(Duration::from_millis(500));
            let event = match tokio::time::timeout(budget, self.swarm.select_next_some()).await {
                Ok(event) => event,
                Err(_) => continue,
            };

            match event {
                SwarmEvent::Behaviour(NodeBehaviourEvent::Kademlia(
                    kad::Event::OutboundQueryProgressed { id, result, .. },
                )) if id == query_id => match result {
                    kad::QueryResult::GetRecord(Ok(kad::GetRecordOk::FoundRecord(peer_record))) => {
                        let parsed = serde_json::from_slice::<UserDiscoveryRecord>(
                            &peer_record.record.value,
                        )
                        .ok();
                        if let Some(record) = parsed {
                            return Ok(Some(record));
                        }
                    }
                    kad::QueryResult::GetRecord(Ok(
                        kad::GetRecordOk::FinishedWithNoAdditionalRecord { .. },
                    )) => return Ok(None),
                    kad::QueryResult::GetRecord(Err(_)) => return Ok(None),
                    _ => {}
                },
                SwarmEvent::Behaviour(NodeBehaviourEvent::Gossipsub(
                    gossipsub::Event::Message { message, .. },
                )) => {
                    if ZokaMessage::from_bytes(&message.data).is_err() {
                        warn!("discarding malformed message while waiting DHT query");
                    }
                }
                SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                    info!("Peer connected while waiting DHT query: {}", peer_id);
                }
                SwarmEvent::NewListenAddr { address, .. } => {
                    info!(
                        "Listening address announced while waiting DHT query: {}",
                        address
                    );
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::{PbftMessage, PbftMessageType};
    use crate::network::message::PingMessage;

    #[tokio::test]
    async fn test_node_creation() {
        let node = Node::new(9000).await;
        assert!(node.is_ok());

        let node = node.expect("node");
        assert_ne!(node.peer_id(), PeerId::random());
    }

    #[test]
    fn test_topic_routing() {
        let ping = ZokaMessage::Ping(PingMessage {
            nonce: 1,
            timestamp: 10,
        });
        assert_eq!(Node::topic_name_for_message(&ping), GENERAL_TOPIC_NAME);

        let pbft = ZokaMessage::Pbft(PbftNetworkMessage::Consensus(PbftMessage::new(
            PbftMessageType::Prepare,
            0,
            1,
            "hash".to_string(),
            "n1".to_string(),
        )));
        assert_eq!(Node::topic_name_for_message(&pbft), PBFT_TOPIC_NAME);
    }

    #[tokio::test]
    async fn test_publish_without_peers_returns_error() {
        let mut node = Node::new(0).await.expect("node");
        let message = ZokaMessage::Ping(PingMessage {
            nonce: 99,
            timestamp: 999,
        });
        let result = node.publish_message(&message);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_publish_helpers_without_peers_return_error() {
        let mut node = Node::new(0).await.expect("node");
        assert_eq!(node.connected_peer_count(), 0);

        let sync_result = node.publish_sync_state(10, "root-10".to_string());
        assert!(sync_result.is_err());

        let ping_result = node.publish_ping(123);
        assert!(ping_result.is_err());

        let req_result = node.publish_block_request(5, None);
        assert!(req_result.is_err());

        let resp_result = node.publish_block_response(5, "hash-5".to_string(), vec![1, 2, 3]);
        assert!(resp_result.is_err());
    }

    #[tokio::test]
    async fn test_request_block_with_retry_without_peers_returns_error() {
        let mut node = Node::new(0).await.expect("node");
        let result = node
            .request_block_with_retry(42, None, 2, Duration::from_millis(400))
            .await;
        assert!(result.is_err());
    }

    // ===== PeerManager Tests =====

    #[test]
    fn test_peer_manager_creation() {
        let pm = PeerManager::new(50);
        assert_eq!(pm.peer_count(), 0);
        assert_eq!(pm.max_peers, 50);
        assert!(pm.known_peers.is_empty());
        assert!(pm.peer_scores.is_empty());
        assert!(pm.message_cache.is_empty());
    }

    #[test]
    fn test_peer_manager_default() {
        let pm = PeerManager::default();
        assert!(pm.max_peers > 0);
        assert_eq!(pm.peer_count(), 0);
    }

    #[test]
    fn test_add_peer() {
        let mut pm = PeerManager::new(10);
        let peer1 = PeerId::random();
        let peer2 = PeerId::random();

        pm.add_peer(peer1);
        assert_eq!(pm.peer_count(), 1);
        assert!(pm.known_peers.contains(&peer1));

        pm.add_peer(peer2);
        assert_eq!(pm.peer_count(), 2);
        assert!(pm.known_peers.contains(&peer2));

        // Adding duplicate doesn't change count
        pm.add_peer(peer1);
        assert_eq!(pm.peer_count(), 2);
    }

    #[test]
    fn test_peer_score_initialization() {
        let mut pm = PeerManager::new(10);
        let peer = PeerId::random();

        pm.add_peer(peer);
        let score = pm.get_peer_score(peer);
        assert_eq!(score, 0); // New peers start with 0 score
    }

    #[test]
    fn test_update_peer_score_positive() {
        let mut pm = PeerManager::new(10);
        let peer = PeerId::random();

        pm.add_peer(peer);
        pm.update_peer_score(peer, 5);
        assert_eq!(pm.get_peer_score(peer), 5);

        pm.update_peer_score(peer, 10);
        assert_eq!(pm.get_peer_score(peer), 15);
    }

    #[test]
    fn test_update_peer_score_negative() {
        let mut pm = PeerManager::new(10);
        let peer = PeerId::random();

        pm.add_peer(peer);
        pm.update_peer_score(peer, 20);
        assert_eq!(pm.get_peer_score(peer), 20);

        pm.update_peer_score(peer, -10);
        assert_eq!(pm.get_peer_score(peer), 10);

        pm.update_peer_score(peer, -15);
        assert_eq!(pm.get_peer_score(peer), -5);
    }

    #[test]
    fn test_prune_low_score_peers() {
        let mut pm = PeerManager::new(10);
        let peer1 = PeerId::random();
        let peer2 = PeerId::random();
        let peer3 = PeerId::random();

        pm.add_peer(peer1);
        pm.add_peer(peer2);
        pm.add_peer(peer3);

        // Set different scores
        pm.update_peer_score(peer1, 50); // Good peer
        pm.update_peer_score(peer2, -30); // Bad peer
        pm.update_peer_score(peer3, 10); // Okay peer

        assert_eq!(pm.peer_count(), 3);

        // Prune peers with score < 0
        pm.prune_low_score_peers(0);
        assert_eq!(pm.peer_count(), 2);
        assert!(!pm.known_peers.contains(&peer2));
    }

    #[test]
    fn test_cache_message() {
        let mut pm = PeerManager::new(10);
        let msg_id = "msg-1".to_string();

        pm.cache_message(msg_id.clone(), 1000);
        assert!(pm.has_seen_message("msg-1"));

        // Cache another message
        let msg_id2 = "msg-2".to_string();
        pm.cache_message(msg_id2, 1001);
        assert!(pm.has_seen_message("msg-2"));
        assert!(pm.has_seen_message("msg-1"));
    }

    #[test]
    fn test_has_seen_message_not_cached() {
        let pm = PeerManager::new(10);
        assert!(!pm.has_seen_message("unseen-message"));
    }

    #[test]
    fn test_duplicate_detection() {
        let mut pm = PeerManager::new(10);
        let msg_id = "duplicate-test".to_string();

        // First time: not seen
        assert!(!pm.has_seen_message("duplicate-test"));

        // Cache it
        pm.cache_message(msg_id.clone(), 1000);
        assert!(pm.has_seen_message("duplicate-test"));

        // Try to cache again - should still be tracked
        pm.cache_message(msg_id, 1000);
        assert!(pm.has_seen_message("duplicate-test"));
    }

    #[test]
    fn test_get_top_peers() {
        let mut pm = PeerManager::new(10);
        let peer1 = PeerId::random();
        let peer2 = PeerId::random();
        let peer3 = PeerId::random();
        let peer4 = PeerId::random();

        pm.add_peer(peer1);
        pm.add_peer(peer2);
        pm.add_peer(peer3);
        pm.add_peer(peer4);

        // Set scores
        pm.update_peer_score(peer1, 100);
        pm.update_peer_score(peer2, 50);
        pm.update_peer_score(peer3, 75);
        pm.update_peer_score(peer4, 25);

        // Get top 2 peers
        let top_peers = pm.get_top_peers(2);
        assert_eq!(top_peers.len(), 2);
        // Should be peer1 (100) and peer3 (75)
        assert!(top_peers[0].0 == peer1 || top_peers[1].0 == peer1);
        assert!(top_peers[0].0 == peer3 || top_peers[1].0 == peer3);
    }

    #[test]
    fn test_get_top_peers_more_than_available() {
        let mut pm = PeerManager::new(10);
        let peer1 = PeerId::random();
        let peer2 = PeerId::random();

        pm.add_peer(peer1);
        pm.add_peer(peer2);

        pm.update_peer_score(peer1, 100);
        pm.update_peer_score(peer2, 50);

        // Request more peers than available
        let top_peers = pm.get_top_peers(10);
        assert_eq!(top_peers.len(), 2); // Only 2 peers exist
    }

    #[test]
    fn test_max_peers_limit() {
        let mut pm = PeerManager::new(3);
        let peer1 = PeerId::random();
        let peer2 = PeerId::random();
        let peer3 = PeerId::random();

        pm.add_peer(peer1);
        pm.add_peer(peer2);
        pm.add_peer(peer3);

        assert_eq!(pm.peer_count(), 3);
        assert_eq!(pm.peer_count(), pm.max_peers);
    }

    #[test]
    fn test_gossip_protocol_scenario() {
        let mut pm = PeerManager::new(20);

        // Simulate peer discovery
        let mut peers = vec![];
        for _ in 0..5 {
            let peer = PeerId::random();
            pm.add_peer(peer);
            peers.push(peer);
        }

        assert_eq!(pm.peer_count(), 5);

        // Simulate message propagation with duplicate detection
        let block_hash = "block-hash-123".to_string();
        for i in 0..3 {
            if !pm.has_seen_message("block-hash-123") {
                pm.cache_message(block_hash.clone(), i as u64);
            }
        }
        assert!(pm.has_seen_message("block-hash-123"));

        // Reward good peers
        for peer in &peers[0..3] {
            pm.update_peer_score(*peer, 10);
        }

        // Penalize bad peers
        for peer in &peers[3..] {
            pm.update_peer_score(*peer, -5);
        }

        // Prune low reputation peers
        pm.prune_low_score_peers(-3);
    }

    #[test]
    fn test_peer_reputation_tracking() {
        let mut pm = PeerManager::new(10);
        let peer = PeerId::random();

        pm.add_peer(peer);

        // Initial reputation
        assert_eq!(pm.get_peer_score(peer), 0);

        // Good behavior
        pm.update_peer_score(peer, 5);
        assert_eq!(pm.get_peer_score(peer), 5);

        // More good behavior
        pm.update_peer_score(peer, 10);
        assert_eq!(pm.get_peer_score(peer), 15);

        // One bad behavior
        pm.update_peer_score(peer, -3);
        assert_eq!(pm.get_peer_score(peer), 12);

        // Reputation is positive but degraded
        assert!(pm.get_peer_score(peer) > 0);
    }

    #[test]
    fn test_peer_discovery_and_tracking() {
        let mut pm = PeerManager::new(100);

        // Simulate peer discovery from multiple sources
        let discovered_peers: Vec<_> = (0..10).map(|_| PeerId::random()).collect();

        for peer in discovered_peers.iter() {
            pm.add_peer(*peer);
        }

        assert_eq!(pm.peer_count(), 10);

        // Track message from peer discovery announcement
        let discovery_msg_id = "peer-discovery-announce-1".to_string();
        pm.cache_message(discovery_msg_id, 1000);

        for peer in discovered_peers.iter() {
            assert!(pm.known_peers.contains(peer));
        }

        assert!(pm.has_seen_message("peer-discovery-announce-1"));
    }
}
