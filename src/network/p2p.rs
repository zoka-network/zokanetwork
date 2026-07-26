//! P2P node implementation backed by libp2p.
//!
//! This module now includes gossipsub transport for `ZokaMessage`,
//! including a dedicated PBFT topic.

use futures::{
    future::{BoxFuture, FutureExt},
    AsyncRead as FuturesAsyncRead, AsyncWrite as FuturesAsyncWrite, StreamExt,
};
use libp2p::{
    core::{
        multiaddr::Multiaddr,
        muxing::StreamMuxerBox,
        transport::{ListenerId, TransportError, TransportEvent},
        upgrade::Version,
        Transport,
    },
    gossipsub, identify, identity,
    kad::{self, store::MemoryStore},
    noise, request_response,
    swarm::SwarmEvent,
    tcp, yamux, PeerId, Swarm, SwarmBuilder,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tracing::{debug, info, warn};

use crate::network::block_request_response::{
    protocol_name as block_request_response_protocol, BlockBodyRequest, BlockBodyResponse,
    BlockRequestResponseCodec,
};
use crate::network::handler::MessageHandler;
use crate::network::header_request_response::{
    protocol_name as header_request_response_protocol, GetHeaders, HeaderRequestResponseCodec,
    HeadersResponse,
};
use crate::network::message::{
    BlockAnnounceMessage, BlockResponseMessage, MixerBatchMessage, PbftNetworkMessage, PingMessage,
    PrivateTransactionMessage, PrivateTxAnnounceMessage, StemPrivateTransactionMessage,
    SyncStateMessage, UserDiscoveryRecord, ZokaMessage,
};
use crate::network::private_tx_request_response::{
    protocol_name as private_tx_request_response_protocol, PrivateTxBodyRequest,
    PrivateTxBodyResponse, PrivateTxRequestResponseCodec,
};
use crate::network::snapshot_request_response::{
    protocol_name as snapshot_request_response_protocol, GetSnapshotManifests,
    SnapshotManifestsResponse, SnapshotRequestResponseCodec,
};

pub const GENERAL_TOPIC_NAME: &str = "zoka/messages/v1";
pub const PBFT_TOPIC_NAME: &str = "zoka/pbft/v1";
pub const STEM_TOPIC_NAME: &str = "zoka/stem/v1";
pub const MIXER_TOPIC_NAME: &str = "zoka/mixer/v1";
/// Topic carrying ZKA → USDT offers.
///
/// Deliberately separate from the general topic. Nodes on older builds do not
/// know the offer variants and would discard each one with a warning, so
/// publishing offers on `general` would spray a log line across the whole
/// network for every offer. Peers that predate this topic never subscribe, so
/// they see nothing at all.
pub const OFFERS_TOPIC_NAME: &str = "zoka/offers/v1";
const PRIVATE_TX_BODY_CACHE_TTL: Duration = Duration::from_secs(10 * 60);
const PRIVATE_TX_BODY_CACHE_MAX: usize = 2048;
// Bumped for each release that requires a hard minimum-version cutover: any
// peer running an older binary reports the old id and gets disconnected
// during the libp2p identify handshake (enforce_protocol_freeze_for_peer),
// before it can gossip any block. This makes staying on an old client
// genuinely costly (isolated from P2P sync/gossip) without touching
// consensus rules or risking a chain split - old clients simply stop
// getting peers, they aren't told their blocks are invalid.
//
// Recovery cutover: peers that can still produce pre-recovery consensus
// history must not remain in the same gossip mesh as v1.9.0 nodes.
pub const PROTOCOL_FREEZE_ID: &str = "/zoka/1.9.0-mainnet-recovery";
/// Identify protocol advertised only during the operator-controlled rollout
/// grace window. Released nodes still default to [`PROTOCOL_FREEZE_ID`]. A
/// grace seed must advertise the legacy id as well as accept it, because old
/// peers enforce their freeze from their side of the connection.
pub const LEGACY_PROTOCOL_FREEZE_ID: &str = "/zoka/1.6.0-mainnet-zenith";

/// Whether a node disconnects peers running an older protocol id. Defaults to
/// true so stock/published binaries enforce the freeze. During a staged
/// rollout a seed operator sets `ZOKA_PROTOCOL_FREEZE_ENFORCE=0` (or `false`)
/// to run a grace window: the node keeps old-protocol peers usable for sync so
/// the network stays live while operators upgrade, then flips back to enforcing
/// once the fleet has migrated — no rebuild needed, just an env change + restart.
fn protocol_freeze_enforce_from_env() -> bool {
    match std::env::var("ZOKA_PROTOCOL_FREEZE_ENFORCE") {
        Ok(v) => !matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "false" | "no"),
        Err(_) => true,
    }
}

fn advertised_protocol_version(protocol_freeze_enforce: bool) -> &'static str {
    if protocol_freeze_enforce {
        PROTOCOL_FREEZE_ID
    } else {
        LEGACY_PROTOCOL_FREEZE_ID
    }
}

/// Upper bound on a single gossip frame. Block responses can carry private
/// tx bodies whose JSON encoding inflates the raw bytes ~4x, so the libp2p
/// default is easily exceeded by a block plus a couple of ZK-proof bodies.
/// Raised so most block+body responses fit in one gossip frame; oversized
/// ones are still handled by dropping bodies (see
/// `publish_block_response_with_bodies`) so we never emit a frame this large.
const GOSSIP_MAX_TRANSMIT_SIZE: usize = 1024 * 1024;
/// Safe ceiling (under `GOSSIP_MAX_TRANSMIT_SIZE`, leaving headroom for
/// gossipsub framing/signing) above which a block response publishes without
/// its attached private tx bodies. The block still propagates; receivers
/// backfill the bodies over the point-to-point private-tx request-response.
const MAX_GOSSIP_BLOCK_RESPONSE_BYTES: usize = 900 * 1024;
/// Bound messages preserved while another network operation pumps the swarm.
/// This prevents a BlockResponse replay storm from turning loss prevention
/// into unbounded memory growth.
const MAX_INBOUND_QUEUE: usize = 4096;

/// Returns the message unchanged when it fits a gossip frame, or with its
/// private tx bodies stripped (plus the count dropped) when the encoded size
/// would exceed the safe gossip ceiling. Kept pure and separate from the
/// `publish_*` methods so the size decision is unit-testable without a live
/// swarm. Only `BlockResponse` carries bodies, so other messages pass through.
fn fit_block_response_for_gossip(message: ZokaMessage) -> (ZokaMessage, usize) {
    let too_large = message
        .to_bytes()
        .map(|encoded| encoded.len() > MAX_GOSSIP_BLOCK_RESPONSE_BYTES)
        .unwrap_or(false);
    if !too_large {
        return (message, 0);
    }
    if let ZokaMessage::BlockResponse(mut response) = message {
        let dropped = response.private_tx_bodies.len();
        if dropped > 0 {
            response.private_tx_bodies = Vec::new();
        }
        return (ZokaMessage::BlockResponse(response), dropped);
    }
    (message, 0)
}

/// Whether a peer-announced multiaddr is worth dialing from a public node.
/// A peer listening on `0.0.0.0` announces every local interface via Identify,
/// including datacenter-internal (10/8, 172.16/12, 192.168/16), loopback and
/// link-local addresses that are unreachable from anywhere but the peer's own
/// LAN. Dialing those just burns connection attempts and pollutes the peer
/// store (this is why nodes kept trying the seeds' 10.x droplet-internal
/// addresses). Non-IP transports (dns, onion, i2p) always pass. Set
/// `ZOKA_ALLOW_PRIVATE_PEER_ADDRS=1` to keep private addresses for LAN/dev.
pub(crate) fn discovered_addr_is_publicly_dialable(addr: &str) -> bool {
    use libp2p::core::multiaddr::Protocol;
    if std::env::var("ZOKA_ALLOW_PRIVATE_PEER_ADDRS").as_deref() == Ok("1") {
        return true;
    }
    let parsed: Multiaddr = match addr.parse() {
        Ok(m) => m,
        Err(_) => return false,
    };
    for proto in parsed.iter() {
        match proto {
            Protocol::Ip4(ip) => {
                if ip.is_private() || ip.is_loopback() || ip.is_link_local() || ip.is_unspecified()
                {
                    return false;
                }
            }
            Protocol::Ip6(ip) => {
                let seg0 = ip.segments()[0];
                let is_unique_local = (seg0 & 0xfe00) == 0xfc00; // fc00::/7
                let is_link_local = (seg0 & 0xffc0) == 0xfe80; // fe80::/10
                if ip.is_loopback() || ip.is_unspecified() || is_unique_local || is_link_local {
                    return false;
                }
            }
            _ => {}
        }
    }
    true
}

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

#[derive(Clone, Debug, Default)]
pub struct GossipStats {
    pub rx_total: u64,
    pub rx_block_response: u64,
    pub publish_ok: u64,
    pub publish_err: u64,
    pub last_rx_block_at: u64,
    /// Live count of peers in the gossipsub mesh for the general topic
    /// (not just connected-with-gossipsub-support): the true propagation
    /// mesh size, so an empty mesh with everything falling back to
    /// flood-publish is visible instead of looking "connected and healthy".
    pub mesh_peer_count: u64,
}

#[derive(Clone, Debug, Default)]
pub struct ObservedPeerRecord {
    pub peer_id: String,
    pub addr: String,
    pub protocol_version: Option<String>,
    pub protocols: Vec<String>,
}

#[derive(Clone, Debug, Default)]
struct ObservedPeerProtocols {
    protocol_version: Option<String>,
    protocols: Vec<String>,
}

pub struct PendingBlockBodyRequest {
    pub peer: PeerId,
    pub request: BlockBodyRequest,
    pub channel: request_response::ResponseChannel<BlockBodyResponse>,
    pub received_at: Instant,
}

pub struct PendingHeaderRequest {
    pub peer: PeerId,
    pub request: GetHeaders,
    pub channel: request_response::ResponseChannel<HeadersResponse>,
    pub received_at: Instant,
}

#[derive(Debug)]
pub struct ReceivedHeadersResponse {
    pub peer: PeerId,
    pub response: HeadersResponse,
    pub latency: Duration,
}

pub struct PendingSnapshotManifestRequest {
    pub peer: PeerId,
    pub request: GetSnapshotManifests,
    pub channel: request_response::ResponseChannel<SnapshotManifestsResponse>,
    pub received_at: Instant,
}

#[derive(Debug)]
pub struct ReceivedSnapshotManifests {
    pub peer: PeerId,
    pub response: SnapshotManifestsResponse,
    pub latency: Duration,
}

#[derive(Clone, Debug, Default)]
pub struct BlockRequestResponseStats {
    pub outbound_sent: u64,
    pub inbound_received: u64,
    pub inbound_served: u64,
    pub outbound_responses: u64,
    pub outbound_failures: u64,
    pub inbound_failures: u64,
    pub invalid_responses: u64,
    pub latency_samples: u64,
    pub latency_total_ms: u64,
}

#[derive(Clone, Debug, Default)]
pub struct PeerBlockRequestStats {
    pub successes: u64,
    pub failures: u64,
    pub invalid_responses: u64,
    pub latency_samples: u64,
    pub latency_total_ms: u64,
    pub score: i32,
}

#[derive(Clone, Debug)]
struct PendingOutboundBlockBodyRequest {
    peer: PeerId,
    request: BlockBodyRequest,
    started_at: Instant,
}

#[derive(Clone, Debug)]
struct BlockAnnounceRetry {
    attempts: u8,
    last_attempt_at: Instant,
}

#[derive(Clone, Debug)]
struct ReceivedBlockBodyResponse {
    request_id: request_response::OutboundRequestId,
    peer: PeerId,
    response: BlockBodyResponse,
    latency: Duration,
}

/// A validated gossip message together with its signed libp2p author. Local
/// fanout/interception may use `None`. The author, rather than the immediate
/// mesh relay, identifies the node that can serve the block's ancestry.
#[derive(Clone, Debug)]
pub struct SourcedZokaMessage {
    pub source: Option<PeerId>,
    pub message: ZokaMessage,
}

/// A block body and the peer that actually served it over direct or gossip
/// transport. This is intentionally returned to IBD instead of dropping the
/// responder identity at the network boundary.
#[derive(Clone, Debug)]
pub struct SourcedBlockResponse {
    pub source: PeerId,
    pub response: BlockResponseMessage,
}

#[derive(Clone, Debug)]
struct BlockBodyRequestFailure {
    request_id: request_response::OutboundRequestId,
    peer: PeerId,
    error: String,
}

pub struct PendingPrivateTxBodyRequest {
    pub peer: PeerId,
    pub request: PrivateTxBodyRequest,
    pub channel: request_response::ResponseChannel<PrivateTxBodyResponse>,
    pub received_at: Instant,
}

#[derive(Clone, Debug, Default)]
pub struct PrivateTxRequestResponseStats {
    pub outbound_sent: u64,
    pub inbound_received: u64,
    pub inbound_served: u64,
    pub outbound_responses: u64,
    pub outbound_failures: u64,
    pub inbound_failures: u64,
    pub invalid_responses: u64,
    pub rate_limited: u64,
    pub latency_samples: u64,
    pub latency_total_ms: u64,
}

#[derive(Clone, Debug)]
struct PendingOutboundPrivateTxBodyRequest {
    peer: PeerId,
    request: PrivateTxBodyRequest,
    started_at: Instant,
}

#[derive(Clone, Debug)]
struct ReceivedPrivateTxBodyResponse {
    request_id: request_response::OutboundRequestId,
    peer: PeerId,
    response: PrivateTxBodyResponse,
    latency: Duration,
}

#[derive(Clone, Debug)]
struct PrivateTxBodyRequestFailure {
    request_id: request_response::OutboundRequestId,
    peer: PeerId,
    error: String,
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

#[derive(Clone, Debug)]
struct TorSocksTransport {
    proxy_endpoint: Option<String>,
}

impl TorSocksTransport {
    fn new(proxy_endpoint: Option<String>) -> Self {
        Self {
            proxy_endpoint: proxy_endpoint
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
        }
    }
}

struct TorSocksStream(tokio_socks::tcp::Socks5Stream<tokio::net::TcpStream>);

impl fmt::Debug for TorSocksStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TorSocksStream").finish_non_exhaustive()
    }
}

impl FuturesAsyncRead for TorSocksStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize, io::Error>> {
        let mut read_buf = tokio::io::ReadBuf::new(buf);
        futures::ready!(tokio::io::AsyncRead::poll_read(
            Pin::new(&mut self.0),
            cx,
            &mut read_buf
        ))?;
        Poll::Ready(Ok(read_buf.filled().len()))
    }
}

impl FuturesAsyncWrite for TorSocksStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        tokio::io::AsyncWrite::poll_write(Pin::new(&mut self.0), cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        tokio::io::AsyncWrite::poll_flush(Pin::new(&mut self.0), cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        tokio::io::AsyncWrite::poll_shutdown(Pin::new(&mut self.0), cx)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<Result<usize, io::Error>> {
        tokio::io::AsyncWrite::poll_write_vectored(Pin::new(&mut self.0), cx, bufs)
    }
}

impl Transport for TorSocksTransport {
    type Output = TorSocksStream;
    type Error = io::Error;
    type ListenerUpgrade = futures::future::Pending<Result<Self::Output, Self::Error>>;
    type Dial = BoxFuture<'static, Result<Self::Output, Self::Error>>;

    fn listen_on(
        &mut self,
        _id: ListenerId,
        addr: Multiaddr,
    ) -> Result<(), TransportError<Self::Error>> {
        Err(TransportError::MultiaddrNotSupported(addr))
    }

    fn remove_listener(&mut self, _id: ListenerId) -> bool {
        false
    }

    fn dial(&mut self, addr: Multiaddr) -> Result<Self::Dial, TransportError<Self::Error>> {
        let Some(proxy_endpoint) = self.proxy_endpoint.clone() else {
            return Err(TransportError::MultiaddrNotSupported(addr));
        };
        let Some(target) = Node::dial_target_from_multiaddr(&addr) else {
            return Err(TransportError::MultiaddrNotSupported(addr));
        };
        if target.kind != DialTargetKind::TorOnion {
            return Err(TransportError::MultiaddrNotSupported(addr));
        }

        Ok(async move {
            let stream = tokio_socks::tcp::Socks5Stream::connect(
                proxy_endpoint.as_str(),
                (target.host, target.port),
            )
            .await
            .map_err(|e| io::Error::other(format!("SOCKS5 connect failed: {e}")))?;
            Ok(TorSocksStream(stream))
        }
        .boxed())
    }

    fn dial_as_listener(
        &mut self,
        addr: Multiaddr,
    ) -> Result<Self::Dial, TransportError<Self::Error>> {
        self.dial(addr)
    }

    fn poll(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<TransportEvent<Self::ListenerUpgrade, Self::Error>> {
        Poll::Pending
    }

    fn address_translation(&self, _listen: &Multiaddr, _observed: &Multiaddr) -> Option<Multiaddr> {
        None
    }
}

fn build_tor_socks_transport(
    key: &identity::Keypair,
    proxy_endpoint: Option<String>,
) -> Result<libp2p::core::transport::Boxed<(PeerId, StreamMuxerBox)>, Box<dyn Error + Send + Sync>>
{
    let noise = noise::Config::new(key)
        .map_err(|e| io::Error::other(format!("noise config error: {e}")))?;
    // The plain TCP transport gets a default connect/upgrade timeout for
    // free from SwarmBuilder::with_tcp(); this hand-rolled transport
    // (registered via with_other_transport, which does not add that
    // wrapper) previously had none at all. A Tor hidden-service circuit can
    // legitimately take longer than a raw TCP handshake to build, so this
    // is more generous than the TCP path's timeout rather than matching it
    // exactly -- but without any bound the dial future would hang forever
    // on a stalled SOCKS5/Tor circuit, silently blocking that peer slot.
    const TOR_DIAL_TIMEOUT: Duration = Duration::from_secs(45);
    Ok(TorSocksTransport::new(proxy_endpoint)
        .upgrade(Version::V1Lazy)
        .authenticate(noise)
        .multiplex(yamux::Config::default())
        .timeout(TOR_DIAL_TIMEOUT)
        .boxed())
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
    /// P2P exchange offers topic.
    offers_topic: gossipsub::IdentTopic,
    /// Network privacy policy knobs.
    privacy_config: NetworkPrivacyConfig,
    /// Pending inbound messages generated by local fanout/interception.
    inbound_queue: VecDeque<SourcedZokaMessage>,
    /// Dedup keys for high-volume synchronization messages in `inbound_queue`.
    inbound_queue_keys: HashSet<String>,
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
    /// When false, old-protocol peers are kept usable instead of being
    /// disconnected (a grace period during a staged rollout so the network
    /// stays live while operators upgrade). Set from ZOKA_PROTOCOL_FREEZE_ENFORCE
    /// at startup; defaults to true (hard freeze) so stock/published binaries
    /// enforce the freeze unless a seed operator opts into the grace window.
    protocol_freeze_enforce: bool,
    /// Peer listen addresses observed through Identify.
    observed_peer_addrs: HashMap<PeerId, HashSet<String>>,
    /// Peer protocol metadata observed through Identify.
    observed_peer_protocols: HashMap<PeerId, ObservedPeerProtocols>,
    /// Lightweight local counters for production gossip visibility.
    gossip_stats: GossipStats,
    /// Inbound block body requests waiting for local storage lookup.
    pending_block_body_requests: VecDeque<PendingBlockBodyRequest>,
    /// Outbound block body requests waiting for direct responses.
    pending_outbound_block_body_requests:
        HashMap<request_response::OutboundRequestId, PendingOutboundBlockBodyRequest>,
    /// BlockAnnounce-triggered fetches still awaiting a body, keyed by
    /// (height, hash) so a sweep can retry them on a backoff instead of
    /// silently dropping the block if the first gossip request is lost.
    pending_block_announce_retries: HashMap<(u64, String), BlockAnnounceRetry>,
    /// Direct block body responses received while the caller was not waiting.
    received_block_body_responses: VecDeque<ReceivedBlockBodyResponse>,
    /// Direct block body request failures received from libp2p.
    block_body_request_failures: VecDeque<BlockBodyRequestFailure>,
    /// Lightweight stats for direct block request/response.
    block_request_response_stats: BlockRequestResponseStats,
    /// Inbound private tx body requests waiting for local storage lookup.
    pending_private_tx_body_requests: VecDeque<PendingPrivateTxBodyRequest>,
    /// Outbound private tx body requests waiting for direct responses.
    pending_outbound_private_tx_body_requests:
        HashMap<request_response::OutboundRequestId, PendingOutboundPrivateTxBodyRequest>,
    /// Direct private tx body responses received while the caller was not waiting.
    received_private_tx_body_responses: VecDeque<ReceivedPrivateTxBodyResponse>,
    /// Direct private tx body request failures received from libp2p.
    private_tx_body_request_failures: VecDeque<PrivateTxBodyRequestFailure>,
    /// Lightweight stats for private tx body request/response.
    private_tx_request_response_stats: PrivateTxRequestResponseStats,
    /// Peer score used to prefer private tx body providers.
    private_tx_request_peer_scores: HashMap<PeerId, i32>,
    /// Short-lived private tx body cache for relays/miners serving direct fetches.
    recent_private_tx_bodies: HashMap<String, (Vec<u8>, Instant)>,
    /// Peers that recently announced availability for a private tx body.
    private_tx_announce_sources: HashMap<String, HashSet<PeerId>>,
    /// Peer score used to prefer low-latency / valid direct block responders.
    block_request_peer_scores: HashMap<PeerId, i32>,
    /// Sources that delivered a consensus-invalid block during this process.
    /// They may remain connected as relays, but can no longer drive IBD.
    block_sync_quarantined_peers: HashSet<PeerId>,
    /// Per-peer request/response counters used for diagnostics and peer choice.
    block_request_peer_stats: HashMap<PeerId, PeerBlockRequestStats>,
    /// Inbound GetHeaders requests waiting for storage lookup.
    pending_header_requests: VecDeque<PendingHeaderRequest>,
    /// Outbound GetHeaders responses received while the caller was not waiting.
    received_header_responses: VecDeque<ReceivedHeadersResponse>,
    /// Inbound snapshot manifest requests.
    pending_snapshot_requests: VecDeque<PendingSnapshotManifestRequest>,
    /// Outbound snapshot manifest responses received from peers.
    received_snapshot_manifests: VecDeque<ReceivedSnapshotManifests>,
    /// Optional immediate responders for inbound block/header requests. When
    /// set (the fullnode wires these to its open chain storage), the swarm
    /// answers an inbound request the instant it arrives, inside the event
    /// handler, instead of parking it on a queue that the main loop only
    /// drains between (potentially slow) sync steps. That deferral was what
    /// starved IBD: a peer's block request could sit unanswered for the whole
    /// duration of an outbound request the server happened to be waiting on.
    #[allow(clippy::type_complexity)]
    block_body_responder: Option<Box<dyn Fn(&BlockBodyRequest) -> BlockBodyResponse + Send>>,
    #[allow(clippy::type_complexity)]
    header_responder: Option<Box<dyn Fn(&GetHeaders) -> HeadersResponse + Send>>,
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
    /// Direct block body request/response transport.
    block_request_response: request_response::Behaviour<BlockRequestResponseCodec>,
    /// Direct private tx body request/response transport.
    private_tx_request_response: request_response::Behaviour<PrivateTxRequestResponseCodec>,
    /// Header chain request/response for headers-first IBD.
    header_request_response: request_response::Behaviour<HeaderRequestResponseCodec>,
    /// Snapshot manifest discovery.
    snapshot_request_response: request_response::Behaviour<SnapshotRequestResponseCodec>,
    /// Liveness keepalive. Without it, a quiet connection over Tor idles until
    /// the circuit is torn down (tor.log: "assuming established circuits no
    /// longer work"), dropping the only peer link between two NAT'd home nodes.
    /// Periodic pings keep the connection — and its Tor circuit — warm.
    ping: libp2p::ping::Behaviour,
}

impl Node {
    fn node_identity_path() -> Option<PathBuf> {
        if cfg!(test) && std::env::var_os("ZOKA_NODE_KEY_PATH").is_none() {
            return None;
        }

        std::env::var_os("ZOKA_NODE_KEY_PATH")
            .map(PathBuf::from)
            .or_else(|| Some(PathBuf::from(".zoka/node_key")))
    }

    fn load_or_generate_identity_key() -> Result<identity::Keypair, Box<dyn Error>> {
        let Some(path) = Self::node_identity_path() else {
            return Ok(identity::Keypair::generate_ed25519());
        };

        if path.exists() {
            let encoded_key = fs::read(&path)?;
            let key = identity::Keypair::from_protobuf_encoding(&encoded_key).map_err(|e| {
                io::Error::other(format!(
                    "invalid node identity key at {}: {e}",
                    path.display()
                ))
            })?;
            info!("Loaded persistent node identity from {}", path.display());
            return Ok(key);
        }

        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }

        let key = identity::Keypair::generate_ed25519();
        let encoded_key = key
            .to_protobuf_encoding()
            .map_err(|e| io::Error::other(format!("failed to encode node identity key: {e}")))?;
        fs::write(&path, encoded_key)?;
        info!("Generated persistent node identity at {}", path.display());
        Ok(key)
    }

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

    fn kademlia_peer_from_multiaddr(addr: &Multiaddr) -> Option<(PeerId, Multiaddr)> {
        let addr_string = addr.to_string();
        let (dial_addr, peer_id) = addr_string.rsplit_once("/p2p/")?;
        let peer_id = peer_id.parse::<PeerId>().ok()?;
        let dial_addr = dial_addr.parse::<Multiaddr>().ok()?;
        Some((peer_id, dial_addr))
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

    /// Peers currently held in the Kademlia routing table.
    ///
    /// Worth surfacing separately from the connected-peer count: for a long
    /// time this was silently zero on every node (nothing ever populated the
    /// table), and the node still looked perfectly healthy because peers were
    /// arriving from the seed dial, gossipsub and the peerstore instead. A
    /// count that stays at zero while peers are connected means DHT discovery
    /// is dead again.
    pub fn kademlia_routing_peer_count(&mut self) -> u64 {
        self.swarm
            .behaviour_mut()
            .kademlia
            .kbuckets()
            .map(|bucket| bucket.num_entries() as u64)
            .sum()
    }

    pub fn connected_peer_ids(&self) -> Vec<PeerId> {
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

    fn is_private_tx_well_formed_at(
        tx: &PrivateTransactionMessage,
        v2_i1_activation_height: Option<u64>,
        canonical_bundle_activation_height: Option<u64>,
    ) -> bool {
        if tx.tx_hash.is_empty() || tx.commitment.is_empty() || tx.nullifier.is_empty() {
            return false;
        }
        let Ok(profile) = crate::private_wallet::private_proof_profile_spec_for_relay(
            tx.private_proof_version,
            &tx.private_proof_profile,
            v2_i1_activation_height,
            canonical_bundle_activation_height,
        ) else {
            return false;
        };
        if tx.ring_version != 1 {
            return false;
        }
        if tx.ring_members.len() < 3 {
            return false;
        }
        if tx.input_key_images.len() > profile.max_inputs {
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

    fn is_private_tx_well_formed(tx: &PrivateTransactionMessage) -> bool {
        Self::is_private_tx_well_formed_at(
            tx,
            crate::economics::private_tx_v2_i1_activation_height(),
            crate::economics::private_tx_canonical_bundle_activation_height(),
        )
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

    fn is_private_tx_announce_well_formed(announce: &PrivateTxAnnounceMessage) -> bool {
        let hash = announce.tx_hash.trim();
        if hash.is_empty() || hash.len() > 128 {
            return false;
        }
        if hash
            .bytes()
            .any(|byte| !(byte.is_ascii_hexdigit() || matches!(byte, b'-' | b'_')))
        {
            return false;
        }
        if announce
            .body_size_bytes
            .is_some_and(|size| size == 0 || size > 2 * 1024 * 1024)
        {
            return false;
        }
        !announce.private_proof_profile.trim().is_empty()
    }

    fn validate_inbound_message(&self, message: &ZokaMessage) -> Result<(), &'static str> {
        match message {
            ZokaMessage::PrivateTransaction(tx) => {
                if !Self::is_private_tx_well_formed(tx) {
                    return Err("private tx failed protocol freeze validation");
                }
            }
            ZokaMessage::PrivateTxAnnounce(announce) => {
                if !Self::is_private_tx_announce_well_formed(announce) {
                    return Err("private tx announce failed validation");
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
            ZokaMessage::BlockAnnounce(msg) => {
                if msg.block_height == 0 || msg.block_hash.trim().is_empty() {
                    return Err("block announce missing height/hash");
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
            ZokaMessage::OfferAnnounce(announce) => {
                // Runs before the offer is relayed, so a malformed or
                // self-inconsistent one never propagates past this node.
                announce
                    .offer
                    .validate()
                    .map_err(|rejection| rejection.as_str())?;
                if announce.offer.status != crate::p2p_exchange::OfferStatus::Open {
                    // Announcements introduce offers. A claim or a
                    // cancellation is a status update against an offer the
                    // peer already has, and accepting one here would let a
                    // peer inject an already-claimed offer nobody can take.
                    return Err("offer announce must carry an open offer");
                }
            }
            ZokaMessage::OfferStatusUpdate(update) => {
                if update.offer_id.trim().is_empty() || update.seller_address.trim().is_empty() {
                    return Err("offer status update missing offer id or seller");
                }
                match update.status {
                    crate::p2p_exchange::OfferStatus::Claimed
                    | crate::p2p_exchange::OfferStatus::PaymentDetected => {
                        if update.claimed_by.is_none() || update.claim_expires_at.is_none() {
                            return Err("claimed offer update needs a buyer and a deadline");
                        }
                    }
                    crate::p2p_exchange::OfferStatus::Open => {
                        // Offers only ever move away from Open. Allowing a
                        // reopen would let a peer resurrect an offer whose
                        // ZKA has already been spent.
                        return Err("offer cannot be reopened by a status update");
                    }
                    _ => {}
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
        if !self.protocol_freeze_enforce && !Self::is_protocol_version_compatible(version) {
            // Grace period: keep the old-protocol peer usable for sync/gossip
            // so the network stays live while operators upgrade. Clear any
            // stale incompatible marking (e.g. if enforcement was just turned
            // off) and do not disconnect.
            self.incompatible_protocol_peers.remove(&peer_id);
            warn!(
                "peer {} uses old protocol version {} (freeze not enforced: grace period, peer kept)",
                peer_id, version
            );
            return;
        }
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
        match self
            .swarm
            .behaviour_mut()
            .gossipsub
            .publish(self.mixer_topic.clone(), payload)
        {
            Ok(_) => {
                self.record_gossip_publish(true);
                Ok(())
            }
            Err(e) => {
                self.record_gossip_publish(false);
                Err(Box::new(e))
            }
        }
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
        match self
            .swarm
            .behaviour_mut()
            .gossipsub
            .publish(self.stem_topic.clone(), payload)
        {
            Ok(_) => {
                self.record_gossip_publish(true);
                Ok(())
            }
            Err(e) => {
                self.record_gossip_publish(false);
                Err(Box::new(e))
            }
        }
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
            self.enqueue_inbound_message(SourcedZokaMessage {
                source: Some(propagation_source),
                message: ZokaMessage::PrivateTransaction(tx),
            });
        }
        Ok(())
    }

    fn sourced_inbound_message(
        &mut self,
        propagation_source: PeerId,
        message_source: PeerId,
        message: ZokaMessage,
    ) -> SourcedZokaMessage {
        // Private-body point-to-point requests must go to a directly connected
        // relay. Chain synchronization, however, needs the signed gossipsub
        // author so a hash-pinned ancestry request can reach the publisher.
        self.record_private_tx_announce_source(propagation_source, &message);
        self.record_gossip_rx(&message);
        SourcedZokaMessage {
            source: Some(message_source),
            message,
        }
    }

    fn preserve_pumped_message(
        &mut self,
        propagation_source: PeerId,
        message_source: PeerId,
        message: ZokaMessage,
    ) -> bool {
        let sourced = self.sourced_inbound_message(propagation_source, message_source, message);
        self.enqueue_inbound_message(sourced)
    }

    fn inbound_queue_key(message: &SourcedZokaMessage) -> Option<String> {
        let source = message
            .source
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| "local".to_string());
        match &message.message {
            ZokaMessage::BlockResponse(response) => Some(format!(
                "block-response:{source}:{}:{}",
                response.block_height, response.block_hash,
            )),
            ZokaMessage::BlockAnnounce(announce) => Some(format!(
                "block-announce:{source}:{}:{}",
                announce.block_height, announce.block_hash,
            )),
            ZokaMessage::SyncState(sync) => Some(format!(
                "sync-state:{source}:{}:{}",
                sync.last_height, sync.state_root,
            )),
            _ => None,
        }
    }

    fn is_block_sync_message(message: &ZokaMessage) -> bool {
        matches!(
            message,
            ZokaMessage::BlockResponse(_)
                | ZokaMessage::BlockAnnounce(_)
                | ZokaMessage::SyncState(_)
        )
    }

    fn is_quarantined_block_sync_message(&self, message: &SourcedZokaMessage) -> bool {
        Self::is_block_sync_message(&message.message)
            && message
                .source
                .as_ref()
                .is_some_and(|source| self.is_block_sync_peer_quarantined(source))
    }

    fn forget_inbound_queue_key(&mut self, message: &SourcedZokaMessage) {
        if let Some(key) = Self::inbound_queue_key(message) {
            self.inbound_queue_keys.remove(&key);
        }
    }

    fn pop_inbound_message(&mut self) -> Option<SourcedZokaMessage> {
        while let Some(message) = self.inbound_queue.pop_front() {
            self.forget_inbound_queue_key(&message);
            if !self.is_quarantined_block_sync_message(&message) {
                return Some(message);
            }
        }
        None
    }

    fn enqueue_inbound_message(&mut self, message: SourcedZokaMessage) -> bool {
        if self.is_quarantined_block_sync_message(&message) {
            return false;
        }
        let key = Self::inbound_queue_key(&message);
        if key
            .as_ref()
            .is_some_and(|key| self.inbound_queue_keys.contains(key))
        {
            return false;
        }
        while self.inbound_queue.len() >= MAX_INBOUND_QUEUE {
            if let Some(evicted) = self.inbound_queue.pop_front() {
                self.forget_inbound_queue_key(&evicted);
            } else {
                break;
            }
        }
        if let Some(key) = key {
            self.inbound_queue_keys.insert(key);
        }
        self.inbound_queue.push_back(message);
        true
    }

    fn enqueue_inbound_message_front(&mut self, message: SourcedZokaMessage) -> bool {
        if self.is_quarantined_block_sync_message(&message) {
            return false;
        }
        let key = Self::inbound_queue_key(&message);
        if key
            .as_ref()
            .is_some_and(|key| self.inbound_queue_keys.contains(key))
        {
            return false;
        }
        while self.inbound_queue.len() >= MAX_INBOUND_QUEUE {
            if let Some(evicted) = self.inbound_queue.pop_back() {
                self.forget_inbound_queue_key(&evicted);
            } else {
                break;
            }
        }
        if let Some(key) = key {
            self.inbound_queue_keys.insert(key);
        }
        self.inbound_queue.push_front(message);
        true
    }

    /// Create a new node with gossipsub + identify + kademlia.
    pub async fn new(listen_port: u16) -> Result<Self, Box<dyn Error>> {
        Self::new_with_tor_proxy(listen_port, None).await
    }

    /// Create a new node and include a Tor SOCKS transport for onion dials when configured.
    pub async fn new_with_tor_proxy(
        _listen_port: u16,
        tor_proxy: Option<String>,
    ) -> Result<Self, Box<dyn Error>> {
        let local_key = Self::load_or_generate_identity_key()?;
        let peer_id = PeerId::from(local_key.public());
        info!("Local PeerId: {}", peer_id);
        let protocol_freeze_enforce = protocol_freeze_enforce_from_env();
        let identify_protocol_version =
            advertised_protocol_version(protocol_freeze_enforce).to_string();
        if !protocol_freeze_enforce {
            warn!(
                "protocol freeze grace active: advertising legacy identify id {} while validating 1.9 recovery rules",
                identify_protocol_version
            );
        }

        let general_topic = gossipsub::IdentTopic::new(GENERAL_TOPIC_NAME);
        let pbft_topic = gossipsub::IdentTopic::new(PBFT_TOPIC_NAME);
        let stem_topic = gossipsub::IdentTopic::new(STEM_TOPIC_NAME);
        let mixer_topic = gossipsub::IdentTopic::new(MIXER_TOPIC_NAME);
        let offers_topic = gossipsub::IdentTopic::new(OFFERS_TOPIC_NAME);
        let general_topic_for_behaviour = general_topic.clone();
        let pbft_topic_for_behaviour = pbft_topic.clone();
        let stem_topic_for_behaviour = stem_topic.clone();
        let mixer_topic_for_behaviour = mixer_topic.clone();
        let offers_topic_for_behaviour = offers_topic.clone();
        let tor_proxy_for_transport = tor_proxy.clone();

        let swarm = SwarmBuilder::with_existing_identity(local_key)
            .with_tokio()
            .with_tcp(
                tcp::Config::default(),
                noise::Config::new,
                yamux::Config::default,
            )?
            .with_other_transport(move |key| {
                build_tor_socks_transport(key, tor_proxy_for_transport.clone())
            })?
            // Wrap the TCP transport in a DNS resolver so `/dns4/<host>/tcp/<port>`
            // seed multiaddrs dial correctly. Without this the swarm rejects DNS
            // multiaddrs outright, which would pin the network manifest to raw
            // IPs forever — seeds must be re-pointable via DNS records, not new
            // binary releases (the 2026-07 DigitalOcean shutdown proved why).
            .with_dns()?
            .with_behaviour(move |key| {
                let local_peer_id = PeerId::from(key.public());

                let kademlia = kad::Behaviour::with_config(
                    local_peer_id,
                    MemoryStore::new(local_peer_id),
                    kad::Config::default(),
                );

                // Mesh size tuned for the current network scale (roughly 2-20
                // real peers), not libp2p's defaults (mesh_n=6/low=5/high=12),
                // which are sized for 50+ peer networks. At this scale the
                // defaults leave every node permanently below mesh_n_low,
                // causing continuous pointless GRAFT churn every heartbeat.
                // mesh_outbound_min=1 (down from 2) because a 2-seed-only
                // topology can't always satisfy 2 outbound mesh slots.
                let gossipsub_config = gossipsub::ConfigBuilder::default()
                    .heartbeat_interval(Duration::from_secs(1))
                    .validation_mode(gossipsub::ValidationMode::Strict)
                    .flood_publish(true)
                    .mesh_n(4)
                    .mesh_n_low(2)
                    .mesh_n_high(8)
                    .mesh_outbound_min(1)
                    .max_transmit_size(GOSSIP_MAX_TRANSMIT_SIZE)
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
                gossipsub.subscribe(&offers_topic_for_behaviour)?;
                // Request/response timeouts are sized for onion transport:
                // a Tor circuit adds several seconds of round-trip latency and
                // each request opens a fresh substream (multistream-select +
                // the round trip). The old 8-15s budgets were fine for
                // clearnet but too tight once the network's only reliable path
                // is Tor, so IBD block fetches would time out and retry
                // forever. 30s gives comfortable headroom over Tor while still
                // failing a genuinely dead peer in bounded time.
                let block_request_response = request_response::Behaviour::with_codec(
                    BlockRequestResponseCodec,
                    [(
                        block_request_response_protocol(),
                        request_response::ProtocolSupport::Full,
                    )],
                    request_response::Config::default()
                        .with_request_timeout(Duration::from_secs(30)),
                );
                let private_tx_request_response = request_response::Behaviour::with_codec(
                    PrivateTxRequestResponseCodec,
                    [(
                        private_tx_request_response_protocol(),
                        request_response::ProtocolSupport::Full,
                    )],
                    request_response::Config::default()
                        .with_request_timeout(Duration::from_secs(20)),
                );
                let header_request_response = request_response::Behaviour::with_codec(
                    HeaderRequestResponseCodec,
                    [(
                        header_request_response_protocol(),
                        request_response::ProtocolSupport::Full,
                    )],
                    request_response::Config::default()
                        .with_request_timeout(Duration::from_secs(30)),
                );
                let snapshot_request_response = request_response::Behaviour::with_codec(
                    SnapshotRequestResponseCodec,
                    [(
                        snapshot_request_response_protocol(),
                        request_response::ProtocolSupport::Full,
                    )],
                    request_response::Config::default()
                        .with_request_timeout(Duration::from_secs(30)),
                );

                Ok(NodeBehaviour {
                    identify: identify::Behaviour::new(identify::Config::new(
                        identify_protocol_version.clone(),
                        key.public(),
                    )),
                    kademlia,
                    gossipsub,
                    block_request_response,
                    private_tx_request_response,
                    header_request_response,
                    snapshot_request_response,
                    // Default: ping every 15s, 20s timeout. Well under the
                    // libp2p idle timeout (300s) and Tor's circuit idle window,
                    // so the peer link stays up between sparse block gossip.
                    ping: libp2p::ping::Behaviour::new(libp2p::ping::Config::new()),
                })
            })?
            .with_swarm_config(|cfg| {
                // libp2p 0.53+ defaults idle_connection_timeout to 0, closing a
                // connection the instant it has no active stream. Block sync
                // opens a fresh request/response substream per block with gaps
                // in between; over Tor -- where re-dialing an onion peer costs
                // many seconds of circuit build -- that made the connection
                // drop after a block or two and IBD stall (observed: a fresh
                // node stuck at height ~2 while the peer stayed "connected").
                // Keep idle connections alive for several minutes so one peer
                // stays usable across the whole sync.
                cfg.with_idle_connection_timeout(Duration::from_secs(300))
            })
            .build();

        let privacy_config = tor_proxy
            .filter(|proxy| !proxy.trim().is_empty())
            .map(|proxy| NetworkPrivacyConfig::default().with_tor_proxy(proxy))
            .unwrap_or_default();

        Ok(Self {
            peer_id,
            swarm,
            general_topic,
            pbft_topic,
            stem_topic,
            mixer_topic,
            offers_topic,
            privacy_config,
            inbound_queue: VecDeque::new(),
            inbound_queue_keys: HashSet::new(),
            mixer_batcher: MixerBatcher::new(),
            seen_mixer_batches: HashSet::new(),
            mixer_peer_buckets: HashMap::new(),
            seen_stem_routes: HashSet::new(),
            stem_relay_cursor: 0,
            incompatible_protocol_peers: HashSet::new(),
            protocol_freeze_enforce,
            observed_peer_addrs: HashMap::new(),
            observed_peer_protocols: HashMap::new(),
            gossip_stats: GossipStats::default(),
            pending_block_body_requests: VecDeque::new(),
            pending_outbound_block_body_requests: HashMap::new(),
            pending_block_announce_retries: HashMap::new(),
            received_block_body_responses: VecDeque::new(),
            block_body_request_failures: VecDeque::new(),
            block_request_response_stats: BlockRequestResponseStats::default(),
            pending_private_tx_body_requests: VecDeque::new(),
            pending_outbound_private_tx_body_requests: HashMap::new(),
            received_private_tx_body_responses: VecDeque::new(),
            private_tx_body_request_failures: VecDeque::new(),
            private_tx_request_response_stats: PrivateTxRequestResponseStats::default(),
            private_tx_request_peer_scores: HashMap::new(),
            recent_private_tx_bodies: HashMap::new(),
            private_tx_announce_sources: HashMap::new(),
            block_request_peer_scores: HashMap::new(),
            block_sync_quarantined_peers: HashSet::new(),
            block_request_peer_stats: HashMap::new(),
            pending_header_requests: VecDeque::new(),
            received_header_responses: VecDeque::new(),
            pending_snapshot_requests: VecDeque::new(),
            received_snapshot_manifests: VecDeque::new(),
            block_body_responder: None,
            header_responder: None,
        })
    }

    fn record_identify_peer(&mut self, peer_id: PeerId, info: &identify::Info) {
        let mut protocols = info
            .protocols
            .iter()
            .map(|protocol| protocol.to_string())
            .filter(|protocol| !protocol.trim().is_empty())
            .collect::<Vec<_>>();
        protocols.sort();
        protocols.dedup();
        self.observed_peer_protocols.insert(
            peer_id,
            ObservedPeerProtocols {
                protocol_version: Some(info.protocol_version.clone()),
                protocols,
            },
        );

        if info.listen_addrs.is_empty() {
            return;
        }

        // Does this peer actually speak Kademlia? Adding a peer that doesn't
        // pollutes the routing table with entries that can never answer a
        // query. Matched on substring rather than an exact constant so a
        // libp2p version bump to the protocol string doesn't silently turn
        // discovery back off.
        let speaks_kademlia = info
            .protocols
            .iter()
            .any(|protocol| protocol.as_ref().contains("/kad/"));

        let mut dialable = Vec::new();
        let addrs = self.observed_peer_addrs.entry(peer_id).or_default();
        for addr in &info.listen_addrs {
            let addr_str = addr.to_string();
            // Drop unreachable private/loopback/link-local addresses a peer
            // announces (e.g. a seed's datacenter-internal 10.x interface), so
            // they never enter the dial/reconnect pool.
            if discovered_addr_is_publicly_dialable(&addr_str) {
                addrs.insert(addr_str);
                dialable.push(addr.clone());
            }
        }

        // Feed the Kademlia routing table from identify.
        //
        // This used to be the *only* thing missing for DHT discovery to work
        // at all: the sole other `add_address` caller requires the multiaddr
        // to carry an explicit `/p2p/<peer-id>`, and the network manifest's
        // seed addresses don't, so in practice no node ever populated its
        // routing table and peer discovery fell back entirely to the seed
        // dial, gossipsub and the persisted peerstore. Here the peer id is
        // authenticated by the connection itself, so no suffix is needed.
        if speaks_kademlia {
            for addr in dialable {
                self.swarm
                    .behaviour_mut()
                    .kademlia
                    .add_address(&peer_id, addr);
            }
        }
    }

    fn record_gossip_rx(&mut self, message: &ZokaMessage) {
        self.gossip_stats.rx_total = self.gossip_stats.rx_total.saturating_add(1);
        if matches!(message, ZokaMessage::BlockResponse(_)) {
            self.gossip_stats.rx_block_response =
                self.gossip_stats.rx_block_response.saturating_add(1);
            self.gossip_stats.last_rx_block_at = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
        }
    }

    fn record_gossip_publish(&mut self, ok: bool) {
        if ok {
            self.gossip_stats.publish_ok = self.gossip_stats.publish_ok.saturating_add(1);
        } else {
            self.gossip_stats.publish_err = self.gossip_stats.publish_err.saturating_add(1);
        }
    }

    fn record_private_tx_announce_source(&mut self, peer: PeerId, message: &ZokaMessage) {
        if let ZokaMessage::PrivateTxAnnounce(announce) = message {
            let hash = announce.tx_hash.trim();
            if !hash.is_empty() {
                self.private_tx_announce_sources
                    .entry(hash.to_string())
                    .or_default()
                    .insert(peer);
            }
        }
    }

    fn record_direct_block_success(&mut self, peer: PeerId, latency: Duration) {
        let score = self
            .block_request_peer_scores
            .entry(peer.clone())
            .or_default();
        *score = score.saturating_add(10).min(100);
        let latency_ms = latency.as_millis().min(u128::from(u64::MAX)) as u64;
        let peer_stats = self.block_request_peer_stats.entry(peer).or_default();
        peer_stats.successes = peer_stats.successes.saturating_add(1);
        peer_stats.latency_samples = peer_stats.latency_samples.saturating_add(1);
        peer_stats.latency_total_ms = peer_stats.latency_total_ms.saturating_add(latency_ms);
        peer_stats.score = *score;
        self.block_request_response_stats.latency_samples = self
            .block_request_response_stats
            .latency_samples
            .saturating_add(1);
        self.block_request_response_stats.latency_total_ms = self
            .block_request_response_stats
            .latency_total_ms
            .saturating_add(latency_ms);
    }

    fn record_direct_block_failure(&mut self, peer: PeerId) {
        let score = self
            .block_request_peer_scores
            .entry(peer.clone())
            .or_default();
        *score = score.saturating_sub(15).max(-100);
        let peer_stats = self.block_request_peer_stats.entry(peer).or_default();
        peer_stats.failures = peer_stats.failures.saturating_add(1);
        peer_stats.score = *score;
    }

    fn record_direct_block_invalid_response(&mut self, peer: PeerId) {
        self.block_request_response_stats.invalid_responses = self
            .block_request_response_stats
            .invalid_responses
            .saturating_add(1);
        self.record_direct_block_failure(peer.clone());
        let peer_stats = self.block_request_peer_stats.entry(peer).or_default();
        peer_stats.invalid_responses = peer_stats.invalid_responses.saturating_add(1);
    }

    pub fn penalize_block_sync_peer(&mut self, peer: PeerId) {
        self.block_sync_quarantined_peers.insert(peer);
        self.record_direct_block_invalid_response(peer);
    }

    pub fn is_block_sync_peer_quarantined(&self, peer: &PeerId) -> bool {
        self.block_sync_quarantined_peers.contains(peer)
    }

    fn record_private_tx_request_success(&mut self, peer: PeerId, latency: Duration) {
        let score = self.private_tx_request_peer_scores.entry(peer).or_default();
        *score = score.saturating_add(10).min(100);
        self.private_tx_request_response_stats.latency_samples = self
            .private_tx_request_response_stats
            .latency_samples
            .saturating_add(1);
        self.private_tx_request_response_stats.latency_total_ms = self
            .private_tx_request_response_stats
            .latency_total_ms
            .saturating_add(latency.as_millis().min(u128::from(u64::MAX)) as u64);
    }

    fn record_private_tx_request_failure(&mut self, peer: PeerId) {
        let score = self.private_tx_request_peer_scores.entry(peer).or_default();
        *score = score.saturating_sub(15).max(-100);
    }

    fn record_private_tx_invalid_response(&mut self, peer: PeerId) {
        self.private_tx_request_response_stats.invalid_responses = self
            .private_tx_request_response_stats
            .invalid_responses
            .saturating_add(1);
        self.record_private_tx_request_failure(peer);
    }

    fn handle_block_request_response_event(
        &mut self,
        event: request_response::Event<BlockBodyRequest, BlockBodyResponse>,
    ) {
        match event {
            request_response::Event::Message { peer, message } => match message {
                request_response::Message::Request {
                    request, channel, ..
                } => {
                    self.block_request_response_stats.inbound_received = self
                        .block_request_response_stats
                        .inbound_received
                        .saturating_add(1);
                    // Answer immediately when a responder is wired (fullnode
                    // serving from open storage). Compute the response first so
                    // the immutable borrow of `self` for the responder ends
                    // before send_response takes a mutable borrow.
                    let immediate = self
                        .block_body_responder
                        .as_ref()
                        .map(|responder| responder(&request));
                    match immediate {
                        Some(response) => {
                            let received_at = Instant::now();
                            match self
                                .swarm
                                .behaviour_mut()
                                .block_request_response
                                .send_response(channel, response)
                            {
                                Ok(()) => {
                                    self.record_direct_block_success(peer, received_at.elapsed());
                                }
                                Err(_) => self.record_direct_block_failure(peer),
                            }
                        }
                        None => {
                            self.pending_block_body_requests
                                .push_back(PendingBlockBodyRequest {
                                    peer,
                                    request,
                                    channel,
                                    received_at: Instant::now(),
                                });
                        }
                    }
                }
                request_response::Message::Response {
                    request_id,
                    response,
                } => {
                    let pending = self
                        .pending_outbound_block_body_requests
                        .remove(&request_id);
                    let latency = pending
                        .as_ref()
                        .map(|pending| pending.started_at.elapsed())
                        .unwrap_or_default();
                    let response_peer = pending
                        .as_ref()
                        .map(|pending| pending.peer.clone())
                        .unwrap_or_else(|| peer.clone());
                    let requested_height =
                        pending.as_ref().map(|pending| pending.request.block_height);
                    if response_peer != peer {
                        warn!(
                            "direct block response peer mismatch: request_id={request_id:?} expected={} got={}",
                            response_peer, peer
                        );
                    }
                    if let Some(height) = requested_height {
                        debug!(
                            "direct block response received: request_id={request_id:?} peer={} height={} latency_ms={}",
                            response_peer,
                            height,
                            latency.as_millis()
                        );
                    }
                    self.block_request_response_stats.outbound_responses = self
                        .block_request_response_stats
                        .outbound_responses
                        .saturating_add(1);
                    self.record_direct_block_success(response_peer.clone(), latency);
                    self.received_block_body_responses
                        .push_back(ReceivedBlockBodyResponse {
                            request_id,
                            peer: response_peer,
                            response,
                            latency,
                        });
                }
            },
            request_response::Event::OutboundFailure {
                peer,
                request_id,
                error,
            } => {
                self.pending_outbound_block_body_requests
                    .remove(&request_id);
                self.block_request_response_stats.outbound_failures = self
                    .block_request_response_stats
                    .outbound_failures
                    .saturating_add(1);
                self.record_direct_block_failure(peer);
                self.block_body_request_failures
                    .push_back(BlockBodyRequestFailure {
                        request_id,
                        peer,
                        error: error.to_string(),
                    });
            }
            request_response::Event::InboundFailure { peer, error, .. } => {
                self.block_request_response_stats.inbound_failures = self
                    .block_request_response_stats
                    .inbound_failures
                    .saturating_add(1);
                self.record_direct_block_failure(peer);
                warn!(
                    "direct block request inbound failure from {}: {}",
                    peer, error
                );
            }
            request_response::Event::ResponseSent { peer, .. } => {
                self.block_request_response_stats.inbound_served = self
                    .block_request_response_stats
                    .inbound_served
                    .saturating_add(1);
                let score = self.block_request_peer_scores.entry(peer).or_default();
                *score = score.saturating_add(2).min(100);
            }
        }
    }

    fn handle_private_tx_request_response_event(
        &mut self,
        event: request_response::Event<PrivateTxBodyRequest, PrivateTxBodyResponse>,
    ) {
        match event {
            request_response::Event::Message { peer, message } => match message {
                request_response::Message::Request {
                    request, channel, ..
                } => {
                    self.private_tx_request_response_stats.inbound_received = self
                        .private_tx_request_response_stats
                        .inbound_received
                        .saturating_add(1);
                    self.pending_private_tx_body_requests
                        .push_back(PendingPrivateTxBodyRequest {
                            peer,
                            request,
                            channel,
                            received_at: Instant::now(),
                        });
                }
                request_response::Message::Response {
                    request_id,
                    response,
                } => {
                    let pending = self
                        .pending_outbound_private_tx_body_requests
                        .remove(&request_id);
                    let latency = pending
                        .as_ref()
                        .map(|pending| pending.started_at.elapsed())
                        .unwrap_or_default();
                    let response_peer = pending
                        .as_ref()
                        .map(|pending| pending.peer.clone())
                        .unwrap_or_else(|| peer.clone());
                    if let Some(request) = pending.as_ref() {
                        debug!(
                            "direct private tx response received: request_id={request_id:?} peer={} hash={} latency_ms={}",
                            response_peer,
                            request.request.tx_hash,
                            latency.as_millis()
                        );
                    }
                    self.private_tx_request_response_stats.outbound_responses = self
                        .private_tx_request_response_stats
                        .outbound_responses
                        .saturating_add(1);
                    self.record_private_tx_request_success(response_peer.clone(), latency);
                    self.received_private_tx_body_responses.push_back(
                        ReceivedPrivateTxBodyResponse {
                            request_id,
                            peer: response_peer,
                            response,
                            latency,
                        },
                    );
                }
            },
            request_response::Event::OutboundFailure {
                peer,
                request_id,
                error,
            } => {
                self.pending_outbound_private_tx_body_requests
                    .remove(&request_id);
                self.private_tx_request_response_stats.outbound_failures = self
                    .private_tx_request_response_stats
                    .outbound_failures
                    .saturating_add(1);
                self.record_private_tx_request_failure(peer);
                self.private_tx_body_request_failures
                    .push_back(PrivateTxBodyRequestFailure {
                        request_id,
                        peer,
                        error: error.to_string(),
                    });
            }
            request_response::Event::InboundFailure { peer, error, .. } => {
                self.private_tx_request_response_stats.inbound_failures = self
                    .private_tx_request_response_stats
                    .inbound_failures
                    .saturating_add(1);
                self.record_private_tx_request_failure(peer);
                warn!(
                    "direct private tx request inbound failure from {}: {}",
                    peer, error
                );
            }
            request_response::Event::ResponseSent { peer, .. } => {
                self.private_tx_request_response_stats.inbound_served = self
                    .private_tx_request_response_stats
                    .inbound_served
                    .saturating_add(1);
                let score = self.private_tx_request_peer_scores.entry(peer).or_default();
                *score = score.saturating_add(2).min(100);
            }
        }
    }

    fn handle_header_request_response_event(
        &mut self,
        event: request_response::Event<GetHeaders, HeadersResponse>,
    ) {
        match event {
            request_response::Event::Message { peer, message } => match message {
                request_response::Message::Request {
                    request, channel, ..
                } => {
                    let immediate = self
                        .header_responder
                        .as_ref()
                        .map(|responder| responder(&request));
                    match immediate {
                        Some(response) => {
                            let _ = self
                                .swarm
                                .behaviour_mut()
                                .header_request_response
                                .send_response(channel, response);
                        }
                        None => {
                            self.pending_header_requests
                                .push_back(PendingHeaderRequest {
                                    peer,
                                    request,
                                    channel,
                                    received_at: Instant::now(),
                                });
                        }
                    }
                }
                request_response::Message::Response { response, .. } => {
                    self.received_header_responses
                        .push_back(ReceivedHeadersResponse {
                            peer,
                            response,
                            latency: Duration::default(),
                        });
                }
            },
            request_response::Event::OutboundFailure { peer, error, .. } => {
                warn!("header request outbound failure from {}: {}", peer, error);
            }
            request_response::Event::InboundFailure { peer, error, .. } => {
                warn!("header request inbound failure from {}: {}", peer, error);
            }
            request_response::Event::ResponseSent { .. } => {}
        }
    }

    pub fn take_pending_header_request(&mut self) -> Option<PendingHeaderRequest> {
        self.pending_header_requests.pop_front()
    }

    pub fn take_received_headers_response(&mut self) -> Option<ReceivedHeadersResponse> {
        self.received_header_responses.pop_front()
    }

    pub fn respond_header_request(
        &mut self,
        pending: PendingHeaderRequest,
        response: HeadersResponse,
    ) -> Result<(), HeadersResponse> {
        self.swarm
            .behaviour_mut()
            .header_request_response
            .send_response(pending.channel, response)
            .map_err(|e| e)
    }

    pub fn send_header_request(
        &mut self,
        peer: &PeerId,
        request: GetHeaders,
    ) -> Option<request_response::OutboundRequestId> {
        if !self.swarm.is_connected(peer) {
            return None;
        }
        let id = self
            .swarm
            .behaviour_mut()
            .header_request_response
            .send_request(peer, request);
        Some(id)
    }

    fn handle_snapshot_request_response_event(
        &mut self,
        event: request_response::Event<GetSnapshotManifests, SnapshotManifestsResponse>,
    ) {
        match event {
            request_response::Event::Message { peer, message } => match message {
                request_response::Message::Request {
                    request, channel, ..
                } => {
                    self.pending_snapshot_requests
                        .push_back(PendingSnapshotManifestRequest {
                            peer,
                            request,
                            channel,
                            received_at: Instant::now(),
                        });
                }
                request_response::Message::Response { response, .. } => {
                    self.received_snapshot_manifests
                        .push_back(ReceivedSnapshotManifests {
                            peer,
                            response,
                            latency: Duration::default(),
                        });
                }
            },
            request_response::Event::OutboundFailure { peer, error, .. } => {
                debug!("snapshot manifest request failed from {}: {}", peer, error);
            }
            request_response::Event::InboundFailure { peer, error, .. } => {
                debug!("snapshot manifest inbound failure from {}: {}", peer, error);
            }
            request_response::Event::ResponseSent { .. } => {}
        }
    }

    pub fn take_pending_snapshot_manifest_request(
        &mut self,
    ) -> Option<PendingSnapshotManifestRequest> {
        self.pending_snapshot_requests.pop_front()
    }

    pub fn take_received_snapshot_manifests(&mut self) -> Option<ReceivedSnapshotManifests> {
        self.received_snapshot_manifests.pop_front()
    }

    pub fn respond_snapshot_manifest_request(
        &mut self,
        pending: PendingSnapshotManifestRequest,
        response: SnapshotManifestsResponse,
    ) -> Result<(), SnapshotManifestsResponse> {
        self.swarm
            .behaviour_mut()
            .snapshot_request_response
            .send_response(pending.channel, response)
    }

    pub fn send_snapshot_manifest_request(
        &mut self,
        peer: &PeerId,
        request: GetSnapshotManifests,
    ) -> Option<request_response::OutboundRequestId> {
        if !self.swarm.is_connected(peer) {
            return None;
        }
        Some(
            self.swarm
                .behaviour_mut()
                .snapshot_request_response
                .send_request(peer, request),
        )
    }

    fn select_block_request_peers(&self) -> Vec<PeerId> {
        let mut peers = self
            .connected_peer_ids()
            .into_iter()
            .filter(|peer| !self.is_block_sync_peer_quarantined(peer))
            .collect::<Vec<_>>();
        peers.sort_by(|left, right| {
            self.block_request_peer_scores
                .get(right)
                .copied()
                .unwrap_or_default()
                .cmp(
                    &self
                        .block_request_peer_scores
                        .get(left)
                        .copied()
                        .unwrap_or_default(),
                )
                .then_with(|| left.to_string().cmp(&right.to_string()))
        });
        peers
    }

    fn select_block_request_peers_with_affinity(
        &self,
        preferred_peer: Option<&PeerId>,
        strict_peer_affinity: bool,
    ) -> Vec<PeerId> {
        let mut peers = self.select_block_request_peers();
        let Some(preferred_peer) = preferred_peer else {
            return peers;
        };
        if strict_peer_affinity {
            peers.retain(|peer| peer == preferred_peer);
            return peers;
        }
        if let Some(position) = peers.iter().position(|peer| peer == preferred_peer) {
            let preferred = peers.remove(position);
            peers.insert(0, preferred);
        }
        peers
    }

    fn select_private_tx_request_peers(&self, tx_hash: &str) -> Vec<PeerId> {
        let mut peers = self.connected_peer_ids();
        let announced = self.private_tx_announce_sources.get(tx_hash.trim());
        peers.sort_by(|left, right| {
            let right_announced = announced
                .map(|peers| peers.contains(right))
                .unwrap_or(false);
            let left_announced = announced.map(|peers| peers.contains(left)).unwrap_or(false);
            right_announced
                .cmp(&left_announced)
                .then_with(|| {
                    self.private_tx_request_peer_scores
                        .get(right)
                        .copied()
                        .unwrap_or_default()
                        .cmp(
                            &self
                                .private_tx_request_peer_scores
                                .get(left)
                                .copied()
                                .unwrap_or_default(),
                        )
                })
                .then_with(|| left.to_string().cmp(&right.to_string()))
        });
        peers
    }

    fn take_matching_direct_block_response(
        &mut self,
        request_ids: &HashSet<request_response::OutboundRequestId>,
    ) -> Option<ReceivedBlockBodyResponse> {
        let mut deferred = VecDeque::new();
        let mut matched = None;
        while let Some(response) = self.received_block_body_responses.pop_front() {
            if matched.is_none() && request_ids.contains(&response.request_id) {
                matched = Some(response);
            } else {
                deferred.push_back(response);
            }
        }
        self.received_block_body_responses = deferred;
        matched
    }

    fn take_matching_private_tx_body_response(
        &mut self,
        request_ids: &HashSet<request_response::OutboundRequestId>,
    ) -> Option<ReceivedPrivateTxBodyResponse> {
        let mut deferred = VecDeque::new();
        let mut matched = None;
        while let Some(response) = self.received_private_tx_body_responses.pop_front() {
            if matched.is_none() && request_ids.contains(&response.request_id) {
                matched = Some(response);
            } else {
                deferred.push_back(response);
            }
        }
        self.received_private_tx_body_responses = deferred;
        matched
    }

    fn take_matching_direct_block_failure(
        &mut self,
        request_ids: &HashSet<request_response::OutboundRequestId>,
    ) -> Option<BlockBodyRequestFailure> {
        let mut deferred = VecDeque::new();
        let mut matched = None;
        while let Some(failure) = self.block_body_request_failures.pop_front() {
            if matched.is_none() && request_ids.contains(&failure.request_id) {
                matched = Some(failure);
            } else {
                deferred.push_back(failure);
            }
        }
        self.block_body_request_failures = deferred;
        matched
    }

    fn take_matching_private_tx_body_failure(
        &mut self,
        request_ids: &HashSet<request_response::OutboundRequestId>,
    ) -> Option<PrivateTxBodyRequestFailure> {
        let mut deferred = VecDeque::new();
        let mut matched = None;
        while let Some(failure) = self.private_tx_body_request_failures.pop_front() {
            if matched.is_none() && request_ids.contains(&failure.request_id) {
                matched = Some(failure);
            } else {
                deferred.push_back(failure);
            }
        }
        self.private_tx_body_request_failures = deferred;
        matched
    }

    /// Install an immediate responder for inbound block-body requests. Once
    /// set, requests are answered synchronously in the swarm event handler
    /// (see handle_block_request_response_event) rather than queued, which is
    /// what keeps IBD serving from being starved by the server's own sync
    /// waits.
    pub fn set_block_body_responder(
        &mut self,
        responder: impl Fn(&BlockBodyRequest) -> BlockBodyResponse + Send + 'static,
    ) {
        self.block_body_responder = Some(Box::new(responder));
    }

    /// Install an immediate responder for inbound GetHeaders requests.
    pub fn set_header_responder(
        &mut self,
        responder: impl Fn(&GetHeaders) -> HeadersResponse + Send + 'static,
    ) {
        self.header_responder = Some(Box::new(responder));
    }

    pub fn take_pending_block_body_request(&mut self) -> Option<PendingBlockBodyRequest> {
        self.pending_block_body_requests.pop_front()
    }

    pub fn take_pending_private_tx_body_request(&mut self) -> Option<PendingPrivateTxBodyRequest> {
        self.pending_private_tx_body_requests.pop_front()
    }

    pub fn respond_block_body_request(
        &mut self,
        pending: PendingBlockBodyRequest,
        response: BlockBodyResponse,
    ) -> Result<(), BlockBodyResponse> {
        let peer = pending.peer;
        match self
            .swarm
            .behaviour_mut()
            .block_request_response
            .send_response(pending.channel, response)
        {
            Ok(()) => {
                let latency = pending.received_at.elapsed();
                self.record_direct_block_success(peer, latency);
                Ok(())
            }
            Err(response) => {
                self.record_direct_block_failure(peer);
                Err(response)
            }
        }
    }

    pub fn respond_private_tx_body_request(
        &mut self,
        pending: PendingPrivateTxBodyRequest,
        response: PrivateTxBodyResponse,
    ) -> Result<(), PrivateTxBodyResponse> {
        let peer = pending.peer;
        match self
            .swarm
            .behaviour_mut()
            .private_tx_request_response
            .send_response(pending.channel, response)
        {
            Ok(()) => {
                let latency = pending.received_at.elapsed();
                self.record_private_tx_request_success(peer, latency);
                Ok(())
            }
            Err(response) => {
                self.record_private_tx_request_failure(peer);
                Err(response)
            }
        }
    }

    pub fn block_request_response_stats(&self) -> BlockRequestResponseStats {
        self.block_request_response_stats.clone()
    }

    pub fn private_tx_request_response_stats(&self) -> PrivateTxRequestResponseStats {
        self.private_tx_request_response_stats.clone()
    }

    fn prune_recent_private_tx_bodies(&mut self) {
        let now = Instant::now();
        self.recent_private_tx_bodies.retain(|_, (_, inserted_at)| {
            now.duration_since(*inserted_at) <= PRIVATE_TX_BODY_CACHE_TTL
        });
        if self.recent_private_tx_bodies.len() <= PRIVATE_TX_BODY_CACHE_MAX {
            return;
        }
        let mut entries = self
            .recent_private_tx_bodies
            .iter()
            .map(|(hash, (_, inserted_at))| (hash.clone(), *inserted_at))
            .collect::<Vec<_>>();
        entries.sort_by_key(|(_, inserted_at)| *inserted_at);
        let remove_count = entries.len().saturating_sub(PRIVATE_TX_BODY_CACHE_MAX);
        for (hash, _) in entries.into_iter().take(remove_count) {
            self.recent_private_tx_bodies.remove(&hash);
        }
    }

    pub fn cache_private_tx_body(&mut self, tx_hash: &str, body: Vec<u8>) {
        let tx_hash = tx_hash.trim();
        if tx_hash.is_empty() || body.is_empty() {
            return;
        }
        self.prune_recent_private_tx_bodies();
        self.recent_private_tx_bodies
            .insert(tx_hash.to_string(), (body, Instant::now()));
    }

    pub fn cached_private_tx_body(&mut self, tx_hash: &str) -> Option<Vec<u8>> {
        self.prune_recent_private_tx_bodies();
        self.recent_private_tx_bodies
            .get(tx_hash.trim())
            .map(|(body, _)| body.clone())
    }

    pub fn penalize_private_tx_announcers(&mut self, tx_hash: &str) {
        let peers = self
            .private_tx_announce_sources
            .get(tx_hash.trim())
            .cloned()
            .unwrap_or_default();
        for peer in peers {
            self.record_private_tx_request_failure(peer);
        }
    }

    /// Listen on a multiaddr string.
    pub async fn listen(&mut self, addr: &str) -> Result<(), Box<dyn Error>> {
        let addr: Multiaddr = addr.parse()?;
        self.swarm.listen_on(addr.clone())?;
        info!("Node listening on {}", addr);
        Ok(())
    }

    /// Advertise an externally reachable multiaddr in Identify.
    pub fn add_external_address(&mut self, addr: &str) -> Result<(), Box<dyn Error>> {
        let addr: Multiaddr = addr.parse()?;
        self.swarm.add_external_address(addr.clone());
        info!("Node external address advertised: {}", addr);
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

    /// Add a known peer address to Kademlia when the multiaddr carries `/p2p/<peer-id>`.
    pub fn add_kademlia_peer_addr(&mut self, peer_addr: &str) -> Result<bool, Box<dyn Error>> {
        let addr: Multiaddr = peer_addr.parse()?;
        let Some((peer_id, dial_addr)) = Self::kademlia_peer_from_multiaddr(&addr) else {
            return Ok(false);
        };
        self.swarm
            .behaviour_mut()
            .kademlia
            .add_address(&peer_id, dial_addr);
        Ok(true)
    }

    /// Add all usable Kademlia seed addresses and return the accepted count.
    pub fn add_kademlia_peer_addrs<'a>(
        &mut self,
        peer_addrs: impl IntoIterator<Item = &'a String>,
    ) -> usize {
        let mut added = 0usize;
        for addr in peer_addrs {
            match self.add_kademlia_peer_addr(addr) {
                Ok(true) => added += 1,
                Ok(false) => {}
                Err(err) => warn!("failed to add Kademlia peer address {}: {}", addr, err),
            }
        }
        added
    }

    /// Start a Kademlia bootstrap query against known peers.
    pub fn bootstrap_kademlia(&mut self) -> Result<kad::QueryId, Box<dyn Error>> {
        self.swarm
            .behaviour_mut()
            .kademlia
            .bootstrap()
            .map_err(|e| -> Box<dyn Error> { Box::new(e) })
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
                    self.record_identify_peer(peer_id, &info);
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
                    self.record_identify_peer(peer_id, &info);
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
                    self.record_identify_peer(peer_id, &info);
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
                    let message_source = message
                        .source
                        .unwrap_or_else(|| propagation_source.clone());
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
                            self.record_gossip_rx(&ZokaMessage::StemPrivateTransaction(
                                stem.clone(),
                            ));
                            if let Some(message) = self.maybe_intercept_stem_message(stem).await? {
                                self.enqueue_inbound_message(SourcedZokaMessage {
                                    source: Some(propagation_source),
                                    message,
                                });
                            }
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
                            self.record_gossip_rx(&ZokaMessage::PrivateMixerBatch(batch.clone()));
                            self.maybe_intercept_mixer_message(propagation_source, batch)?;
                        }
                        Ok(msg) => {
                            if let Err(err) = self.validate_inbound_message(&msg) {
                                warn!("discarding invalid message while pumping swarm: {}", err);
                            } else {
                                self.preserve_pumped_message(
                                    propagation_source,
                                    message_source,
                                    msg,
                                );
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
                            self.record_identify_peer(peer_id, &info);
                            self.enforce_protocol_freeze_for_peer(peer_id, &info.protocol_version);
                        }
                        SwarmEvent::Behaviour(NodeBehaviourEvent::BlockRequestResponse(event)) => {
                            self.handle_block_request_response_event(event);
                        }
                        SwarmEvent::Behaviour(NodeBehaviourEvent::PrivateTxRequestResponse(event)) => {
                            self.handle_private_tx_request_response_event(event);
                        }
                        SwarmEvent::Behaviour(NodeBehaviourEvent::HeaderRequestResponse(event)) => {
                            self.handle_header_request_response_event(event);
                        }
                        SwarmEvent::Behaviour(NodeBehaviourEvent::SnapshotRequestResponse(event)) => {
                            self.handle_snapshot_request_response_event(event);
                        }
                        SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                            info!("Peer connected while pumping: {}", peer_id);
                        }
                        SwarmEvent::ConnectionClosed { peer_id, cause, .. } => {
                            if let Some(cause) = cause {
                                warn!("Peer connection closed while pumping: {} ({})", peer_id, cause);
                            } else {
                                info!("Peer connection closed while pumping: {}", peer_id);
                            }
                        }
                        SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                            warn!(
                                "Outgoing peer connection error while pumping: {:?} ({})",
                                peer_id, error
                            );
                        }
                        SwarmEvent::IncomingConnectionError {
                            local_addr,
                            send_back_addr,
                            error,
                            ..
                        } => {
                            warn!(
                                "Incoming peer connection error while pumping: {} from {} ({})",
                                local_addr, send_back_addr, error
                            );
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
            ZokaMessage::OfferAnnounce(_) | ZokaMessage::OfferStatusUpdate(_) => {
                self.offers_topic.clone()
            }
            _ => self.general_topic.clone(),
        };
        match self.swarm.behaviour_mut().gossipsub.publish(topic, payload) {
            Ok(_) => {
                self.record_gossip_publish(true);
                Ok(())
            }
            Err(e) => {
                self.record_gossip_publish(false);
                Err(Box::new(e))
            }
        }
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
        block_hash: Option<String>,
    ) -> Result<(), Box<dyn Error>> {
        self.publish_message(&ZokaMessage::BlockRequest {
            block_height,
            block_hash,
        })
    }

    /// Publish lightweight block inventory on general topic.
    pub fn publish_block_announce(
        &mut self,
        block_height: u64,
        block_hash: String,
        cumulative_work: String,
        difficulty_bits: Option<u32>,
    ) -> Result<(), Box<dyn Error>> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.publish_message(&ZokaMessage::BlockAnnounce(BlockAnnounceMessage {
            block_height,
            block_hash,
            cumulative_work,
            difficulty_bits,
            timestamp: now,
        }))
    }

    /// Publish BlockResponse message on general topic.
    pub fn publish_block_response(
        &mut self,
        block_height: u64,
        block_hash: String,
        encrypted_block: Vec<u8>,
    ) -> Result<(), Box<dyn Error>> {
        self.publish_block_response_with_bodies(
            block_height,
            block_hash,
            encrypted_block,
            Vec::new(),
        )
    }

    /// Publish BlockResponse with private transaction bodies attached so
    /// receivers can persist them under `priv_tx:<hash>` and serve them via
    /// `/private/tx/:hash` without depending on the original sender ever
    /// having POSTed to a central archive. Each body is the serialized
    /// `PrivateTransaction` JSON whose `tx_hash` MUST be one of the
    /// `private_tx_hashes` committed in the block.
    pub fn publish_block_response_with_bodies(
        &mut self,
        block_height: u64,
        block_hash: String,
        encrypted_block: Vec<u8>,
        private_tx_bodies: Vec<Vec<u8>>,
    ) -> Result<(), Box<dyn Error>> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let message = ZokaMessage::BlockResponse(BlockResponseMessage {
            block_height,
            encrypted_block,
            block_hash,
            difficulty_bits: None,
            private_tx_bodies,
            timestamp: now,
        });
        // Gossip frames are size-bounded and JSON inflates binary bodies ~4x,
        // so a couple of ZK-proof bodies can push a block response past the
        // limit and gossipsub drops the whole thing (MessageTooLarge). When
        // that would happen, publish the block WITHOUT its bodies — the block
        // still propagates, and receivers backfill the bodies over the
        // point-to-point private-tx request-response protocol.
        let (message, dropped_bodies) = fit_block_response_for_gossip(message);
        if dropped_bodies > 0 {
            warn!(
                "block response for height {block_height} too large for gossip; publishing block-only, {dropped_bodies} body(ies) will be backfilled"
            );
        }
        self.publish_message(&message)
    }

    fn restore_deferred_messages(&mut self, deferred: &mut Vec<SourcedZokaMessage>) {
        while let Some(message) = deferred.pop() {
            self.enqueue_inbound_message_front(message);
        }
    }

    pub async fn request_private_tx_body_with_retry(
        &mut self,
        tx_hash: &str,
        attempts: u32,
        max_wait: Duration,
    ) -> Result<Vec<u8>, Box<dyn Error>> {
        let tx_hash = tx_hash.trim();
        if tx_hash.is_empty() {
            return Err(Box::new(io::Error::new(
                io::ErrorKind::InvalidInput,
                "private tx hash is empty",
            )));
        }

        let peers = self.select_private_tx_request_peers(tx_hash);
        if peers.is_empty() {
            return Err(Box::new(io::Error::new(
                io::ErrorKind::NotConnected,
                "no connected peers available for private tx body request",
            )));
        }

        let max_attempts = attempts.max(1) as usize;
        let request = PrivateTxBodyRequest {
            tx_hash: tx_hash.to_string(),
        };
        let mut request_ids = HashSet::new();
        for peer in peers.into_iter().take(max_attempts) {
            let request_id = self
                .swarm
                .behaviour_mut()
                .private_tx_request_response
                .send_request(&peer, request.clone());
            self.pending_outbound_private_tx_body_requests.insert(
                request_id,
                PendingOutboundPrivateTxBodyRequest {
                    peer,
                    request: request.clone(),
                    started_at: Instant::now(),
                },
            );
            self.private_tx_request_response_stats.outbound_sent = self
                .private_tx_request_response_stats
                .outbound_sent
                .saturating_add(1);
            request_ids.insert(request_id);
        }

        let deadline = tokio::time::Instant::now() + max_wait;
        let mut last_error = String::new();
        loop {
            if let Some(received) = self.take_matching_private_tx_body_response(&request_ids) {
                match received.response {
                    PrivateTxBodyResponse::Body { tx_hash: got, body } => {
                        if got != tx_hash || body.is_empty() {
                            self.record_private_tx_invalid_response(received.peer.clone());
                            last_error = format!(
                                "peer {} returned invalid private tx body for {}",
                                received.peer, tx_hash
                            );
                            continue;
                        }
                        info!(
                            "direct private tx body response hash={} from {} in {}ms",
                            tx_hash,
                            received.peer,
                            received.latency.as_millis()
                        );
                        return Ok(body);
                    }
                    PrivateTxBodyResponse::NotFound { reason, .. } => {
                        self.record_private_tx_request_failure(received.peer.clone());
                        last_error = format!(
                            "peer {} did not have private tx body: {reason}",
                            received.peer
                        );
                    }
                    PrivateTxBodyResponse::RateLimited {
                        retry_after_secs, ..
                    } => {
                        self.private_tx_request_response_stats.rate_limited = self
                            .private_tx_request_response_stats
                            .rate_limited
                            .saturating_add(1);
                        self.record_private_tx_request_failure(received.peer.clone());
                        last_error = format!(
                            "peer {} rate limited private tx body request for {}s",
                            received.peer, retry_after_secs
                        );
                    }
                }
            }

            if let Some(failure) = self.take_matching_private_tx_body_failure(&request_ids) {
                last_error = format!(
                    "peer {} private tx body request failed: {}",
                    failure.peer, failure.error
                );
            }

            if tokio::time::Instant::now() >= deadline {
                let detail = if last_error.is_empty() {
                    "no direct PrivateTxBody response received".to_string()
                } else {
                    last_error
                };
                // Peers that announced this tx but never answered our request at
                // all (no Body/NotFound/RateLimited, no OutboundFailure event
                // either) get penalized here, since none of the other response
                // branches above ever ran for them.
                let announced_by = self.private_tx_announce_sources.get(tx_hash).cloned();
                for request_id in &request_ids {
                    if let Some(pending) = self
                        .pending_outbound_private_tx_body_requests
                        .remove(request_id)
                    {
                        let announced = announced_by
                            .as_ref()
                            .map(|peers| peers.contains(&pending.peer))
                            .unwrap_or(false);
                        if announced {
                            self.record_private_tx_request_failure(pending.peer);
                        }
                    }
                }
                return Err(Box::new(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("timed out waiting for private tx body response: {detail}"),
                )));
            }

            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            self.pump_for(remaining.min(Duration::from_millis(150)))
                .await?;
        }
    }

    async fn request_block_direct_with_retry(
        &mut self,
        block_height: u64,
        expected_hash: Option<String>,
        preferred_peer: Option<PeerId>,
        strict_peer_affinity: bool,
        attempts: u32,
        max_wait: Duration,
    ) -> Result<SourcedBlockResponse, Box<dyn Error>> {
        let peers = self.select_block_request_peers_with_affinity(
            preferred_peer.as_ref(),
            strict_peer_affinity,
        );
        if peers.is_empty() {
            return Err(Box::new(io::Error::new(
                io::ErrorKind::NotConnected,
                "no connected peers available for direct block request",
            )));
        }

        let max_attempts = attempts.max(1) as usize;
        let mut request_ids = HashSet::new();
        let request = BlockBodyRequest {
            block_height,
            block_hash: expected_hash.clone(),
        };
        for peer in peers.into_iter().take(max_attempts) {
            let request_id = self
                .swarm
                .behaviour_mut()
                .block_request_response
                .send_request(&peer, request.clone());
            self.pending_outbound_block_body_requests.insert(
                request_id,
                PendingOutboundBlockBodyRequest {
                    peer,
                    request: request.clone(),
                    started_at: Instant::now(),
                },
            );
            self.block_request_response_stats.outbound_sent = self
                .block_request_response_stats
                .outbound_sent
                .saturating_add(1);
            request_ids.insert(request_id);
        }

        if request_ids.is_empty() {
            return Err(Box::new(io::Error::new(
                io::ErrorKind::NotConnected,
                "no direct block request could be scheduled",
            )));
        }

        let deadline = tokio::time::Instant::now() + max_wait;
        let mut last_error = String::new();
        loop {
            if let Some(received) = self.take_matching_direct_block_response(&request_ids) {
                request_ids.remove(&received.request_id);
                match received.response {
                    BlockBodyResponse::Found(response) => {
                        if response.block_height != block_height {
                            self.record_direct_block_invalid_response(received.peer.clone());
                            last_error = format!(
                                "peer {} returned height {}, expected {}",
                                received.peer, response.block_height, block_height
                            );
                            continue;
                        }
                        if let Some(hash) = expected_hash.as_ref() {
                            if &response.block_hash != hash {
                                self.record_direct_block_invalid_response(received.peer.clone());
                                last_error = format!(
                                    "peer {} returned hash {}, expected {}",
                                    received.peer, response.block_hash, hash
                                );
                                continue;
                            }
                        }
                        info!(
                            "direct block response height={} hash={} from {} in {}ms",
                            response.block_height,
                            response.block_hash,
                            received.peer,
                            received.latency.as_millis()
                        );
                        return Ok(SourcedBlockResponse {
                            source: received.peer,
                            response,
                        });
                    }
                    BlockBodyResponse::NotFound { reason, .. } => {
                        self.record_direct_block_failure(received.peer.clone());
                        last_error = format!("peer {} did not have block: {reason}", received.peer);
                    }
                    BlockBodyResponse::Rejected { reason, .. } => {
                        self.record_direct_block_failure(received.peer.clone());
                        last_error = format!(
                            "peer {} rejected direct block request: {reason}",
                            received.peer
                        );
                    }
                }
            }

            if let Some(failure) = self.take_matching_direct_block_failure(&request_ids) {
                request_ids.remove(&failure.request_id);
                last_error = format!(
                    "peer {} direct request failed: {}",
                    failure.peer, failure.error
                );
            }

            if request_ids.is_empty() {
                let detail = if last_error.is_empty() {
                    "all direct block responders completed without a usable block".to_string()
                } else {
                    last_error
                };
                return Err(Box::new(io::Error::new(io::ErrorKind::NotFound, detail)));
            }

            if tokio::time::Instant::now() >= deadline {
                let detail = if last_error.is_empty() {
                    "no direct BlockData response received".to_string()
                } else {
                    last_error
                };
                return Err(Box::new(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("timed out waiting for direct block response: {detail}"),
                )));
            }

            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            self.pump_for(remaining.min(Duration::from_millis(150)))
                .await?;
        }
    }

    /// Fires one direct block-body request per height in `heights` without
    /// awaiting between them (Fase 4 IBD parallel body download: fetches N
    /// heights concurrently instead of the one-at-a-time
    /// request/wait/apply/request cycle `run_sync_step` used to run
    /// serially). Each height gets exactly one peer, chosen round-robin
    /// from `select_block_request_peers()` so the in-flight requests
    /// spread across the peer pool. Unlike `request_block_direct_with_retry`
    /// (which races multiple PEERS for ONE height), this fans out across
    /// multiple HEIGHTS; a height missing from the result map simply falls
    /// back to the normal single-height retry path on the caller's next
    /// tick. Intentionally does not pin an expected hash per height (that
    /// stays the job of the hash-pinned branch-backfill path) - whatever
    /// comes back still has to pass full consensus validation in
    /// `apply_block` before it lands in storage, so an unpinned or even
    /// dishonest response here can't corrupt the chain, only waste a
    /// prefetch slot.
    pub async fn request_blocks_batch_direct(
        &mut self,
        heights: &[u64],
        max_wait: Duration,
    ) -> HashMap<u64, Result<SourcedBlockResponse, String>> {
        let mut results: HashMap<u64, Result<SourcedBlockResponse, String>> = HashMap::new();
        if heights.is_empty() {
            return results;
        }
        let peers = self.select_block_request_peers();
        if peers.is_empty() {
            return results;
        }

        let mut request_id_to_height = HashMap::new();
        let mut pending_heights: HashSet<u64> = HashSet::new();
        for (i, &height) in heights.iter().enumerate() {
            let peer = peers[i % peers.len()].clone();
            let request = BlockBodyRequest {
                block_height: height,
                block_hash: None,
            };
            let request_id = self
                .swarm
                .behaviour_mut()
                .block_request_response
                .send_request(&peer, request.clone());
            self.pending_outbound_block_body_requests.insert(
                request_id,
                PendingOutboundBlockBodyRequest {
                    peer,
                    request,
                    started_at: Instant::now(),
                },
            );
            self.block_request_response_stats.outbound_sent = self
                .block_request_response_stats
                .outbound_sent
                .saturating_add(1);
            request_id_to_height.insert(request_id, height);
            pending_heights.insert(height);
        }

        let request_ids: HashSet<_> = request_id_to_height.keys().copied().collect();
        let deadline = tokio::time::Instant::now() + max_wait;
        while !pending_heights.is_empty() && tokio::time::Instant::now() < deadline {
            if let Some(received) = self.take_matching_direct_block_response(&request_ids) {
                let Some(height) = request_id_to_height.get(&received.request_id).copied() else {
                    continue;
                };
                if !pending_heights.remove(&height) {
                    continue;
                }
                match received.response {
                    BlockBodyResponse::Found(response) => {
                        if response.block_height != height {
                            self.record_direct_block_invalid_response(received.peer.clone());
                            results.insert(
                                height,
                                Err(format!(
                                    "peer {} returned height {}, expected {height}",
                                    received.peer, response.block_height
                                )),
                            );
                            continue;
                        }
                        info!(
                            "direct block response height={} hash={} from {} in {}ms (batch)",
                            response.block_height,
                            response.block_hash,
                            received.peer,
                            received.latency.as_millis()
                        );
                        results.insert(
                            height,
                            Ok(SourcedBlockResponse {
                                source: received.peer,
                                response,
                            }),
                        );
                    }
                    BlockBodyResponse::NotFound { reason, .. } => {
                        self.record_direct_block_failure(received.peer.clone());
                        results.insert(
                            height,
                            Err(format!(
                                "peer {} did not have block: {reason}",
                                received.peer
                            )),
                        );
                    }
                    BlockBodyResponse::Rejected { reason, .. } => {
                        self.record_direct_block_failure(received.peer.clone());
                        results.insert(
                            height,
                            Err(format!(
                                "peer {} rejected direct block request: {reason}",
                                received.peer
                            )),
                        );
                    }
                }
                continue;
            }
            if let Some(failure) = self.take_matching_direct_block_failure(&request_ids) {
                if let Some(height) = request_id_to_height.get(&failure.request_id).copied() {
                    if pending_heights.remove(&height) {
                        results.insert(
                            height,
                            Err(format!(
                                "peer {} direct request failed: {}",
                                failure.peer, failure.error
                            )),
                        );
                    }
                }
                continue;
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if self
                .pump_for(remaining.min(Duration::from_millis(150)))
                .await
                .is_err()
            {
                break;
            }
        }
        results
    }

    async fn request_block_via_gossip_with_retry(
        &mut self,
        block_height: u64,
        expected_hash: Option<String>,
        preferred_peer: Option<PeerId>,
        strict_peer_affinity: bool,
        publish_attempts: u32,
        max_wait: Duration,
    ) -> Result<SourcedBlockResponse, Box<dyn Error>> {
        let request = ZokaMessage::BlockRequest {
            block_height,
            block_hash: expected_hash.clone(),
        };

        self.publish_message_with_retry(&request, publish_attempts, Duration::from_millis(120))
            .await?;

        let deadline = tokio::time::Instant::now() + max_wait;
        let mut last_error = String::new();
        let mut deferred = Vec::new();
        loop {
            if tokio::time::Instant::now() >= deadline {
                let detail = if last_error.is_empty() {
                    "no matching BlockResponse received".to_string()
                } else {
                    last_error
                };
                self.restore_deferred_messages(&mut deferred);
                return Err(Box::new(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("timed out waiting for block response: {detail}"),
                )));
            }

            let wait_budget = deadline
                .saturating_duration_since(tokio::time::Instant::now())
                .min(Duration::from_millis(800));
            match tokio::time::timeout(wait_budget, self.next_sourced_message()).await {
                Ok(Ok(sourced)) => match sourced.message {
                    ZokaMessage::BlockResponse(resp) => {
                        if resp.block_height != block_height {
                            deferred.push(SourcedZokaMessage {
                                source: sourced.source,
                                message: ZokaMessage::BlockResponse(resp),
                            });
                            continue;
                        }
                        if let Some(hash) = expected_hash.as_ref() {
                            if &resp.block_hash != hash {
                                deferred.push(SourcedZokaMessage {
                                    source: sourced.source,
                                    message: ZokaMessage::BlockResponse(resp),
                                });
                                continue;
                            }
                        }
                        if strict_peer_affinity
                            && sourced.source.as_ref() != preferred_peer.as_ref()
                        {
                            deferred.push(SourcedZokaMessage {
                                source: sourced.source,
                                message: ZokaMessage::BlockResponse(resp),
                            });
                            continue;
                        }
                        let Some(source) = sourced.source else {
                            deferred.push(SourcedZokaMessage {
                                source: None,
                                message: ZokaMessage::BlockResponse(resp),
                            });
                            continue;
                        };
                        self.restore_deferred_messages(&mut deferred);
                        return Ok(SourcedBlockResponse {
                            source,
                            response: resp,
                        });
                    }
                    other => deferred.push(SourcedZokaMessage {
                        source: sourced.source,
                        message: other,
                    }),
                },
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

    fn should_request_pinned_block_via_gossip_first(
        &self,
        expected_hash: Option<&str>,
        preferred_peer: Option<&PeerId>,
    ) -> bool {
        expected_hash.is_some()
            && preferred_peer
                .map(|peer| !self.swarm.is_connected(peer))
                .unwrap_or(false)
    }

    /// Request a block while retaining and optionally enforcing peer affinity.
    pub async fn request_block_with_affinity(
        &mut self,
        block_height: u64,
        expected_hash: Option<String>,
        preferred_peer: Option<PeerId>,
        strict_peer_affinity: bool,
        publish_attempts: u32,
        max_wait: Duration,
    ) -> Result<SourcedBlockResponse, Box<dyn Error>> {
        // During hash-pinned ancestry recovery the signed branch author may be
        // reachable only through a connected relay. Asking that relay for the
        // body via request-response first makes every ancestor wait for the
        // direct timeout before gossip can reach the author. Publish the
        // hash-pinned request first in that case; the expected hash and normal
        // consensus validation still prevent a relay from switching branches.
        if self.should_request_pinned_block_via_gossip_first(
            expected_hash.as_deref(),
            preferred_peer.as_ref(),
        ) {
            match self
                .request_block_via_gossip_with_retry(
                    block_height,
                    expected_hash.clone(),
                    preferred_peer,
                    strict_peer_affinity,
                    publish_attempts,
                    max_wait,
                )
                .await
            {
                Ok(response) => return Ok(response),
                Err(gossip_err) => {
                    warn!(
                        "gossip-first pinned block request failed for height {} ({}); falling back to direct peers",
                        block_height, gossip_err
                    );
                    return self
                        .request_block_direct_with_retry(
                            block_height,
                            expected_hash,
                            preferred_peer,
                            strict_peer_affinity,
                            publish_attempts,
                            max_wait,
                        )
                        .await
                        .map_err(|direct_err| {
                            Box::new(io::Error::other(format!(
                                "gossip-first request failed: {gossip_err}; direct fallback failed: {direct_err}"
                            ))) as Box<dyn Error>
                        });
                }
            }
        }

        match self
            .request_block_direct_with_retry(
                block_height,
                expected_hash.clone(),
                preferred_peer,
                strict_peer_affinity,
                publish_attempts,
                max_wait,
            )
            .await
        {
            Ok(response) => Ok(response),
            Err(direct_err) => {
                warn!(
                    "direct block request failed for height {} ({}); falling back to gossip legacy",
                    block_height, direct_err
                );
                self.request_block_via_gossip_with_retry(
                    block_height,
                    expected_hash,
                    preferred_peer,
                    strict_peer_affinity,
                    publish_attempts,
                    max_wait,
                )
                .await
                .map_err(|gossip_err| {
                    Box::new(io::Error::other(format!(
                        "direct request failed: {direct_err}; gossip fallback failed: {gossip_err}"
                    ))) as Box<dyn Error>
                })
            }
        }
    }

    /// Request a block and wait for a matching response from any peer.
    pub async fn request_block_with_retry(
        &mut self,
        block_height: u64,
        expected_hash: Option<String>,
        publish_attempts: u32,
        max_wait: Duration,
    ) -> Result<BlockResponseMessage, Box<dyn Error>> {
        self.request_block_with_affinity(
            block_height,
            expected_hash,
            None,
            false,
            publish_attempts,
            max_wait,
        )
        .await
        .map(|sourced| sourced.response)
    }

    /// Receive the next protocol message while preserving the peer that
    /// propagated it. Synchronization callers should use this method.
    pub async fn next_sourced_message(&mut self) -> Result<SourcedZokaMessage, Box<dyn Error>> {
        loop {
            self.flush_due_private_batch()?;
            if let Some(message) = self.pop_inbound_message() {
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
                    let message_source =
                        message.source.unwrap_or_else(|| propagation_source.clone());
                    match ZokaMessage::from_bytes(&message.data) {
                        Ok(ZokaMessage::StemPrivateTransaction(stem)) => {
                            if let Err(err) = self.validate_inbound_message(
                                &ZokaMessage::StemPrivateTransaction(stem.clone()),
                            ) {
                                warn!("discarding invalid stem message: {}", err);
                                continue;
                            }
                            self.record_gossip_rx(&ZokaMessage::StemPrivateTransaction(
                                stem.clone(),
                            ));
                            if let Some(msg) = self.maybe_intercept_stem_message(stem).await? {
                                return Ok(SourcedZokaMessage {
                                    source: Some(propagation_source),
                                    message: msg,
                                });
                            }
                        }
                        Ok(ZokaMessage::PrivateMixerBatch(batch)) => {
                            if let Err(err) = self.validate_inbound_message(
                                &ZokaMessage::PrivateMixerBatch(batch.clone()),
                            ) {
                                warn!("discarding invalid mixer batch: {}", err);
                                continue;
                            }
                            self.record_gossip_rx(&ZokaMessage::PrivateMixerBatch(batch.clone()));
                            self.maybe_intercept_mixer_message(propagation_source, batch)?;
                            if let Some(message) = self.pop_inbound_message() {
                                return Ok(message);
                            }
                        }
                        Ok(msg) => {
                            if let Err(err) = self.validate_inbound_message(&msg) {
                                warn!("discarding invalid message: {}", err);
                                continue;
                            }
                            // Diagnostic for an intermittent resync stall on
                            // follower nodes (--seed configured): confirms
                            // whether a BlockResponse genuinely never arrives
                            // over gossip versus arriving and being dropped
                            // somewhere downstream after this point.
                            if let ZokaMessage::BlockResponse(ref r) = msg {
                                info!(
                                    "gossip: received BlockResponse height={} hash={} author={} via={}",
                                    r.block_height,
                                    r.block_hash,
                                    message_source,
                                    propagation_source
                                );
                            }
                            let sourced = self.sourced_inbound_message(
                                propagation_source,
                                message_source,
                                msg,
                            );
                            if self.is_quarantined_block_sync_message(&sourced) {
                                continue;
                            }
                            return Ok(sourced);
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
                    self.record_identify_peer(peer_id, &info);
                    self.enforce_protocol_freeze_for_peer(peer_id, &info.protocol_version);
                }
                SwarmEvent::Behaviour(NodeBehaviourEvent::BlockRequestResponse(event)) => {
                    self.handle_block_request_response_event(event);
                }
                SwarmEvent::Behaviour(NodeBehaviourEvent::PrivateTxRequestResponse(event)) => {
                    self.handle_private_tx_request_response_event(event);
                }
                SwarmEvent::Behaviour(NodeBehaviourEvent::HeaderRequestResponse(event)) => {
                    self.handle_header_request_response_event(event);
                }
                SwarmEvent::Behaviour(NodeBehaviourEvent::SnapshotRequestResponse(event)) => {
                    self.handle_snapshot_request_response_event(event);
                }
                _ => {}
            }
        }
    }

    /// Backwards-compatible message-only receive path for callers that do not
    /// make synchronization decisions from the sender identity.
    pub async fn next_message(&mut self) -> Result<ZokaMessage, Box<dyn Error>> {
        Ok(self.next_sourced_message().await?.message)
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

    /// Return peer addresses learned through Identify for persistence/redial.
    pub fn observed_peer_addresses(&self) -> Vec<(String, String)> {
        self.observed_peer_addrs
            .iter()
            .flat_map(|(peer_id, addrs)| {
                addrs
                    .iter()
                    .cloned()
                    .map(|addr| (peer_id.to_string(), addr))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// For each *currently connected* peer, its id plus a representative
    /// observed address, for the `/peers` observability endpoint. Prefers a
    /// Tor onion address when we know one so the transport (Tor vs clearnet)
    /// is visible; falls back to any known address, or `None` if we have not
    /// learned one via Identify yet.
    pub fn connected_peer_addr_pairs(&self) -> Vec<(String, Option<String>)> {
        self.swarm
            .connected_peers()
            .copied()
            .collect::<Vec<_>>()
            .into_iter()
            .map(|peer_id| {
                let addr = self.observed_peer_addrs.get(&peer_id).and_then(|addrs| {
                    addrs
                        .iter()
                        .find(|a| a.contains("/onion3/"))
                        .or_else(|| addrs.iter().next())
                        .cloned()
                });
                (peer_id.to_string(), addr)
            })
            .collect()
    }

    /// Return peer addresses and protocol metadata learned through Identify.
    pub fn observed_peer_records(&self) -> Vec<ObservedPeerRecord> {
        self.observed_peer_addrs
            .iter()
            .flat_map(|(peer_id, addrs)| {
                let protocols = self.observed_peer_protocols.get(peer_id);
                addrs
                    .iter()
                    .cloned()
                    .map(|addr| ObservedPeerRecord {
                        peer_id: peer_id.to_string(),
                        addr,
                        protocol_version: protocols.and_then(|info| info.protocol_version.clone()),
                        protocols: protocols
                            .map(|info| info.protocols.clone())
                            .unwrap_or_default(),
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// Return local gossip counters for runtime observability.
    pub fn gossip_stats(&self) -> GossipStats {
        let mut stats = self.gossip_stats.clone();
        stats.mesh_peer_count = self
            .swarm
            .behaviour()
            .gossipsub
            .mesh_peers(&self.general_topic.hash())
            .count() as u64;
        stats
    }

    /// Return number of peers blocked by protocol-freeze compatibility rules.
    pub fn incompatible_peer_count(&self) -> usize {
        self.incompatible_protocol_peers.len()
    }

    /// Start (or restart) tracking a BlockAnnounce-triggered fetch so a
    /// sweep can retry it later if the body never arrives.
    pub fn track_block_announce_retry(&mut self, height: u64, hash: &str) {
        const MAX_TRACKED: usize = 256;
        let key = (height, hash.to_string());
        if !self.pending_block_announce_retries.contains_key(&key)
            && self.pending_block_announce_retries.len() >= MAX_TRACKED
        {
            if let Some(oldest) = self
                .pending_block_announce_retries
                .iter()
                .min_by_key(|(_, retry)| retry.last_attempt_at)
                .map(|(k, _)| k.clone())
            {
                self.pending_block_announce_retries.remove(&oldest);
            }
        }
        self.pending_block_announce_retries.insert(
            key,
            BlockAnnounceRetry {
                attempts: 1,
                last_attempt_at: Instant::now(),
            },
        );
    }

    /// Return `(height, hash, attempts)` for tracked announces whose backoff
    /// has elapsed. Does not mutate tracking state - callers decide whether
    /// to retry (`bump_block_announce_retry`) or stop (`forget_block_announce_retry`).
    pub fn due_block_announce_retries(&self) -> Vec<(u64, String, u8)> {
        const BACKOFF_SECS: [u64; 3] = [2, 4, 8];
        let now = Instant::now();
        self.pending_block_announce_retries
            .iter()
            .filter_map(|((height, hash), retry)| {
                let idx = (retry.attempts as usize)
                    .saturating_sub(1)
                    .min(BACKOFF_SECS.len() - 1);
                let backoff = Duration::from_secs(BACKOFF_SECS[idx]);
                if now.duration_since(retry.last_attempt_at) >= backoff {
                    Some((*height, hash.clone(), retry.attempts))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Record another retry attempt for a tracked announce. Returns `false`
    /// (and stops tracking it) once the attempt cap is reached.
    pub fn bump_block_announce_retry(&mut self, height: u64, hash: &str) -> bool {
        const MAX_ATTEMPTS: u8 = 3;
        let key = (height, hash.to_string());
        let Some(retry) = self.pending_block_announce_retries.get_mut(&key) else {
            return false;
        };
        if retry.attempts >= MAX_ATTEMPTS {
            self.pending_block_announce_retries.remove(&key);
            return false;
        }
        retry.attempts += 1;
        retry.last_attempt_at = Instant::now();
        true
    }

    /// Stop tracking a BlockAnnounce fetch (block arrived, or giving up).
    pub fn forget_block_announce_retry(&mut self, height: u64, hash: &str) {
        self.pending_block_announce_retries
            .remove(&(height, hash.to_string()));
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
            private_proof_version: crate::private_wallet::PRIVATE_PROOF_VERSION_V1,
            private_proof_profile: crate::private_wallet::PRIVATE_PROOF_PROFILE_V1.to_string(),
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
        assert_eq!(advertised_protocol_version(true), PROTOCOL_FREEZE_ID);
        assert_eq!(
            advertised_protocol_version(false),
            LEGACY_PROTOCOL_FREEZE_ID
        );
    }

    #[test]
    fn discovered_addr_filter_rejects_private_and_loopback() {
        // Public routable addresses and non-IP transports are dialable.
        assert!(discovered_addr_is_publicly_dialable(
            "/ip4/159.223.119.216/tcp/28080"
        ));
        assert!(discovered_addr_is_publicly_dialable(
            "/dns4/seed.example.com/tcp/28080"
        ));
        // Datacenter-internal / private / loopback / link-local are not.
        assert!(!discovered_addr_is_publicly_dialable(
            "/ip4/10.10.0.5/tcp/28080"
        ));
        assert!(!discovered_addr_is_publicly_dialable(
            "/ip4/172.16.4.9/tcp/28080"
        ));
        assert!(!discovered_addr_is_publicly_dialable(
            "/ip4/192.168.1.20/tcp/28080"
        ));
        assert!(!discovered_addr_is_publicly_dialable(
            "/ip4/127.0.0.1/tcp/28080"
        ));
        assert!(!discovered_addr_is_publicly_dialable("/ip6/::1/tcp/28080"));
        assert!(!discovered_addr_is_publicly_dialable(
            "/ip6/fc00::1/tcp/28080"
        ));
    }

    #[test]
    fn oversized_block_response_drops_bodies_for_gossip() {
        // JSON-encoding binary bodies inflates them ~4x, so two ~400 KB bodies
        // (~3 MB encoded) blow past the gossip ceiling. The response must
        // publish block-only; the bodies get backfilled point-to-point.
        let big_body = vec![0u8; 400 * 1024];
        let oversized = ZokaMessage::BlockResponse(BlockResponseMessage {
            block_height: 1,
            encrypted_block: vec![1, 2, 3],
            block_hash: "h".to_string(),
            difficulty_bits: None,
            private_tx_bodies: vec![big_body.clone(), big_body],
            timestamp: 0,
        });
        assert!(
            oversized.to_bytes().unwrap().len() > MAX_GOSSIP_BLOCK_RESPONSE_BYTES,
            "fixture must actually exceed the gossip ceiling"
        );
        let (fitted, dropped) = fit_block_response_for_gossip(oversized);
        assert_eq!(dropped, 2);
        match &fitted {
            ZokaMessage::BlockResponse(r) => assert!(r.private_tx_bodies.is_empty()),
            other => panic!("expected BlockResponse, got {other:?}"),
        }
        assert!(
            fitted.to_bytes().unwrap().len() <= MAX_GOSSIP_BLOCK_RESPONSE_BYTES,
            "block-only response must fit the gossip ceiling"
        );

        // A small block response keeps its bodies untouched.
        let small = ZokaMessage::BlockResponse(BlockResponseMessage {
            block_height: 2,
            encrypted_block: vec![1],
            block_hash: "h2".to_string(),
            difficulty_bits: None,
            private_tx_bodies: vec![vec![9u8; 16]],
            timestamp: 0,
        });
        let (fitted_small, dropped_small) = fit_block_response_for_gossip(small);
        assert_eq!(dropped_small, 0);
        match &fitted_small {
            ZokaMessage::BlockResponse(r) => assert_eq!(r.private_tx_bodies.len(), 1),
            other => panic!("expected BlockResponse, got {other:?}"),
        }
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

    #[tokio::test]
    async fn test_protocol_freeze_rejects_unknown_private_proof_profile() {
        let node = Node::new(0).await.expect("node");
        let mut tx = sample_private_tx("tx-invalid-proof-profile");
        tx.private_proof_version = 2;
        tx.private_proof_profile = "private_tx_v2_i1".to_string();

        assert!(node
            .validate_inbound_message(&ZokaMessage::PrivateTransaction(tx))
            .is_err());
    }

    #[test]
    fn test_protocol_freeze_compact_profile_is_dormant_but_width_bounded_when_configured() {
        let mut tx = sample_private_tx("tx-v2-i1");
        tx.private_proof_version = crate::private_wallet::PRIVATE_PROOF_VERSION_V2;
        tx.private_proof_profile = crate::private_wallet::PRIVATE_PROOF_PROFILE_V2_I1.to_string();
        tx.ring_members.push("member-c".to_string());
        tx.ring_signature = vec![3u8; 160];

        assert!(!Node::is_private_tx_well_formed(&tx));
        assert!(!Node::is_private_tx_well_formed_at(&tx, Some(100), None));
        assert!(Node::is_private_tx_well_formed_at(
            &tx,
            Some(100),
            Some(100)
        ));

        tx.input_key_images.push("key-image-2".to_string());
        tx.ring_signature = vec![3u8; 320];
        assert!(!Node::is_private_tx_well_formed_at(
            &tx,
            Some(100),
            Some(100)
        ));
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
                private_proof_version: crate::private_wallet::PRIVATE_PROOF_VERSION_V1,
                private_proof_profile: crate::private_wallet::PRIVATE_PROOF_PROFILE_V1.to_string(),
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

    #[tokio::test]
    async fn test_mixer_batch_rejects_unknown_private_proof_profile() {
        let mut node = Node::new(0).await.expect("node");
        node.set_privacy_config(NetworkPrivacyConfig {
            mixer_max_batch: 8,
            ..NetworkPrivacyConfig::default()
        });

        let mut invalid = sample_private_tx("tx-invalid-proof-profile");
        invalid.private_proof_version = 2;
        invalid.private_proof_profile = "private_tx_v2_i1".to_string();
        let batch = MixerBatchMessage {
            batch_id: "batch-invalid-proof-profile".to_string(),
            txs: vec![invalid],
            padding: vec![],
            timestamp: 1,
        };
        node.maybe_intercept_mixer_message(PeerId::random(), batch)
            .expect("intercept should not error");
        assert!(node.inbound_queue.is_empty());
    }

    #[tokio::test]
    async fn inbound_queue_preserves_source_deduplicates_blocks_and_is_bounded() {
        let mut node = Node::new(0).await.expect("node");
        let first_source = PeerId::random();
        let duplicate_source = PeerId::random();
        let response = BlockResponseMessage {
            block_height: 10_887,
            encrypted_block: vec![1, 2, 3],
            block_hash: "checkpoint-block".to_string(),
            difficulty_bits: None,
            private_tx_bodies: Vec::new(),
            timestamp: 1,
        };

        assert!(node.enqueue_inbound_message(SourcedZokaMessage {
            source: Some(first_source),
            message: ZokaMessage::BlockResponse(response.clone()),
        }));
        assert!(node.enqueue_inbound_message(SourcedZokaMessage {
            source: Some(duplicate_source),
            message: ZokaMessage::BlockResponse(response.clone()),
        }));
        assert!(!node.enqueue_inbound_message(SourcedZokaMessage {
            source: Some(first_source),
            message: ZokaMessage::BlockResponse(response),
        }));
        assert_eq!(node.inbound_queue.len(), 2);

        let preserved = node.pop_inbound_message().expect("queued response");
        assert_eq!(preserved.source, Some(first_source));
        assert!(matches!(
            preserved.message,
            ZokaMessage::BlockResponse(BlockResponseMessage {
                block_height: 10_887,
                ..
            })
        ));
        let failover = node.pop_inbound_message().expect("failover response");
        assert_eq!(failover.source, Some(duplicate_source));

        for nonce in 0..(MAX_INBOUND_QUEUE + 10) {
            node.enqueue_inbound_message(SourcedZokaMessage {
                source: Some(first_source),
                message: ZokaMessage::Ping(PingMessage {
                    nonce: nonce as u64,
                    timestamp: nonce as u64,
                }),
            });
        }
        assert_eq!(node.inbound_queue.len(), MAX_INBOUND_QUEUE);
    }

    #[tokio::test]
    async fn pumped_gossip_preserves_signed_author_for_next_message() {
        let mut node = Node::new(0).await.expect("node");
        let relay = PeerId::random();
        let author = PeerId::random();
        let ping = ZokaMessage::Ping(PingMessage {
            nonce: 42,
            timestamp: 99,
        });

        assert!(node.preserve_pumped_message(relay, author, ping));
        let preserved = node
            .next_sourced_message()
            .await
            .expect("queued message must be returned without waiting on swarm");
        assert_eq!(preserved.source, Some(author));
        assert!(matches!(
            preserved.message,
            ZokaMessage::Ping(PingMessage { nonce: 42, .. })
        ));
    }

    #[tokio::test]
    async fn consensus_invalid_peer_is_quarantined_from_future_block_sync() {
        let mut node = Node::new(0).await.expect("node");
        let bad_source = PeerId::random();
        let block = SourcedZokaMessage {
            source: Some(bad_source),
            message: ZokaMessage::BlockResponse(BlockResponseMessage {
                block_height: 9,
                encrypted_block: b"invalid-branch".to_vec(),
                block_hash: "bad-9".to_string(),
                difficulty_bits: None,
                private_tx_bodies: Vec::new(),
                timestamp: 99,
            }),
        };
        assert!(node.enqueue_inbound_message(block.clone()));

        node.penalize_block_sync_peer(bad_source);

        assert!(node.is_block_sync_peer_quarantined(&bad_source));
        assert!(node.pop_inbound_message().is_none());
        assert!(!node.enqueue_inbound_message(block));

        // Quarantine is scoped to consensus data; the connection can still
        // relay harmless control traffic for the rest of the mesh.
        assert!(node.enqueue_inbound_message(SourcedZokaMessage {
            source: Some(bad_source),
            message: ZokaMessage::Ping(PingMessage {
                nonce: 7,
                timestamp: 99,
            }),
        }));
        assert!(matches!(
            node.pop_inbound_message().map(|message| message.message),
            Some(ZokaMessage::Ping(PingMessage { nonce: 7, .. }))
        ));
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
    fn test_private_tx_announce_uses_general_topic() {
        let message = ZokaMessage::PrivateTxAnnounce(PrivateTxAnnounceMessage {
            tx_hash: "abcdef0123456789".to_string(),
            body_size_bytes: Some(512),
            private_proof_profile: crate::private_wallet::PRIVATE_PROOF_PROFILE_V1.to_string(),
            timestamp: 1,
        });
        assert_eq!(Node::topic_name_for_message(&message), GENERAL_TOPIC_NAME);
    }

    #[test]
    fn test_private_tx_announce_validation_rejects_bad_payloads() {
        let valid = PrivateTxAnnounceMessage {
            tx_hash: "abcdef0123456789".to_string(),
            body_size_bytes: Some(512),
            private_proof_profile: crate::private_wallet::PRIVATE_PROOF_PROFILE_V1.to_string(),
            timestamp: 1,
        };
        assert!(Node::is_private_tx_announce_well_formed(&valid));

        let mut invalid_hash = valid.clone();
        invalid_hash.tx_hash = "not valid!".to_string();
        assert!(!Node::is_private_tx_announce_well_formed(&invalid_hash));

        let mut invalid_size = valid;
        invalid_size.body_size_bytes = Some(0);
        assert!(!Node::is_private_tx_announce_well_formed(&invalid_size));
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
            private_proof_version: crate::private_wallet::PRIVATE_PROOF_VERSION_V1,
            private_proof_profile: crate::private_wallet::PRIVATE_PROOF_PROFILE_V1.to_string(),
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
            private_proof_version: crate::private_wallet::PRIVATE_PROOF_VERSION_V1,
            private_proof_profile: crate::private_wallet::PRIVATE_PROOF_PROFILE_V1.to_string(),
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

    #[test]
    fn test_kademlia_peer_from_multiaddr_requires_p2p_suffix() {
        let peer_id = PeerId::random();
        let with_peer: Multiaddr = format!("/ip4/127.0.0.1/tcp/28080/p2p/{peer_id}")
            .parse()
            .expect("multiaddr with peer");
        let (parsed_peer, dial_addr) =
            Node::kademlia_peer_from_multiaddr(&with_peer).expect("kademlia peer");
        assert_eq!(parsed_peer, peer_id);
        assert_eq!(dial_addr.to_string(), "/ip4/127.0.0.1/tcp/28080");

        let without_peer: Multiaddr = "/ip4/127.0.0.1/tcp/28080"
            .parse()
            .expect("multiaddr without peer");
        assert!(Node::kademlia_peer_from_multiaddr(&without_peer).is_none());
    }

    fn identify_info_for(protocols: &[&'static str], listen_addrs: &[&str]) -> identify::Info {
        identify::Info {
            public_key: libp2p::identity::Keypair::generate_ed25519().public(),
            protocol_version: PROTOCOL_FREEZE_ID.to_string(),
            agent_version: "zoka-test".to_string(),
            listen_addrs: listen_addrs
                .iter()
                .map(|a| a.parse().expect("listen addr"))
                .collect(),
            protocols: protocols
                .iter()
                .map(|p| libp2p::StreamProtocol::new(p))
                .collect(),
            observed_addr: "/ip4/203.0.113.9/tcp/1".parse().expect("observed addr"),
        }
    }

    fn kademlia_knows(node: &mut Node, peer: &PeerId) -> bool {
        node.swarm
            .behaviour_mut()
            .kademlia
            .kbuckets()
            .any(|bucket| bucket.iter().any(|entry| entry.node.key.preimage() == peer))
    }

    /// Regression: the routing table was empty on every node in the network.
    /// The only code path that added addresses required the multiaddr to carry
    /// an explicit `/p2p/<peer-id>`, which the network manifest's seed entries
    /// don't, so DHT discovery never worked at all and peer-finding fell back
    /// entirely to the seed dial, gossipsub and the persisted peerstore.
    #[tokio::test]
    async fn identify_populates_kademlia_routing_table_without_p2p_suffix() {
        let mut node = Node::new(0).await.expect("node");
        let peer = PeerId::random();
        let info = identify_info_for(&["/ipfs/kad/1.0.0"], &["/ip4/203.0.113.5/tcp/28080"]);

        node.record_identify_peer(peer, &info);

        assert!(
            kademlia_knows(&mut node, &peer),
            "a peer identified over a live connection must enter the routing table \
             even though its announced address carries no /p2p/ suffix"
        );
    }

    #[tokio::test]
    async fn identify_skips_kademlia_for_peers_that_do_not_speak_it() {
        // Adding a non-Kademlia peer would put an entry in the table that can
        // never answer a query.
        let mut node = Node::new(0).await.expect("node");
        let peer = PeerId::random();
        let info = identify_info_for(&["/zoka/messages/v1"], &["/ip4/203.0.113.6/tcp/28080"]);

        node.record_identify_peer(peer, &info);

        assert!(!kademlia_knows(&mut node, &peer));
    }

    #[tokio::test]
    async fn identify_does_not_route_unreachable_private_addresses() {
        // A seed announcing a datacenter-internal interface must not have that
        // address propagated through the DHT to everyone else.
        let mut node = Node::new(0).await.expect("node");
        let peer = PeerId::random();
        let info = identify_info_for(&["/ipfs/kad/1.0.0"], &["/ip4/10.10.0.5/tcp/28080"]);

        node.record_identify_peer(peer, &info);

        assert!(!kademlia_knows(&mut node, &peer));
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
    async fn test_tor_socks_transport_dials_onion_via_mock_proxy() {
        let (proxy_endpoint, proxy_task) = spawn_mock_socks5_proxy().await;
        let mut transport = TorSocksTransport::new(Some(proxy_endpoint));
        let onion_addr: Multiaddr =
            "/onion3/vww6ybal4bd7szmgncyruucpgfkqahzddi37ktceo3ah7ngmcopnpyyd:1234"
                .parse()
                .expect("parse onion3");

        let stream = Transport::dial(&mut transport, onion_addr)
            .expect("onion dial should be supported")
            .await
            .expect("mock socks dial should succeed");
        drop(stream);
        let _ = proxy_task.await;
    }

    #[test]
    fn test_tor_socks_transport_rejects_direct_addr() {
        let mut transport = TorSocksTransport::new(Some("127.0.0.1:9050".to_string()));
        let direct_addr: Multiaddr = "/ip4/127.0.0.1/tcp/28080"
            .parse()
            .expect("parse direct addr");
        assert!(matches!(
            Transport::dial(&mut transport, direct_addr),
            Err(TransportError::MultiaddrNotSupported(_))
        ));
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
            private_proof_version: crate::private_wallet::PRIVATE_PROOF_VERSION_V1,
            private_proof_profile: crate::private_wallet::PRIVATE_PROOF_PROFILE_V1.to_string(),
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

    #[tokio::test]
    async fn test_remote_pinned_branch_author_prefers_gossip() {
        let node = Node::new(0).await.expect("node");
        let remote_author = PeerId::random();

        assert!(node.should_request_pinned_block_via_gossip_first(
            Some("expected-branch-hash"),
            Some(&remote_author),
        ));
        assert!(!node.should_request_pinned_block_via_gossip_first(None, Some(&remote_author)));
        assert!(
            !node.should_request_pinned_block_via_gossip_first(Some("expected-branch-hash"), None,)
        );
    }
}
