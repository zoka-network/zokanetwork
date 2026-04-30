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
use std::collections::{HashMap, HashSet, VecDeque};
use std::error::Error;
use std::io;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tracing::{info, warn};

use crate::network::handler::MessageHandler;
use crate::network::message::{
    BlockResponseMessage, MixerBatchMessage, PbftNetworkMessage, PingMessage,
    PrivateTransactionMessage, StemPrivateTransactionMessage, SyncStateMessage,
    UserDiscoveryRecord, ZokaMessage,
};

pub const GENERAL_TOPIC_NAME: &str = "zoka/messages/v1";
pub const PBFT_TOPIC_NAME: &str = "zoka/pbft/v1";
pub const STEM_TOPIC_NAME: &str = "zoka/stem/v1";
pub const MIXER_TOPIC_NAME: &str = "zoka/mixer/v1";
pub const PROTOCOL_FREEZE_ID: &str = "/zoka/1.0.0";
pub const DEFAULT_PRIVATE_PADDING_BYTES: usize = 1024;
pub const DEFAULT_STEM_HOPS: u8 = 2;
pub const DEFAULT_MIXER_HOLD_MS: u64 = 2000;
pub const DEFAULT_MIXER_MAX_BATCH: usize = 8;
pub const DEFAULT_MIXER_MAX_BATCHES_PER_MINUTE: u32 = 120;

#[derive(Clone, Debug)]
pub struct NetworkPrivacyConfig {
    pub enable_dandelion: bool,
    pub enable_private_mixer: bool,
    pub stem_hops: u8,
    pub stem_jitter_ms: u64,
    pub private_padding_bytes: usize,
    pub mixer_hold_ms: u64,
    pub mixer_max_batch: usize,
    pub mixer_max_batches_per_minute: u32,
    pub tor_proxy: Option<String>,
    pub i2p_sam_endpoint: Option<String>,
    pub enforce_anonymous_transport: bool,
}

impl NetworkPrivacyConfig {
    pub fn with_tor_proxy(mut self, tor_proxy: impl Into<String>) -> Self {
        self.tor_proxy = Some(tor_proxy.into());
        self
    }

    pub fn tor_enforced(mut self, tor_proxy: impl Into<String>) -> Self {
        self.tor_proxy = Some(tor_proxy.into());
        self.enforce_anonymous_transport = true;
        self.i2p_sam_endpoint = None;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrivacyTransportMode {
    Direct,
    Tor,
    I2p,
    Hybrid,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrivacyTransportStatus {
    pub mode: PrivacyTransportMode,
    pub tor_reachable: bool,
    pub i2p_reachable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum DialTargetKind {
    Direct,
    TorOnion,
    I2pB32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DialTarget {
    host: String,
    port: u16,
    kind: DialTargetKind,
}

impl Default for NetworkPrivacyConfig {
    fn default() -> Self {
        Self {
            enable_dandelion: true,
            enable_private_mixer: true,
            stem_hops: DEFAULT_STEM_HOPS,
            stem_jitter_ms: DEFAULT_MIXER_HOLD_MS,
            private_padding_bytes: DEFAULT_PRIVATE_PADDING_BYTES,
            mixer_hold_ms: DEFAULT_MIXER_HOLD_MS,
            mixer_max_batch: DEFAULT_MIXER_MAX_BATCH,
            mixer_max_batches_per_minute: DEFAULT_MIXER_MAX_BATCHES_PER_MINUTE,
            tor_proxy: None,
            i2p_sam_endpoint: None,
            enforce_anonymous_transport: false,
        }
    }
}

#[derive(Clone, Debug)]
struct MixerBatcher {
    queue: VecDeque<PrivateTransactionMessage>,
    queued_at: Option<Instant>,
}

impl MixerBatcher {
    fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            queued_at: None,
        }
    }

    fn enqueue(
        &mut self,
        tx: PrivateTransactionMessage,
        hold_ms: u64,
        max_batch: usize,
    ) -> Option<Vec<PrivateTransactionMessage>> {
        if self.queue.is_empty() {
            self.queued_at = Some(Instant::now());
        }
        self.queue.push_back(tx);
        if self.queue.len() >= max_batch.max(1) {
            return self.drain_all();
        }
        if hold_ms == 0 {
            return self.drain_all();
        }
        None
    }

    fn drain_due(&mut self, hold_ms: u64) -> Option<Vec<PrivateTransactionMessage>> {
        if self.queue.is_empty() {
            return None;
        }
        let Some(queued_at) = self.queued_at else {
            return self.drain_all();
        };
        if queued_at.elapsed() >= Duration::from_millis(hold_ms) {
            return self.drain_all();
        }
        None
    }

    fn drain_all(&mut self) -> Option<Vec<PrivateTransactionMessage>> {
        if self.queue.is_empty() {
            self.queued_at = None;
            return None;
        }
        let drained = self.queue.drain(..).collect::<Vec<_>>();
        self.queued_at = None;
        Some(drained)
    }
}

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
    /// Dandelion++ stem topic for private transaction relays.
    stem_topic: gossipsub::IdentTopic,
    /// Mixer topic for batched private transaction payloads.
    mixer_topic: gossipsub::IdentTopic,
    /// Network privacy policy knobs.
    privacy_config: NetworkPrivacyConfig,
    /// Pending inbound messages generated by local fanout/interception.
    inbound_queue: VecDeque<ZokaMessage>,
    /// Pending local private txs waiting to be batched.
    mixer_batcher: MixerBatcher,
    /// Dedup cache for processed mixer batches.
    seen_mixer_batches: HashSet<String>,
    /// Per-peer rate limit buckets for mixer batches.
    mixer_peer_buckets: HashMap<PeerId, (u64, u32)>,
    /// Dedup cache for processed stem routes.
    seen_stem_routes: HashSet<String>,
    /// Round-robin cursor for relay hint selection.
    stem_relay_cursor: usize,
    /// Peers observed with incompatible identify protocol version.
    incompatible_protocol_peers: HashSet<PeerId>,
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
    fn endpoint_reachable(endpoint: &str) -> bool {
        use std::net::{TcpStream, ToSocketAddrs};

        let Ok(mut addrs) = endpoint.to_socket_addrs() else {
            return false;
        };
        let Some(addr) = addrs.next() else {
            return false;
        };
        TcpStream::connect_timeout(&addr, Duration::from_millis(250)).is_ok()
    }

    fn privacy_transport_mode_for(config: &NetworkPrivacyConfig) -> PrivacyTransportMode {
        match (
            config.tor_proxy.as_ref().filter(|v| !v.is_empty()),
            config.i2p_sam_endpoint.as_ref().filter(|v| !v.is_empty()),
        ) {
            (Some(_), Some(_)) => PrivacyTransportMode::Hybrid,
            (Some(_), None) => PrivacyTransportMode::Tor,
            (None, Some(_)) => PrivacyTransportMode::I2p,
            (None, None) => PrivacyTransportMode::Direct,
        }
    }

    pub fn privacy_transport_status(&self) -> PrivacyTransportStatus {
        let tor_reachable = self
            .privacy_config
            .tor_proxy
            .as_ref()
            .map(|endpoint| Self::endpoint_reachable(endpoint))
            .unwrap_or(false);
        let i2p_reachable = self
            .privacy_config
            .i2p_sam_endpoint
            .as_ref()
            .map(|endpoint| Self::endpoint_reachable(endpoint))
            .unwrap_or(false);

        PrivacyTransportStatus {
            mode: Self::privacy_transport_mode_for(&self.privacy_config),
            tor_reachable,
            i2p_reachable,
        }
    }

    pub fn configure_tor_proxy(&mut self, tor_proxy: impl Into<String>, enforce: bool) {
        self.privacy_config.tor_proxy = Some(tor_proxy.into());
        if enforce {
            self.privacy_config.enforce_anonymous_transport = true;
            self.privacy_config.i2p_sam_endpoint = None;
        }
    }

    pub fn configure_i2p_endpoint(&mut self, i2p_sam_endpoint: impl Into<String>, enforce: bool) {
        self.privacy_config.i2p_sam_endpoint = Some(i2p_sam_endpoint.into());
        if enforce {
            self.privacy_config.enforce_anonymous_transport = true;
            self.privacy_config.tor_proxy = None;
        }
    }

    fn parse_onion_component(component: &str) -> Option<(String, u16)> {
        let (id, port_str) = component.rsplit_once(':')?;
        let port = port_str.parse::<u16>().ok()?;
        if id.is_empty() {
            return None;
        }
        Some((format!("{id}.onion"), port))
    }

    fn dial_target_from_multiaddr(addr: &Multiaddr) -> Option<DialTarget> {
        let addr_string = addr.to_string();
        let parts: Vec<&str> = addr_string.split('/').filter(|p| !p.is_empty()).collect();
        if parts.len() < 2 {
            return None;
        }

        // /onion3/<id:port> or /onion/<id:port>
        if parts[0] == "onion3" || parts[0] == "onion" {
            let (host, port) = Self::parse_onion_component(parts.get(1).copied()?)?;
            return Some(DialTarget {
                host,
                port,
                kind: DialTargetKind::TorOnion,
            });
        }

        // /dns4/<host>/tcp/<port>, /dns6/... or /dns/...
        if (parts[0] == "dns" || parts[0] == "dns4" || parts[0] == "dns6")
            && parts.len() >= 4
            && parts[2] == "tcp"
        {
            let host = parts[1].to_string();
            let port = parts[3].parse::<u16>().ok()?;
            let kind = if host.ends_with(".b32.i2p") {
                DialTargetKind::I2pB32
            } else if host.ends_with(".onion") {
                DialTargetKind::TorOnion
            } else {
                DialTargetKind::Direct
            };
            return Some(DialTarget { host, port, kind });
        }

        // /ip4/<addr>/tcp/<port> or /ip6/<addr>/tcp/<port>
        if (parts[0] == "ip4" || parts[0] == "ip6") && parts.len() >= 4 && parts[2] == "tcp" {
            return Some(DialTarget {
                host: parts[1].to_string(),
                port: parts[3].parse::<u16>().ok()?,
                kind: DialTargetKind::Direct,
            });
        }

        None
    }

    async fn probe_tor_proxy_route(
        proxy_endpoint: &str,
        target: &DialTarget,
    ) -> Result<(), io::Error> {
        use tokio_socks::tcp::Socks5Stream;

        let stream = Socks5Stream::connect(proxy_endpoint, (target.host.as_str(), target.port))
            .await
            .map_err(|e| io::Error::other(format!("SOCKS5 connect failed: {e}")))?;
        drop(stream);
        Ok(())
    }

    pub async fn probe_anonymous_route(&self, peer_addr: &str) -> Result<(), Box<dyn Error>> {
        let addr: Multiaddr = peer_addr.parse()?;
        let target = Self::dial_target_from_multiaddr(&addr).ok_or_else(|| {
            io::Error::other("peer multiaddr is not a supported tcp/onion/dns target")
        })?;

        let mode = Self::privacy_transport_mode_for(&self.privacy_config);
        match mode {
            PrivacyTransportMode::Direct => Ok(()),
            PrivacyTransportMode::Tor => {
                if target.kind != DialTargetKind::TorOnion {
                    return Err(Box::new(io::Error::other(
                        "Tor mode only allows onion destinations",
                    )));
                }
                let proxy = self
                    .privacy_config
                    .tor_proxy
                    .as_ref()
                    .ok_or_else(|| io::Error::other("tor_proxy is not configured"))?;
                Self::probe_tor_proxy_route(proxy, &target).await?;
                Ok(())
            }
            PrivacyTransportMode::I2p => {
                if target.kind != DialTargetKind::I2pB32 {
                    return Err(Box::new(io::Error::other(
                        "I2P mode only allows .b32.i2p destinations",
                    )));
                }
                Ok(())
            }
            PrivacyTransportMode::Hybrid => {
                if target.kind == DialTargetKind::TorOnion {
                    let proxy = self
                        .privacy_config
                        .tor_proxy
                        .as_ref()
                        .ok_or_else(|| io::Error::other("tor_proxy is not configured"))?;
                    Self::probe_tor_proxy_route(proxy, &target).await?;
                    Ok(())
                } else if target.kind == DialTargetKind::I2pB32 {
                    Ok(())
                } else {
                    Err(Box::new(io::Error::other(
                        "Hybrid mode only allows onion or .b32.i2p destinations",
                    )))
                }
            }
        }
    }

    fn ensure_private_transport_ready(&self) -> Result<(), Box<dyn Error>> {
        if !self.privacy_config.enforce_anonymous_transport {
            return Ok(());
        }

        let status = self.privacy_transport_status();
        let ready = match status.mode {
            PrivacyTransportMode::Direct => false,
            PrivacyTransportMode::Tor => status.tor_reachable,
            PrivacyTransportMode::I2p => status.i2p_reachable,
            PrivacyTransportMode::Hybrid => status.tor_reachable || status.i2p_reachable,
        };
        if ready {
            return Ok(());
        }

        Err(Box::new(io::Error::other(
            "anonymous transport required for private transactions but configured Tor/I2P endpoint is not reachable",
        )))
    }

    fn dht_user_key(username: &str) -> kad::RecordKey {
        let normalized = username.trim().to_ascii_lowercase();
        let key = format!("zoka:user:{normalized}");
        kad::RecordKey::new(&key)
    }

    fn connected_peer_ids(&self) -> Vec<PeerId> {
        self.swarm.connected_peers().copied().collect()
    }

    fn next_stem_relay_hint(&mut self) -> String {
        let peers = self.connected_peer_ids();
        if peers.is_empty() {
            return String::new();
        }
        let idx = self.stem_relay_cursor % peers.len();
        self.stem_relay_cursor = self.stem_relay_cursor.wrapping_add(1);
        peers[idx].to_string()
    }

    fn padded_private_tx_size(&self, tx: &PrivateTransactionMessage) -> usize {
        let tx_size = serde_json::to_vec(tx).map(|v| v.len()).unwrap_or(0);
        self.privacy_config
            .private_padding_bytes
            .saturating_sub(tx_size)
    }

    fn compute_mixer_batch_id(txs: &[PrivateTransactionMessage]) -> String {
        use sha2::{Digest, Sha256};

        let mut tx_hashes = txs
            .iter()
            .map(|tx| tx.tx_hash.clone())
            .collect::<Vec<String>>();
        tx_hashes.sort_unstable();
        let mut hasher = Sha256::new();
        for hash in tx_hashes {
            hasher.update(hash.as_bytes());
            hasher.update([0x1f]);
        }
        hex::encode(hasher.finalize())
    }

    fn padded_mixer_batch_size(&self, txs: &[PrivateTransactionMessage]) -> usize {
        let payload_size = serde_json::to_vec(txs).map(|v| v.len()).unwrap_or(0);
        let target = self
            .privacy_config
            .private_padding_bytes
            .saturating_mul(txs.len().max(1));
        target.saturating_sub(payload_size)
    }

    fn build_mixer_batch_message(&self, txs: Vec<PrivateTransactionMessage>) -> MixerBatchMessage {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let padding = vec![0u8; self.padded_mixer_batch_size(&txs)];
        MixerBatchMessage {
            batch_id: Self::compute_mixer_batch_id(&txs),
            txs,
            padding,
            timestamp,
        }
    }

    fn is_private_tx_well_formed(tx: &PrivateTransactionMessage) -> bool {
        if tx.tx_hash.is_empty() || tx.commitment.is_empty() || tx.nullifier.is_empty() {
            return false;
        }
        if tx.ring_version != 1 {
            return false;
        }
        if tx.ring_members.len() < 3 {
            return false;
        }
        if tx.input_key_images.len() > crate::zk::groth16_backend::PRIVATE_TX_MAX_INPUTS {
            return false;
        }
        if !tx.input_key_images.is_empty() {
            let per_input_signature_len =
                match 64usize.checked_add(tx.ring_members.len().checked_mul(32).unwrap_or(0)) {
                    Some(v) => v,
                    None => return false,
                };
            if tx.ring_signature.len()
                != tx
                    .input_key_images
                    .len()
                    .checked_mul(per_input_signature_len)
                    .unwrap_or(usize::MAX)
            {
                return false;
            }
        }
        if tx.outputs.is_empty() {
            return false;
        }
        if !tx.input_asset_tags.is_empty() && tx.input_asset_tags.len() != tx.input_nullifiers.len()
        {
            return false;
        }
        tx.outputs.iter().all(|output| {
            !output.asset_tag.trim().is_empty()
                && !output.asset_tag_commitment.trim().is_empty()
                && !output.commitment.is_empty()
                && !output.amount_commitment.is_empty()
                && !output.recipient_commitment.is_empty()
        })
    }

    fn validate_mixer_batch(&self, batch: &MixerBatchMessage) -> bool {
        if batch.txs.is_empty() || batch.txs.len() > self.privacy_config.mixer_max_batch.max(1) {
            return false;
        }
        let mut seen_hashes = HashSet::with_capacity(batch.txs.len());
        for tx in &batch.txs {
            if !Self::is_private_tx_well_formed(tx) || !seen_hashes.insert(tx.tx_hash.clone()) {
                return false;
            }
        }
        Self::compute_mixer_batch_id(&batch.txs) == batch.batch_id
    }

    fn is_protocol_version_compatible(version: &str) -> bool {
        version == PROTOCOL_FREEZE_ID
    }

    fn is_hex_digest_64(value: &str) -> bool {
        value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
    }

    fn validate_inbound_message(&self, message: &ZokaMessage) -> Result<(), &'static str> {
        match message {
            ZokaMessage::PrivateTransaction(tx) => {
                if !Self::is_private_tx_well_formed(tx) {
                    return Err("private tx failed protocol freeze validation");
                }
            }
            ZokaMessage::StemPrivateTransaction(stem) => {
                if stem.route_id.is_empty() || stem.stem_hops == 0 {
                    return Err("stem payload has invalid routing metadata");
                }
                if !Self::is_private_tx_well_formed(&stem.private_tx) {
                    return Err("stem private tx failed protocol freeze validation");
                }
            }
            ZokaMessage::PrivateMixerBatch(batch) => {
                if !self.validate_mixer_batch(batch) {
                    return Err("mixer batch failed protocol freeze validation");
                }
            }
            ZokaMessage::Pbft(pbft) => match pbft {
                PbftNetworkMessage::Consensus(msg) => {
                    if msg.sender.trim().is_empty() || msg.block_hash.trim().is_empty() {
                        return Err("pbft consensus payload is missing sender/block hash");
                    }
                }
                PbftNetworkMessage::ViewChange(msg) => {
                    if msg.sender.trim().is_empty() {
                        return Err("pbft view-change payload is missing sender");
                    }
                }
                PbftNetworkMessage::NewView(msg) => {
                    if msg.sender.trim().is_empty() {
                        return Err("pbft new-view payload is missing sender");
                    }
                }
            },
            ZokaMessage::BlockResponse(msg) => {
                if msg.block_hash.trim().is_empty() {
                    return Err("block response missing block hash");
                }
            }
            ZokaMessage::SyncState(msg) => {
                if msg.state_root.trim().is_empty() {
                    return Err("sync state missing state root");
                }
            }
            ZokaMessage::UserRegistry {
                username,
                record_digest,
                ..
            } => {
                if username.trim().is_empty() {
                    return Err("user registry missing username");
                }
                if !record_digest.is_empty() && !Self::is_hex_digest_64(record_digest) {
                    return Err("user registry has invalid digest format");
                }
            }
            ZokaMessage::UserQuery { username }
            | ZokaMessage::UserQueryResponse { username, .. } => {
                if username.trim().is_empty() {
                    return Err("user query payload missing username");
                }
            }
            ZokaMessage::Transaction(tx) => {
                if tx.hash.trim().is_empty()
                    || tx.sender.trim().is_empty()
                    || tx.receiver.trim().is_empty()
                {
                    return Err("public tx missing canonical fields");
                }
                if tx.amount == 0 {
                    return Err("public tx amount must be > 0");
                }
                if tx.compute_hash() != tx.hash {
                    return Err("public tx hash does not match canonical encoding");
                }
            }
            ZokaMessage::BlockRequest { .. } | ZokaMessage::Ping(_) => {}
        }
        Ok(())
    }

    fn mark_peer_protocol_version(&mut self, peer_id: PeerId, version: &str) -> bool {
        if Self::is_protocol_version_compatible(version) {
            self.incompatible_protocol_peers.remove(&peer_id);
            return true;
        }
        self.incompatible_protocol_peers.insert(peer_id);
        warn!(
            "peer {} uses incompatible protocol version {}; expecting {}",
            peer_id, version, PROTOCOL_FREEZE_ID
        );
        false
    }

    fn enforce_protocol_freeze_for_peer(&mut self, peer_id: PeerId, version: &str) {
        if self.mark_peer_protocol_version(peer_id, version) {
            return;
        }
        if self.swarm.disconnect_peer_id(peer_id).is_err() {
            warn!(
                "failed to disconnect incompatible peer {} after identify mismatch",
                peer_id
            );
        } else {
            warn!(
                "disconnected incompatible peer {} due to protocol freeze mismatch",
                peer_id
            );
        }
    }

    fn is_peer_protocol_allowed(&self, peer_id: &PeerId) -> bool {
        !self.incompatible_protocol_peers.contains(peer_id)
    }

    fn allow_mixer_batch_from_peer(&mut self, peer: &PeerId) -> bool {
        let current_minute = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            / 60;
        let bucket = self
            .mixer_peer_buckets
            .entry(*peer)
            .or_insert((current_minute, 0));
        if bucket.0 != current_minute {
            *bucket = (current_minute, 0);
        }
        if bucket.1 >= self.privacy_config.mixer_max_batches_per_minute.max(1) {
            return false;
        }
        bucket.1 += 1;
        true
    }

    fn publish_mixer_batch(
        &mut self,
        txs: Vec<PrivateTransactionMessage>,
    ) -> Result<(), Box<dyn Error>> {
        if txs.is_empty() {
            return Ok(());
        }
        let batch = self.build_mixer_batch_message(txs);
        let payload = ZokaMessage::PrivateMixerBatch(batch).to_bytes()?;
        self.swarm
            .behaviour_mut()
            .gossipsub
            .publish(self.mixer_topic.clone(), payload)
            .map(|_| ())
            .map_err(|e| -> Box<dyn Error> { Box::new(e) })
    }

    fn flush_due_private_batch(&mut self) -> Result<(), Box<dyn Error>> {
        if let Some(txs) = self
            .mixer_batcher
            .drain_due(self.privacy_config.mixer_hold_ms)
        {
            self.publish_mixer_batch(txs)?;
        }
        Ok(())
    }

    fn build_stem_private_message(
        &mut self,
        tx: PrivateTransactionMessage,
    ) -> StemPrivateTransactionMessage {
        use sha2::{Digest, Sha256};

        let payload = serde_json::to_vec(&tx).unwrap_or_default();
        let mut hasher = Sha256::new();
        hasher.update(tx.tx_hash.as_bytes());
        hasher.update([0x1f]);
        hasher.update(tx.timestamp.to_le_bytes());
        hasher.update([0x1f]);
        hasher.update(&payload);
        let route_id = hex::encode(hasher.finalize());

        StemPrivateTransactionMessage {
            route_id,
            relay_hint: self.next_stem_relay_hint(),
            stem_hops: self.privacy_config.stem_hops.max(1),
            padding: vec![0u8; self.padded_private_tx_size(&tx)],
            private_tx: tx,
        }
    }

    fn publish_stem_private_transaction(
        &mut self,
        stem: StemPrivateTransactionMessage,
    ) -> Result<(), Box<dyn Error>> {
        let payload = ZokaMessage::StemPrivateTransaction(stem).to_bytes()?;
        self.swarm
            .behaviour_mut()
            .gossipsub
            .publish(self.stem_topic.clone(), payload)
            .map(|_| ())
            .map_err(|e| -> Box<dyn Error> { Box::new(e) })
    }

    fn stem_jitter_delay(&self, route_id: &str) -> Duration {
        use sha2::{Digest, Sha256};

        let max_ms = self.privacy_config.stem_jitter_ms;
        if max_ms == 0 {
            return Duration::from_millis(0);
        }
        let mut hasher = Sha256::new();
        hasher.update(route_id.as_bytes());
        hasher.update(self.peer_id.to_string().as_bytes());
        let digest = hasher.finalize();
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&digest[..8]);
        let value = u64::from_le_bytes(bytes);
        Duration::from_millis(value % (max_ms + 1))
    }

    async fn maybe_intercept_stem_message(
        &mut self,
        stem: StemPrivateTransactionMessage,
    ) -> Result<Option<ZokaMessage>, Box<dyn Error>> {
        if !self.privacy_config.enable_dandelion {
            return Ok(Some(ZokaMessage::PrivateTransaction(stem.private_tx)));
        }

        if !self.seen_stem_routes.insert(stem.route_id.clone()) {
            return Ok(None);
        }

        let my_peer_id = self.peer_id.to_string();
        if !stem.relay_hint.is_empty() && stem.relay_hint != my_peer_id {
            return Ok(None);
        }

        if stem.stem_hops > 1 && self.connected_peer_count() > 0 {
            let jitter = self.stem_jitter_delay(&stem.route_id);
            if !jitter.is_zero() {
                tokio::time::sleep(jitter).await;
            }
            let next_stem = StemPrivateTransactionMessage {
                route_id: stem.route_id,
                relay_hint: self.next_stem_relay_hint(),
                stem_hops: stem.stem_hops - 1,
                padding: stem.padding,
                private_tx: stem.private_tx.clone(),
            };
            self.publish_stem_private_transaction(next_stem)?;
            return Ok(Some(ZokaMessage::PrivateTransaction(stem.private_tx)));
        }

        let private_tx = stem.private_tx;
        let jitter = self.stem_jitter_delay(&private_tx.tx_hash);
        if !jitter.is_zero() {
            tokio::time::sleep(jitter).await;
        }
        self.publish_message_direct(&ZokaMessage::PrivateTransaction(private_tx.clone()))?;
        Ok(Some(ZokaMessage::PrivateTransaction(private_tx)))
    }

    fn maybe_intercept_mixer_message(
        &mut self,
        propagation_source: PeerId,
        batch: MixerBatchMessage,
    ) -> Result<(), Box<dyn Error>> {
        if !self.allow_mixer_batch_from_peer(&propagation_source) {
            return Ok(());
        }
        if self.seen_mixer_batches.contains(&batch.batch_id) {
            return Ok(());
        }
        if !self.validate_mixer_batch(&batch) {
            return Ok(());
        }
        self.seen_mixer_batches.insert(batch.batch_id.clone());
        for tx in batch.txs {
            self.inbound_queue
                .push_back(ZokaMessage::PrivateTransaction(tx));
        }
        Ok(())
    }

    /// Create a new node with gossipsub + identify + kademlia.
    pub async fn new(_listen_port: u16) -> Result<Self, Box<dyn Error>> {
        let local_key = identity::Keypair::generate_ed25519();
        let peer_id = PeerId::from(local_key.public());
        info!("Local PeerId: {}", peer_id);

        let general_topic = gossipsub::IdentTopic::new(GENERAL_TOPIC_NAME);
        let pbft_topic = gossipsub::IdentTopic::new(PBFT_TOPIC_NAME);
        let stem_topic = gossipsub::IdentTopic::new(STEM_TOPIC_NAME);
        let mixer_topic = gossipsub::IdentTopic::new(MIXER_TOPIC_NAME);
        let general_topic_for_behaviour = general_topic.clone();
        let pbft_topic_for_behaviour = pbft_topic.clone();
        let stem_topic_for_behaviour = stem_topic.clone();
        let mixer_topic_for_behaviour = mixer_topic.clone();

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
                gossipsub.subscribe(&stem_topic_for_behaviour)?;
                gossipsub.subscribe(&mixer_topic_for_behaviour)?;

                Ok(NodeBehaviour {
                    identify: identify::Behaviour::new(identify::Config::new(
                        PROTOCOL_FREEZE_ID.to_string(),
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
            stem_topic,
            mixer_topic,
            privacy_config: NetworkPrivacyConfig::default(),
            inbound_queue: VecDeque::new(),
            mixer_batcher: MixerBatcher::new(),
            seen_mixer_batches: HashSet::new(),
            mixer_peer_buckets: HashMap::new(),
            seen_stem_routes: HashSet::new(),
            stem_relay_cursor: 0,
            incompatible_protocol_peers: HashSet::new(),
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
        if self.privacy_config.enforce_anonymous_transport {
            self.probe_anonymous_route(peer_addr).await?;
        }

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
                    gossipsub::Event::Message {
                        propagation_source,
                        message,
                        ..
                    },
                )) => {
                    if !self.is_peer_protocol_allowed(&propagation_source) {
                        warn!(
                            "discarding message from incompatible peer {} while waiting listen addr",
                            propagation_source
                        );
                        continue;
                    }
                    let decoded = ZokaMessage::from_bytes(&message.data);
                    match decoded {
                        Ok(msg) => {
                            if let Err(err) = self.validate_inbound_message(&msg) {
                                warn!(
                                    "discarding invalid message while waiting listen addr: {}",
                                    err
                                );
                            }
                        }
                        Err(_) => {
                            warn!("discarding malformed message while waiting for listen address");
                        }
                    }
                }
                SwarmEvent::Behaviour(NodeBehaviourEvent::Identify(
                    identify::Event::Received { peer_id, info, .. },
                )) => {
                    self.enforce_protocol_freeze_for_peer(peer_id, &info.protocol_version);
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
                    gossipsub::Event::Message {
                        propagation_source,
                        message,
                        ..
                    },
                )) => {
                    if !self.is_peer_protocol_allowed(&propagation_source) {
                        warn!(
                            "discarding message from incompatible peer {} while waiting for peer connection",
                            propagation_source
                        );
                        continue;
                    }
                    let decoded = ZokaMessage::from_bytes(&message.data);
                    match decoded {
                        Ok(msg) => {
                            if let Err(err) = self.validate_inbound_message(&msg) {
                                warn!(
                                    "discarding invalid message while waiting for peer connection: {}",
                                    err
                                );
                            }
                        }
                        Err(_) => {
                            warn!("discarding malformed message while waiting for peer connection");
                        }
                    }
                }
                SwarmEvent::Behaviour(NodeBehaviourEvent::Identify(
                    identify::Event::Received { peer_id, info, .. },
                )) => {
                    self.enforce_protocol_freeze_for_peer(peer_id, &info.protocol_version);
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
                    gossipsub::Event::Message {
                        propagation_source,
                        message,
                        ..
                    },
                )) => {
                    if !self.is_peer_protocol_allowed(&propagation_source) {
                        warn!(
                            "discarding message from incompatible peer {} while waiting any connection",
                            propagation_source
                        );
                        continue;
                    }
                    let decoded = ZokaMessage::from_bytes(&message.data);
                    match decoded {
                        Ok(msg) => {
                            if let Err(err) = self.validate_inbound_message(&msg) {
                                warn!(
                                    "discarding invalid message while waiting any connection: {}",
                                    err
                                );
                            }
                        }
                        Err(_) => {
                            warn!("discarding malformed message while waiting for any connection");
                        }
                    }
                }
                SwarmEvent::Behaviour(NodeBehaviourEvent::Identify(
                    identify::Event::Received { peer_id, info, .. },
                )) => {
                    self.enforce_protocol_freeze_for_peer(peer_id, &info.protocol_version);
                }
                _ => {}
            }
        }
    }

    /// Pump swarm events for a bounded duration.
    pub async fn pump_for(&mut self, duration: Duration) -> Result<(), Box<dyn Error>> {
        let deadline = tokio::time::Instant::now() + duration;
        loop {
            self.flush_due_private_batch()?;
            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => break,
                event = self.swarm.select_next_some() => {
                    match event {
                SwarmEvent::Behaviour(NodeBehaviourEvent::Gossipsub(
                    gossipsub::Event::Message { propagation_source, message, .. },
                )) => {
                    if !self.is_peer_protocol_allowed(&propagation_source) {
                        warn!(
                            "discarding message from incompatible peer {} while pumping",
                            propagation_source
                        );
                        continue;
                    }
                    match ZokaMessage::from_bytes(&message.data) {
                        Ok(ZokaMessage::StemPrivateTransaction(stem)) => {
                            if let Err(err) = self
                                .validate_inbound_message(&ZokaMessage::StemPrivateTransaction(
                                    stem.clone(),
                                ))
                            {
                                warn!("discarding invalid stem message while pumping: {}", err);
                                continue;
                            }
                            let _ = self.maybe_intercept_stem_message(stem).await?;
                        }
                        Ok(ZokaMessage::PrivateMixerBatch(batch)) => {
                            if let Err(err) = self
                                .validate_inbound_message(&ZokaMessage::PrivateMixerBatch(
                                    batch.clone(),
                                ))
                            {
                                warn!("discarding invalid mixer batch while pumping: {}", err);
                                continue;
                            }
                            self.maybe_intercept_mixer_message(propagation_source, batch)?;
                        }
                        Ok(msg) => {
                            if let Err(err) = self.validate_inbound_message(&msg) {
                                warn!("discarding invalid message while pumping swarm: {}", err);
                            }
                        }
                        Err(_) => {
                            warn!("discarding malformed message while pumping swarm");
                        }
                    }
                }
                        SwarmEvent::Behaviour(NodeBehaviourEvent::Identify(identify::Event::Received {
                            peer_id,
                            info,
                            ..
                        })) => {
                            self.enforce_protocol_freeze_for_peer(peer_id, &info.protocol_version);
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

    fn publish_message_direct(&mut self, message: &ZokaMessage) -> Result<(), Box<dyn Error>> {
        let payload = message.to_bytes()?;
        let topic = match message {
            ZokaMessage::Pbft(_) => self.pbft_topic.clone(),
            ZokaMessage::StemPrivateTransaction(_) => self.stem_topic.clone(),
            ZokaMessage::PrivateMixerBatch(_) => self.mixer_topic.clone(),
            _ => self.general_topic.clone(),
        };
        self.swarm
            .behaviour_mut()
            .gossipsub
            .publish(topic, payload)
            .map(|_| ())
            .map_err(|e| -> Box<dyn Error> { Box::new(e) })
    }

    /// Publish a protocol message on the appropriate gossipsub topic.
    pub fn publish_message(&mut self, message: &ZokaMessage) -> Result<(), Box<dyn Error>> {
        self.flush_due_private_batch()?;
        match message {
            ZokaMessage::PrivateTransaction(tx) if self.privacy_config.enable_private_mixer => {
                self.ensure_private_transport_ready()?;
                if let Some(txs) = self.mixer_batcher.enqueue(
                    tx.clone(),
                    self.privacy_config.mixer_hold_ms,
                    self.privacy_config.mixer_max_batch,
                ) {
                    self.publish_mixer_batch(txs)?;
                }
                Ok(())
            }
            ZokaMessage::PrivateTransaction(tx) if self.privacy_config.enable_dandelion => {
                self.ensure_private_transport_ready()?;
                let stem = self.build_stem_private_message(tx.clone());
                self.publish_stem_private_transaction(stem)
            }
            _ => self.publish_message_direct(message),
        }
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
            self.flush_due_private_batch()?;
            if let Some(message) = self.inbound_queue.pop_front() {
                return Ok(message);
            }
            match self.swarm.select_next_some().await {
                SwarmEvent::Behaviour(NodeBehaviourEvent::Gossipsub(
                    gossipsub::Event::Message {
                        propagation_source,
                        message,
                        ..
                    },
                )) => {
                    if !self.is_peer_protocol_allowed(&propagation_source) {
                        warn!(
                            "discarding message from incompatible peer {}",
                            propagation_source
                        );
                        continue;
                    }
                    match ZokaMessage::from_bytes(&message.data) {
                        Ok(ZokaMessage::StemPrivateTransaction(stem)) => {
                            if let Err(err) = self.validate_inbound_message(
                                &ZokaMessage::StemPrivateTransaction(stem.clone()),
                            ) {
                                warn!("discarding invalid stem message: {}", err);
                                continue;
                            }
                            if let Some(msg) = self.maybe_intercept_stem_message(stem).await? {
                                return Ok(msg);
                            }
                        }
                        Ok(ZokaMessage::PrivateMixerBatch(batch)) => {
                            if let Err(err) = self.validate_inbound_message(
                                &ZokaMessage::PrivateMixerBatch(batch.clone()),
                            ) {
                                warn!("discarding invalid mixer batch: {}", err);
                                continue;
                            }
                            self.maybe_intercept_mixer_message(propagation_source, batch)?;
                            if let Some(message) = self.inbound_queue.pop_front() {
                                return Ok(message);
                            }
                        }
                        Ok(msg) => {
                            if let Err(err) = self.validate_inbound_message(&msg) {
                                warn!("discarding invalid message: {}", err);
                                continue;
                            }
                            return Ok(msg);
                        }
                        Err(err) => warn!("discarding malformed message: {}", err),
                    }
                }
                SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                    info!("Peer connected: {}", peer_id);
                }
                SwarmEvent::NewListenAddr { address, .. } => {
                    info!("Listening address announced: {}", address);
                }
                SwarmEvent::Behaviour(NodeBehaviourEvent::Identify(
                    identify::Event::Received { peer_id, info, .. },
                )) => {
                    self.enforce_protocol_freeze_for_peer(peer_id, &info.protocol_version);
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
            ZokaMessage::StemPrivateTransaction(_) => STEM_TOPIC_NAME,
            ZokaMessage::PrivateMixerBatch(_) => MIXER_TOPIC_NAME,
            _ => GENERAL_TOPIC_NAME,
        }
    }

    pub fn set_privacy_config(&mut self, config: NetworkPrivacyConfig) {
        self.privacy_config = NetworkPrivacyConfig {
            stem_hops: config.stem_hops.max(1),
            mixer_max_batch: config.mixer_max_batch.clamp(1, 16),
            ..config
        };
    }

    pub fn privacy_config(&self) -> &NetworkPrivacyConfig {
        &self.privacy_config
    }

    /// Return local peer id.
    pub fn peer_id(&self) -> PeerId {
        self.peer_id
    }

    /// Return current number of connected peers.
    pub fn connected_peer_count(&self) -> usize {
        self.swarm.connected_peers().count()
    }

    /// Return number of peers blocked by protocol-freeze compatibility rules.
    pub fn incompatible_peer_count(&self) -> usize {
        self.incompatible_protocol_peers.len()
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
    use crate::network::message::{PingMessage, PrivateOutputMessage};
    use crate::types::{Signature, Transaction};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn spawn_mock_socks5_proxy() -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock socks5 listener");
        let endpoint = format!(
            "127.0.0.1:{}",
            listener.local_addr().expect("local addr").port()
        );
        let handle = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept socks5 conn");

            let mut greeting = [0u8; 2];
            socket
                .read_exact(&mut greeting)
                .await
                .expect("read greeting");
            let mut methods = vec![0u8; greeting[1] as usize];
            socket.read_exact(&mut methods).await.expect("read methods");
            socket
                .write_all(&[0x05, 0x00])
                .await
                .expect("write auth ok");

            let mut req_hdr = [0u8; 4];
            socket
                .read_exact(&mut req_hdr)
                .await
                .expect("read request header");
            match req_hdr[3] {
                0x01 => {
                    let mut payload = [0u8; 6];
                    socket
                        .read_exact(&mut payload)
                        .await
                        .expect("read ipv4 payload");
                }
                0x03 => {
                    let mut len = [0u8; 1];
                    socket.read_exact(&mut len).await.expect("read domain len");
                    let mut payload = vec![0u8; len[0] as usize + 2];
                    socket
                        .read_exact(&mut payload)
                        .await
                        .expect("read domain payload");
                }
                0x04 => {
                    let mut payload = [0u8; 18];
                    socket
                        .read_exact(&mut payload)
                        .await
                        .expect("read ipv6 payload");
                }
                atyp => panic!("unexpected socks atyp: {atyp}"),
            }

            let reply = [0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0];
            socket.write_all(&reply).await.expect("write connect reply");
        });

        (endpoint, handle)
    }

    fn sample_private_tx(hash: &str) -> PrivateTransactionMessage {
        PrivateTransactionMessage {
            tx_hash: hash.to_string(),
            commitment: "commitment".to_string(),
            stealth_address: "stealth".to_string(),
            ephemeral_pubkey: vec![1u8; 32],
            nullifier: format!("nullifier-{hash}"),
            zk_proof: None,
            timestamp: 10,
            recipient_commitment: "recipient".to_string(),
            encrypted_amount: vec![7u8; 8],
            proof_commitment: 1,
            proof_nullifier: 2,
            proof_input_sum: 3,
            proof_output_sum: 3,
            proof_fee: 0,
            proof_key_image: 4,
            proof_private_root_words: vec![1, 2, 3, 4],
            fee: 0,
            ring_id: "ring".to_string(),
            ring_version: 1,
            ring_members: vec!["member-a".to_string(), "member-b".to_string()],
            ring_signature: vec![3u8; 128],
            input_nullifiers: vec![format!("nullifier-{hash}")],
            input_commitments: vec!["commitment".to_string()],
            input_asset_tags: vec!["ZOKA".to_string()],
            outputs: vec![PrivateOutputMessage {
                output_id: "out-1".to_string(),
                asset_tag: "ZOKA".to_string(),
                asset_tag_commitment: "asset-tag-commitment".to_string(),
                recipient_commitment: "recipient".to_string(),
                stealth_address: "stealth".to_string(),
                ephemeral_pubkey: vec![1u8; 32],
                commitment: "commitment".to_string(),
                amount_commitment: "amount-commitment".to_string(),
                range_proof: vec![8u8; 32],
                one_time_public_key: "one-time-key".to_string(),
                encrypted_amount: vec![7u8; 8],
                commitment_blind: vec![4u8; 32],
                is_change: false,
            }],
            input_public_keys: vec!["input-pub-1".to_string()],
            input_key_images: vec!["key-image-1".to_string()],
            input_amount_commitments: vec!["amount-commitment-in".to_string()],
            amount_balance_commitment: "balance-commitment".to_string(),
            fee_asset_tag: "ZOKA".to_string(),
            asset_balance_commitments: vec![
                crate::network::message::AssetBalanceCommitmentMessage {
                    asset_tag: "ZOKA".to_string(),
                    balance_commitment: "balance-commitment".to_string(),
                },
            ],
            fee_commitment: String::new(),
            fee_range_proof: Vec::new(),
            private_commitment_root: String::new(),
            input_inclusion_proofs: vec![],
            commitment_blind: vec![4u8; 32],
            nullifier_nonce: vec![5u8; 16],
        }
    }

    #[tokio::test]
    async fn test_node_creation() {
        let node = Node::new(9000).await;
        assert!(node.is_ok());

        let node = node.expect("node");
        assert_ne!(node.peer_id(), PeerId::random());
    }

    #[test]
    fn test_protocol_freeze_version_match() {
        assert!(Node::is_protocol_version_compatible(PROTOCOL_FREEZE_ID));
        assert!(!Node::is_protocol_version_compatible("/zoka/0.9.0"));
    }

    #[tokio::test]
    async fn test_mark_incompatible_peer_blocks_messages() {
        let mut node = Node::new(0).await.expect("node");
        let peer = PeerId::random();

        assert!(!node.mark_peer_protocol_version(peer, "/zoka/0.9.0"));
        assert!(!node.is_peer_protocol_allowed(&peer));
        assert_eq!(node.incompatible_peer_count(), 1);

        assert!(node.mark_peer_protocol_version(peer, PROTOCOL_FREEZE_ID));
        assert!(node.is_peer_protocol_allowed(&peer));
        assert_eq!(node.incompatible_peer_count(), 0);
    }

    #[tokio::test]
    async fn test_protocol_freeze_rejects_invalid_user_registry_digest() {
        let node = Node::new(0).await.expect("node");
        let message = ZokaMessage::UserRegistry {
            username: "alice".to_string(),
            record_digest: "not-a-digest".to_string(),
            updated_at: 1,
        };
        assert!(node.validate_inbound_message(&message).is_err());
    }

    #[tokio::test]
    async fn test_protocol_freeze_rejects_public_tx_with_hash_mismatch() {
        let node = Node::new(0).await.expect("node");
        let tx = Transaction {
            hash: "wrong-hash".to_string(),
            sender: "sender-a".to_string(),
            receiver: "receiver-b".to_string(),
            amount: 10,
            nonce: 1,
            timestamp: 123,
            signature: Signature(vec![1, 2, 3]),
        };
        let message = ZokaMessage::Transaction(tx);
        assert!(node.validate_inbound_message(&message).is_err());
    }

    #[test]
    fn test_topic_routing() {
        let ping = ZokaMessage::Ping(PingMessage {
            nonce: 1,
            timestamp: 10,
        });
        assert_eq!(Node::topic_name_for_message(&ping), GENERAL_TOPIC_NAME);

        let stem = ZokaMessage::StemPrivateTransaction(StemPrivateTransactionMessage {
            route_id: "route-1".to_string(),
            relay_hint: "peer-a".to_string(),
            stem_hops: 2,
            padding: vec![0u8; 16],
            private_tx: PrivateTransactionMessage {
                tx_hash: "tx-1".to_string(),
                commitment: "commitment".to_string(),
                stealth_address: "stealth".to_string(),
                ephemeral_pubkey: vec![1u8; 32],
                nullifier: "nullifier".to_string(),
                zk_proof: None,
                timestamp: 10,
                recipient_commitment: "recipient".to_string(),
                encrypted_amount: vec![7u8; 8],
                proof_commitment: 1,
                proof_nullifier: 2,
                proof_input_sum: 3,
                proof_output_sum: 3,
                proof_fee: 0,
                proof_key_image: 4,
                proof_private_root_words: vec![1, 2, 3, 4],
                fee: 0,
                ring_id: "ring".to_string(),
                ring_version: 1,
                ring_members: vec!["member".to_string()],
                ring_signature: vec![3u8; 128],
                input_nullifiers: vec!["nullifier".to_string()],
                input_commitments: vec!["commitment".to_string()],
                input_asset_tags: vec!["ZOKA".to_string()],
                outputs: vec![PrivateOutputMessage {
                    output_id: "out-1".to_string(),
                    asset_tag: "ZOKA".to_string(),
                    asset_tag_commitment: "asset-tag-commitment".to_string(),
                    recipient_commitment: "recipient".to_string(),
                    stealth_address: "stealth".to_string(),
                    ephemeral_pubkey: vec![1u8; 32],
                    commitment: "commitment".to_string(),
                    amount_commitment: "amount-commitment".to_string(),
                    range_proof: vec![8u8; 32],
                    one_time_public_key: "one-time-key".to_string(),
                    encrypted_amount: vec![7u8; 8],
                    commitment_blind: vec![4u8; 32],
                    is_change: false,
                }],
                input_public_keys: vec!["input-pub-1".to_string()],
                input_key_images: vec!["key-image-1".to_string()],
                input_amount_commitments: vec!["amount-commitment-in".to_string()],
                amount_balance_commitment: "balance-commitment".to_string(),
                fee_asset_tag: "ZOKA".to_string(),
                asset_balance_commitments: vec![
                    crate::network::message::AssetBalanceCommitmentMessage {
                        asset_tag: "ZOKA".to_string(),
                        balance_commitment: "balance-commitment".to_string(),
                    },
                ],
                fee_commitment: String::new(),
                fee_range_proof: Vec::new(),
                private_commitment_root: String::new(),
                input_inclusion_proofs: vec![],
                commitment_blind: vec![4u8; 32],
                nullifier_nonce: vec![5u8; 16],
            },
        });
        assert_eq!(Node::topic_name_for_message(&stem), STEM_TOPIC_NAME);

        let pbft = ZokaMessage::Pbft(PbftNetworkMessage::Consensus(PbftMessage::new(
            PbftMessageType::Prepare,
            0,
            1,
            "hash".to_string(),
            "n1".to_string(),
        )));
        assert_eq!(Node::topic_name_for_message(&pbft), PBFT_TOPIC_NAME);

        let mixer = ZokaMessage::PrivateMixerBatch(MixerBatchMessage {
            batch_id: "batch-1".to_string(),
            txs: vec![sample_private_tx("tx-mixer")],
            padding: vec![0u8; 8],
            timestamp: 55,
        });
        assert_eq!(Node::topic_name_for_message(&mixer), MIXER_TOPIC_NAME);
    }

    #[tokio::test]
    async fn test_set_privacy_config_clamps_mixer_batch() {
        let mut node = Node::new(0).await.expect("node");
        node.set_privacy_config(NetworkPrivacyConfig {
            mixer_max_batch: 100,
            stem_hops: 0,
            ..NetworkPrivacyConfig::default()
        });
        assert_eq!(node.privacy_config().mixer_max_batch, 16);
        assert_eq!(node.privacy_config().stem_hops, 1);
    }

    #[tokio::test]
    async fn test_mixer_batch_rejects_invalid_transaction_entirely() {
        let mut node = Node::new(0).await.expect("node");
        node.set_privacy_config(NetworkPrivacyConfig {
            mixer_max_batch: 8,
            ..NetworkPrivacyConfig::default()
        });

        let mut invalid = sample_private_tx("tx-invalid");
        invalid.tx_hash.clear();

        let batch = MixerBatchMessage {
            batch_id: "batch-invalid".to_string(),
            txs: vec![sample_private_tx("tx-valid"), invalid],
            padding: vec![],
            timestamp: 1,
        };
        node.maybe_intercept_mixer_message(PeerId::random(), batch)
            .expect("intercept should not error");
        assert!(
            node.inbound_queue.is_empty(),
            "invalid tx must reject whole batch"
        );
    }

    #[tokio::test]
    async fn test_mixer_batch_rejects_unknown_ring_version() {
        let mut node = Node::new(0).await.expect("node");
        node.set_privacy_config(NetworkPrivacyConfig {
            mixer_max_batch: 8,
            ..NetworkPrivacyConfig::default()
        });

        let mut invalid = sample_private_tx("tx-invalid-version");
        invalid.ring_version = 77;
        let batch = MixerBatchMessage {
            batch_id: "batch-invalid-version".to_string(),
            txs: vec![invalid],
            padding: vec![],
            timestamp: 1,
        };
        node.maybe_intercept_mixer_message(PeerId::random(), batch)
            .expect("intercept should not error");
        assert!(node.inbound_queue.is_empty());
    }

    #[test]
    fn test_compute_mixer_batch_id_is_order_independent() {
        let a = sample_private_tx("a");
        let b = sample_private_tx("b");
        let id_ab = Node::compute_mixer_batch_id(&[a.clone(), b.clone()]);
        let id_ba = Node::compute_mixer_batch_id(&[b, a]);
        assert_eq!(id_ab, id_ba);
    }

    #[test]
    fn test_mixer_batcher_groups_burst_into_three_batches() {
        let mut batcher = MixerBatcher::new();
        let mut batch_sizes = Vec::new();
        for i in 0..10 {
            let maybe = batcher.enqueue(
                sample_private_tx(&format!("tx-{i}")),
                DEFAULT_MIXER_HOLD_MS,
                4,
            );
            if let Some(batch) = maybe {
                batch_sizes.push(batch.len());
            }
        }
        if let Some(last) = batcher.drain_all() {
            batch_sizes.push(last.len());
        }
        assert_eq!(batch_sizes, vec![4, 4, 2]);
    }

    #[test]
    fn test_privacy_transport_mode_resolution() {
        let mut config = NetworkPrivacyConfig::default();
        assert_eq!(
            Node::privacy_transport_mode_for(&config),
            PrivacyTransportMode::Direct
        );

        config.tor_proxy = Some("127.0.0.1:9050".to_string());
        assert_eq!(
            Node::privacy_transport_mode_for(&config),
            PrivacyTransportMode::Tor
        );

        config.tor_proxy = None;
        config.i2p_sam_endpoint = Some("127.0.0.1:7656".to_string());
        assert_eq!(
            Node::privacy_transport_mode_for(&config),
            PrivacyTransportMode::I2p
        );

        config.tor_proxy = Some("127.0.0.1:9050".to_string());
        assert_eq!(
            Node::privacy_transport_mode_for(&config),
            PrivacyTransportMode::Hybrid
        );
    }

    #[test]
    fn test_tor_enforced_helper_sets_expected_flags() {
        let config = NetworkPrivacyConfig::default().tor_enforced("127.0.0.1:9050");
        assert_eq!(config.tor_proxy.as_deref(), Some("127.0.0.1:9050"));
        assert!(config.enforce_anonymous_transport);
        assert!(config.i2p_sam_endpoint.is_none());
    }

    #[tokio::test]
    async fn test_private_transport_enforcement_rejects_unreachable_proxy() {
        let mut node = Node::new(0).await.expect("node");
        node.set_privacy_config(NetworkPrivacyConfig {
            enforce_anonymous_transport: true,
            tor_proxy: Some("127.0.0.1:1".to_string()),
            ..NetworkPrivacyConfig::default()
        });
        let tx = PrivateTransactionMessage {
            tx_hash: "tx-1".to_string(),
            commitment: "commitment".to_string(),
            stealth_address: "stealth".to_string(),
            ephemeral_pubkey: vec![1u8; 32],
            nullifier: "nullifier".to_string(),
            zk_proof: None,
            timestamp: 10,
            recipient_commitment: "recipient".to_string(),
            encrypted_amount: vec![7u8; 8],
            proof_commitment: 1,
            proof_nullifier: 2,
            proof_input_sum: 3,
            proof_output_sum: 3,
            proof_fee: 0,
            proof_key_image: 4,
            proof_private_root_words: vec![1, 2, 3, 4],
            fee: 0,
            ring_id: "ring".to_string(),
            ring_version: 1,
            ring_members: vec!["member-a".to_string(), "member-b".to_string()],
            ring_signature: vec![3u8; 128],
            input_nullifiers: vec!["nullifier".to_string()],
            input_commitments: vec!["commitment".to_string()],
            input_asset_tags: vec!["ZOKA".to_string()],
            outputs: vec![PrivateOutputMessage {
                output_id: "out-1".to_string(),
                asset_tag: "ZOKA".to_string(),
                asset_tag_commitment: "asset-tag-commitment".to_string(),
                recipient_commitment: "recipient".to_string(),
                stealth_address: "stealth".to_string(),
                ephemeral_pubkey: vec![1u8; 32],
                commitment: "commitment".to_string(),
                amount_commitment: "amount-commitment".to_string(),
                range_proof: vec![8u8; 32],
                one_time_public_key: "one-time-key".to_string(),
                encrypted_amount: vec![7u8; 8],
                commitment_blind: vec![4u8; 32],
                is_change: false,
            }],
            input_public_keys: vec!["input-pub-1".to_string()],
            input_key_images: vec!["key-image-1".to_string()],
            input_amount_commitments: vec!["amount-commitment-in".to_string()],
            amount_balance_commitment: "balance-commitment".to_string(),
            fee_asset_tag: "ZOKA".to_string(),
            asset_balance_commitments: vec![
                crate::network::message::AssetBalanceCommitmentMessage {
                    asset_tag: "ZOKA".to_string(),
                    balance_commitment: "balance-commitment".to_string(),
                },
            ],
            fee_commitment: String::new(),
            fee_range_proof: Vec::new(),
            private_commitment_root: String::new(),
            input_inclusion_proofs: vec![],
            commitment_blind: vec![4u8; 32],
            nullifier_nonce: vec![5u8; 16],
        };

        assert!(node
            .publish_message(&ZokaMessage::PrivateTransaction(tx))
            .is_err());
    }

    #[tokio::test]
    async fn test_private_transport_enforcement_rejects_direct_mode() {
        let mut node = Node::new(0).await.expect("node");
        node.set_privacy_config(NetworkPrivacyConfig {
            enforce_anonymous_transport: true,
            ..NetworkPrivacyConfig::default()
        });
        let tx = PrivateTransactionMessage {
            tx_hash: "tx-1".to_string(),
            commitment: "commitment".to_string(),
            stealth_address: "stealth".to_string(),
            ephemeral_pubkey: vec![1u8; 32],
            nullifier: "nullifier".to_string(),
            zk_proof: None,
            timestamp: 10,
            recipient_commitment: "recipient".to_string(),
            encrypted_amount: vec![7u8; 8],
            proof_commitment: 1,
            proof_nullifier: 2,
            proof_input_sum: 3,
            proof_output_sum: 3,
            proof_fee: 0,
            proof_key_image: 4,
            proof_private_root_words: vec![1, 2, 3, 4],
            fee: 0,
            ring_id: "ring".to_string(),
            ring_version: 1,
            ring_members: vec!["member-a".to_string(), "member-b".to_string()],
            ring_signature: vec![3u8; 128],
            input_nullifiers: vec!["nullifier".to_string()],
            input_commitments: vec!["commitment".to_string()],
            input_asset_tags: vec!["ZOKA".to_string()],
            outputs: vec![PrivateOutputMessage {
                output_id: "out-1".to_string(),
                asset_tag: "ZOKA".to_string(),
                asset_tag_commitment: "asset-tag-commitment".to_string(),
                recipient_commitment: "recipient".to_string(),
                stealth_address: "stealth".to_string(),
                ephemeral_pubkey: vec![1u8; 32],
                commitment: "commitment".to_string(),
                amount_commitment: "amount-commitment".to_string(),
                range_proof: vec![8u8; 32],
                one_time_public_key: "one-time-key".to_string(),
                encrypted_amount: vec![7u8; 8],
                commitment_blind: vec![4u8; 32],
                is_change: false,
            }],
            input_public_keys: vec!["input-pub-1".to_string()],
            input_key_images: vec!["key-image-1".to_string()],
            input_amount_commitments: vec!["amount-commitment-in".to_string()],
            amount_balance_commitment: "balance-commitment".to_string(),
            fee_asset_tag: "ZOKA".to_string(),
            asset_balance_commitments: vec![
                crate::network::message::AssetBalanceCommitmentMessage {
                    asset_tag: "ZOKA".to_string(),
                    balance_commitment: "balance-commitment".to_string(),
                },
            ],
            fee_commitment: String::new(),
            fee_range_proof: Vec::new(),
            private_commitment_root: String::new(),
            input_inclusion_proofs: vec![],
            commitment_blind: vec![4u8; 32],
            nullifier_nonce: vec![5u8; 16],
        };

        assert!(node
            .publish_message(&ZokaMessage::PrivateTransaction(tx))
            .is_err());
    }

    #[test]
    fn test_dial_target_from_multiaddr_onion_and_i2p() {
        let onion: Multiaddr =
            "/onion3/vww6ybal4bd7szmgncyruucpgfkqahzddi37ktceo3ah7ngmcopnpyyd:1234"
                .parse()
                .expect("parse onion3");
        let onion_target = Node::dial_target_from_multiaddr(&onion).expect("onion target");
        assert_eq!(onion_target.kind, DialTargetKind::TorOnion);
        assert_eq!(onion_target.port, 1234);
        assert!(onion_target.host.ends_with(".onion"));

        let i2p: Multiaddr = "/dns4/example.b32.i2p/tcp/9001".parse().expect("parse i2p");
        let i2p_target = Node::dial_target_from_multiaddr(&i2p).expect("i2p target");
        assert_eq!(i2p_target.kind, DialTargetKind::I2pB32);
        assert_eq!(i2p_target.host, "example.b32.i2p");
        assert_eq!(i2p_target.port, 9001);
    }

    #[tokio::test]
    async fn test_probe_anonymous_route_uses_tor_proxy() {
        let (proxy_endpoint, proxy_task) = spawn_mock_socks5_proxy().await;

        let mut node = Node::new(0).await.expect("node");
        node.set_privacy_config(NetworkPrivacyConfig {
            tor_proxy: Some(proxy_endpoint),
            enforce_anonymous_transport: true,
            ..NetworkPrivacyConfig::default()
        });

        let onion_addr = "/onion3/vww6ybal4bd7szmgncyruucpgfkqahzddi37ktceo3ah7ngmcopnpyyd:1234";
        let result = node.probe_anonymous_route(onion_addr).await;
        assert!(
            result.is_ok(),
            "socks5 probe should succeed for onion destination"
        );

        let _ = proxy_task.await;
    }

    #[tokio::test]
    async fn test_probe_anonymous_route_rejects_non_anonymous_target() {
        let mut node = Node::new(0).await.expect("node");
        node.set_privacy_config(NetworkPrivacyConfig {
            tor_proxy: Some("127.0.0.1:9050".to_string()),
            enforce_anonymous_transport: true,
            ..NetworkPrivacyConfig::default()
        });

        let direct_addr = "/ip4/127.0.0.1/tcp/30333";
        let result = node.probe_anonymous_route(direct_addr).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_build_stem_private_message_applies_padding_and_hops() {
        let mut node = Node::new(0).await.expect("node");
        let tx = PrivateTransactionMessage {
            tx_hash: "tx-1".to_string(),
            commitment: "commitment".to_string(),
            stealth_address: "stealth".to_string(),
            ephemeral_pubkey: vec![1u8; 32],
            nullifier: "nullifier".to_string(),
            zk_proof: None,
            timestamp: 10,
            recipient_commitment: "recipient".to_string(),
            encrypted_amount: vec![7u8; 8],
            proof_commitment: 1,
            proof_nullifier: 2,
            proof_input_sum: 3,
            proof_output_sum: 3,
            proof_fee: 0,
            proof_key_image: 4,
            proof_private_root_words: vec![1, 2, 3, 4],
            fee: 0,
            ring_id: "ring".to_string(),
            ring_version: 1,
            ring_members: vec!["member-a".to_string(), "member-b".to_string()],
            ring_signature: vec![3u8; 128],
            input_nullifiers: vec!["nullifier".to_string()],
            input_commitments: vec!["commitment".to_string()],
            input_asset_tags: vec!["ZOKA".to_string()],
            outputs: vec![PrivateOutputMessage {
                output_id: "out-1".to_string(),
                asset_tag: "ZOKA".to_string(),
                asset_tag_commitment: "asset-tag-commitment".to_string(),
                recipient_commitment: "recipient".to_string(),
                stealth_address: "stealth".to_string(),
                ephemeral_pubkey: vec![1u8; 32],
                commitment: "commitment".to_string(),
                amount_commitment: "amount-commitment".to_string(),
                range_proof: vec![8u8; 32],
                one_time_public_key: "one-time-key".to_string(),
                encrypted_amount: vec![7u8; 8],
                commitment_blind: vec![4u8; 32],
                is_change: false,
            }],
            input_public_keys: vec!["input-pub-1".to_string()],
            input_key_images: vec!["key-image-1".to_string()],
            input_amount_commitments: vec!["amount-commitment-in".to_string()],
            amount_balance_commitment: "balance-commitment".to_string(),
            fee_asset_tag: "ZOKA".to_string(),
            asset_balance_commitments: vec![
                crate::network::message::AssetBalanceCommitmentMessage {
                    asset_tag: "ZOKA".to_string(),
                    balance_commitment: "balance-commitment".to_string(),
                },
            ],
            fee_commitment: String::new(),
            fee_range_proof: Vec::new(),
            private_commitment_root: String::new(),
            input_inclusion_proofs: vec![],
            commitment_blind: vec![4u8; 32],
            nullifier_nonce: vec![5u8; 16],
        };

        let expected_padding = node.padded_private_tx_size(&tx);
        let stem = node.build_stem_private_message(tx);

        assert_eq!(stem.stem_hops, DEFAULT_STEM_HOPS);
        assert_eq!(stem.padding.len(), expected_padding);
        assert!(!stem.route_id.is_empty());
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
