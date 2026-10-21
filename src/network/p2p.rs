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
use crate::network::private_tx_bodies_request_response::{
    protocol_name as private_tx_bodies_protocol, PrivateTxBodiesOutcome, PrivateTxBodiesRequest,
    PrivateTxBodiesRequestResponseCodec, PrivateTxBodiesResponse,
};
use crate::network::private_tx_digests_request_response::{
    protocol_name as private_tx_digests_protocol, PrivateTxDigestsOutcome,
    PrivateTxDigestsRequest, PrivateTxDigestsRequestResponseCodec, PrivateTxDigestsResponse,
};
use crate::network::wallet_digest_request_response::{
    protocol_name as wallet_digest_protocol, BlockDigest, GetWalletDigests,
    WalletDigestRequestResponseCodec, WalletDigestsResponse,
};
use crate::network::quantic_light_request_response::{
    protocol_name as quantic_light_protocol, QuanticLightCodec, QuanticLightRequest,
    QuanticLightResponse, MAX_COMPACT_BLOCKS_PER_REQUEST,
};
use crate::private_wallet::PrivateTransaction;

pub const GENERAL_TOPIC_NAME: &str = "zoka/messages/v1";
pub const PBFT_TOPIC_NAME: &str = "zoka/pbft/v1";
pub const STEM_TOPIC_NAME: &str = "zoka/stem/v1";
pub const MIXER_TOPIC_NAME: &str = "zoka/mixer/v1";
/// Team-mining coordination stays isolated from ordinary transaction gossip.
/// Older nodes never subscribe, so high-frequency shares do not become
/// decode warnings or bandwidth on nodes that did not opt into the protocol.
pub const TEAM_MINING_TOPIC_NAME: &str = "zoka/team-mining/v1";
/// Checkpoints firmados por el nodo del equipo. Tópico propio: los nodos que
/// no los entienden no se suscriben y no reciben nada.
pub const CHECKPOINTS_TOPIC_NAME: &str = "zoka/checkpoints/v1";
/// Shared outer budget for one mobile P2P data operation. The request/response
/// handler timeout below must outlive it so caller-side failover owns expiry.
pub(crate) const MOBILE_P2P_OPERATION_WAIT: Duration = Duration::from_secs(60);
pub(crate) const PUBLISH_RETRY_PAUSE: Duration = Duration::from_millis(50);
const DATA_REQUEST_RESPONSE_SLACK: Duration = Duration::from_secs(15);
const DATA_REQUEST_RESPONSE_TIMEOUT: Duration =
    MOBILE_P2P_OPERATION_WAIT.saturating_add(DATA_REQUEST_RESPONSE_SLACK);
const PRIVATE_TX_BODY_CACHE_TTL: Duration = Duration::from_secs(10 * 60);
const PRIVATE_TX_BODY_CACHE_MAX: usize = 2048;
/// Request/response events can arrive after their synchronous caller timed out.
/// Keep those events bounded until the caller gets a chance to discard them.
const MAX_DEFERRED_DIRECT_RESPONSES: usize = 256;
const MAX_DEFERRED_WALLET_DIGEST_RESPONSES: usize = 32;
/// Keep digest failover useful over Tor without letting one request walk the
/// whole peer set. Rotation still gives every peer a turn across calls.
const MAX_WALLET_DIGEST_PEER_ATTEMPTS: usize = 3;

fn push_bounded<T>(queue: &mut VecDeque<T>, value: T, max_len: usize) {
    while queue.len() >= max_len {
        queue.pop_front();
    }
    queue.push_back(value);
}

fn rotate_and_limit_wallet_digest_peers(mut peers: Vec<PeerId>, cursor: usize) -> Vec<PeerId> {
    if peers.is_empty() {
        return peers;
    }
    let rotation = cursor % peers.len();
    peers.rotate_left(rotation);
    peers.truncate(MAX_WALLET_DIGEST_PEER_ATTEMPTS);
    peers
}

fn wallet_digest_attempt_budget(remaining: Duration, candidates_left: usize) -> Duration {
    remaining
        .checked_div(candidates_left.max(1) as u32)
        .unwrap_or(remaining)
}

fn data_request_response_config() -> request_response::Config {
    request_response::Config::default().with_request_timeout(DATA_REQUEST_RESPONSE_TIMEOUT)
}

/// Lo que un par puede pedirle por minuto a `/zoka-quantic/light/1`, en
/// unidades de [`quantic_light_cost`]: 600 son doce lotes de 500 bloques
/// compactos. Un teléfono por Tor no llega ni cerca; un par que pide sin parar
/// no le come la CPU al nodo (el respondedor corre en el bucle de la red).
const QUANTIC_LIGHT_BUDGET_PER_MINUTE: u32 = 600;

/// Lo que cuesta atender un pedido: info 1, los bloques compactos uno cada 10
/// bloques, y un envío 3 por acción (verificar su prueba lleva tiempo por
/// acción, dentro del bucle de la red): uno de 2 acciones vale 6 y uno de 64,
/// casi 200, así que un par no puede frenar el nodo con pruebas falsas grandes.
fn quantic_light_cost(request: &QuanticLightRequest) -> u32 {
    match request {
        QuanticLightRequest::Info => 1,
        QuanticLightRequest::SubmitTx { tx_hex } => {
            // Una acción ocupa unos 4.255 bytes del envío (8.510 en hex).
            let actions = (tx_hex.len() / 8_510).max(2);
            3u32.saturating_mul(actions as u32)
        }
        QuanticLightRequest::Compact { from, to } => {
            let blocks = to
                .saturating_sub(*from)
                .saturating_add(1)
                .min(MAX_COMPACT_BLOCKS_PER_REQUEST);
            (blocks / 10).max(1) as u32
        }
    }
}

/// `/zoka-quantic/light/1` existe solo en las redes de Quantic: un nodo de
/// Zenith ni lo anuncia ni lo atiende. Las pruebas lo tienen siempre.
fn quantic_light_protocols() -> Vec<(libp2p::StreamProtocol, request_response::ProtocolSupport)> {
    if NETWORK_PROTOCOL.get().is_some() || cfg!(test) {
        vec![(quantic_light_protocol(), request_response::ProtocolSupport::Full)]
    } else {
        Vec::new()
    }
}
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

/// Zoka Quantic (etapa 5): cada red de Quantic anuncia su propio protocolo y
/// usa sus propios tópicos. Un nodo de Zenith desconecta a uno de Quantic en
/// el saludo inicial (y al revés), así que las redes nunca se mezclan.
pub const QUANTIC_MAINNET_PROTOCOL_ID: &str = "/zoka-quantic/1.0.0";
pub const QUANTIC_TESTNET_PROTOCOL_ID: &str = "/zoka-quantic-testnet/0.1.0";
pub const QUANTIC_REGTEST_PROTOCOL_ID: &str = "/zoka-quantic-regtest/0.1.0";
/// Lo que reemplaza a `zoka` en los nombres de los tópicos de cada red.
pub const QUANTIC_MAINNET_TOPIC_PREFIX: &str = "zoka-quantic";
pub const QUANTIC_TESTNET_TOPIC_PREFIX: &str = "zoka-quantic-testnet";
pub const QUANTIC_REGTEST_TOPIC_PREFIX: &str = "zoka-quantic-regtest";
/// Una transferencia de Quantic de 64 acciones pesa unos 270 KB; en hex, el doble.
const QUANTIC_TX_HEX_MAX: usize = 2 * 300 * 1024;

/// El protocolo y el prefijo de tópicos de una red de Quantic por su nombre
/// (`quantic-testnet`, `quantic-regtest`), para quien no es el binario del
/// nodo (la wallet del teléfono). Zenith no tiene: es la red por omisión.
pub fn quantic_network_protocol(name: &str) -> Option<(&'static str, &'static str)> {
    match name {
        "quantic-mainnet" => Some((QUANTIC_MAINNET_PROTOCOL_ID, QUANTIC_MAINNET_TOPIC_PREFIX)),
        "quantic-testnet" => Some((QUANTIC_TESTNET_PROTOCOL_ID, QUANTIC_TESTNET_TOPIC_PREFIX)),
        "quantic-regtest" => Some((QUANTIC_REGTEST_PROTOCOL_ID, QUANTIC_REGTEST_TOPIC_PREFIX)),
        _ => None,
    }
}

/// Protocolo y prefijo de tópicos de la red de este proceso. Sin fijar, los
/// de Zenith. Lo fija el binario una sola vez, antes de crear el nodo.
static NETWORK_PROTOCOL: std::sync::OnceLock<(&'static str, &'static str)> = std::sync::OnceLock::new();

/// Elige la red de este proceso: `protocol_id` para el saludo y
/// `topic_prefix` en lugar de `zoka` en los tópicos. Devuelve `false` si ya
/// estaba elegida otra.
pub fn set_network_protocol(protocol_id: &'static str, topic_prefix: &'static str) -> bool {
    NETWORK_PROTOCOL.set((protocol_id, topic_prefix)).is_ok()
        || NETWORK_PROTOCOL.get() == Some(&(protocol_id, topic_prefix))
}

/// Si este proceso ya eligió una red de Quantic (después no puede volver a
/// Zenith sin reiniciarse).
pub fn network_protocol_is_set() -> bool {
    NETWORK_PROTOCOL.get().is_some()
}

/// El protocolo que este proceso anuncia y exige.
pub fn active_protocol_id() -> &'static str {
    NETWORK_PROTOCOL.get().map(|(id, _)| *id).unwrap_or(PROTOCOL_FREEZE_ID)
}

fn topic_name(zenith_name: &str) -> String {
    match NETWORK_PROTOCOL.get() {
        Some((_, prefix)) => format!("{prefix}/{}", zenith_name.trim_start_matches("zoka/")),
        None => zenith_name.to_string(),
    }
}

fn advertised_protocol_version(protocol_freeze_enforce: bool) -> &'static str {
    if NETWORK_PROTOCOL.get().is_some() {
        // Las redes de Quantic no tienen versiones viejas que tolerar.
        return active_protocol_id();
    }
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

/// Cuantos identificadores "ya lo vi" se recuerdan por canal de la capa de
/// anonimato.
///
/// Sirven para no reenviar dos veces el mismo lote del mixer ni la misma ruta
/// de Dandelion. Los duplicados llegan en segundos, asi que recordar decenas
/// de miles cubre de sobra la ventana util; lo que no se puede es recordarlos
/// para siempre.
const MAX_SEEN_GOSSIP_IDS: usize = 50_000;

/// Cuanto se bombea el bucle entre revisiones al esperar resumenes.
const PUMP_SLICE_DIGESTS: Duration = Duration::from_millis(200);

/// Conjunto de "ya lo vi" con techo y desalojo por antiguedad.
///
/// Reemplaza a un `HashSet` que solo crecia. Un nodo reporto 22,6 GB de RSS y
/// dos muertes por falta de memoria con 16 pares, y estos dos conjuntos --
/// lotes del mixer y rutas de Dandelion -- eran los unicos candidatos que
/// quedaban en pie tras descartar la cola de entrada (acotada) y el dataset de
/// RandomX (compartido por Arc, no por hilo): se insertaban en cada mensaje y
/// no se limpiaban nunca. Crecen con el trafico, o sea mas rapido cuantos mas
/// pares hay, que es exactamente lo que el reporte describia.
///
/// Se desaloja el mas viejo, no uno al azar: un identificador recien visto es
/// el que mas probabilidad tiene de volver a aparecer.
#[derive(Debug, Default)]
struct SeenIdSet {
    ids: HashSet<String>,
    orden: VecDeque<String>,
}

impl SeenIdSet {
    fn contains(&self, id: &str) -> bool {
        self.ids.contains(id)
    }

    /// Devuelve `true` si el identificador es nuevo, igual que `HashSet::insert`.
    fn insert(&mut self, id: String) -> bool {
        if !self.ids.insert(id.clone()) {
            return false;
        }
        self.orden.push_back(id);
        while self.orden.len() > MAX_SEEN_GOSSIP_IDS {
            if let Some(viejo) = self.orden.pop_front() {
                self.ids.remove(&viejo);
            }
        }
        true
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.ids.len()
    }
}

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

#[derive(Clone, Debug)]
struct ReceivedWalletDigestsResponse {
    request_id: request_response::OutboundRequestId,
    peer: PeerId,
    response: WalletDigestsResponse,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WalletDigestValidationError {
    MissingPrevHash,
    InvalidRange,
    BrokenChain,
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
pub(crate) enum MobileBlockProbe {
    Found(SourcedBlockResponse),
    NotFound,
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

    fn dial(
        &mut self,
        addr: Multiaddr,
        _opts: libp2p::core::transport::DialOpts,
    ) -> Result<Self::Dial, TransportError<Self::Error>> {
        // Por un proxy SOCKS no hay hole punching: el rol y el uso del puerto
        // no cambian nada.
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

    fn poll(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<TransportEvent<Self::ListenerUpgrade, Self::Error>> {
        Poll::Pending
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
    /// Non-custodial mining team coordination topic.
    team_mining_topic: gossipsub::IdentTopic,
    /// Signed checkpoints from the team's authority node.
    checkpoints_topic: gossipsub::IdentTopic,
    /// Network privacy policy knobs.
    privacy_config: NetworkPrivacyConfig,
    /// Pending inbound messages generated by local fanout/interception.
    inbound_queue: VecDeque<SourcedZokaMessage>,
    /// Dedup keys for high-volume synchronization messages in `inbound_queue`.
    inbound_queue_keys: HashSet<String>,
    /// Pending local private txs waiting to be batched.
    mixer_batcher: MixerBatcher,
    /// Dedup cache for processed mixer batches.
    seen_mixer_batches: SeenIdSet,
    /// Per-peer rate limit buckets for mixer batches.
    mixer_peer_buckets: HashMap<PeerId, (u64, u32)>,
    /// Lo que pidió cada par a `/zoka-quantic/light/1` en el minuto en curso.
    quantic_light_peer_buckets: HashMap<PeerId, (u64, u32)>,
    /// Dedup cache for processed stem routes.
    seen_stem_routes: SeenIdSet,
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
    /// Altura mas alta anunciada por algun par desde que arranco el nodo.
    ///
    /// Es una observacion pasiva: anotarla no cambia ninguna decision del nodo
    /// completo, que sigue derivando su objetivo de sincronizacion como
    /// siempre. Existe para el cliente movil, que no guarda cadena y necesita
    /// saber hasta donde escanear sin preguntarle la altura a ningun servidor.
    highest_announced_height: u64,
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
    /// Digest requests that are still owned by the active sequential failover.
    /// Removing an id marks late libp2p events as stale so the handler drops them.
    pending_outbound_private_tx_bodies_requests:
        HashMap<request_response::OutboundRequestId, PeerId>,
    received_private_tx_bodies: VecDeque<(PeerId, PrivateTxBodiesResponse)>,
    private_tx_bodies_responder:
        Option<Box<dyn Fn(&PrivateTxBodiesRequest) -> PrivateTxBodiesResponse + Send>>,
    pending_outbound_private_tx_digests_requests:
        HashMap<request_response::OutboundRequestId, PeerId>,
    received_private_tx_digests: VecDeque<(PeerId, PrivateTxDigestsResponse)>,
    private_tx_digests_responder:
        Option<Box<dyn Fn(&PrivateTxDigestsRequest) -> PrivateTxDigestsResponse + Send>>,
    pending_outbound_quantic_light_requests: HashMap<request_response::OutboundRequestId, PeerId>,
    /// Respuestas de wallet liviana de Quantic, con el pedido que contestan.
    received_quantic_light: VecDeque<(request_response::OutboundRequestId, QuanticLightResponse)>,
    #[allow(clippy::type_complexity)]
    quantic_light_responder: Option<Box<dyn Fn(&QuanticLightRequest) -> QuanticLightResponse + Send>>,
    pending_outbound_wallet_digest_requests: HashMap<request_response::OutboundRequestId, PeerId>,
    /// Resumenes de bloque recibidos, esperando a que alguien los recoja.
    received_wallet_digests: VecDeque<ReceivedWalletDigestsResponse>,
    /// Rotates the first digest provider between chunks without fanning a chunk
    /// out to every connected peer.
    wallet_digest_peer_cursor: usize,
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
    #[allow(clippy::type_complexity)]
    wallet_digest_responder: Option<Box<dyn Fn(&GetWalletDigests) -> WalletDigestsResponse + Send>>,
    /// Para qué se usa este nodo: una wallet no guarda la cadena.
    profile: NodeProfile,
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
    private_tx_bodies_request_response:
        request_response::Behaviour<PrivateTxBodiesRequestResponseCodec>,
    /// Resúmenes de transacciones privadas para wallets livianas.
    private_tx_digests_request_response:
        request_response::Behaviour<PrivateTxDigestsRequestResponseCodec>,
    wallet_digest_request_response: request_response::Behaviour<WalletDigestRequestResponseCodec>,
    /// Zoka Quantic para wallets livianas (vacío fuera de las redes de Quantic).
    quantic_light_request_response: request_response::Behaviour<QuanticLightCodec>,
    /// Snapshot manifest discovery.
    snapshot_request_response: request_response::Behaviour<SnapshotRequestResponseCodec>,
    /// Liveness keepalive. Without it, a quiet connection over Tor idles until
    /// the circuit is torn down (tor.log: "assuming established circuits no
    /// longer work"), dropping the only peer link between two NAT'd home nodes.
    /// Periodic pings keep the connection — and its Tor circuit — warm.
    ping: libp2p::ping::Behaviour,
}

/// Si el sistema tiene una configuracion de DNS que libp2p pueda leer.
///
/// En Unix eso es `/etc/resolv.conf`, que Android no trae: sus consultas van
/// por el sistema, no por un archivo. Windows la guarda en el registro y
/// siempre esta. Se comprueba el archivo en vez de mirar el sistema operativo
/// porque hay Linux embebidos que tampoco lo tienen.
fn dns_del_sistema_disponible() -> bool {
    if cfg!(windows) {
        return true;
    }
    std::path::Path::new("/etc/resolv.conf").exists()
}

/// Para que se conecta a la red quien esta usando este nodo.
///
/// Un nodo completo participa de todo: consenso, ofertas del intercambio,
/// mineria en equipo. Una wallet en un telefono solo quiere saber su saldo y
/// publicar sus transacciones -- suscribirla al resto significa recibir y
/// **reenviar** mensajes que va a descartar, y en un telefono con Tor eso son
/// datos, bateria y circuitos gastados en nada.
///
/// No saca codigo: el crate es el mismo. Solo deja de pedir lo que no va a
/// mirar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NodeProfile {
    /// Todo: consenso y mineria en equipo.
    #[default]
    Full,
    /// Lo justo para una wallet: bloques, transacciones privadas y su
    /// transporte con privacidad.
    Wallet,
}

impl NodeProfile {
    fn subscribes_to_consensus_and_team_mining(&self) -> bool {
        matches!(self, NodeProfile::Full)
    }
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

    fn load_or_generate_identity_key(
        override_path: Option<PathBuf>,
    ) -> Result<identity::Keypair, Box<dyn Error>> {
        let ruta = match override_path {
            Some(ruta) => Some(ruta),
            None => Self::node_identity_path(),
        };
        let Some(path) = ruta else {
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
        version == active_protocol_id()
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
            ZokaMessage::TeamAdvertise(advertisement) => {
                advertisement
                    .validate()
                    .map_err(|_| "invalid team advertisement")?;
            }
            ZokaMessage::TeamJoin(membership) => {
                membership
                    .validate()
                    .map_err(|_| "invalid team membership request")?;
            }
            ZokaMessage::ShareSubmit(share) => {
                // The coordinator performs the expensive PoW recomputation
                // against its current template. The transport gate only
                // rejects malformed payloads before they are relayed.
                share
                    .validate_shape()
                    .map_err(|_| "invalid team share shape")?;
            }
            ZokaMessage::TurnUpdate(update) => {
                update.validate().map_err(|_| "invalid team turn update")?;
            }
            ZokaMessage::SignedCheckpoint(checkpoint) => {
                // Se verifica la firma antes de reenviar: sin la clave del
                // equipo un checkpoint no pasa de este nodo. Mientras convivan
                // las versiones 1 y 2, la que no es la de esta cadena no es un
                // ataque: sigue de largo y el nodo la descarta sin ruido.
                match checkpoint.verify_for_declared_chain() {
                    Ok(())
                    | Err(
                        crate::checkpoint_authority::CheckpointError::HybridRequired
                        | crate::checkpoint_authority::CheckpointError::NoHybridAuthority,
                    ) => {}
                    Err(_) => return Err("invalid signed checkpoint"),
                }
            }
            ZokaMessage::QuanticTransaction { tx_hex } => {
                // Solo la forma: la prueba y las reglas las revisa el mempool,
                // que es quien decide si la retransmite al minar.
                if NETWORK_PROTOCOL.get().is_none() {
                    return Err("quantic transaction on a non-Quantic network");
                }
                if tx_hex.is_empty()
                    || tx_hex.len() > QUANTIC_TX_HEX_MAX
                    || tx_hex.len() % 2 != 0
                    || !tx_hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
                {
                    return Err("malformed quantic transaction");
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

    /// Descuenta `cost` del presupuesto del minuto de `peer`; `false` si no le alcanza.
    fn allow_quantic_light_from_peer(&mut self, peer: &PeerId, cost: u32) -> bool {
        let current_minute = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            / 60;
        self.allow_quantic_light_in_minute(peer, cost, current_minute)
    }

    fn allow_quantic_light_in_minute(&mut self, peer: &PeerId, cost: u32, current_minute: u64) -> bool {
        // Solo importa el minuto en curso: lo viejo se tira para que la tabla
        // no crezca con cada par que pasó.
        if self.quantic_light_peer_buckets.len() > 1024 {
            self.quantic_light_peer_buckets
                .retain(|_, bucket| bucket.0 == current_minute);
        }
        let bucket = self
            .quantic_light_peer_buckets
            .entry(*peer)
            .or_insert((current_minute, 0));
        if bucket.0 != current_minute {
            *bucket = (current_minute, 0);
        }
        if bucket.1.saturating_add(cost) > QUANTIC_LIGHT_BUDGET_PER_MINUTE {
            return false;
        }
        bucket.1 += cost;
        true
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
        listen_port: u16,
        tor_proxy: Option<String>,
    ) -> Result<Self, Box<dyn Error>> {
        Self::new_with_tor_proxy_at(listen_port, tor_proxy, None).await
    }

    /// Igual que [`Node::new_with_tor_proxy`], pero permite decirle donde
    /// guardar la identidad de red.
    ///
    /// Por defecto va a `.zoka/node_key`, **relativa al directorio de trabajo**.
    /// En un servidor eso es el directorio del nodo y funciona; en Android el
    /// directorio de trabajo es `/`, que es de solo lectura, y el nodo ni
    /// siquiera llega a arrancar: falla con "Read-only file system". Una
    /// aplicacion tiene que poder decir donde escribe, porque el unico lugar al
    /// que puede es su propio directorio privado.
    pub async fn new_with_tor_proxy_at(
        listen_port: u16,
        tor_proxy: Option<String>,
        identity_path: Option<PathBuf>,
    ) -> Result<Self, Box<dyn Error>> {
        Self::new_with_profile(listen_port, tor_proxy, identity_path, NodeProfile::Full).await
    }

    /// Igual, pero eligiendo a que se suscribe segun para que se use.
    pub async fn new_with_profile(
        _listen_port: u16,
        tor_proxy: Option<String>,
        identity_path: Option<PathBuf>,
        profile: NodeProfile,
    ) -> Result<Self, Box<dyn Error>> {
        let local_key = Self::load_or_generate_identity_key(identity_path)?;
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

        let general_topic = gossipsub::IdentTopic::new(topic_name(GENERAL_TOPIC_NAME));
        let pbft_topic = gossipsub::IdentTopic::new(topic_name(PBFT_TOPIC_NAME));
        let stem_topic = gossipsub::IdentTopic::new(topic_name(STEM_TOPIC_NAME));
        let mixer_topic = gossipsub::IdentTopic::new(topic_name(MIXER_TOPIC_NAME));
        let team_mining_topic = gossipsub::IdentTopic::new(topic_name(TEAM_MINING_TOPIC_NAME));
        let checkpoints_topic = gossipsub::IdentTopic::new(topic_name(CHECKPOINTS_TOPIC_NAME));
        let general_topic_for_behaviour = general_topic.clone();
        let pbft_topic_for_behaviour = pbft_topic.clone();
        let stem_topic_for_behaviour = stem_topic.clone();
        let mixer_topic_for_behaviour = mixer_topic.clone();
        let team_mining_topic_for_behaviour = team_mining_topic.clone();
        let checkpoints_topic_for_behaviour = checkpoints_topic.clone();
        let tor_proxy_for_transport = tor_proxy.clone();

        let base_transporte = SwarmBuilder::with_existing_identity(local_key)
            .with_tokio()
            .with_tcp(
                tcp::Config::default(),
                noise::Config::new,
                yamux::Config::default,
            )?
            .with_other_transport(move |key| {
                build_tor_socks_transport(key, tor_proxy_for_transport.clone())
            })?;
        // El resolutor de DNS del sistema no existe en todas partes.
        //
        // `with_dns()` lee la configuracion del sistema al construirse: en
        // Unix, `/etc/resolv.conf`. Android no tiene ese archivo, asi que la
        // creacion entera del swarm moria con "No such file or directory"
        // antes de tocar la red -- el nodo no llegaba ni a existir.
        //
        // Donde no hay configuracion se arma un resolutor vacio en vez de
        // cablear uno publico. Las semillas onion no necesitan DNS (las
        // resuelve Tor del otro lado del proxy), y mandarle las consultas a
        // un resolutor de terceros le contaria a ese tercero que este
        // telefono habla con Zoka, que es exactamente lo que el protocolo
        // existe para evitar. Se pierden las semillas `/dns4/`, que en un
        // telefono no se usan.
        let comportamiento = move |key: &identity::Keypair| {
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

            // Lo que cualquiera necesita: anuncios de bloque, transacciones
            // privadas, y los dos temas por donde una tx propia viaja con
            // privacidad.
            gossipsub.subscribe(&general_topic_for_behaviour)?;
            gossipsub.subscribe(&stem_topic_for_behaviour)?;
            gossipsub.subscribe(&mixer_topic_for_behaviour)?;
            // Consenso y mineria en equipo: una wallet no participa de nada de
            // eso, y estar suscrita la obligaria a reenviarlo.
            if profile.subscribes_to_consensus_and_team_mining() {
                gossipsub.subscribe(&pbft_topic_for_behaviour)?;
                gossipsub.subscribe(&team_mining_topic_for_behaviour)?;
                // Un nodo completo aplica los checkpoints firmados; la
                // wallet movil no valida la cadena y no los necesita.
                gossipsub.subscribe(&checkpoints_topic_for_behaviour)?;
            }
            // In libp2p-request-response 0.26.3 this timeout bounds the
            // read/write task after substream negotiation, in both directions.
            // It is separate from dialing and must exceed the mobile caller's
            // 60s deadline; otherwise the behaviour reports failure before the
            // caller can finish its own sequential failover policy.
            let block_request_response = request_response::Behaviour::with_codec(
                BlockRequestResponseCodec,
                [(
                    block_request_response_protocol(),
                    request_response::ProtocolSupport::Full,
                )],
                data_request_response_config(),
            );
            let private_tx_request_response = request_response::Behaviour::with_codec(
                PrivateTxRequestResponseCodec,
                [(
                    private_tx_request_response_protocol(),
                    request_response::ProtocolSupport::Full,
                )],
                data_request_response_config(),
            );
            let header_request_response = request_response::Behaviour::with_codec(
                HeaderRequestResponseCodec,
                [(
                    header_request_response_protocol(),
                    request_response::ProtocolSupport::Full,
                )],
                request_response::Config::default().with_request_timeout(Duration::from_secs(30)),
            );
            let private_tx_bodies_request_response = request_response::Behaviour::with_codec(
                PrivateTxBodiesRequestResponseCodec,
                [(
                    private_tx_bodies_protocol(),
                    request_response::ProtocolSupport::Full,
                )],
                // Mismo presupuesto que los resumenes: un lote de cuerpos es
                // grande y viaja por Tor, asi que el plazo corto de los pedidos
                // sueltos no alcanza.
                data_request_response_config(),
            );
            let private_tx_digests_request_response = request_response::Behaviour::with_codec(
                PrivateTxDigestsRequestResponseCodec,
                [(
                    private_tx_digests_protocol(),
                    request_response::ProtocolSupport::Full,
                )],
                data_request_response_config(),
            );
            let wallet_digest_request_response = request_response::Behaviour::with_codec(
                WalletDigestRequestResponseCodec,
                [(
                    wallet_digest_protocol(),
                    request_response::ProtocolSupport::Full,
                )],
                data_request_response_config(),
            );
            let quantic_light_request_response = request_response::Behaviour::with_codec(
                QuanticLightCodec,
                quantic_light_protocols(),
                data_request_response_config(),
            );
            let snapshot_request_response = request_response::Behaviour::with_codec(
                SnapshotRequestResponseCodec,
                [(
                    snapshot_request_response_protocol(),
                    request_response::ProtocolSupport::Full,
                )],
                request_response::Config::default().with_request_timeout(Duration::from_secs(30)),
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
                private_tx_bodies_request_response,
                private_tx_digests_request_response,
                wallet_digest_request_response,
                quantic_light_request_response,
                snapshot_request_response,
                // Default: ping every 15s, 20s timeout. Well under the
                // libp2p idle timeout (300s) and Tor's circuit idle window,
                // so the peer link stays up between sparse block gossip.
                ping: libp2p::ping::Behaviour::new(libp2p::ping::Config::new()),
            })
        };
        let configurar_swarm = |cfg: libp2p::swarm::Config| {
            // libp2p 0.53+ cierra la conexion apenas se queda sin flujos
            // activos. El sync abre un flujo por bloque con huecos entre
            // medio, y sobre Tor volver a marcar una onion cuesta muchos
            // segundos de circuito: asi la conexion se caia despues de un
            // bloque o dos y el sync quedaba clavado.
            cfg.with_idle_connection_timeout(Duration::from_secs(300))
        };

        let swarm = if dns_del_sistema_disponible() {
            base_transporte
                .with_dns()?
                .with_behaviour(comportamiento)?
                .with_swarm_config(configurar_swarm)
                .build()
        } else {
            base_transporte
                .with_dns_config(
                    libp2p::dns::ResolverConfig::from_parts(None, Vec::new(), Vec::new()),
                    libp2p::dns::ResolverOpts::default(),
                )
                .with_behaviour(comportamiento)?
                .with_swarm_config(configurar_swarm)
                .build()
        };

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
            team_mining_topic,
            checkpoints_topic,
            privacy_config,
            inbound_queue: VecDeque::new(),
            inbound_queue_keys: HashSet::new(),
            mixer_batcher: MixerBatcher::new(),
            seen_mixer_batches: SeenIdSet::default(),
            mixer_peer_buckets: HashMap::new(),
            quantic_light_peer_buckets: HashMap::new(),
            seen_stem_routes: SeenIdSet::default(),
            stem_relay_cursor: 0,
            incompatible_protocol_peers: HashSet::new(),
            protocol_freeze_enforce,
            observed_peer_addrs: HashMap::new(),
            observed_peer_protocols: HashMap::new(),
            gossip_stats: GossipStats::default(),
            pending_block_body_requests: VecDeque::new(),
            pending_outbound_block_body_requests: HashMap::new(),
            pending_block_announce_retries: HashMap::new(),
            highest_announced_height: 0,
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
            pending_outbound_private_tx_bodies_requests: HashMap::new(),
            received_private_tx_bodies: VecDeque::new(),
            private_tx_bodies_responder: None,
            pending_outbound_private_tx_digests_requests: HashMap::new(),
            received_private_tx_digests: VecDeque::new(),
            private_tx_digests_responder: None,
            pending_outbound_quantic_light_requests: HashMap::new(),
            received_quantic_light: VecDeque::new(),
            quantic_light_responder: None,
            pending_outbound_wallet_digest_requests: HashMap::new(),
            received_wallet_digests: VecDeque::new(),
            wallet_digest_peer_cursor: 0,
            pending_snapshot_requests: VecDeque::new(),
            received_snapshot_manifests: VecDeque::new(),
            block_body_responder: None,
            header_responder: None,
            wallet_digest_responder: None,
            profile,
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
            request_response::Event::Message { peer, message, .. } => match message {
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
                        // Una wallet (el teléfono) no guarda bloques: contesta
                        // enseguida que no lo tiene. Sin esto el pedido quedaba
                        // en una cola que nadie atiende, y un nodo completo cuyo
                        // único par era un teléfono esperaba el plazo entero en
                        // cada vuelta, con el bucle (y la minería) frenados.
                        None if self.profile == NodeProfile::Wallet => {
                            let response = BlockBodyResponse::NotFound {
                                block_height: request.block_height,
                                block_hash: request.block_hash.clone(),
                                reason: "this peer is a wallet and keeps no blocks".to_string(),
                            };
                            let _ = self
                                .swarm
                                .behaviour_mut()
                                .block_request_response
                                .send_response(channel, response);
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
                    let Some(pending) = self
                        .pending_outbound_block_body_requests
                        .remove(&request_id)
                    else {
                        debug!(
                            "discarding stale direct block response: request_id={request_id:?} peer={peer}"
                        );
                        return;
                    };
                    let latency = pending.started_at.elapsed();
                    let response_peer = pending.peer.clone();
                    let requested_height = pending.request.block_height;
                    if response_peer != peer {
                        warn!(
                            "direct block response peer mismatch: request_id={request_id:?} expected={} got={}",
                            response_peer, peer
                        );
                    }
                    debug!(
                        "direct block response received: request_id={request_id:?} peer={} height={} latency_ms={}",
                        response_peer,
                        requested_height,
                        latency.as_millis()
                    );
                    self.block_request_response_stats.outbound_responses = self
                        .block_request_response_stats
                        .outbound_responses
                        .saturating_add(1);
                    // Un "no tengo ese bloque" NO es una respuesta util.
                    //
                    // Se puntuaba como exito cualquier respuesta que llegara,
                    // sin mirar que decia. Un nodo recien instalado, que no
                    // tiene la altura pedida, contestaba NotFound y sumaba +10
                    // igual que uno que entrego el bloque. Como los pares se
                    // ordenan por puntaje para elegir a quien preguntarle, esos
                    // nodos subian al tope y se les volvia a preguntar primero:
                    // un bucle que se refuerza solo.
                    //
                    // El sintoma era un nodo que recibia miles de respuestas
                    // por minuto sin avanzar un bloque, porque todas eran
                    // NotFound. Con quince pares y la mayoria sincronizando
                    // todavia, la cola de preferidos se llenaba justamente con
                    // los que no podian servir nada.
                    let entrego_bloque = matches!(
                        response,
                        crate::network::block_request_response::BlockBodyResponse::Found(_)
                    );
                    if entrego_bloque {
                        self.record_direct_block_success(response_peer.clone(), latency);
                    } else {
                        self.record_direct_block_failure(response_peer.clone());
                    }
                    push_bounded(
                        &mut self.received_block_body_responses,
                        ReceivedBlockBodyResponse {
                            request_id,
                            peer: response_peer,
                            response,
                            latency,
                        },
                        MAX_DEFERRED_DIRECT_RESPONSES,
                    );
                }
            },
            request_response::Event::OutboundFailure {
                peer,
                request_id,
                error,
                ..
            } => {
                if self
                    .pending_outbound_block_body_requests
                    .remove(&request_id)
                    .is_none()
                {
                    debug!(
                        "discarding stale direct block failure: request_id={request_id:?} peer={peer}"
                    );
                    return;
                }
                self.block_request_response_stats.outbound_failures = self
                    .block_request_response_stats
                    .outbound_failures
                    .saturating_add(1);
                self.record_direct_block_failure(peer);
                push_bounded(
                    &mut self.block_body_request_failures,
                    BlockBodyRequestFailure {
                        request_id,
                        peer,
                        error: error.to_string(),
                    },
                    MAX_DEFERRED_DIRECT_RESPONSES,
                );
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
            request_response::Event::Message { peer, message, .. } => match message {
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
                    let Some(pending) = self
                        .pending_outbound_private_tx_body_requests
                        .remove(&request_id)
                    else {
                        debug!(
                            "discarding stale private tx response: request_id={request_id:?} peer={peer}"
                        );
                        return;
                    };
                    let latency = pending.started_at.elapsed();
                    let response_peer = pending.peer.clone();
                    debug!(
                        "direct private tx response received: request_id={request_id:?} peer={} hash={} latency_ms={}",
                        response_peer,
                        pending.request.tx_hash,
                        latency.as_millis()
                    );
                    self.private_tx_request_response_stats.outbound_responses = self
                        .private_tx_request_response_stats
                        .outbound_responses
                        .saturating_add(1);
                    self.record_private_tx_request_success(response_peer.clone(), latency);
                    push_bounded(
                        &mut self.received_private_tx_body_responses,
                        ReceivedPrivateTxBodyResponse {
                            request_id,
                            peer: response_peer,
                            response,
                            latency,
                        },
                        MAX_DEFERRED_DIRECT_RESPONSES,
                    );
                }
            },
            request_response::Event::OutboundFailure {
                peer,
                request_id,
                error,
                ..
            } => {
                if self
                    .pending_outbound_private_tx_body_requests
                    .remove(&request_id)
                    .is_none()
                {
                    debug!(
                        "discarding stale private tx failure: request_id={request_id:?} peer={peer}"
                    );
                    return;
                }
                self.private_tx_request_response_stats.outbound_failures = self
                    .private_tx_request_response_stats
                    .outbound_failures
                    .saturating_add(1);
                self.record_private_tx_request_failure(peer);
                push_bounded(
                    &mut self.private_tx_body_request_failures,
                    PrivateTxBodyRequestFailure {
                        request_id,
                        peer,
                        error: error.to_string(),
                    },
                    MAX_DEFERRED_DIRECT_RESPONSES,
                );
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
            request_response::Event::Message { peer, message, .. } => match message {
                request_response::Message::Request {
                    request, channel, ..
                } => {
                    let immediate = self
                        .header_responder
                        .as_ref()
                        .map(|responder| responder(&request))
                        .or_else(|| {
                            // Igual que con los bloques: una wallet no tiene cabeceras.
                            (self.profile == NodeProfile::Wallet).then(|| HeadersResponse::NotFound {
                                reason: "this peer is a wallet and keeps no blocks".to_string(),
                            })
                        });
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

    fn handle_private_tx_bodies_event(
        &mut self,
        event: request_response::Event<PrivateTxBodiesRequest, PrivateTxBodiesResponse>,
    ) {
        match event {
            request_response::Event::Message { peer, message, .. } => match message {
                request_response::Message::Request {
                    request, channel, ..
                } => {
                    // Sin responder configurado se contesta que no se tiene
                    // ninguno, en vez de dejar el pedido colgado: quien
                    // pregunta cae enseguida al protocolo de a uno en lugar de
                    // esperar el plazo completo. Una wallet no tiene cuerpos
                    // que servir y este es su caso normal.
                    let respuesta = match self.private_tx_bodies_responder.as_ref() {
                        Some(responder) => responder(&request),
                        None => PrivateTxBodiesResponse {
                            bodies: Vec::new(),
                            missing: request.tx_hashes.clone(),
                        },
                    };
                    let _ = self
                        .swarm
                        .behaviour_mut()
                        .private_tx_bodies_request_response
                        .send_response(channel, respuesta);
                }
                request_response::Message::Response {
                    request_id,
                    response,
                } => {
                    self.pending_outbound_private_tx_bodies_requests
                        .remove(&request_id);
                    self.received_private_tx_bodies.push_back((peer, response));
                }
            },
            request_response::Event::OutboundFailure { request_id, .. } => {
                self.pending_outbound_private_tx_bodies_requests
                    .remove(&request_id);
            }
            _ => {}
        }
    }

    /// Quien sirve los cuerpos cuando alguien los pide en lote.
    pub fn set_private_tx_bodies_responder(
        &mut self,
        responder: Box<dyn Fn(&PrivateTxBodiesRequest) -> PrivateTxBodiesResponse + Send>,
    ) {
        self.private_tx_bodies_responder = Some(responder);
    }

    /// Pide varios cuerpos de una sola vez.
    ///
    /// Devuelve `None` si el par no esta conectado, para que quien llama pruebe
    /// con otro en vez de esperar un plazo que no va a llegar.
    pub fn request_private_tx_bodies(
        &mut self,
        peer: &PeerId,
        tx_hashes: Vec<String>,
    ) -> Option<request_response::OutboundRequestId> {
        if !self.swarm.is_connected(peer) || tx_hashes.is_empty() {
            return None;
        }
        let id = self
            .swarm
            .behaviour_mut()
            .private_tx_bodies_request_response
            .send_request(peer, PrivateTxBodiesRequest { tx_hashes });
        self.pending_outbound_private_tx_bodies_requests
            .insert(id, *peer);
        Some(id)
    }

    /// Saca una respuesta de cuerpos en lote ya recibida, si hay.
    pub fn take_private_tx_bodies_response(&mut self) -> Option<(PeerId, PrivateTxBodiesResponse)> {
        self.received_private_tx_bodies.pop_front()
    }

    fn handle_private_tx_digests_event(
        &mut self,
        event: request_response::Event<PrivateTxDigestsRequest, PrivateTxDigestsResponse>,
    ) {
        match event {
            request_response::Event::Message { peer, message, .. } => match message {
                request_response::Message::Request {
                    request, channel, ..
                } => {
                    // Igual que el lote de cuerpos: sin responder (una wallet
                    // no guarda la cadena) se contesta que no hay ninguno, para
                    // que quien pregunta pase a otro par enseguida.
                    let respuesta = match self.private_tx_digests_responder.as_ref() {
                        Some(responder) => responder(&request),
                        None => PrivateTxDigestsResponse {
                            missing: request.tx_hashes.clone(),
                            ..Default::default()
                        },
                    };
                    let _ = self
                        .swarm
                        .behaviour_mut()
                        .private_tx_digests_request_response
                        .send_response(channel, respuesta);
                }
                request_response::Message::Response {
                    request_id,
                    response,
                } => {
                    // Una respuesta que llega después de que el pedido se dio
                    // por perdido no le sirve a nadie y no se encola.
                    if self
                        .pending_outbound_private_tx_digests_requests
                        .remove(&request_id)
                        .is_some()
                    {
                        self.received_private_tx_digests.push_back((peer, response));
                    }
                }
            },
            request_response::Event::OutboundFailure { request_id, .. } => {
                self.pending_outbound_private_tx_digests_requests
                    .remove(&request_id);
            }
            _ => {}
        }
    }

    /// Quien sirve los resúmenes de transacciones privadas.
    pub fn set_private_tx_digests_responder(
        &mut self,
        responder: Box<dyn Fn(&PrivateTxDigestsRequest) -> PrivateTxDigestsResponse + Send>,
    ) {
        self.private_tx_digests_responder = Some(responder);
    }

    /// Pide resúmenes a un par. `None` si no está conectado.
    pub fn request_private_tx_digests(
        &mut self,
        peer: &PeerId,
        tx_hashes: Vec<String>,
    ) -> Option<request_response::OutboundRequestId> {
        if !self.swarm.is_connected(peer) || tx_hashes.is_empty() {
            return None;
        }
        let id = self
            .swarm
            .behaviour_mut()
            .private_tx_digests_request_response
            .send_request(peer, PrivateTxDigestsRequest { tx_hashes });
        self.pending_outbound_private_tx_digests_requests
            .insert(id, *peer);
        Some(id)
    }

    /// Saca una respuesta de resúmenes ya recibida, si hay.
    pub fn take_private_tx_digests_response(
        &mut self,
    ) -> Option<(PeerId, PrivateTxDigestsResponse)> {
        self.received_private_tx_digests.pop_front()
    }

    fn handle_quantic_light_event(
        &mut self,
        event: request_response::Event<QuanticLightRequest, QuanticLightResponse>,
    ) {
        match event {
            request_response::Event::Message { peer, message, .. } => match message {
                request_response::Message::Request {
                    request, channel, ..
                } => {
                    // Un nodo sin la cadena (una wallet) contesta igual, para
                    // que el teléfono pase enseguida a otro par.
                    let respuesta = if self.quantic_light_responder.is_none() {
                        QuanticLightResponse::Error {
                            message: "this node does not serve Zoka Quantic light wallets".to_string(),
                        }
                    } else if !self.allow_quantic_light_from_peer(&peer, quantic_light_cost(&request)) {
                        QuanticLightResponse::Error {
                            message: "rate limited: ask again in a minute".to_string(),
                        }
                    } else {
                        let responder = self.quantic_light_responder.as_ref().expect("checked above");
                        responder(&request)
                    };
                    let _ = self
                        .swarm
                        .behaviour_mut()
                        .quantic_light_request_response
                        .send_response(channel, respuesta);
                }
                request_response::Message::Response {
                    request_id,
                    response,
                } => {
                    if self
                        .pending_outbound_quantic_light_requests
                        .remove(&request_id)
                        .is_some()
                    {
                        self.received_quantic_light.push_back((request_id, response));
                    }
                }
            },
            request_response::Event::OutboundFailure { request_id, .. } => {
                self.pending_outbound_quantic_light_requests
                    .remove(&request_id);
            }
            _ => {}
        }
    }

    /// Quien atiende a las wallets livianas de Quantic (un nodo con la cadena).
    pub fn set_quantic_light_responder(
        &mut self,
        responder: Box<dyn Fn(&QuanticLightRequest) -> QuanticLightResponse + Send>,
    ) {
        self.quantic_light_responder = Some(responder);
    }

    /// Le hace a `peer` un pedido de wallet liviana de Quantic y espera la
    /// respuesta hasta `max_wait`. `None` si no está conectado, no habla el
    /// protocolo o no contestó a tiempo; una respuesta tardía se descarta.
    pub async fn request_quantic_light(
        &mut self,
        peer: &PeerId,
        request: QuanticLightRequest,
        max_wait: Duration,
    ) -> Option<QuanticLightResponse> {
        if !self.swarm.is_connected(peer) {
            return None;
        }
        let id = self
            .swarm
            .behaviour_mut()
            .quantic_light_request_response
            .send_request(peer, request);
        self.pending_outbound_quantic_light_requests.insert(id, *peer);
        let limite = Instant::now() + max_wait;
        while Instant::now() < limite {
            let _ = self.pump_for(Duration::from_millis(200)).await;
            if let Some(pos) = self
                .received_quantic_light
                .iter()
                .position(|(respondido, _)| *respondido == id)
            {
                return self.received_quantic_light.remove(pos).map(|(_, respuesta)| respuesta);
            }
            if !self.pending_outbound_quantic_light_requests.contains_key(&id) {
                // Falló (el par se fue o no habla el protocolo) sin respuesta.
                return None;
            }
        }
        self.pending_outbound_quantic_light_requests.remove(&id);
        None
    }

    /// El mismo pedido a los pares conectados, de a uno, hasta que alguno
    /// conteste algo que no sea un error. Devuelve quién contestó.
    pub async fn request_quantic_light_from_peers(
        &mut self,
        request: QuanticLightRequest,
        max_wait_per_peer: Duration,
    ) -> Option<(PeerId, QuanticLightResponse)> {
        let peers: Vec<PeerId> = self.swarm.connected_peers().copied().collect();
        for peer in peers {
            match self
                .request_quantic_light(&peer, request.clone(), max_wait_per_peer)
                .await
            {
                None | Some(QuanticLightResponse::Error { .. }) => continue,
                Some(response) => return Some((peer, response)),
            }
        }
        None
    }

    fn handle_wallet_digest_event(
        &mut self,
        event: request_response::Event<GetWalletDigests, WalletDigestsResponse>,
    ) {
        match event {
            request_response::Event::Message { peer, message, .. } => match message {
                request_response::Message::Request {
                    request, channel, ..
                } => {
                    // Sin responder configurado se contesta que no, en vez de
                    // dejar el pedido colgado: quien pregunta cae enseguida al
                    // protocolo de bloques enteros en lugar de esperar el
                    // tiempo de espera completo. Un cliente liviano no tiene
                    // cadena que servir y este es su caso normal.
                    let respuesta = match self.wallet_digest_responder.as_ref() {
                        Some(responder) => responder(&request),
                        None => WalletDigestsResponse::NotFound {
                            reason: "este par no sirve resumenes".to_string(),
                        },
                    };
                    let _ = self
                        .swarm
                        .behaviour_mut()
                        .wallet_digest_request_response
                        .send_response(channel, respuesta);
                }
                request_response::Message::Response {
                    request_id,
                    response,
                } => {
                    let Some(expected_peer) = self
                        .pending_outbound_wallet_digest_requests
                        .remove(&request_id)
                    else {
                        debug!(
                            "discarding stale wallet digest response: request_id={request_id:?} peer={peer}"
                        );
                        return;
                    };
                    if expected_peer != peer {
                        warn!(
                            "wallet digest response peer mismatch: request_id={request_id:?} expected={expected_peer} got={peer}"
                        );
                    }
                    push_bounded(
                        &mut self.received_wallet_digests,
                        ReceivedWalletDigestsResponse {
                            request_id,
                            peer,
                            response,
                        },
                        MAX_DEFERRED_WALLET_DIGEST_RESPONSES,
                    );
                }
            },
            request_response::Event::OutboundFailure {
                peer,
                request_id,
                error,
                ..
            } => {
                if self
                    .pending_outbound_wallet_digest_requests
                    .remove(&request_id)
                    .is_none()
                {
                    debug!(
                        "discarding stale wallet digest failure: request_id={request_id:?} peer={peer}"
                    );
                    return;
                }
                warn!("wallet digest outbound failure from {}: {}", peer, error);
                // Un fallo tiene que llegar a quien espera, si no se queda
                // esperando algo que nunca va a venir en vez de caer al
                // camino lento. Un par viejo que no conoce el protocolo entra
                // por aca.
                push_bounded(
                    &mut self.received_wallet_digests,
                    ReceivedWalletDigestsResponse {
                        request_id,
                        peer,
                        response: WalletDigestsResponse::NotFound {
                            reason: format!("el par no respondio: {error}"),
                        },
                    },
                    MAX_DEFERRED_WALLET_DIGEST_RESPONSES,
                );
            }
            request_response::Event::InboundFailure { peer, error, .. } => {
                warn!("wallet digest inbound failure from {}: {}", peer, error);
            }
            request_response::Event::ResponseSent { .. } => {}
        }
    }

    /// Quien responde los pedidos de resumen. Lo cablea el nodo completo a su
    /// almacenamiento; un cliente liviano lo deja sin poner.
    #[allow(clippy::type_complexity)]
    pub fn set_wallet_digest_responder(
        &mut self,
        responder: Box<dyn Fn(&GetWalletDigests) -> WalletDigestsResponse + Send>,
    ) {
        self.wallet_digest_responder = Some(responder);
    }

    /// Pide los resumenes de un tramo y espera la respuesta.
    ///
    /// Devuelve `None` si ningun par contesto a tiempo o si el que contesto no
    /// sirve resumenes. Quien llama tiene que caer al protocolo de bloques
    /// enteros en ese caso: los pares viejos no conocen este mensaje y la
    /// wallet tiene que seguir funcionando con ellos.
    fn validate_wallet_digest_range(
        mut digests: Vec<BlockDigest>,
        from_height: u64,
        to_height: u64,
        expected_prev_hash: Option<&str>,
    ) -> Result<Vec<BlockDigest>, WalletDigestValidationError> {
        let Some(expected_count) = to_height
            .checked_sub(from_height)
            .and_then(|distance| distance.checked_add(1))
            .and_then(|count| usize::try_from(count).ok())
        else {
            return Err(WalletDigestValidationError::InvalidRange);
        };
        if digests.len() != expected_count {
            return Err(WalletDigestValidationError::InvalidRange);
        }

        digests.sort_by_key(|digest| digest.block_height);
        let mut previous_hash = expected_prev_hash.map(str::to_owned);
        for (offset, digest) in digests.iter().enumerate() {
            let Some(expected_height) = from_height.checked_add(offset as u64) else {
                return Err(WalletDigestValidationError::InvalidRange);
            };
            if digest.block_height != expected_height || digest.block_hash.trim().is_empty() {
                return Err(WalletDigestValidationError::InvalidRange);
            }
            // `prev_hash` was added compatibly to protocol v1. An empty value
            // means the remote peer encoded the older v1 shape: do not confuse
            // that with a valid unanchored one-block response.
            if digest.prev_hash.trim().is_empty() {
                return Err(WalletDigestValidationError::MissingPrevHash);
            }
            if let Some(previous_hash) = previous_hash.as_deref() {
                if digest.prev_hash != previous_hash {
                    return Err(WalletDigestValidationError::BrokenChain);
                }
            }
            previous_hash = Some(digest.block_hash.clone());
        }
        Ok(digests)
    }

    #[cfg(test)]
    pub(crate) fn wallet_digests_cover_range(
        digests: &[BlockDigest],
        from_height: u64,
        to_height: u64,
    ) -> bool {
        Self::validate_wallet_digest_range(digests.to_vec(), from_height, to_height, None).is_ok()
    }

    fn take_wallet_digest_response(
        &mut self,
        request_id: request_response::OutboundRequestId,
    ) -> Option<ReceivedWalletDigestsResponse> {
        while let Some(response) = self.received_wallet_digests.pop_front() {
            if response.request_id == request_id {
                return Some(response);
            }
            debug!(
                "discarding unmatched wallet digest response: request_id={:?}",
                response.request_id
            );
        }
        None
    }

    fn abandon_wallet_digest_request(&mut self, request_id: request_response::OutboundRequestId) {
        self.pending_outbound_wallet_digest_requests
            .remove(&request_id);
        self.received_wallet_digests
            .retain(|response| response.request_id != request_id);
    }

    /// Trae muchos cuerpos, probando pares hasta juntarlos.
    ///
    /// Devuelve los que consiguio y los que ningun par consultado tenia. Un
    /// cuerpo que nadie tiene no es un error de red: son transacciones cuyo
    /// cuerpo se perdio de la malla, y quien llama necesita distinguirlas para
    /// no reintentarlas para siempre.
    ///
    /// Se pregunta de a un par por vez y no a todos a la vez: sobre Tor cada
    /// pedido paralelo abre su circuito, y varios a la vez es justo lo que hace
    /// que ninguno termine.
    pub async fn request_private_tx_bodies_from_peers(
        &mut self,
        tx_hashes: &[String],
        max_wait: Duration,
    ) -> PrivateTxBodiesOutcome {
        use std::collections::HashSet;
        let mut faltan: HashSet<String> = tx_hashes.iter().cloned().collect();
        // Solo lo que un par dijo explicitamente que no tiene.
        //
        // Antes se devolvia "todo lo que no llego" en una sola bolsa, y quien
        // llamaba lo trataba como "nadie lo tiene". Con un par que no habla este
        // protocolo -- porque corre un binario viejo -- eso marcaba las 266
        // transacciones de la cadena como perdidas para siempre y dejaba el
        // saldo en cero. La distincion entre "no lo tengo" y "no te pude
        // contestar" estaba escrita en el protocolo y no se respetaba aca.
        let mut dichos_faltantes: HashSet<String> = HashSet::new();
        let mut conseguidos: Vec<(String, Vec<u8>)> = Vec::new();
        if faltan.is_empty() {
            return PrivateTxBodiesOutcome::default();
        }

        let limite = Instant::now() + max_wait;
        let pares = self.connected_peer_ids();
        for peer in pares {
            if faltan.is_empty() || Instant::now() >= limite {
                break;
            }
            let pedido: Vec<String> = faltan.iter().cloned().collect();
            let mut restante_del_par = pedido;
            // Un par puede tener que contestar varios lotes si faltan mas que
            // el maximo por pedido.
            while !restante_del_par.is_empty() && Instant::now() < limite {
                let este_lote: Vec<String> = restante_del_par
                    .iter()
                    .take(crate::network::private_tx_bodies_request_response::MAX_BODIES_PER_REQUEST)
                    .cloned()
                    .collect();
                if self
                    .request_private_tx_bodies(&peer, este_lote.clone())
                    .is_none()
                {
                    break;
                }
                let mut contesto = false;
                while Instant::now() < limite {
                    let _ = self.pump_for(Duration::from_millis(200)).await;
                    if let Some((_, respuesta)) = self.take_private_tx_bodies_response() {
                        for entrada in respuesta.bodies {
                            if faltan.remove(&entrada.tx_hash) {
                                conseguidos.push((entrada.tx_hash, entrada.body));
                            }
                        }
                        // Un par que contesta "no lo tengo" es informacion; que
                        // no contesta, no.
                        for hash in respuesta.missing {
                            if faltan.contains(&hash) {
                                dichos_faltantes.insert(hash);
                            }
                        }
                        contesto = true;
                        break;
                    }
                }
                if !contesto {
                    break;
                }
                restante_del_par.retain(|h| faltan.contains(h));
                restante_del_par.retain(|h| !este_lote.contains(h));
            }
        }
        // Lo que ningun par alcanzo a responder queda como "sin respuesta", no
        // como perdido: se reintenta por el camino de a uno.
        let sin_respuesta: Vec<String> = faltan
            .iter()
            .filter(|h| !dichos_faltantes.contains(*h))
            .cloned()
            .collect();
        PrivateTxBodiesOutcome {
            conseguidos,
            dichos_faltantes: dichos_faltantes.into_iter().collect(),
            sin_respuesta,
        }
    }

    /// Pide resúmenes de transacciones privadas a los pares conectados.
    ///
    /// Se le pregunta a un par por vez; lo que uno no tiene o no alcanza a
    /// contestar va al siguiente. Lo aplazado por el tope de un pedido se
    /// vuelve a pedir al mismo par. Igual que en el lote de cuerpos, "no lo
    /// tengo" y "no contestó" quedan separados.
    pub async fn request_private_tx_digests_from_peers(
        &mut self,
        tx_hashes: &[String],
        max_wait: Duration,
    ) -> PrivateTxDigestsOutcome {
        use crate::network::private_tx_digests_request_response::MAX_DIGESTS_PER_REQUEST;
        use std::collections::HashSet;

        let mut vistos = HashSet::new();
        let orden: Vec<String> = tx_hashes
            .iter()
            .map(|hash| hash.trim().to_string())
            .filter(|hash| !hash.is_empty() && vistos.insert(hash.clone()))
            .collect();
        let mut faltan: HashSet<String> = orden.iter().cloned().collect();
        let mut dichos_faltantes: HashSet<String> = HashSet::new();
        let mut conseguidos = Vec::new();
        if faltan.is_empty() {
            return PrivateTxDigestsOutcome::default();
        }

        let limite = Instant::now() + max_wait;
        for peer in self.connected_peer_ids() {
            if faltan.is_empty() || Instant::now() >= limite {
                break;
            }
            let mut por_preguntar: Vec<String> = orden
                .iter()
                .filter(|hash| faltan.contains(*hash))
                .cloned()
                .collect();
            while !por_preguntar.is_empty() && Instant::now() < limite {
                let lote: Vec<String> = por_preguntar
                    .iter()
                    .take(MAX_DIGESTS_PER_REQUEST)
                    .cloned()
                    .collect();
                let Some(id) = self.request_private_tx_digests(&peer, lote.clone()) else {
                    break;
                };
                let Some(respuesta) = self
                    .wait_private_tx_digests_response(id, &peer, limite)
                    .await
                else {
                    break;
                };
                let aplazados: HashSet<String> = respuesta.deferred.into_iter().collect();
                for digest in respuesta.digests {
                    if faltan.remove(&digest.tx_hash) {
                        dichos_faltantes.remove(&digest.tx_hash);
                        conseguidos.push(digest);
                    }
                }
                for hash in respuesta.missing {
                    if faltan.contains(&hash) {
                        dichos_faltantes.insert(hash);
                    }
                }
                let pedidos: HashSet<&String> = lote.iter().collect();
                por_preguntar.retain(|hash| {
                    faltan.contains(hash) && (!pedidos.contains(hash) || aplazados.contains(hash))
                });
            }
        }

        let sin_respuesta = orden
            .into_iter()
            .filter(|hash| faltan.contains(hash) && !dichos_faltantes.contains(hash))
            .collect();
        PrivateTxDigestsOutcome {
            conseguidos,
            dichos_faltantes: dichos_faltantes.into_iter().collect(),
            sin_respuesta,
        }
    }

    /// Espera la respuesta de un pedido de resúmenes hasta `limite`. Si no
    /// llega, lo abandona para que una respuesta tardía no se mezcle con el
    /// pedido siguiente.
    async fn wait_private_tx_digests_response(
        &mut self,
        id: request_response::OutboundRequestId,
        peer: &PeerId,
        limite: Instant,
    ) -> Option<PrivateTxDigestsResponse> {
        while Instant::now() < limite {
            let _ = self.pump_for(Duration::from_millis(200)).await;
            while let Some((de, respuesta)) = self.take_private_tx_digests_response() {
                if de == *peer {
                    return Some(respuesta);
                }
            }
            if !self
                .pending_outbound_private_tx_digests_requests
                .contains_key(&id)
            {
                // Falló (el par se fue o no habla el protocolo) sin respuesta.
                return None;
            }
        }
        self.pending_outbound_private_tx_digests_requests.remove(&id);
        None
    }

    pub async fn request_wallet_digests(
        &mut self,
        from_height: u64,
        to_height: u64,
        max_wait: Duration,
    ) -> Option<Vec<BlockDigest>> {
        self.request_wallet_digests_with_anchor(from_height, to_height, None, max_wait)
            .await
    }

    pub async fn request_wallet_digests_with_anchor(
        &mut self,
        from_height: u64,
        to_height: u64,
        expected_prev_hash: Option<&str>,
        max_wait: Duration,
    ) -> Option<Vec<BlockDigest>> {
        if from_height > to_height
            || to_height.saturating_sub(from_height)
                >= crate::network::wallet_digest_request_response::MAX_DIGESTS_PER_RESPONSE
        {
            return None;
        }
        let mut peers = self
            .select_block_request_peers()
            .into_iter()
            .filter(|peer| self.swarm.is_connected(peer))
            .collect::<Vec<_>>();
        if peers.is_empty() {
            return None;
        }
        peers = rotate_and_limit_wallet_digest_peers(peers, self.wallet_digest_peer_cursor);
        self.wallet_digest_peer_cursor = self.wallet_digest_peer_cursor.wrapping_add(1);

        // `&mut self` serializa estas llamadas. Todo lo anterior ya vencio:
        // quitar los ids hace que el handler descarte cualquier llegada tardia.
        self.pending_outbound_wallet_digest_requests.clear();
        self.received_wallet_digests.clear();
        let deadline = tokio::time::Instant::now() + max_wait;
        let peer_count = peers.len();

        for (peer_index, peer) in peers.into_iter().enumerate() {
            let now = tokio::time::Instant::now();
            if now >= deadline {
                break;
            }
            // Give every remaining peer a bounded chance. Fast NotFound or
            // invalid replies immediately hand the unused share to the next.
            let remaining = deadline.saturating_duration_since(now);
            let attempt_budget =
                wallet_digest_attempt_budget(remaining, peer_count - peer_index);
            let attempt_deadline = now + attempt_budget;
            let request_id = self
                .swarm
                .behaviour_mut()
                .wallet_digest_request_response
                .send_request(
                    &peer,
                    GetWalletDigests {
                        from_height,
                        to_height,
                    },
                );
            self.pending_outbound_wallet_digest_requests
                .insert(request_id, peer.clone());

            loop {
                if let Some(recibida) = self.take_wallet_digest_response(request_id) {
                    self.abandon_wallet_digest_request(request_id);
                    match recibida.response {
                        WalletDigestsResponse::Digests { digests } => {
                            match Self::validate_wallet_digest_range(
                                digests,
                                from_height,
                                to_height,
                                expected_prev_hash,
                            ) {
                                Ok(digests) => return Some(digests),
                                Err(WalletDigestValidationError::MissingPrevHash) => info!(
                                    "peer {} serves legacy wallet digests without prev_hash",
                                    recibida.peer
                                ),
                                Err(WalletDigestValidationError::BrokenChain) => warn!(
                                    "peer {} returned wallet digests from a different or mixed branch",
                                    recibida.peer
                                ),
                                Err(WalletDigestValidationError::InvalidRange) => warn!(
                                    "peer {} returned an incomplete wallet digest range",
                                    recibida.peer
                                ),
                            }
                        }
                        WalletDigestsResponse::NotFound { reason } => {
                            info!("el par {} no sirve resumenes ({reason})", recibida.peer);
                        }
                    }
                    break;
                }

                let now = tokio::time::Instant::now();
                if now >= attempt_deadline || now >= deadline {
                    self.abandon_wallet_digest_request(request_id);
                    break;
                }
                let pump_for =
                    PUMP_SLICE_DIGESTS.min(attempt_deadline.saturating_duration_since(now));
                if self.pump_for(pump_for).await.is_err() {
                    self.abandon_wallet_digest_request(request_id);
                    break;
                }
            }
        }
        self.pending_outbound_wallet_digest_requests.clear();
        self.received_wallet_digests.clear();
        None
    }

    fn handle_snapshot_request_response_event(
        &mut self,
        event: request_response::Event<GetSnapshotManifests, SnapshotManifestsResponse>,
    ) {
        match event {
            request_response::Event::Message { peer, message, .. } => match message {
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

    pub(crate) fn mobile_block_request_peers(&self) -> Vec<PeerId> {
        self.select_block_request_peers()
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
            let solo_el_preferido: Vec<PeerId> = peers
                .iter()
                .filter(|peer| *peer == preferred_peer)
                .cloned()
                .collect();
            // Quedarse sin nadie a quien preguntarle no es fidelidad a una
            // rama: es no sincronizar.
            //
            // El par preferido sale de los que ANUNCIARON una altura, que no
            // son necesariamente los CONECTADOS. Cuando ese par no esta
            // conectado, filtrar la lista dejaba cero candidatos y el pedido
            // moria con "no connected peers available for direct block
            // request" -- teniendo pares sanos al lado. Medido en un nodo con
            // la base recien creada: no pudo bajar ni el bloque 3, con dos
            // pares conectados.
            //
            // La afinidad estricta existe para no mezclar ramas preguntandole
            // a cualquiera. Si el par de esa rama no esta, preguntar a otro es
            // estrictamente mejor que no preguntar: la respuesta se valida
            // igual -- contra el hash fijado cuando lo hay, y contra las
            // reglas de cadena siempre.
            if !solo_el_preferido.is_empty() {
                return solo_el_preferido;
            }
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

    fn abandon_block_body_requests(
        &mut self,
        request_ids: &HashSet<request_response::OutboundRequestId>,
    ) {
        for request_id in request_ids {
            self.pending_outbound_block_body_requests.remove(request_id);
        }
        self.received_block_body_responses
            .retain(|response| !request_ids.contains(&response.request_id));
        self.block_body_request_failures
            .retain(|failure| !request_ids.contains(&failure.request_id));
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

    fn abandon_private_tx_body_requests(
        &mut self,
        request_ids: &HashSet<request_response::OutboundRequestId>,
    ) {
        for request_id in request_ids {
            self.pending_outbound_private_tx_body_requests
                .remove(request_id);
        }
        self.received_private_tx_body_responses
            .retain(|response| !request_ids.contains(&response.request_id));
        self.private_tx_body_request_failures
            .retain(|failure| !request_ids.contains(&failure.request_id));
    }

    fn private_tx_body_matches_hash(tx_hash: &str, body: &[u8]) -> bool {
        let Ok(tx) = serde_json::from_slice::<PrivateTransaction>(body) else {
            return false;
        };
        tx.tx_hash == tx_hash && crate::private_wallet::expected_private_tx_hash(&tx) == tx.tx_hash
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
                                if let ZokaMessage::BlockAnnounce(announce) = &msg {
                                    self.record_announced_height(announce.block_height);
                                }
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
                        SwarmEvent::Behaviour(NodeBehaviourEvent::PrivateTxBodiesRequestResponse(event)) => {
                            self.handle_private_tx_bodies_event(event);
                        }
                        SwarmEvent::Behaviour(NodeBehaviourEvent::PrivateTxDigestsRequestResponse(event)) => {
                            self.handle_private_tx_digests_event(event);
                        }
                        SwarmEvent::Behaviour(NodeBehaviourEvent::WalletDigestRequestResponse(event)) => {
                            self.handle_wallet_digest_event(event);
                        }
                        SwarmEvent::Behaviour(NodeBehaviourEvent::QuanticLightRequestResponse(event)) => {
                            self.handle_quantic_light_event(event);
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
            ZokaMessage::TeamAdvertise(_)
            | ZokaMessage::TeamJoin(_)
            | ZokaMessage::ShareSubmit(_)
            | ZokaMessage::TurnUpdate(_) => self.team_mining_topic.clone(),
            ZokaMessage::SignedCheckpoint(_) => self.checkpoints_topic.clone(),
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
            tokio::time::sleep(PUBLISH_RETRY_PAUSE).await;
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
                request_ids.remove(&received.request_id);
                match received.response {
                    PrivateTxBodyResponse::Body { tx_hash: got, body } => {
                        if got != tx_hash
                            || body.is_empty()
                            || !Self::private_tx_body_matches_hash(tx_hash, &body)
                        {
                            self.record_private_tx_invalid_response(received.peer.clone());
                            last_error = format!(
                                "peer {} returned invalid private tx body for {}",
                                received.peer, tx_hash
                            );
                        } else {
                            info!(
                                "direct private tx body response hash={} from {} in {}ms",
                                tx_hash,
                                received.peer,
                                received.latency.as_millis()
                            );
                            self.abandon_private_tx_body_requests(&request_ids);
                            return Ok(body);
                        }
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
                request_ids.remove(&failure.request_id);
                last_error = format!(
                    "peer {} private tx body request failed: {}",
                    failure.peer, failure.error
                );
            }

            if request_ids.is_empty() {
                let detail = if last_error.is_empty() {
                    "no peer returned the requested private tx body".to_string()
                } else {
                    last_error
                };
                return Err(Box::new(io::Error::new(io::ErrorKind::NotFound, detail)));
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
                self.received_private_tx_body_responses
                    .retain(|response| !request_ids.contains(&response.request_id));
                self.private_tx_body_request_failures
                    .retain(|failure| !request_ids.contains(&failure.request_id));
                return Err(Box::new(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("timed out waiting for private tx body response: {detail}"),
                )));
            }

            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if let Err(error) = self
                .pump_for(remaining.min(Duration::from_millis(150)))
                .await
            {
                self.abandon_private_tx_body_requests(&request_ids);
                return Err(error);
            }
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
                        self.abandon_block_body_requests(&request_ids);
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
                self.abandon_block_body_requests(&request_ids);
                return Err(Box::new(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("timed out waiting for direct block response: {detail}"),
                )));
            }

            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if let Err(error) = self
                .pump_for(remaining.min(Duration::from_millis(150)))
                .await
            {
                self.abandon_block_body_requests(&request_ids);
                return Err(error);
            }
        }
    }

    /// Probe one explicitly selected peer without allowing strict-affinity
    /// fallback to turn a sequential mobile probe into fanout. `NotFound` is
    /// kept distinct from transport/rejection failures so callers may only
    /// infer absence after every selected peer answered conclusively.
    pub(crate) async fn request_mobile_block_probe_from_peer(
        &mut self,
        block_height: u64,
        peer: &PeerId,
        max_wait: Duration,
    ) -> Result<MobileBlockProbe, String> {
        if max_wait.is_zero() {
            return Err("el sondeo no tiene tiempo restante".to_string());
        }
        let mut responses = self
            .request_blocks_batch_direct_from_peer(&[block_height], peer, max_wait)
            .await;
        match responses.remove(&block_height) {
            Some(Ok(response)) => Ok(MobileBlockProbe::Found(response)),
            Some(Err(error)) if error.contains(" did not have block:") => {
                Ok(MobileBlockProbe::NotFound)
            }
            Some(Err(error)) => Err(error),
            None => Err(format!(
                "el par {peer} no completo el sondeo del bloque {block_height}"
            )),
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
        let peers = self.select_block_request_peers();
        self.request_blocks_batch_direct_from_candidates(heights, peers, max_wait)
            .await
    }

    /// Mobile scans need a coherent contiguous candidate: all heights are sent
    /// to one peer concurrently, then the caller can reject the whole candidate
    /// and retry a second peer within the same chunk deadline.
    pub(crate) async fn request_blocks_batch_direct_from_peer(
        &mut self,
        heights: &[u64],
        peer: &PeerId,
        max_wait: Duration,
    ) -> HashMap<u64, Result<SourcedBlockResponse, String>> {
        let peers = if self.swarm.is_connected(peer) && !self.is_block_sync_peer_quarantined(peer) {
            vec![peer.clone()]
        } else {
            Vec::new()
        };
        self.request_blocks_batch_direct_from_candidates(heights, peers, max_wait)
            .await
    }

    async fn request_blocks_batch_direct_from_candidates(
        &mut self,
        heights: &[u64],
        peers: Vec<PeerId>,
        max_wait: Duration,
    ) -> HashMap<u64, Result<SourcedBlockResponse, String>> {
        let mut results: HashMap<u64, Result<SourcedBlockResponse, String>> = HashMap::new();
        if heights.is_empty() {
            return results;
        }
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
                    BlockBodyResponse::NotFound {
                        block_height,
                        reason,
                        ..
                    } => {
                        if block_height != height {
                            self.record_direct_block_invalid_response(received.peer.clone());
                            results.insert(
                                height,
                                Err(format!(
                                    "peer {} returned NotFound height {block_height}, expected {height}",
                                    received.peer
                                )),
                            );
                        } else {
                            self.record_direct_block_failure(received.peer.clone());
                            results.insert(
                                height,
                                Err(format!(
                                    "peer {} did not have block: {reason}",
                                    received.peer
                                )),
                            );
                        }
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
        self.abandon_block_body_requests(&request_ids);
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

    /// Tantea si algún par conectado tiene el bloque `block_height`, solo por
    /// pedido directo. Para cuando ningún par anunció su altura todavía: por
    /// gossip no aparecería nada que un pedido directo no traiga, y una wallet
    /// contesta enseguida que no lo tiene.
    pub async fn probe_block_direct(
        &mut self,
        block_height: u64,
        max_wait: Duration,
    ) -> Result<SourcedBlockResponse, Box<dyn Error>> {
        let attempts = self.connected_peer_count().max(1) as u32;
        self.request_block_direct_with_retry(block_height, None, None, false, attempts, max_wait)
            .await
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
                // Los pedidos en lote también llegan mientras el nodo espera
                // mensajes acá, que es donde pasa casi todo el tiempo. Antes
                // caían en `_` y se tiraban sin contestar: el teléfono esperaba
                // el plazo entero y anotaba "sin respuesta" (15/10/2026: lotes
                // de cuerpos que nunca volvían mientras los de a uno sí).
                SwarmEvent::Behaviour(NodeBehaviourEvent::PrivateTxBodiesRequestResponse(event)) => {
                    self.handle_private_tx_bodies_event(event);
                }
                SwarmEvent::Behaviour(NodeBehaviourEvent::PrivateTxDigestsRequestResponse(event)) => {
                    self.handle_private_tx_digests_event(event);
                }
                SwarmEvent::Behaviour(NodeBehaviourEvent::WalletDigestRequestResponse(event)) => {
                    self.handle_wallet_digest_event(event);
                }
                SwarmEvent::Behaviour(NodeBehaviourEvent::QuanticLightRequestResponse(event)) => {
                    self.handle_quantic_light_event(event);
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
            ZokaMessage::TeamAdvertise(_)
            | ZokaMessage::TeamJoin(_)
            | ZokaMessage::ShareSubmit(_)
            | ZokaMessage::TurnUpdate(_) => TEAM_MINING_TOPIC_NAME,
            ZokaMessage::SignedCheckpoint(_) => CHECKPOINTS_TOPIC_NAME,
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
    /// Direcciones de los pares que ya están conectados.
    ///
    /// Marcar de nuevo a un par conectado no suma un vecino: el marcado
    /// "funciona" al instante porque la conexión ya existe, pero la cuenta no
    /// se mueve, así que la espera posterior vence y se contabiliza como
    /// fallo. Con un presupuesto de dos intentos por barrido, un solo par
    /// conectado se lleva la mitad de cada ronda para siempre, y las
    /// direcciones a las que todavía NO estamos conectados no reciben ni un
    /// intento. Ese es exactamente el estado en que un nodo se queda clavado
    /// en un vecino y no sube nunca.
    pub fn connected_peer_addrs(&self) -> Vec<String> {
        let conectados = self
            .connected_peer_ids()
            .into_iter()
            .map(|peer| peer.to_string())
            .collect::<std::collections::HashSet<_>>();
        self.observed_peer_records()
            .into_iter()
            .filter(|record| conectados.contains(&record.peer_id))
            .map(|record| record.addr)
            .collect()
    }

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

    /// Anota la altura de un anuncio si es la mas alta vista hasta ahora.
    pub fn record_announced_height(&mut self, height: u64) {
        if height > self.highest_announced_height {
            self.highest_announced_height = height;
        }
    }

    /// La altura mas alta que algun par haya anunciado, o 0 si todavia no llego
    /// ningun anuncio.
    ///
    /// Cero no significa "la cadena esta vacia" sino "todavia no se". Quien la
    /// use tiene que distinguir los dos casos: escanear hasta 0 no es lo mismo
    /// que no tener nada que escanear.
    ///
    /// No es un dato verificado: un par mentiroso puede anunciar una altura
    /// inventada. Lo peor que consigue es que un cliente liviano pida bloques
    /// que nadie tiene y le falle el escaneo -- no le hace creer un saldo que
    /// no existe, porque los bloques si se piden y se leen de verdad.
    pub fn highest_announced_height(&self) -> u64 {
        self.highest_announced_height
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

    /// Por cada tópico: cuántos pares lo tienen suscrito y cuántos están en su
    /// malla de propagación.
    ///
    /// Son dos números distintos y la diferencia importa. *Suscrito* significa
    /// que ese par declaró interés en el tópico; *malla* significa que además
    /// nos elegimos mutuamente para reenviarnos mensajes de ese tópico. Un
    /// mensaje viaja por la malla: con suscriptores pero malla vacía, lo que
    /// se publique sale por flood y lo que publiquen otros puede no llegar
    /// nunca.
    ///
    /// Sin esto, "no me aparece el equipo que creó otro" es indistinguible de
    /// "no hay equipos". Existía el equivalente solo para PBFT, y los tópicos
    /// nuevos —como la minería en equipo— quedaban a ciegas: un cliente viejo
    /// se conecta y sincroniza bloques con normalidad, pero nunca se suscribe
    /// a un tópico que no conoce, así que tener pares no implica tener con
    /// quién coordinar.
    pub fn topic_peer_counts(&self) -> Vec<(String, usize, usize)> {
        let gossipsub = &self.swarm.behaviour().gossipsub;
        [
            (GENERAL_TOPIC_NAME, &self.general_topic),
            (PBFT_TOPIC_NAME, &self.pbft_topic),
            (STEM_TOPIC_NAME, &self.stem_topic),
            (MIXER_TOPIC_NAME, &self.mixer_topic),
            (TEAM_MINING_TOPIC_NAME, &self.team_mining_topic),
            (CHECKPOINTS_TOPIC_NAME, &self.checkpoints_topic),
        ]
        .into_iter()
        .map(|(name, topic)| {
            let hash = topic.hash();
            let suscritos = gossipsub
                .all_peers()
                .filter(|(_, topics)| topics.iter().any(|t| *t == &hash))
                .count();
            let malla = gossipsub.mesh_peers(&hash).count();
            (name.to_string(), suscritos, malla)
        })
        .collect()
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

    /// Los conjuntos de "ya lo vi" de la capa de anonimato tienen que tener
    /// techo.
    ///
    /// Eran `HashSet` que solo crecian: se insertaba un identificador por cada
    /// lote del mixer y por cada ruta de Dandelion, y nunca se limpiaba nada.
    /// Un nodo reporto 22,6 GB de RSS y dos muertes por falta de memoria con
    /// 16 pares; estos dos eran los unicos candidatos que quedaban tras
    /// descartar la cola de entrada (ya acotada) y el dataset de RandomX
    /// (compartido, no por hilo).
    #[test]
    fn seen_ids_stop_growing_at_the_cap() {
        let mut vistos = SeenIdSet::default();
        for n in 0..(MAX_SEEN_GOSSIP_IDS + 5_000) {
            assert!(vistos.insert(format!("id-{n}")), "cada id es nuevo");
        }
        assert_eq!(
            vistos.len(),
            MAX_SEEN_GOSSIP_IDS,
            "por encima del techo se desaloja, no se acumula"
        );
    }

    /// Y el desalojo tiene que ser por antiguedad: lo recien visto es lo que
    /// mas probabilidad tiene de volver a llegar duplicado, asi que es lo
    /// ultimo que conviene olvidar.
    #[test]
    fn the_oldest_seen_id_is_the_one_forgotten() {
        let mut vistos = SeenIdSet::default();
        for n in 0..MAX_SEEN_GOSSIP_IDS {
            vistos.insert(format!("id-{n}"));
        }
        assert!(vistos.contains("id-0"), "todavia entra sin desalojar");

        vistos.insert("id-nuevo".to_string());

        assert!(!vistos.contains("id-0"), "el mas viejo es el que sale");
        assert!(vistos.contains("id-nuevo"), "el recien llegado queda");
        assert!(
            vistos.contains(&format!("id-{}", MAX_SEEN_GOSSIP_IDS - 1)),
            "el anteultimo sigue adentro"
        );
    }

    /// Un duplicado no puede consumir lugar ni empujar a nadie afuera.
    #[test]
    fn a_repeated_id_does_not_evict_anything() {
        let mut vistos = SeenIdSet::default();
        assert!(vistos.insert("mismo".to_string()));
        assert!(
            !vistos.insert("mismo".to_string()),
            "el repetido se rechaza"
        );
        assert_eq!(vistos.len(), 1);
    }

    /// Quedarse sin nadie a quien preguntarle no es fidelidad a una rama.
    ///
    /// El par preferido sale de los que ANUNCIARON altura, que no son
    /// necesariamente los CONECTADOS. Con afinidad estricta la lista se
    /// filtraba dejando solo a ese par, y si no estaba conectado quedaba
    /// vacia: el pedido moria con "no connected peers available" teniendo
    /// pares sanos al lado. Medido en un nodo con la base recien creada: no
    /// bajo ni el bloque 3, con dos pares conectados.
    #[test]
    fn strict_affinity_never_leaves_the_node_with_nobody_to_ask() {
        let conectados = vec![PeerId::random(), PeerId::random()];
        let preferido_desconectado = PeerId::random();

        let elegir = |preferido: &PeerId, estricta: bool| -> Vec<PeerId> {
            let mut peers = conectados.clone();
            if estricta {
                let solo: Vec<PeerId> = peers.iter().filter(|p| *p == preferido).cloned().collect();
                if !solo.is_empty() {
                    return solo;
                }
            } else if let Some(pos) = peers.iter().position(|p| p == preferido) {
                let p = peers.remove(pos);
                peers.insert(0, p);
            }
            peers
        };

        // El preferido no esta conectado: se pregunta a los que si estan.
        let elegidos = elegir(&preferido_desconectado, true);
        assert_eq!(
            elegidos.len(),
            2,
            "sin el preferido hay que preguntarle a los conectados, no a nadie"
        );

        // El preferido si esta conectado: la afinidad estricta se respeta.
        let elegidos = elegir(&conectados[1], true);
        assert_eq!(elegidos, vec![conectados[1]]);
    }

    /// Un "no tengo ese bloque" no puede puntuar como si lo hubiera entregado.
    ///
    /// Se puntuaba como exito cualquier respuesta que llegara. Un nodo recien
    /// instalado, que todavia no tiene la altura pedida, contestaba NotFound y
    /// sumaba reputacion igual que uno que entrego el bloque. Como los pares se
    /// ordenan por puntaje para decidir a quien preguntarle, esos nodos subian
    /// al tope y se les volvia a preguntar primero: un bucle que se refuerza
    /// solo, con miles de respuestas por minuto y cero bloques aplicados.
    #[test]
    fn only_a_delivered_block_counts_as_a_useful_response() {
        use crate::network::block_request_response::BlockBodyResponse;

        let no_lo_tengo = BlockBodyResponse::NotFound {
            block_height: 15642,
            block_hash: None,
            reason: "canonical block not found".to_string(),
        };
        let rechazado = BlockBodyResponse::Rejected {
            block_height: 15642,
            block_hash: None,
            reason: "read failed".to_string(),
        };

        let util = |r: &BlockBodyResponse| matches!(r, BlockBodyResponse::Found(_));
        assert!(!util(&no_lo_tengo), "NotFound no sirve para avanzar");
        assert!(!util(&rechazado), "Rejected tampoco");
    }
    use super::*;
    use crate::consensus::{PbftMessage, PbftMessageType};
    use crate::network::message::{PingMessage, PrivateOutputMessage};
    use crate::types::{Signature, Transaction};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn data_protocol_timeout_outlives_mobile_operation_budget() {
        assert_eq!(MOBILE_P2P_OPERATION_WAIT, Duration::from_secs(60));
        assert_eq!(DATA_REQUEST_RESPONSE_SLACK, Duration::from_secs(15));
        assert_eq!(DATA_REQUEST_RESPONSE_TIMEOUT, Duration::from_secs(75));
        assert!(DATA_REQUEST_RESPONSE_TIMEOUT > MOBILE_P2P_OPERATION_WAIT);

        let config = format!("{:?}", data_request_response_config());
        assert!(
            config.contains("request_timeout: 75s"),
            "data protocol config did not retain its 75s timeout: {config}"
        );
    }

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
                output_version: 1,
                kem_ciphertext: Vec::new(),
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

    // Esta prueba describe la configuracion de consenso, donde estas
    // activaciones estan dormidas. El build `mobile` las enciende desde genesis
    // a proposito -- es la compuerta local del telefono, documentada en
    // economics.rs -- asi que aca no puede pasar y no deberia intentarlo.
    //
    // Sin este marcado, `cargo test --features mobile` quedaba con cinco rojos
    // permanentes y dejaba de servir como senal: un fallo real se perdia entre
    // fallos esperados. Se descubrio usandolo justamente para validar un
    // protocolo nuevo.
    #[cfg(not(feature = "mobile"))]
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
                    output_version: 1,
                    kem_ciphertext: Vec::new(),
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

        let share = ZokaMessage::ShareSubmit(crate::team_mining::SubmittedShare {
            team_id: "team".to_string(),
            template_id: "b".repeat(64),
            miner_address: "member".to_string(),
            nonce: 1,
            pow_hash_hex: "c".repeat(64),
            difficulty_bits: 1,
            timestamp: 55,
        });
        assert_eq!(Node::topic_name_for_message(&share), TEAM_MINING_TOPIC_NAME);

        let checkpoint = ZokaMessage::SignedCheckpoint(
            crate::checkpoint_authority::SignedCheckpoint::sign(
                &ed25519_dalek::SigningKey::from_bytes(&[5u8; 32]),
                "zoka-test",
                10,
                &"a".repeat(64),
            ),
        );
        assert_eq!(Node::topic_name_for_message(&checkpoint), CHECKPOINTS_TOPIC_NAME);
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
                output_version: 1,
                kem_ciphertext: Vec::new(),
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
                output_version: 1,
                kem_ciphertext: Vec::new(),
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
            signed_peer_record: None,
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

        let stream = Transport::dial(&mut transport, onion_addr, libp2p::core::transport::DialOpts { role: libp2p::core::Endpoint::Dialer, port_use: libp2p::core::transport::PortUse::Reuse })
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
            Transport::dial(&mut transport, direct_addr, libp2p::core::transport::DialOpts { role: libp2p::core::Endpoint::Dialer, port_use: libp2p::core::transport::PortUse::Reuse }),
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
                output_version: 1,
                kem_ciphertext: Vec::new(),
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
    async fn una_wallet_no_se_suscribe_a_lo_que_no_va_a_mirar() {
        // Estar suscrito a un tema no es gratis: gossipsub obliga a recibir y
        // **reenviar** lo que pasa por ahi. En un telefono con Tor, reenviar
        // ofertas del intercambio o mensajes de consenso son datos y bateria
        // gastados en algo que se descarta.
        //
        // Los temas que si necesita no se tocan: sin `general` no se entera de
        // los bloques, y sin `stem`/`mixer` sus propias transacciones pierden
        // la capa de privacidad al publicarse.
        let wallet = Node::new_with_profile(0, None, None, NodeProfile::Wallet)
            .await
            .expect("nodo wallet");
        let completo = Node::new_with_profile(0, None, None, NodeProfile::Full)
            .await
            .expect("nodo completo");

        let temas_wallet: Vec<String> = wallet
            .swarm
            .behaviour()
            .gossipsub
            .topics()
            .map(|t| t.to_string())
            .collect();
        let temas_completo: Vec<String> = completo
            .swarm
            .behaviour()
            .gossipsub
            .topics()
            .map(|t| t.to_string())
            .collect();

        assert!(
            temas_wallet.len() < temas_completo.len(),
            "una wallet tiene que escuchar menos que un nodo completo; wallet {temas_wallet:?}, completo {temas_completo:?}"
        );
        for imprescindible in ["messages", "stem", "mixer"] {
            assert!(
                temas_wallet.iter().any(|t| t.contains(imprescindible)),
                "sin '{imprescindible}' la wallet no puede ver bloques ni publicar en privado: {temas_wallet:?}"
            );
        }
        for ajeno in ["pbft", "team"] {
            assert!(
                !temas_wallet.iter().any(|t| t.contains(ajeno)),
                "la wallet no participa de '{ajeno}' y no deberia reenviarlo: {temas_wallet:?}"
            );
            assert!(
                temas_completo.iter().any(|t| t.contains(ajeno)),
                "un nodo completo si tiene que estar en '{ajeno}': {temas_completo:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_peer_cannot_ask_more_than_its_budget_per_minute() {
        use crate::network::quantic_light_request_response::QuanticLightRequest;

        assert_eq!(quantic_light_cost(&QuanticLightRequest::Info), 1);
        // Un envío de 2 acciones (11.351 bytes) y uno de 64 (275.161).
        let envio = |bytes: usize| QuanticLightRequest::SubmitTx { tx_hex: "ab".repeat(bytes) };
        assert_eq!(quantic_light_cost(&envio(11_351)), 6);
        assert_eq!(quantic_light_cost(&envio(275_161)), 3 * 64);
        assert_eq!(quantic_light_cost(&QuanticLightRequest::SubmitTx { tx_hex: String::new() }), 6);
        assert_eq!(quantic_light_cost(&QuanticLightRequest::Compact { from: 1, to: 500 }), 50);
        assert_eq!(quantic_light_cost(&QuanticLightRequest::Compact { from: 1, to: u64::MAX }), 50);
        assert_eq!(quantic_light_cost(&QuanticLightRequest::Compact { from: 9, to: 1 }), 1);
        let mut nodo = Node::new(0).await.expect("nodo");
        let par = PeerId::random();
        let otro = PeerId::random();
        let lote = quantic_light_cost(&QuanticLightRequest::Compact { from: 1, to: 500 });
        let minuto = 29_000_000;
        for _ in 0..QUANTIC_LIGHT_BUDGET_PER_MINUTE / lote {
            assert!(nodo.allow_quantic_light_in_minute(&par, lote, minuto));
        }
        assert!(!nodo.allow_quantic_light_in_minute(&par, lote, minuto), "doce lotes por minuto");
        assert!(!nodo.allow_quantic_light_in_minute(&par, 1, minuto));
        assert!(nodo.allow_quantic_light_in_minute(&otro, lote, minuto), "cada par tiene el suyo");
        assert!(nodo.allow_quantic_light_in_minute(&par, lote, minuto + 1), "al minuto siguiente, de nuevo");
    }

    /// La dirección completa (con `/p2p/`) de un nodo que escucha en TCP local.
    async fn escuchando_en_local(nodo: &mut Node) -> String {
        nodo.listen("/ip4/127.0.0.1/tcp/0").await.expect("escuchar");
        for _ in 0..40 {
            let _ = nodo.pump_for(Duration::from_millis(50)).await;
            if let Some(addr) = nodo.swarm.listeners().next() {
                return format!("{addr}/p2p/{}", nodo.peer_id());
            }
        }
        panic!("el nodo tiene que escuchar en algun lado");
    }

    #[tokio::test]
    async fn una_wallet_contesta_al_toque_que_no_tiene_bloques() {
        // Un telefono no guarda la cadena. Si no contesta, el nodo completo que
        // le pide un bloque espera el plazo entero con su bucle frenado (asi
        // se freno la mineria de un nodo cuyo unico par era un telefono).
        let mut wallet = Node::new_with_profile(0, None, None, NodeProfile::Wallet)
            .await
            .expect("wallet");
        let direccion = escuchando_en_local(&mut wallet).await;
        let mut completo = Node::new(0).await.expect("nodo completo");
        completo.connect_to_peer(&direccion).await.expect("marcar");
        for _ in 0..80 {
            let _ = completo.pump_for(Duration::from_millis(30)).await;
            let _ = wallet.pump_for(Duration::from_millis(30)).await;
            if completo.connected_peer_count() > 0 {
                break;
            }
        }
        assert_eq!(completo.connected_peer_count(), 1);
        let atender = tokio::spawn(async move {
            for _ in 0..400 {
                let _ = wallet.pump_for(Duration::from_millis(15)).await;
            }
        });
        let inicio = Instant::now();
        let resultado = completo.probe_block_direct(5, Duration::from_secs(10)).await;
        assert!(resultado.is_err(), "la wallet no tiene el bloque");
        assert!(
            inicio.elapsed() < Duration::from_secs(5),
            "contesta al toque, no al vencer el plazo: {:?}",
            inicio.elapsed()
        );
        atender.abort();
    }

    #[tokio::test]
    async fn un_telefono_pide_los_datos_de_quantic_a_un_nodo_por_p2p() {
        // `/zoka-quantic/light/1` entre nodos de verdad: el que tiene la cadena
        // contesta info, bloques compactos y envios; uno sin la cadena dice que
        // no la sirve, y el telefono pasa al siguiente par en vez de esperar.
        use crate::network::quantic_light_request_response::{
            CompactBlockWire, QuanticLightRequest, QuanticLightResponse,
        };

        let mut servidor = Node::new(0).await.expect("nodo con la cadena");
        servidor.set_quantic_light_responder(Box::new(|pedido| match pedido {
            QuanticLightRequest::Info => QuanticLightResponse::Info {
                chain_id: "zoka-quantic-regtest-v1".to_string(),
                height: 300,
                tip: "ab".repeat(32),
                tree_root: Some("cd".repeat(32)),
            },
            QuanticLightRequest::Compact { from, to } => QuanticLightResponse::Compact {
                blocks: (*from..=*to)
                    .map(|height| CompactBlockWire {
                        height,
                        hash: format!("{height:064x}"),
                        prev_hash: format!("{:064x}", height - 1),
                        state_root: "quantic-v1:...".to_string(),
                        // Una accion compacta pesa 1.236 bytes.
                        transactions: vec![format!("{height:04x}").repeat(618)],
                    })
                    .collect(),
            },
            QuanticLightRequest::SubmitTx { tx_hex } => QuanticLightResponse::Submitted {
                accepted: tx_hex == "c0ffee",
                txid: (tx_hex == "c0ffee").then(|| "11".repeat(32)),
                reason: (tx_hex != "c0ffee").then(|| "invalid proof".to_string()),
            },
        }));
        let direccion_servidor = escuchando_en_local(&mut servidor).await;
        let id_servidor = servidor.peer_id();
        let mut sin_cadena = Node::new(0).await.expect("nodo sin la cadena");
        let direccion_sin_cadena = escuchando_en_local(&mut sin_cadena).await;
        let id_sin_cadena = sin_cadena.peer_id();

        let mut telefono = Node::new(0).await.expect("telefono");
        for direccion in [&direccion_sin_cadena, &direccion_servidor] {
            telefono.connect_to_peer(direccion).await.expect("marcar");
        }
        for _ in 0..80 {
            let _ = telefono.pump_for(Duration::from_millis(30)).await;
            let _ = servidor.pump_for(Duration::from_millis(30)).await;
            let _ = sin_cadena.pump_for(Duration::from_millis(30)).await;
            if telefono.connected_peer_count() >= 2 {
                break;
            }
        }
        assert_eq!(telefono.connected_peer_count(), 2, "el telefono tiene que ver a los dos");

        let atender = tokio::spawn(async move {
            for _ in 0..600 {
                let _ = servidor.pump_for(Duration::from_millis(15)).await;
                let _ = sin_cadena.pump_for(Duration::from_millis(15)).await;
            }
        });
        let espera = Duration::from_secs(15);

        let no_la_sirve = telefono
            .request_quantic_light(&id_sin_cadena, QuanticLightRequest::Info, espera)
            .await;
        assert!(
            matches!(no_la_sirve, Some(QuanticLightResponse::Error { .. })),
            "sin la cadena contesta que no la sirve: {no_la_sirve:?}"
        );
        match telefono
            .request_quantic_light_from_peers(QuanticLightRequest::Info, espera)
            .await
        {
            Some((quien, QuanticLightResponse::Info { height: 300, .. })) => {
                assert_eq!(quien, id_servidor, "contesta el que tiene la cadena")
            }
            otra => panic!("info: {otra:?}"),
        }
        match telefono
            .request_quantic_light(&id_servidor, QuanticLightRequest::Compact { from: 1, to: 500 }, espera)
            .await
        {
            Some(QuanticLightResponse::Compact { blocks }) => {
                assert_eq!(blocks.len(), 500, "500 bloques compactos en un solo pedido");
                assert_eq!(blocks[499].height, 500);
                assert_eq!(blocks[499].transactions[0].len(), 2 * 1236);
            }
            otra => panic!("compactos: {otra:?}"),
        }
        for (tx_hex, aceptada) in [("c0ffee", true), ("bad", false)] {
            match telefono
                .request_quantic_light(
                    &id_servidor,
                    QuanticLightRequest::SubmitTx { tx_hex: tx_hex.to_string() },
                    espera,
                )
                .await
            {
                Some(QuanticLightResponse::Submitted { accepted, .. }) => assert_eq!(accepted, aceptada),
                otra => panic!("envio: {otra:?}"),
            }
        }
        atender.abort();
    }

    #[tokio::test]
    async fn dos_nodos_se_pasan_muchos_cuerpos_en_un_solo_pedido() {
        // Dos nodos de verdad por TCP local. Que el codigo compile no dice nada
        // sobre si los pares se entienden, y este protocolo existe para reemplazar
        // 266 idas y vueltas por Tor: si no funciona de verdad, no sirve de nada.
        use crate::network::private_tx_bodies_request_response::{
            PrivateTxBodiesResponse, PrivateTxBodyEntry,
        };

        let mut servidor = Node::new(0).await.expect("nodo servidor");
        servidor
            .listen("/ip4/127.0.0.1/tcp/0")
            .await
            .expect("escuchar");
        // El servidor tiene los pares y no tiene los impares: hay que poder
        // distinguir "no lo tengo" de "no contesto".
        servidor.set_private_tx_bodies_responder(Box::new(|pedido| {
            let mut bodies = Vec::new();
            let mut missing = Vec::new();
            for hash in &pedido.tx_hashes {
                let numero: u64 = hash.trim_start_matches("tx").parse().unwrap_or(0);
                if numero % 2 == 0 {
                    bodies.push(PrivateTxBodyEntry {
                        tx_hash: hash.clone(),
                        body: format!("cuerpo-de-{hash}").into_bytes(),
                    });
                } else {
                    missing.push(hash.clone());
                }
            }
            PrivateTxBodiesResponse { bodies, missing }
        }));

        let mut direccion = None;
        for _ in 0..40 {
            let _ = servidor.pump_for(Duration::from_millis(50)).await;
            if let Some(addr) = servidor.swarm.listeners().next() {
                direccion = Some(addr.clone());
                break;
            }
        }
        let direccion = direccion.expect("el servidor tiene que escuchar en algun lado");
        let addr_con_peer = format!("{direccion}/p2p/{}", servidor.peer_id());

        let mut cliente = Node::new(0).await.expect("nodo cliente");
        cliente
            .connect_to_peer(&addr_con_peer)
            .await
            .expect("marcar al servidor");

        for _ in 0..60 {
            let _ = cliente.pump_for(Duration::from_millis(30)).await;
            let _ = servidor.pump_for(Duration::from_millis(30)).await;
            if cliente.connected_peer_count() > 0 {
                break;
            }
        }
        assert!(
            cliente.connected_peer_count() > 0,
            "los nodos tienen que conectarse antes de pedirse nada"
        );

        let atender = tokio::spawn(async move {
            for _ in 0..300 {
                let _ = servidor.pump_for(Duration::from_millis(25)).await;
            }
            servidor
        });

        let pedidos: Vec<String> = (0..10).map(|i| format!("tx{i}")).collect();
        let r = cliente
            .request_private_tx_bodies_from_peers(&pedidos, Duration::from_secs(15))
            .await;
        let _ = atender.await;
        let conseguidos = r.conseguidos;
        let faltantes = r.dichos_faltantes;

        assert_eq!(
            conseguidos.len(),
            5,
            "los cinco pares tenian que llegar en un solo viaje: {conseguidos:?}"
        );
        for (hash, cuerpo) in &conseguidos {
            assert_eq!(
                cuerpo,
                &format!("cuerpo-de-{hash}").into_bytes(),
                "cada cuerpo tiene que corresponder a su hash"
            );
        }
        assert_eq!(
            faltantes.len(),
            5,
            "los que el par no tiene se informan, no se callan: {faltantes:?}"
        );
    }

    #[tokio::test]
    async fn un_par_sin_el_protocolo_deja_los_cuerpos_sin_respuesta_no_perdidos() {
        // El caso que costo un saldo entero.
        //
        // Un par que corre un binario sin `/zoka/private-tx-bodies/1` no puede
        // contestar. Si eso se devuelve junto con los "no lo tengo", quien
        // llama marca transacciones vivas como perdidas para siempre -- y con
        // una cadena de 266 transacciones privadas contra un par viejo, eso
        // dejo el saldo de la wallet en cero.
        //
        // Para reproducirlo, el servidor se conecta y despues DEJA DE ATENDER:
        // no bombea su bucle de eventos mientras el cliente pregunta, asi la
        // peticion nunca se contesta. Es lo mismo que le pasa al cliente contra
        // un par que no conoce el protocolo -- no hay respuesta que leer.
        //
        // No se puede simular con dos nodos de este binario "quitandole" el
        // protocolo al servidor: los dos lo tienen compilado, y un par que TIENE
        // el protocolo pero no sirve cuerpos si contesta "no los tengo" (eso lo
        // cubre la otra prueba). Lo que hay que distinguir es el silencio.
        let mut servidor = Node::new(0).await.expect("nodo servidor");
        servidor
            .listen("/ip4/127.0.0.1/tcp/0")
            .await
            .expect("escuchar");

        let mut direccion = None;
        for _ in 0..40 {
            let _ = servidor.pump_for(Duration::from_millis(50)).await;
            if let Some(addr) = servidor.swarm.listeners().next() {
                direccion = Some(addr.clone());
                break;
            }
        }
        let direccion = direccion.expect("el servidor tiene que escuchar");
        let addr_con_peer = format!("{direccion}/p2p/{}", servidor.peer_id());

        let mut cliente = Node::new(0).await.expect("nodo cliente");
        cliente
            .connect_to_peer(&addr_con_peer)
            .await
            .expect("marcar al servidor");
        for _ in 0..60 {
            let _ = cliente.pump_for(Duration::from_millis(30)).await;
            let _ = servidor.pump_for(Duration::from_millis(30)).await;
            if cliente.connected_peer_count() > 0 {
                break;
            }
        }

        // A partir de aca el servidor no bombea: queda conectado y mudo.
        let pedidos: Vec<String> = (0..5).map(|i| format!("tx{i}")).collect();
        let r = cliente
            .request_private_tx_bodies_from_peers(&pedidos, Duration::from_secs(3))
            .await;
        drop(servidor);

        assert!(r.conseguidos.is_empty(), "no habia cuerpos que traer");
        assert!(
            r.dichos_faltantes.is_empty(),
            "un par que no contesta NO dice que no los tiene: {:?}",
            r.dichos_faltantes
        );
        assert_eq!(
            r.sin_respuesta.len(),
            5,
            "los cinco tienen que quedar como sin respuesta, para reintentarlos: {:?}",
            r.sin_respuesta
        );
    }

    #[tokio::test]
    async fn un_par_que_no_sirve_cuerpos_contesta_que_no_los_tiene() {
        // Una wallet no tiene cuerpos que servir. Sin responder configurado
        // tiene que contestar "no los tengo" en vez de dejar el pedido colgado,
        // asi quien pregunta cae al protocolo de a uno enseguida en lugar de
        // esperar el plazo completo.
        let mut servidor = Node::new(0).await.expect("nodo servidor");
        servidor
            .listen("/ip4/127.0.0.1/tcp/0")
            .await
            .expect("escuchar");

        let mut direccion = None;
        for _ in 0..40 {
            let _ = servidor.pump_for(Duration::from_millis(50)).await;
            if let Some(addr) = servidor.swarm.listeners().next() {
                direccion = Some(addr.clone());
                break;
            }
        }
        let direccion = direccion.expect("el servidor tiene que escuchar");
        let addr_con_peer = format!("{direccion}/p2p/{}", servidor.peer_id());

        let mut cliente = Node::new(0).await.expect("nodo cliente");
        cliente
            .connect_to_peer(&addr_con_peer)
            .await
            .expect("marcar al servidor");
        for _ in 0..60 {
            let _ = cliente.pump_for(Duration::from_millis(30)).await;
            let _ = servidor.pump_for(Duration::from_millis(30)).await;
            if cliente.connected_peer_count() > 0 {
                break;
            }
        }

        let atender = tokio::spawn(async move {
            for _ in 0..200 {
                let _ = servidor.pump_for(Duration::from_millis(25)).await;
            }
            servidor
        });

        let r = cliente
            .request_private_tx_bodies_from_peers(
                &["tx1".to_string(), "tx2".to_string()],
                Duration::from_secs(10),
            )
            .await;
        let _ = atender.await;

        assert!(r.conseguidos.is_empty(), "no tenia ninguno");
        assert_eq!(
            r.dichos_faltantes.len(),
            2,
            "un par CON el protocolo si dice que no los tiene: {:?}",
            r.dichos_faltantes
        );
        assert!(
            r.sin_respuesta.is_empty(),
            "y como contesto, nada queda sin respuesta: {:?}",
            r.sin_respuesta
        );
    }

    #[tokio::test]
    async fn dos_nodos_se_pasan_resumenes_de_bloque() {
        // Dos nodos de verdad hablando por TCP local. Es la unica forma de
        // probar un protocolo nuevo sin desplegarlo en la red de alguien: que
        // el codigo compile no dice nada sobre si los pares se entienden.
        let mut servidor = Node::new(0).await.expect("nodo servidor");
        servidor
            .listen("/ip4/127.0.0.1/tcp/0")
            .await
            .expect("escuchar");
        servidor.set_wallet_digest_responder(Box::new(|pedido| {
            let mut digests = Vec::new();
            for altura in pedido.from_height..=pedido.to_height {
                digests.push(BlockDigest {
                    block_height: altura,
                    block_hash: format!("hash-{altura}"),
                    prev_hash: format!("hash-{}", altura.saturating_sub(1)),
                    timestamp: 1000 + altura,
                    coinbase_recipient: Some("zka1destino".to_string()),
                    coinbase_amount: Some(12_000_000),
                    coinbase_output_id: Some(format!("out-{altura}")),
                    coinbase_commitment: Some(format!("commit-{altura}")),
                    coinbase_unlock_height: Some(altura + 100),
                    private_tx_hashes: vec![],
                    private_nullifiers: vec![],
                });
            }
            WalletDigestsResponse::Digests { digests }
        }));

        // Dejar que el servidor publique su direccion de escucha.
        let mut direccion = None;
        for _ in 0..40 {
            let _ = servidor.pump_for(Duration::from_millis(50)).await;
            if let Some(addr) = servidor.swarm.listeners().next() {
                direccion = Some(addr.clone());
                break;
            }
        }
        let direccion = direccion.expect("el servidor tiene que escuchar en algun lado");
        let addr_con_peer = format!("{direccion}/p2p/{}", servidor.peer_id());

        let mut cliente = Node::new(0).await.expect("nodo cliente");
        cliente
            .connect_to_peer(&addr_con_peer)
            .await
            .expect("marcar al servidor");

        // Los dos tienen que bombear: el apreton de manos ocurre en los dos
        // bucles de eventos, no en el que llama.
        for _ in 0..60 {
            let _ = cliente.pump_for(Duration::from_millis(30)).await;
            let _ = servidor.pump_for(Duration::from_millis(30)).await;
            if cliente.connected_peer_count() > 0 {
                break;
            }
        }
        assert!(
            cliente.connected_peer_count() > 0,
            "los nodos tienen que conectarse antes de pedirse nada"
        );

        // El servidor tiene que seguir atendiendo mientras el cliente espera.
        let atender = tokio::spawn(async move {
            for _ in 0..200 {
                let _ = servidor.pump_for(Duration::from_millis(25)).await;
            }
            servidor
        });

        let resumenes = cliente
            .request_wallet_digests(10, 14, Duration::from_secs(10))
            .await
            .expect("el par sirve resumenes");
        let _ = atender.await;

        assert_eq!(resumenes.len(), 5, "cinco alturas pedidas, cinco resumenes");
        assert_eq!(resumenes[0].block_height, 10);
        assert_eq!(resumenes[4].block_height, 14);
        assert_eq!(
            resumenes[2].coinbase_output_id.as_deref(),
            Some("out-12"),
            "el resumen tiene que traer la identidad de la nota del coinbase"
        );
    }

    #[tokio::test]
    async fn resumenes_hacen_failover_secuencial_sin_fanout() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let bad_calls = Arc::new(AtomicUsize::new(0));
        let good_calls = Arc::new(AtomicUsize::new(0));
        let unused_calls = Arc::new(AtomicUsize::new(0));
        let counters = [bad_calls.clone(), good_calls.clone(), unused_calls.clone()];
        let mut servers = Vec::new();

        for (index, counter) in counters.into_iter().enumerate() {
            let mut server = Node::new(0).await.expect("digest server");
            server.listen("/ip4/127.0.0.1/tcp/0").await.expect("listen");
            server.set_wallet_digest_responder(Box::new(move |request| {
                counter.fetch_add(1, Ordering::SeqCst);
                let digests = (request.from_height..=request.to_height)
                    .map(|height| BlockDigest {
                        block_height: height,
                        block_hash: format!("hash-{height}"),
                        prev_hash: if index == 0 && height > request.from_height {
                            "broken-branch".to_string()
                        } else {
                            format!("hash-{}", height.saturating_sub(1))
                        },
                        timestamp: height,
                        coinbase_recipient: None,
                        coinbase_amount: None,
                        coinbase_output_id: None,
                        coinbase_commitment: None,
                        coinbase_unlock_height: None,
                        private_tx_hashes: Vec::new(),
                        private_nullifiers: Vec::new(),
                    })
                    .collect();
                WalletDigestsResponse::Digests { digests }
            }));
            servers.push(server);
        }

        let mut addresses = None;
        for _ in 0..80 {
            for server in &mut servers {
                let _ = server.pump_for(Duration::from_millis(10)).await;
            }
            let current = servers
                .iter()
                .map(|server| {
                    server
                        .swarm
                        .listeners()
                        .next()
                        .map(|address| format!("{address}/p2p/{}", server.peer_id()))
                })
                .collect::<Option<Vec<_>>>();
            if current.is_some() {
                addresses = current;
                break;
            }
        }
        let addresses = addresses.expect("all digest servers listen");
        let peer_ids = servers.iter().map(Node::peer_id).collect::<Vec<_>>();

        let mut client = Node::new(0).await.expect("digest client");
        for address in &addresses {
            client.connect_to_peer(address).await.expect("dial server");
        }
        for _ in 0..120 {
            let _ = client.pump_for(Duration::from_millis(10)).await;
            for server in &mut servers {
                let _ = server.pump_for(Duration::from_millis(10)).await;
            }
            if client.connected_peer_count() == servers.len() {
                break;
            }
        }
        assert_eq!(client.connected_peer_count(), servers.len());

        client.block_request_peer_scores.insert(peer_ids[0], 100);
        client.block_request_peer_scores.insert(peer_ids[1], 50);
        client.block_request_peer_scores.insert(peer_ids[2], 0);
        client.wallet_digest_peer_cursor = 0;

        let serving = tokio::spawn(async move {
            for _ in 0..100 {
                for server in &mut servers {
                    let _ = server.pump_for(Duration::from_millis(2)).await;
                }
            }
        });
        let digests = client
            .request_wallet_digests_with_anchor(10, 12, Some("hash-9"), Duration::from_secs(3))
            .await
            .expect("second peer returns a coherent range");
        serving.await.expect("servers task");

        assert_eq!(digests.len(), 3);
        assert_eq!(bad_calls.load(Ordering::SeqCst), 1);
        assert_eq!(good_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            unused_calls.load(Ordering::SeqCst),
            0,
            "a successful second peer stops failover instead of fanning out"
        );
    }

    #[test]
    fn resumenes_rotan_pero_solo_intentan_tres_peers() {
        let peers = (0..5).map(|_| PeerId::random()).collect::<Vec<_>>();

        let selected = rotate_and_limit_wallet_digest_peers(peers.clone(), 4);

        assert_eq!(selected, vec![peers[4], peers[0], peers[1]]);
        assert_eq!(selected.len(), MAX_WALLET_DIGEST_PEER_ATTEMPTS);
        assert_eq!(
            wallet_digest_attempt_budget(Duration::from_secs(60), selected.len()),
            Duration::from_secs(20),
            "tres candidatos conservan una ventana Tor util para cada intento"
        );
    }

    #[test]
    fn resumenes_exigen_cobertura_exacta_del_rango() {
        let digest = |altura| BlockDigest {
            block_height: altura,
            block_hash: format!("hash-{altura}"),
            prev_hash: format!("hash-{}", altura.saturating_sub(1)),
            timestamp: altura,
            coinbase_recipient: None,
            coinbase_amount: None,
            coinbase_output_id: None,
            coinbase_commitment: None,
            coinbase_unlock_height: None,
            private_tx_hashes: Vec::new(),
            private_nullifiers: Vec::new(),
        };

        assert!(Node::wallet_digests_cover_range(
            &[digest(10), digest(11), digest(12)],
            10,
            12
        ));
        assert!(!Node::wallet_digests_cover_range(
            &[digest(10), digest(12)],
            10,
            12
        ));
        assert!(!Node::wallet_digests_cover_range(
            &[digest(10), digest(10), digest(12)],
            10,
            12
        ));
        assert!(!Node::wallet_digests_cover_range(
            &[digest(9), digest(10), digest(11)],
            10,
            12
        ));
        assert!(Node::wallet_digests_cover_range(
            &[digest(12), digest(10), digest(11)],
            10,
            12
        ));

        let mut rama_rota = vec![digest(10), digest(11), digest(12)];
        rama_rota[1].prev_hash = "otra-rama".to_string();
        assert!(!Node::wallet_digests_cover_range(&rama_rota, 10, 12));

        let mut legacy = vec![digest(10)];
        legacy[0].prev_hash.clear();
        assert!(matches!(
            Node::validate_wallet_digest_range(legacy, 10, 10, None),
            Err(WalletDigestValidationError::MissingPrevHash)
        ));

        assert!(Node::validate_wallet_digest_range(
            vec![digest(10), digest(11)],
            10,
            11,
            Some("hash-9")
        )
        .is_ok());
        assert!(matches!(
            Node::validate_wallet_digest_range(
                vec![digest(10), digest(11)],
                10,
                11,
                Some("fork-9")
            ),
            Err(WalletDigestValidationError::BrokenChain)
        ));
    }

    #[test]
    fn deferred_response_queue_is_bounded() {
        let mut queue = VecDeque::new();
        for value in 0..10 {
            push_bounded(&mut queue, value, 3);
        }
        assert_eq!(queue.into_iter().collect::<Vec<_>>(), vec![7, 8, 9]);
    }

    #[test]
    fn private_tx_body_must_decode_and_match_its_canonical_hash() {
        let mut tx: PrivateTransaction = serde_json::from_value(serde_json::json!({
            "tx_hash": "pending",
            "commitment": "commitment",
            "nullifier": "nullifier",
            "zk_proof": null,
            "timestamp": 1,
            "status": "Confirmed"
        }))
        .expect("minimal private tx");
        tx.tx_hash = crate::private_wallet::expected_private_tx_hash(&tx);
        let encoded = serde_json::to_vec(&tx).expect("encode private tx");

        assert!(Node::private_tx_body_matches_hash(&tx.tx_hash, &encoded));
        assert!(!Node::private_tx_body_matches_hash("otro-hash", &encoded));

        let mut corrupted = tx.clone();
        corrupted
            .input_nullifiers
            .push("nullifier-corrupto".to_string());
        let corrupted = serde_json::to_vec(&corrupted).expect("encode corrupt tx");
        assert!(!Node::private_tx_body_matches_hash(&tx.tx_hash, &corrupted));
        assert!(!Node::private_tx_body_matches_hash(
            &tx.tx_hash,
            b"not-json"
        ));
    }

    #[tokio::test]
    async fn un_par_sin_resumenes_deja_caer_a_bloques_enteros() {
        // Un nodo sin responder -- un cliente liviano, o cualquier nodo viejo
        // que no conozca el protocolo -- tiene que contestar que no en vez de
        // dejar la peticion colgada. Si se quedara callado, quien pregunta
        // esperaria el tiempo completo antes de caer al camino lento, y una
        // wallet parece congelada.
        let mut servidor = Node::new(0).await.expect("nodo servidor");
        servidor
            .listen("/ip4/127.0.0.1/tcp/0")
            .await
            .expect("escuchar");

        let mut direccion = None;
        for _ in 0..40 {
            let _ = servidor.pump_for(Duration::from_millis(50)).await;
            if let Some(addr) = servidor.swarm.listeners().next() {
                direccion = Some(addr.clone());
                break;
            }
        }
        let direccion = direccion.expect("el servidor escucha");
        let addr_con_peer = format!("{direccion}/p2p/{}", servidor.peer_id());

        let mut cliente = Node::new(0).await.expect("nodo cliente");
        cliente
            .connect_to_peer(&addr_con_peer)
            .await
            .expect("marcar");
        for _ in 0..60 {
            let _ = cliente.pump_for(Duration::from_millis(30)).await;
            let _ = servidor.pump_for(Duration::from_millis(30)).await;
            if cliente.connected_peer_count() > 0 {
                break;
            }
        }
        assert!(cliente.connected_peer_count() > 0);

        let atender = tokio::spawn(async move {
            for _ in 0..200 {
                let _ = servidor.pump_for(Duration::from_millis(25)).await;
            }
            servidor
        });
        let resumenes = cliente
            .request_wallet_digests(1, 5, Duration::from_secs(10))
            .await;
        let _ = atender.await;

        assert!(
            resumenes.is_none(),
            "sin responder tiene que devolver None para que quien pregunta use bloques enteros"
        );
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
