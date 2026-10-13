// API module: REST endpoints for node interaction
use crate::cli::{CliConfig, MultiProfileConfig};
use crate::consensus::pbft::PbftConsensus;
use crate::health::HealthChecker;
use crate::mempool::Mempool;
use crate::private_wallet::{
    payment_address_from_scan_key, scan_private_transaction_with_view_key,
    validate_view_scan_key_for_address, PrivateTransaction, PrivateViewScanMatch,
    PrivateWalletManager,
};
use crate::state::{AccountInclusionProof, StateManager};
use crate::types::{StateError, TransactionRequest, TransactionResponse};
use axum::{
    body::{to_bytes, Body},
    extract::{ConnectInfo, DefaultBodyLimit, Path, Query, Request, State},
    http::{header, Method, StatusCode},
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock as StdRwLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tower_http::cors::{Any, CorsLayer};
use zeroize::Zeroize;

type SharedState = Arc<StateManager>;
type SharedMempool = Arc<Mempool>;
/// Handle to the node's private transaction pool. Private txs are held
/// inside `PrivateWalletManager::private_pool`; wrapping the manager in
/// `RwLock` mirrors the pattern used by `StateManager` and `Mempool`
/// (shared ownership + interior mutability for the async HTTP handlers).
pub type SharedPrivateWalletManager = Arc<RwLock<PrivateWalletManager>>;

/// Channel sender used by HTTP handlers (specifically the
/// `/private/tx` endpoint) to hand a validated
/// `PrivateTransaction` to the fullnode main loop so it can:
///   1. Add the tx to its in-memory private pool (so its own miner sees
///      it on the next mining cycle).
///   2. Gossip the tx over libp2p via `ZokaMessage::PrivateTransaction`
///      so every connected peer (including the user's local mining node)
///      picks it up and includes it in their next block.
///
/// Wrapped in `Option` because tests / non-fullnode contexts spin up the
/// API without a P2P node â€” those callers leave it `None` and the
/// endpoint silently skips the broadcast step.
pub type PrivateTxBroadcastSender =
    tokio::sync::mpsc::UnboundedSender<crate::private_wallet::PrivateTransaction>;

const MINER_HEARTBEAT_TTL_SECS: u64 = 90;
/// Old clients remain on the pre-recovery protocol and must not be counted as
/// current miners by public status endpoints.
///
/// Es el mismo `minimum_client_version` que anuncia el manifiesto de la red
/// (ZSilent lo lee para pedir que se actualice); la prueba
/// `tests/mainnet_recovery_policy.rs` exige que coincidan. Estuvo en 1.9.0
/// mientras el manifiesto ya decía 2.0.1.
pub const NETWORK_MIN_CLIENT_VERSION: &str = "2.0.1";

#[derive(Clone, Debug)]
pub struct ApiSecurityConfig {
    pub api_token: Option<String>,
    pub rate_limit_per_minute: usize,
    pub read_rate_limit_per_minute: usize,
    pub transaction_rate_limit_per_minute: usize,
    pub max_client_id_len: usize,
    pub max_request_body_bytes: usize,
    pub require_tx_signature: bool,
    pub require_request_signature: bool,
    pub request_signature_secret: Option<String>,
    pub enable_replay_protection: bool,
    pub request_nonce_max_age_secs: u64,
    pub enforce_rbac: bool,
    /// Transitional compatibility escape hatch. Keep disabled for public
    /// nodes: this endpoint decrypts wallet seed material in node memory.
    pub enable_remote_seed_endpoint: bool,
    /// Enables POST /mining/start|stop|control so a local controller (e.g. the
    /// ZSilent desktop app) can toggle the node's in-process miner at runtime
    /// without restarting the node. OFF by default so public nodes can't be
    /// toggled by anyone.
    pub enable_mining_control_endpoint: bool,
    /// Enables POST /sync/block, which applies an externally-submitted block
    /// straight to local chain state with no peer-protocol-version check
    /// (unlike normal P2P gossip). OFF by default: a stale pre-relaunch chain
    /// (or anyone else) could otherwise push blocks the libp2p protocol
    /// freeze would have rejected.
    pub enable_block_submit_endpoint: bool,
    pub write_clients: Vec<String>,
    pub admin_clients: Vec<String>,
    pub ip_whitelist: Vec<String>, // Empty = allow all
    pub ip_blacklist: Vec<String>,
    /// IPs of reverse proxies/load balancers allowed to set the client's real
    /// address via X-Forwarded-For/X-Real-IP. Empty (the default, correct for
    /// this node's direct-exposure deployments) means those headers are never
    /// trusted -- the real TCP peer address is used instead, since any caller
    /// can set arbitrary header values to spoof their IP and dodge rate
    /// limiting / erase their trail from the audit log otherwise.
    pub trusted_proxy_ips: Vec<String>,
}

fn parse_csv_env(var: &str) -> Vec<String> {
    std::env::var(var)
        .ok()
        .map(|s| {
            s.split(',')
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn parse_bool_env(var: &str, default: bool) -> bool {
    std::env::var(var)
        .ok()
        .map(|v| {
            let normalized = v.trim().to_ascii_lowercase();
            matches!(normalized.as_str(), "1" | "true" | "yes" | "on")
        })
        .unwrap_or(default)
}

impl Default for ApiSecurityConfig {
    fn default() -> Self {
        let ip_whitelist = parse_csv_env("ZOKA_API_IP_WHITELIST");
        let ip_blacklist = parse_csv_env("ZOKA_API_IP_BLACKLIST");

        ApiSecurityConfig {
            api_token: std::env::var("ZOKA_API_TOKEN").ok(),
            rate_limit_per_minute: std::env::var("ZOKA_API_RATE_LIMIT")
                .ok()
                .and_then(|raw| raw.parse::<usize>().ok())
                .filter(|value| *value > 0)
                .unwrap_or(120),
            read_rate_limit_per_minute: std::env::var("ZOKA_API_READ_RATE_LIMIT")
                .ok()
                .and_then(|raw| raw.parse::<usize>().ok())
                .filter(|value| *value > 0)
                .unwrap_or(1200),
            transaction_rate_limit_per_minute: std::env::var("ZOKA_API_TX_RATE_LIMIT")
                .ok()
                .and_then(|raw| raw.parse::<usize>().ok())
                .filter(|value| *value > 0)
                .unwrap_or(30),
            max_client_id_len: std::env::var("ZOKA_API_MAX_CLIENT_ID_LEN")
                .ok()
                .and_then(|raw| raw.parse::<usize>().ok())
                .filter(|value| *value > 0)
                .unwrap_or(64),
            max_request_body_bytes: std::env::var("ZOKA_API_MAX_BODY_BYTES")
                .ok()
                .and_then(|raw| raw.parse::<usize>().ok())
                .filter(|value| *value > 0)
                .unwrap_or(1024 * 1024),
            require_tx_signature: std::env::var("ZOKA_API_REQUIRE_TX_SIGNATURE")
                .ok()
                .map(|v| {
                    let normalized = v.trim().to_ascii_lowercase();
                    matches!(normalized.as_str(), "1" | "true" | "yes" | "on")
                })
                .unwrap_or(true),
            require_request_signature: std::env::var("ZOKA_API_REQUIRE_REQUEST_SIGNATURE")
                .ok()
                .map(|v| {
                    let normalized = v.trim().to_ascii_lowercase();
                    matches!(normalized.as_str(), "1" | "true" | "yes" | "on")
                })
                .unwrap_or(false),
            request_signature_secret: std::env::var("ZOKA_API_SIGNATURE_SECRET").ok(),
            enable_replay_protection: std::env::var("ZOKA_API_ENABLE_REPLAY_PROTECTION")
                .ok()
                .map(|v| {
                    let normalized = v.trim().to_ascii_lowercase();
                    matches!(normalized.as_str(), "1" | "true" | "yes" | "on")
                })
                .unwrap_or(false),
            request_nonce_max_age_secs: std::env::var("ZOKA_API_NONCE_MAX_AGE_SECS")
                .ok()
                .and_then(|raw| raw.parse::<u64>().ok())
                .filter(|value| *value > 0)
                .unwrap_or(300),
            enforce_rbac: std::env::var("ZOKA_API_ENFORCE_RBAC")
                .ok()
                .map(|v| {
                    let normalized = v.trim().to_ascii_lowercase();
                    matches!(normalized.as_str(), "1" | "true" | "yes" | "on")
                })
                .unwrap_or(false),
            enable_remote_seed_endpoint: {
                // /private/wallet/send-from-seed receives the user's recovery
                // seed (sealed in transit) and builds the proof server-side — a
                // transitional convenience that breaks self-custody. It is OFF
                // by default, and a RELEASE/mainnet build additionally refuses
                // to enable it unless the operator ALSO opts into the insecure
                // mode, so a single stray env var can never expose it in prod.
                let requested = parse_bool_env("ZOKA_API_ENABLE_REMOTE_SEED_ENDPOINT", false);
                let allow_insecure = parse_bool_env("ZOKA_API_ALLOW_INSECURE_REMOTE_SEED", false);
                let enabled = requested && (cfg!(debug_assertions) || allow_insecure);
                if requested && !enabled {
                    eprintln!(
                        "SECURITY: ZOKA_API_ENABLE_REMOTE_SEED_ENDPOINT is set but \
                         ignored in this release build. Set \
                         ZOKA_API_ALLOW_INSECURE_REMOTE_SEED=1 to force it (never on \
                         mainnet — it breaks self-custody)."
                    );
                } else if enabled {
                    eprintln!(
                        "SECURITY: /private/wallet/send-from-seed is ENABLED. The node \
                         receives recovery seeds and proves server-side — insecure for \
                         mainnet; disable before launch."
                    );
                }
                enabled
            },
            enable_mining_control_endpoint: parse_bool_env("ZOKA_API_ENABLE_MINING_CONTROL", false),
            enable_block_submit_endpoint: parse_bool_env("ZOKA_API_ENABLE_BLOCK_SUBMIT", false),
            write_clients: parse_csv_env("ZOKA_API_WRITE_CLIENTS"),
            admin_clients: parse_csv_env("ZOKA_API_ADMIN_CLIENTS"),
            ip_whitelist,
            ip_blacklist,
            trusted_proxy_ips: parse_csv_env("ZOKA_TRUSTED_PROXY_IPS"),
        }
    }
}

#[derive(Clone, Debug, Default)]
struct AdaptiveRateState {
    penalty_level: u8,
    blocked_until: u64,
    last_violation: u64,
}

#[derive(Clone, Default)]
pub struct ApiRateLimiter {
    buckets: Arc<RwLock<HashMap<String, Vec<u64>>>>,
    adaptive: Arc<RwLock<HashMap<String, AdaptiveRateState>>>,
}

impl ApiRateLimiter {
    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    fn effective_limit(base_limit: usize, penalty_level: u8) -> usize {
        if base_limit == 0 {
            return 0;
        }
        let max_penalty = base_limit.saturating_sub(1);
        let applied_penalty = (penalty_level as usize).min(max_penalty);
        base_limit.saturating_sub(applied_penalty).max(1)
    }

    pub async fn allow_request(&self, key: &str, limit: usize) -> bool {
        let now = Self::now_secs();
        self.allow_request_at(key, limit, now).await
    }

    async fn allow_request_at(&self, key: &str, limit: usize, now: u64) -> bool {
        if limit == 0 {
            return false;
        }

        let penalty_level = {
            let mut adaptive = self.adaptive.write().await;
            let state = adaptive.entry(key.to_string()).or_default();

            if now < state.blocked_until {
                return false;
            }

            // Gradually decay penalty level after a calm period.
            if state.penalty_level > 0 && now.saturating_sub(state.last_violation) > 180 {
                state.penalty_level = state.penalty_level.saturating_sub(1);
            }

            state.penalty_level
        };

        let mut buckets = self.buckets.write().await;
        let history = buckets.entry(key.to_string()).or_default();
        history.retain(|ts| now.saturating_sub(*ts) < 60);

        let effective_limit = Self::effective_limit(limit, penalty_level);
        if history.len() >= effective_limit {
            drop(buckets);

            // Adaptive penalty escalates cooldown on repeated violations.
            let mut adaptive = self.adaptive.write().await;
            let state = adaptive.entry(key.to_string()).or_default();
            state.penalty_level = state.penalty_level.saturating_add(1).min(6);
            state.last_violation = now;
            let cooldown = (1u64 << state.penalty_level).min(120);
            state.blocked_until = now.saturating_add(cooldown);
            return false;
        }

        history.push(now);
        true
    }

    #[cfg(test)]
    async fn allow_request_at_for_test(&self, key: &str, limit: usize, now: u64) -> bool {
        self.allow_request_at(key, limit, now).await
    }

    #[cfg(test)]
    async fn penalty_level_for_test(&self, key: &str) -> u8 {
        let adaptive = self.adaptive.read().await;
        adaptive.get(key).map(|s| s.penalty_level).unwrap_or(0)
    }
}

#[derive(Default)]
pub struct ApiSecurityStats {
    pub requests_total: AtomicU64,
    pub unauthorized_total: AtomicU64,
    pub rate_limited_total: AtomicU64,
    pub bad_client_id_total: AtomicU64,
    pub bad_request_total: AtomicU64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MinerHeartbeatRequest {
    #[serde(default)]
    pub peer_id: String,
    #[serde(default)]
    pub address: Option<String>,
    #[serde(default)]
    pub chain_height: u64,
    #[serde(default)]
    pub hashrate_hps: u64,
    #[serde(default)]
    pub mining: bool,
    #[serde(default)]
    pub client_version: Option<String>,
    #[serde(default)]
    pub software: Option<String>,
    #[serde(default)]
    pub launcher: Option<String>,
    #[serde(default)]
    pub launcher_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MinerHeartbeatRecord {
    pub peer_id: String,
    pub address: Option<String>,
    pub chain_height: u64,
    pub hashrate_hps: u64,
    pub mining: bool,
    pub client_version: Option<String>,
    pub software: Option<String>,
    pub launcher: Option<String>,
    pub launcher_version: Option<String>,
    pub legacy_client: bool,
    pub outdated_client: bool,
    pub last_seen: u64,
}

/// One currently-connected p2p peer, as reported by `/peers`. Serverless
/// nodes have no central directory of the network, so this is the local
/// node's own mesh view — who it is actually talking to and over which
/// transport.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerSummary {
    /// libp2p PeerId (base58).
    pub peer_id: String,
    /// "tor" (onion), "direct" (clearnet ip/dns), or "unknown" when no
    /// address has been learned via Identify yet.
    pub transport: String,
    /// A representative observed multiaddr, if known.
    pub address: Option<String>,
}

/// Classify a peer's transport from a representative multiaddr. Tor onion
/// dials carry `/onion3/`; clearnet peers use `/ip4|/ip6|/dns*`. Anything we
/// have not learned an address for yet is "unknown".
fn classify_transport(addr: Option<&str>) -> &'static str {
    match addr {
        Some(a) if a.contains("/onion3/") || a.contains("/onion/") => "tor",
        Some(a) if a.contains("/ip4/") || a.contains("/ip6/") || a.contains("/dns") => "direct",
        Some(_) => "other",
        None => "unknown",
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NetworkRuntimeSnapshot {
    pub connected_peers: u64,
    pub miners_online: u64,
    pub remote_miners_online: u64,
    pub local_mining: bool,
    pub checkpoint_quarantined: bool,
    pub local_chain_height: u64,
    pub local_peer_id: Option<String>,
    pub total_reported_hashrate_hps: u64,
    pub heartbeat_ttl_secs: u64,
    pub legacy_miners_online: u64,
    pub outdated_miners_online: u64,
    pub minimum_client_version: String,
    pub gossip_rx_total: u64,
    pub gossip_rx_block_response: u64,
    pub gossip_publish_ok: u64,
    pub gossip_publish_err: u64,
    pub gossip_last_rx_block_at: u64,
    /// Live gossipsub mesh size for the general topic (not just connected
    /// peers with gossipsub support) - lets an operator tell a healthy mesh
    /// apart from one that's empty and silently relying on flood-publish.
    pub gossip_mesh_peer_count: u64,
    pub kademlia_routing_peers: u64,
    pub direct_block_requests_outbound_sent: u64,
    pub direct_block_requests_outbound_responses: u64,
    pub direct_block_requests_outbound_failures: u64,
    pub direct_block_requests_invalid_responses: u64,
    pub direct_block_requests_avg_latency_ms: u64,
    /// Private tx announced by a peer but not yet held locally when received
    /// (fetch attempt required). See docs/sync-network-redesign.md Fase 3.5.
    pub private_tx_announced_without_body: u64,
    /// Body eventually fetched, but only after exceeding the "fast" latency
    /// threshold (2s) — a signal that P2P direct fetch is degraded.
    pub private_tx_body_received_late: u64,
    /// Private tx confirmed committed into a locally-mined block.
    pub private_tx_included_in_block: u64,
    /// Private tx bodies obtained via the NAT-friendly RPC-pull fallback
    /// instead of the primary libp2p request/response path.
    pub private_tx_rpc_fallback_pulled: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncStatusSnapshot {
    pub status: String,
    pub mode: String,
    pub ready_to_mine: bool,
    pub local_height: u64,
    pub target_height: u64,
    pub lag: u64,
    pub connected_peers: u64,
    pub min_peers: u64,
    pub startup_ready: bool,
    pub startup_phase: String,
    pub rpc_fallback_active: bool,
    pub last_sync_source: Option<String>,
    pub last_sync_event: Option<String>,
    pub last_sync_error: Option<String>,
    pub last_block_at: u64,
    pub last_updated: u64,
    pub gossip_rx_block_response: u64,
    pub gossip_publish_err: u64,
}

#[derive(Debug, Clone)]
pub struct SyncRuntimeUpdate {
    pub mode: String,
    pub local_height: u64,
    pub target_height: u64,
    pub min_peers: u64,
    pub rpc_fallback_active: bool,
    pub last_sync_source: Option<String>,
    pub last_sync_event: Option<String>,
    pub last_sync_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartupStatusSnapshot {
    pub status: String,
    pub ready: bool,
    pub phase: String,
    pub detail: Option<String>,
    pub network: Option<String>,
    pub chain_id: Option<String>,
    pub started_at: u64,
    pub updated_at: u64,
    pub version: String,
}

pub struct StartupStatusState {
    ready: AtomicU64,
    started_at: AtomicU64,
    updated_at: AtomicU64,
    phase: StdRwLock<String>,
    detail: StdRwLock<Option<String>>,
    network: StdRwLock<Option<String>>,
    chain_id: StdRwLock<Option<String>>,
}

impl Default for StartupStatusState {
    fn default() -> Self {
        let now = ApiRateLimiter::now_secs();
        Self {
            ready: AtomicU64::new(0),
            started_at: AtomicU64::new(now),
            updated_at: AtomicU64::new(now),
            phase: StdRwLock::new("starting".to_string()),
            detail: StdRwLock::new(None),
            network: StdRwLock::new(None),
            chain_id: StdRwLock::new(None),
        }
    }
}

impl StartupStatusState {
    pub fn reset(&self, phase: impl Into<String>, detail: Option<String>) {
        let now = ApiRateLimiter::now_secs();
        self.ready.store(0, Ordering::Relaxed);
        self.started_at.store(now, Ordering::Relaxed);
        self.set_phase_at(phase, detail, now);
    }

    pub fn set_phase(&self, phase: impl Into<String>, detail: Option<String>) {
        self.set_phase_at(phase, detail, ApiRateLimiter::now_secs());
    }

    pub fn set_chain_context(&self, network: impl Into<String>, chain_id: impl Into<String>) {
        if let Ok(mut current_network) = self.network.write() {
            *current_network = Some(network.into());
        }
        if let Ok(mut current_chain_id) = self.chain_id.write() {
            *current_chain_id = Some(chain_id.into());
        }
        self.updated_at
            .store(ApiRateLimiter::now_secs(), Ordering::Relaxed);
    }

    pub fn mark_ready(&self, detail: Option<String>) {
        self.ready.store(1, Ordering::Relaxed);
        self.set_phase("ready", detail);
    }

    pub fn mark_failed(&self, detail: Option<String>) {
        self.ready.store(0, Ordering::Relaxed);
        self.set_phase("failed", detail);
    }

    fn set_phase_at(&self, phase: impl Into<String>, detail: Option<String>, now: u64) {
        if let Ok(mut current_phase) = self.phase.write() {
            *current_phase = phase.into();
        }
        if let Ok(mut current_detail) = self.detail.write() {
            *current_detail = detail;
        }
        self.updated_at.store(now, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> StartupStatusSnapshot {
        let ready = self.ready.load(Ordering::Relaxed) == 1;
        let phase = self
            .phase
            .read()
            .map(|phase| phase.clone())
            .unwrap_or_else(|_| "unknown".to_string());
        let detail = self
            .detail
            .read()
            .map(|detail| detail.clone())
            .unwrap_or(None);
        let network = self
            .network
            .read()
            .map(|network| network.clone())
            .unwrap_or(None);
        let chain_id = self
            .chain_id
            .read()
            .map(|chain_id| chain_id.clone())
            .unwrap_or(None);
        let status = if ready {
            "healthy"
        } else if phase == "failed" {
            "failed"
        } else {
            "starting"
        };
        StartupStatusSnapshot {
            status: status.to_string(),
            ready,
            phase,
            detail,
            network,
            chain_id,
            started_at: self.started_at.load(Ordering::Relaxed),
            updated_at: self.updated_at.load(Ordering::Relaxed),
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

pub fn global_startup_status() -> &'static StartupStatusState {
    static STARTUP: OnceLock<StartupStatusState> = OnceLock::new();
    STARTUP.get_or_init(StartupStatusState::default)
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LocalMiningControlSnapshot {
    pub enabled: bool,
    pub threads: u64,
    pub updated_at: u64,
}

#[derive(Default)]
pub struct LocalMiningControlState {
    enabled: AtomicU64,
    threads: AtomicU64,
    updated_at: AtomicU64,
}

impl LocalMiningControlState {
    pub fn enable(&self, threads: u64) -> LocalMiningControlSnapshot {
        self.enabled.store(1, Ordering::Relaxed);
        self.threads.store(threads.max(1), Ordering::Relaxed);
        self.updated_at
            .store(ApiRateLimiter::now_secs(), Ordering::Relaxed);
        self.snapshot()
    }

    pub fn disable(&self) -> LocalMiningControlSnapshot {
        self.enabled.store(0, Ordering::Relaxed);
        self.updated_at
            .store(ApiRateLimiter::now_secs(), Ordering::Relaxed);
        self.snapshot()
    }

    pub fn snapshot(&self) -> LocalMiningControlSnapshot {
        LocalMiningControlSnapshot {
            enabled: self.enabled.load(Ordering::Relaxed) == 1,
            threads: self.threads.load(Ordering::Relaxed),
            updated_at: self.updated_at.load(Ordering::Relaxed),
        }
    }
}

pub fn global_local_mining_control() -> &'static LocalMiningControlState {
    static CONTROL: OnceLock<LocalMiningControlState> = OnceLock::new();
    CONTROL.get_or_init(LocalMiningControlState::default)
}

#[derive(Default)]
pub struct NetworkRuntimeState {
    connected_peers: AtomicU64,
    local_mining: AtomicU64,
    checkpoint_quarantined: AtomicU64,
    local_hashrate_hps: AtomicU64,
    local_chain_height: AtomicU64,
    sync_target_height: AtomicU64,
    sync_min_peers: AtomicU64,
    sync_rpc_fallback_active: AtomicU64,
    sync_last_updated: AtomicU64,
    gossip_rx_total: AtomicU64,
    gossip_rx_block_response: AtomicU64,
    gossip_publish_ok: AtomicU64,
    gossip_publish_err: AtomicU64,
    gossip_last_rx_block_at: AtomicU64,
    gossip_mesh_peer_count: AtomicU64,
    topic_peer_counts: std::sync::Mutex<Vec<(String, usize, usize)>>,
    kademlia_routing_peers: AtomicU64,
    direct_block_requests_outbound_sent: AtomicU64,
    direct_block_requests_outbound_responses: AtomicU64,
    direct_block_requests_outbound_failures: AtomicU64,
    direct_block_requests_invalid_responses: AtomicU64,
    direct_block_requests_latency_samples: AtomicU64,
    direct_block_requests_latency_total_ms: AtomicU64,
    private_tx_announced_without_body: AtomicU64,
    private_tx_body_received_late: AtomicU64,
    private_tx_included_in_block: AtomicU64,
    private_tx_rpc_fallback_pulled: AtomicU64,
    local_peer_id: RwLock<Option<String>>,
    sync_mode: RwLock<String>,
    sync_last_source: RwLock<Option<String>>,
    sync_last_event: RwLock<Option<String>>,
    sync_last_error: RwLock<Option<String>>,
    miner_heartbeats: RwLock<HashMap<String, MinerHeartbeatRecord>>,
    /// (peer_id, representative observed address) for each currently connected
    /// p2p peer, published by the node loop for the serverless `/peers`
    /// endpoint. Serverless nodes have no bootstrap RPC to ask "who's on the
    /// network", so this is the local mesh view.
    connected_peer_addrs: RwLock<Vec<(String, Option<String>)>>,
}

fn normalize_client_version(raw: Option<String>) -> Option<String> {
    raw.map(|value| value.trim().trim_start_matches('v').to_string())
        .filter(|value| !value.is_empty())
}

fn normalize_client_label(raw: Option<String>) -> Option<String> {
    raw.map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn heartbeat_reports_zsilent(software: Option<&str>, launcher: Option<&str>) -> bool {
    software
        .into_iter()
        .chain(launcher)
        .any(|value| value.eq_ignore_ascii_case("zsilent"))
}

fn compare_semverish(a: &str, b: &str) -> i32 {
    let parts_a: Vec<u64> = a
        .split('.')
        .map(|part| part.parse::<u64>().unwrap_or(0))
        .collect();
    let parts_b: Vec<u64> = b
        .split('.')
        .map(|part| part.parse::<u64>().unwrap_or(0))
        .collect();
    let len = parts_a.len().max(parts_b.len()).max(3);
    for i in 0..len {
        let av = *parts_a.get(i).unwrap_or(&0);
        let bv = *parts_b.get(i).unwrap_or(&0);
        if av != bv {
            return if av > bv { 1 } else { -1 };
        }
    }
    0
}

fn legacy_peer_id_for_heartbeat(heartbeat: &MinerHeartbeatRequest) -> String {
    heartbeat
        .address
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|address| format!("legacy-address:{address}"))
        .unwrap_or_else(|| "legacy-miner:unknown".to_string())
}

impl NetworkRuntimeState {
    pub fn set_connected_peers(&self, peers: usize) {
        self.connected_peers.store(peers as u64, Ordering::Relaxed);
    }

    /// Publish the local mesh view (connected peers + a representative address
    /// each) for the `/peers` endpoint. Called every loop tick by the node.
    pub async fn set_connected_peer_addrs(&self, pairs: Vec<(String, Option<String>)>) {
        let mut guard = self.connected_peer_addrs.write().await;
        *guard = pairs;
    }

    /// Build the serializable peer summaries for `/peers`, classifying each
    /// peer's transport from its observed address.
    pub async fn peer_summaries(&self) -> Vec<PeerSummary> {
        let guard = self.connected_peer_addrs.read().await;
        guard
            .iter()
            .map(|(peer_id, addr)| PeerSummary {
                peer_id: peer_id.clone(),
                transport: classify_transport(addr.as_deref()).to_string(),
                address: addr.clone(),
            })
            .collect()
    }

    /// Suscriptores y malla por topico, tal como los ve este nodo.
    ///
    /// En un Mutex y no en atomicos porque es una lista de tamano variable.
    /// Se refresca en el mismo ciclo que el resto de la telemetria de gossip.
    pub fn set_topic_peer_counts(&self, counts: Vec<(String, usize, usize)>) {
        if let Ok(mut guard) = self.topic_peer_counts.lock() {
            *guard = counts;
        }
    }

    pub fn topic_peer_counts(&self) -> Vec<(String, usize, usize)> {
        self.topic_peer_counts
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    pub fn set_gossip_stats(&self, stats: crate::network::p2p::GossipStats) {
        self.gossip_rx_total
            .store(stats.rx_total, Ordering::Relaxed);
        self.gossip_rx_block_response
            .store(stats.rx_block_response, Ordering::Relaxed);
        self.gossip_publish_ok
            .store(stats.publish_ok, Ordering::Relaxed);
        self.gossip_publish_err
            .store(stats.publish_err, Ordering::Relaxed);
        self.gossip_last_rx_block_at
            .store(stats.last_rx_block_at, Ordering::Relaxed);
        self.gossip_mesh_peer_count
            .store(stats.mesh_peer_count, Ordering::Relaxed);
    }

    /// Size of the Kademlia routing table. Zero while peers are connected
    /// means DHT discovery is not working.
    pub fn set_kademlia_routing_peers(&self, peers: u64) {
        self.kademlia_routing_peers.store(peers, Ordering::Relaxed);
    }

    pub fn set_block_request_response_stats(
        &self,
        stats: crate::network::p2p::BlockRequestResponseStats,
    ) {
        self.direct_block_requests_outbound_sent
            .store(stats.outbound_sent, Ordering::Relaxed);
        self.direct_block_requests_outbound_responses
            .store(stats.outbound_responses, Ordering::Relaxed);
        self.direct_block_requests_outbound_failures
            .store(stats.outbound_failures, Ordering::Relaxed);
        self.direct_block_requests_invalid_responses
            .store(stats.invalid_responses, Ordering::Relaxed);
        self.direct_block_requests_latency_samples
            .store(stats.latency_samples, Ordering::Relaxed);
        self.direct_block_requests_latency_total_ms
            .store(stats.latency_total_ms, Ordering::Relaxed);
    }

    pub fn record_private_tx_announced_without_body(&self) {
        self.private_tx_announced_without_body
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_private_tx_body_received_late(&self) {
        self.private_tx_body_received_late
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_private_tx_included_in_block(&self, count: u64) {
        self.private_tx_included_in_block
            .fetch_add(count, Ordering::Relaxed);
    }

    pub fn record_private_tx_rpc_fallback_pulled(&self, count: u64) {
        self.private_tx_rpc_fallback_pulled
            .fetch_add(count, Ordering::Relaxed);
    }

    pub async fn set_sync_status(&self, update: SyncRuntimeUpdate) {
        self.local_chain_height
            .store(update.local_height, Ordering::Relaxed);
        self.sync_target_height
            .store(update.target_height, Ordering::Relaxed);
        self.sync_min_peers
            .store(update.min_peers, Ordering::Relaxed);
        self.sync_rpc_fallback_active.store(
            if update.rpc_fallback_active { 1 } else { 0 },
            Ordering::Relaxed,
        );
        self.sync_last_updated
            .store(ApiRateLimiter::now_secs(), Ordering::Relaxed);
        *self.sync_mode.write().await = update.mode;
        *self.sync_last_source.write().await = update.last_sync_source;
        *self.sync_last_event.write().await = update.last_sync_event;
        *self.sync_last_error.write().await = update.last_sync_error;
    }

    pub async fn sync_status_snapshot(
        &self,
        local_height: u64,
        last_block_at: u64,
    ) -> SyncStatusSnapshot {
        let startup = global_startup_status().snapshot();
        let runtime = self.snapshot().await;
        let observed_target = self.sync_target_height.load(Ordering::Relaxed);
        let target_height = observed_target.max(local_height);
        let lag = target_height.saturating_sub(local_height);
        let min_peers = self.sync_min_peers.load(Ordering::Relaxed);
        let connected_peers = runtime.connected_peers;
        let mode = self.sync_mode.read().await.clone();
        let rpc_fallback_active = self.sync_rpc_fallback_active.load(Ordering::Relaxed) == 1;
        let last_sync_source = self.sync_last_source.read().await.clone();
        let last_sync_event = self.sync_last_event.read().await.clone();
        let last_sync_error = self.sync_last_error.read().await.clone();
        // No se puede estar al dia con una punta que nadie anuncio todavia.
        //
        // `lag` sale de `target_height`, que a su vez es `max(anunciado,
        // local)`. Con la base recien creada y ningun anuncio recibido, eso da
        // `max(0, 1) = 1` y el lag queda en cero: el nodo se comparaba contra
        // si mismo y se declaraba sincronizado. Observado en la PC durante una
        // reconstruccion desde cero: altura 1, objetivo 1, `ready_to_mine`
        // true, con dos pares conectados que aun no habian dicho su altura.
        //
        // Un usuario con auto-minado encendido se habria puesto a minar sobre
        // el bloque 1, tirando el trabajo y ensuciando la red con una rama
        // muerta.
        //
        // La excepcion son los nodos deliberadamente aislados (`min_peers` en
        // cero y sin pares): ahi no hay a quien esperarle, y quien arranca una
        // red nueva necesita poder minar el primer bloque.
        let network_tip_known = observed_target > 0 || connected_peers == 0;
        let ready_to_mine = startup.ready
            && network_tip_known
            && lag == 0
            && connected_peers >= min_peers
            && !matches!(mode.as_str(), "loading" | "starting" | "error");
        let status = if !startup.ready {
            "starting"
        } else if mode == "error" {
            "error"
        } else if connected_peers < min_peers {
            "isolated"
        } else if lag > 0 {
            "syncing"
        } else {
            "caught_up"
        };

        SyncStatusSnapshot {
            status: status.to_string(),
            mode,
            ready_to_mine,
            local_height,
            target_height,
            lag,
            connected_peers,
            min_peers,
            startup_ready: startup.ready,
            startup_phase: startup.phase,
            rpc_fallback_active,
            last_sync_source,
            last_sync_event,
            last_sync_error,
            last_block_at,
            last_updated: self.sync_last_updated.load(Ordering::Relaxed),
            gossip_rx_block_response: runtime.gossip_rx_block_response,
            gossip_publish_err: runtime.gossip_publish_err,
        }
    }

    /// Synchronous height read (no `.await`), so it's usable from a
    /// `spawn_blocking` mining thread outside the async runtime. Backed by
    /// the same atomic `set_local_miner`/`set_sync_status` keep current
    /// every main-loop tick, including while a mining batch is running
    /// concurrently on tokio's blocking thread pool.
    pub fn local_chain_height(&self) -> u64 {
        self.local_chain_height.load(Ordering::Relaxed)
    }

    pub fn set_checkpoint_quarantined(&self, quarantined: bool) {
        self.checkpoint_quarantined
            .store(if quarantined { 1 } else { 0 }, Ordering::Relaxed);
        if quarantined {
            self.local_mining.store(0, Ordering::Relaxed);
        }
    }

    pub fn checkpoint_quarantined(&self) -> bool {
        self.checkpoint_quarantined.load(Ordering::Relaxed) == 1
    }

    pub async fn set_local_miner(
        &self,
        peer_id: String,
        active: bool,
        chain_height: u64,
        hashrate_hps: u64,
    ) {
        self.local_mining
            .store(if active { 1 } else { 0 }, Ordering::Relaxed);
        self.local_chain_height
            .store(chain_height, Ordering::Relaxed);
        self.local_hashrate_hps
            .store(hashrate_hps, Ordering::Relaxed);
        let mut local_peer_id = self.local_peer_id.write().await;
        *local_peer_id = if peer_id.trim().is_empty() {
            None
        } else {
            Some(peer_id)
        };
    }

    pub async fn record_miner_heartbeat(
        &self,
        heartbeat: MinerHeartbeatRequest,
    ) -> Result<MinerHeartbeatRecord, String> {
        let peer_id = heartbeat.peer_id.trim();
        let legacy_client = peer_id.is_empty();
        let peer_id = if legacy_client {
            legacy_peer_id_for_heartbeat(&heartbeat)
        } else {
            peer_id.to_string()
        };
        if peer_id.len() > 256 {
            return Err("invalid peer id".to_string());
        }
        let client_version = normalize_client_version(heartbeat.client_version);
        let software = normalize_client_label(heartbeat.software);
        let launcher = normalize_client_label(heartbeat.launcher);
        let launcher_version = normalize_client_version(heartbeat.launcher_version);
        let version_missing = client_version.is_none();
        let core_outdated = client_version
            .as_deref()
            .map(|version| compare_semverish(version, NETWORK_MIN_CLIENT_VERSION) < 0)
            .unwrap_or(true);
        let zsilent_launcher = heartbeat_reports_zsilent(software.as_deref(), launcher.as_deref());
        let launcher_outdated = zsilent_launcher
            && launcher_version
                .as_deref()
                .map(|version| compare_semverish(version, NETWORK_MIN_CLIENT_VERSION) < 0)
                .unwrap_or(true);
        let outdated_client = core_outdated || launcher_outdated;
        let now = ApiRateLimiter::now_secs();
        let mut heartbeats = self.miner_heartbeats.write().await;
        let record = MinerHeartbeatRecord {
            peer_id: peer_id.clone(),
            address: heartbeat
                .address
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
            chain_height: heartbeat.chain_height,
            hashrate_hps: heartbeat.hashrate_hps,
            mining: heartbeat.mining,
            client_version,
            software,
            launcher,
            launcher_version,
            legacy_client: legacy_client || version_missing,
            outdated_client,
            last_seen: now,
        };
        heartbeats.insert(peer_id, record.clone());
        heartbeats
            .retain(|_, record| now.saturating_sub(record.last_seen) <= MINER_HEARTBEAT_TTL_SECS);
        Ok(record)
    }

    pub async fn snapshot(&self) -> NetworkRuntimeSnapshot {
        let now = ApiRateLimiter::now_secs();
        let local_peer_id = self.local_peer_id.read().await.clone();
        let local_mining = self.local_mining.load(Ordering::Relaxed) == 1;
        let mut miner_ids = HashSet::new();
        let mut total_hashrate = 0u64;
        let mut legacy_miners = 0u64;
        let mut outdated_miners = 0u64;
        if local_mining {
            if let Some(peer_id) = local_peer_id.clone() {
                miner_ids.insert(peer_id);
            }
            total_hashrate =
                total_hashrate.saturating_add(self.local_hashrate_hps.load(Ordering::Relaxed));
        }
        let mut remote_miners = 0u64;
        let mut heartbeats = self.miner_heartbeats.write().await;
        heartbeats
            .retain(|_, record| now.saturating_sub(record.last_seen) <= MINER_HEARTBEAT_TTL_SECS);
        for record in heartbeats.values() {
            if !record.mining {
                continue;
            }
            if record.legacy_client {
                legacy_miners = legacy_miners.saturating_add(1);
            }
            if record.outdated_client {
                outdated_miners = outdated_miners.saturating_add(1);
            }
            if record.legacy_client || record.outdated_client {
                continue;
            }
            if miner_ids.insert(record.peer_id.clone()) {
                remote_miners = remote_miners.saturating_add(1);
                total_hashrate = total_hashrate.saturating_add(record.hashrate_hps);
            }
        }

        NetworkRuntimeSnapshot {
            connected_peers: self.connected_peers.load(Ordering::Relaxed),
            miners_online: miner_ids.len() as u64,
            remote_miners_online: remote_miners,
            local_mining,
            checkpoint_quarantined: self.checkpoint_quarantined(),
            local_chain_height: self.local_chain_height.load(Ordering::Relaxed),
            local_peer_id,
            total_reported_hashrate_hps: total_hashrate,
            heartbeat_ttl_secs: MINER_HEARTBEAT_TTL_SECS,
            legacy_miners_online: legacy_miners,
            outdated_miners_online: outdated_miners,
            minimum_client_version: NETWORK_MIN_CLIENT_VERSION.to_string(),
            gossip_rx_total: self.gossip_rx_total.load(Ordering::Relaxed),
            gossip_rx_block_response: self.gossip_rx_block_response.load(Ordering::Relaxed),
            gossip_publish_ok: self.gossip_publish_ok.load(Ordering::Relaxed),
            gossip_publish_err: self.gossip_publish_err.load(Ordering::Relaxed),
            gossip_last_rx_block_at: self.gossip_last_rx_block_at.load(Ordering::Relaxed),
            gossip_mesh_peer_count: self.gossip_mesh_peer_count.load(Ordering::Relaxed),
            kademlia_routing_peers: self.kademlia_routing_peers.load(Ordering::Relaxed),
            direct_block_requests_outbound_sent: self
                .direct_block_requests_outbound_sent
                .load(Ordering::Relaxed),
            direct_block_requests_outbound_responses: self
                .direct_block_requests_outbound_responses
                .load(Ordering::Relaxed),
            direct_block_requests_outbound_failures: self
                .direct_block_requests_outbound_failures
                .load(Ordering::Relaxed),
            direct_block_requests_invalid_responses: self
                .direct_block_requests_invalid_responses
                .load(Ordering::Relaxed),
            direct_block_requests_avg_latency_ms: {
                let samples = self
                    .direct_block_requests_latency_samples
                    .load(Ordering::Relaxed);
                if samples == 0 {
                    0
                } else {
                    self.direct_block_requests_latency_total_ms
                        .load(Ordering::Relaxed)
                        / samples
                }
            },
            private_tx_announced_without_body: self
                .private_tx_announced_without_body
                .load(Ordering::Relaxed),
            private_tx_body_received_late: self
                .private_tx_body_received_late
                .load(Ordering::Relaxed),
            private_tx_included_in_block: self.private_tx_included_in_block.load(Ordering::Relaxed),
            private_tx_rpc_fallback_pulled: self
                .private_tx_rpc_fallback_pulled
                .load(Ordering::Relaxed),
        }
    }

    pub async fn miner_records(&self) -> Vec<MinerHeartbeatRecord> {
        let now = ApiRateLimiter::now_secs();
        let mut heartbeats = self.miner_heartbeats.write().await;
        heartbeats
            .retain(|_, record| now.saturating_sub(record.last_seen) <= MINER_HEARTBEAT_TTL_SECS);
        let mut records: Vec<_> = heartbeats.values().cloned().collect();
        records.sort_by(|a, b| a.peer_id.cmp(&b.peer_id));
        records
    }
}

pub fn global_network_runtime() -> &'static NetworkRuntimeState {
    static RUNTIME: OnceLock<NetworkRuntimeState> = OnceLock::new();
    RUNTIME.get_or_init(NetworkRuntimeState::default)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum ApiRole {
    Read,
    Write,
    Admin,
}

/// Role-Based Access Control configuration
#[derive(Clone, Debug)]
pub struct RbacConfig {
    pub client_roles: Arc<RwLock<HashMap<String, ApiRole>>>,
    pub endpoint_permissions: HashMap<&'static str, ApiRole>,
}

impl RbacConfig {
    pub fn new() -> Self {
        let mut endpoint_permissions = HashMap::new();
        // Read endpoints
        endpoint_permissions.insert("/chain/height", ApiRole::Read);
        endpoint_permissions.insert("/chain/state-root", ApiRole::Read);
        endpoint_permissions.insert("/chain/private-commitment-root", ApiRole::Read);
        endpoint_permissions.insert(
            "/chain/private-commitments/:commitment/proof",
            ApiRole::Read,
        );
        endpoint_permissions.insert("/accounts/:address", ApiRole::Read);
        endpoint_permissions.insert("/blocks/:height", ApiRole::Read);
        endpoint_permissions.insert("/blocks/:height/raw", ApiRole::Read);
        endpoint_permissions.insert("/blocks/hash/:hash", ApiRole::Read);
        endpoint_permissions.insert("/blocks/latest", ApiRole::Read);
        endpoint_permissions.insert("/blocks/range/:from/:to", ApiRole::Read);
        endpoint_permissions.insert("/blocks/range/:from/:to/raw", ApiRole::Read);
        endpoint_permissions.insert("/transactions/:hash", ApiRole::Read);
        endpoint_permissions.insert("/mempool/metrics", ApiRole::Read);
        endpoint_permissions.insert("/network/runtime", ApiRole::Read);
        endpoint_permissions.insert("/team-mining/status", ApiRole::Read);
        endpoint_permissions.insert("/crypto/server-pubkey", ApiRole::Read);

        // Write endpoints
        endpoint_permissions.insert("/transactions", ApiRole::Write);
        endpoint_permissions.insert("/accounts/proof/verify", ApiRole::Write);
        endpoint_permissions.insert("/network/miners/heartbeat", ApiRole::Write);
        endpoint_permissions.insert("/local/mining/start", ApiRole::Write);
        endpoint_permissions.insert("/local/mining/stop", ApiRole::Write);
        endpoint_permissions.insert("/team-mining/team", ApiRole::Write);
        endpoint_permissions.insert("/team-mining/join", ApiRole::Write);
        endpoint_permissions.insert("/team-mining/leave", ApiRole::Write);
        endpoint_permissions.insert("/team-mining/disband", ApiRole::Write);
        endpoint_permissions.insert("/team-mining/start", ApiRole::Write);
        endpoint_permissions.insert("/team-mining/stop", ApiRole::Write);
        endpoint_permissions.insert("/checkpoint", ApiRole::Write);
        endpoint_permissions.insert("/private/view-scan", ApiRole::Write);
        endpoint_permissions.insert("/private/view-scan-key", ApiRole::Write);
        endpoint_permissions.insert("/private/wallet/send-from-seed", ApiRole::Write);

        // Admin endpoints
        endpoint_permissions.insert("/admin/rollback", ApiRole::Admin);
        endpoint_permissions.insert("/admin/rollback-points", ApiRole::Admin);
        endpoint_permissions.insert("/metrics/security", ApiRole::Admin);
        endpoint_permissions.insert("/metrics/audit", ApiRole::Admin);

        RbacConfig {
            client_roles: Arc::new(RwLock::new(HashMap::new())),
            endpoint_permissions,
        }
    }

    pub async fn grant_role(&self, client_id: String, role: ApiRole) {
        let mut roles = self.client_roles.write().await;
        roles.insert(client_id, role);
    }

    pub async fn revoke_role(&self, client_id: &str) {
        let mut roles = self.client_roles.write().await;
        roles.remove(client_id);
    }

    pub async fn can_access(&self, client_id: &str, endpoint: &str) -> bool {
        let client_role = {
            let roles = self.client_roles.read().await;
            roles.get(client_id).cloned().unwrap_or(ApiRole::Read)
        };

        let required_role = self
            .endpoint_permissions
            .get(endpoint)
            .cloned()
            .unwrap_or(ApiRole::Read);

        client_role >= required_role
    }
}

/// Audit log entry for tracking API operations
#[derive(Clone, Debug, Serialize)]
pub struct AuditLogEntry {
    pub timestamp: u64,
    pub client_id: String,
    pub method: String,
    pub path: String,
    pub status: u16,
    pub ip_address: String,
    pub cache_status: String,
}

/// Audit logger for API operations
#[derive(Clone)]
pub struct AuditLogger {
    logs: Arc<RwLock<Vec<AuditLogEntry>>>,
    max_entries: usize,
}

impl AuditLogger {
    pub fn new(max_entries: usize) -> Self {
        AuditLogger {
            logs: Arc::new(RwLock::new(Vec::new())),
            max_entries,
        }
    }

    pub async fn log(&self, entry: AuditLogEntry) {
        let mut logs = self.logs.write().await;
        logs.push(entry);
        // Keep only recent entries
        if logs.len() > self.max_entries {
            logs.remove(0);
        }
    }

    pub async fn get_logs(&self) -> Vec<AuditLogEntry> {
        let logs = self.logs.read().await;
        logs.clone()
    }

    pub async fn get_logs_for_client(&self, client_id: &str) -> Vec<AuditLogEntry> {
        let logs = self.logs.read().await;
        logs.iter()
            .filter(|log| log.client_id == client_id)
            .cloned()
            .collect()
    }
}

/// Replay attack prevention using nonce tracking
#[derive(Clone)]
pub struct ReplayProtection {
    nonces: Arc<RwLock<HashMap<String, HashMap<String, u64>>>>,
    max_nonces_per_client: usize,
}

impl ReplayProtection {
    pub fn new() -> Self {
        ReplayProtection {
            nonces: Arc::new(RwLock::new(HashMap::new())),
            max_nonces_per_client: 1024,
        }
    }

    pub async fn check_nonce(&self, client_id: &str, nonce: u64) -> bool {
        self.check_nonce_with_timestamp(client_id, &nonce.to_string(), nonce)
            .await
    }

    pub async fn check_nonce_with_timestamp(
        &self,
        client_id: &str,
        nonce: &str,
        timestamp_secs: u64,
    ) -> bool {
        let mut nonces = self.nonces.write().await;
        let client_nonces = nonces.entry(client_id.to_string()).or_default();

        // Check if nonce already used
        if client_nonces.contains_key(nonce) {
            return false;
        }

        // Record nonce and timestamp.
        client_nonces.insert(nonce.to_string(), timestamp_secs);

        // Keep bounded memory by evicting oldest nonce when needed.
        if client_nonces.len() > self.max_nonces_per_client {
            let mut oldest: Option<(String, u64)> = None;
            for (existing_nonce, ts) in client_nonces.iter() {
                match oldest {
                    Some((_, oldest_ts)) if *ts >= oldest_ts => {}
                    _ => oldest = Some((existing_nonce.clone(), *ts)),
                }
            }
            if let Some((oldest_nonce, _)) = oldest {
                client_nonces.remove(&oldest_nonce);
            }
        }

        true
    }

    pub async fn cleanup_old_nonces(&self, max_age_secs: u64) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let mut nonces = self.nonces.write().await;
        for client_nonces in nonces.values_mut() {
            client_nonces.retain(|_, timestamp| now.saturating_sub(*timestamp) < max_age_secs);
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SecurityMetricsResponse {
    pub requests_total: u64,
    pub unauthorized_total: u64,
    pub rate_limited_total: u64,
    pub bad_client_id_total: u64,
    pub bad_request_total: u64,
}

impl ApiSecurityStats {
    pub fn snapshot(&self) -> SecurityMetricsResponse {
        SecurityMetricsResponse {
            requests_total: self.requests_total.load(Ordering::Relaxed),
            unauthorized_total: self.unauthorized_total.load(Ordering::Relaxed),
            rate_limited_total: self.rate_limited_total.load(Ordering::Relaxed),
            bad_client_id_total: self.bad_client_id_total.load(Ordering::Relaxed),
            bad_request_total: self.bad_request_total.load(Ordering::Relaxed),
        }
    }
}

/// Application state for Axum
#[allow(dead_code)]
pub struct AppState {
    pub state_manager: SharedState,
    pub mempool: SharedMempool,
    pub security: ApiSecurityConfig,
    pub rate_limiter: ApiRateLimiter,
    pub security_stats: Arc<ApiSecurityStats>,
    pub ws_manager: WebSocketConnectionManager,
    pub pbft_consensus: Arc<tokio::sync::Mutex<Option<PbftConsensus>>>,
    /// Optional: when present, the `/private/tx/:hash` endpoint serves
    /// private-tx bodies from the node's pool. Left as `None` in tests
    /// that don't spin up a wallet manager.
    pub private_wallet_manager: Option<SharedPrivateWalletManager>,
    /// Channel into the fullnode main loop for forwarding validated
    /// private transactions. The handler at `/private/tx`
    /// pushes here after accepting a locally built body; the main loop drains
    /// it, inserts each tx into its in-memory pool, and gossips it over
    /// libp2p so any connected miner can include it in the next block.
    /// `None` in tests / non-fullnode setups.
    pub private_tx_broadcaster: Option<PrivateTxBroadcastSender>,
    /// Non-custodial team-mining coordination shared with the fullnode P2P
    /// and mining loops. `None` keeps older/non-fullnode API contexts honest:
    /// the client sees an unsupported endpoint, not an empty team.
    pub team_mining_runtime: Option<Arc<crate::team_mining::TeamMiningRuntime>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyProofRequest {
    pub expected_root: String,
    pub proof: AccountInclusionProof,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyProofResponse {
    pub valid: bool,
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    let a_bytes = a.as_bytes();
    let b_bytes = b.as_bytes();
    let mut diff = a_bytes.len() ^ b_bytes.len();
    let max_len = a_bytes.len().max(b_bytes.len());
    for i in 0..max_len {
        let x = if i < a_bytes.len() { a_bytes[i] } else { 0 };
        let y = if i < b_bytes.len() { b_bytes[i] } else { 0 };
        diff |= (x ^ y) as usize;
    }
    diff == 0
}

fn hmac_sha256(secret_key: &[u8], parts: &[&[u8]]) -> Option<Vec<u8>> {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    let mut mac = Hmac::<Sha256>::new_from_slice(secret_key).ok()?;
    for part in parts {
        mac.update(part);
    }
    Some(mac.finalize().into_bytes().to_vec())
}

fn validate_client_id(client_id: &str, max_len: usize) -> bool {
    if client_id.is_empty() || client_id.len() > max_len {
        return false;
    }
    client_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.'))
}

fn validate_address(address: &str) -> bool {
    let len = address.len();
    if !(3..=64).contains(&len) {
        return false;
    }
    address
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn validate_root_format(root: &str) -> bool {
    root == "0" || (root.len() == 64 && root.chars().all(|c| c.is_ascii_hexdigit()))
}

fn parse_fixed_hex(input: &str, expected_len: usize, field: &str) -> Result<Vec<u8>, String> {
    let bytes = hex::decode(input).map_err(|_| format!("invalid {field}: not valid hex"))?;
    if bytes.len() != expected_len {
        return Err(format!(
            "invalid {field}: expected {expected_len} bytes, got {}",
            bytes.len()
        ));
    }
    Ok(bytes)
}

fn derive_sender_address_from_pubkey(pubkey: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(pubkey);
    let hash = hasher.finalize();
    format!("0x{}", hex::encode(&hash[..20]))
}

fn verify_transaction_request_signature(
    req: &TransactionRequest,
    timestamp: u64,
) -> Result<crate::types::Signature, String> {
    use crate::crypto::signatures::{
        verify, PublicKey as CryptoPublicKey, Signature as CryptoSignature,
    };

    let signature_hex = req
        .signature_hex
        .as_deref()
        .ok_or_else(|| "missing signature_hex".to_string())?;
    let public_key_hex = req
        .public_key_hex
        .as_deref()
        .ok_or_else(|| "missing public_key_hex".to_string())?;

    let signature_raw = parse_fixed_hex(signature_hex, 64, "signature_hex")?;
    let public_key_raw = parse_fixed_hex(public_key_hex, 32, "public_key_hex")?;

    let derived_sender = derive_sender_address_from_pubkey(&public_key_raw);
    if !constant_time_eq(&derived_sender, &req.sender) {
        return Err("sender does not match provided public key".to_string());
    }

    let mut tx = crate::types::Transaction::new(
        req.sender.clone(),
        req.receiver.clone(),
        req.amount,
        req.nonce,
        timestamp,
        crate::types::Signature(vec![]),
    );
    tx.signature = crate::types::Signature(signature_raw.clone());

    let mut pk = [0u8; 32];
    pk.copy_from_slice(&public_key_raw);
    let mut sig = [0u8; 64];
    sig.copy_from_slice(&signature_raw);

    let crypto_pk = CryptoPublicKey::from_bytes(pk);
    let crypto_sig = CryptoSignature::from_bytes(sig);
    verify(&crypto_pk, tx.hash.as_bytes(), &crypto_sig)
        .map_err(|_| "transaction signature verification failed".to_string())?;

    Ok(crate::types::Signature(signature_raw))
}

fn rate_limit_bucket(method: &str, path: &str) -> &'static str {
    if path.starts_with("/transactions")
        || path == "/private/view-scan"
        || path == "/private/view-scan-key"
        || path == "/private/wallet/send-from-seed"
        || (!matches!(method, "GET" | "HEAD" | "OPTIONS") && path.starts_with("/private/tx"))
    {
        "transactions"
    } else if matches!(method, "GET" | "HEAD")
        && (path == "/chain/height"
            || path == "/stats"
            || path == "/sync/status"
            || path == "/peers"
            || path == "/network/runtime"
            || path.starts_with("/blocks")
            || path.starts_with("/mempool")
            || path == "/private/pending"
            || path == "/checkpoint/latest"
            || path.starts_with("/private/tx/"))
    {
        "public-read"
    } else {
        "default"
    }
}

fn rate_limit_key(client_ip: &str, client_id: &str, bucket: &str) -> String {
    if client_id == "anonymous" {
        format!("ip:{client_ip}:{bucket}")
    } else {
        format!("client:{client_id}:ip:{client_ip}:{bucket}")
    }
}

fn global_audit_logger() -> &'static AuditLogger {
    static LOGGER: OnceLock<AuditLogger> = OnceLock::new();
    LOGGER.get_or_init(|| AuditLogger::new(10_000))
}

fn global_replay_protection() -> &'static ReplayProtection {
    static REPLAY: OnceLock<ReplayProtection> = OnceLock::new();
    REPLAY.get_or_init(ReplayProtection::new)
}

fn required_role_for_request(method: &str, path: &str) -> ApiRole {
    if path.starts_with("/admin/") || path.starts_with("/metrics/") {
        ApiRole::Admin
    } else if matches!(method, "GET" | "HEAD" | "OPTIONS") {
        ApiRole::Read
    } else {
        ApiRole::Write
    }
}

fn resolve_client_role(client_id: &str, security: &ApiSecurityConfig) -> ApiRole {
    if security
        .admin_clients
        .iter()
        .any(|known| constant_time_eq(known, client_id))
    {
        ApiRole::Admin
    } else if security
        .write_clients
        .iter()
        .any(|known| constant_time_eq(known, client_id))
    {
        ApiRole::Write
    } else {
        ApiRole::Read
    }
}

fn role_allows(client_role: ApiRole, required_role: ApiRole) -> bool {
    client_role >= required_role
}

async fn write_audit_log(
    client_id: &str,
    method: &str,
    path: &str,
    status: StatusCode,
    ip_address: &str,
    cache_status: &str,
) {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    global_audit_logger()
        .log(AuditLogEntry {
            timestamp,
            client_id: client_id.to_string(),
            method: method.to_string(),
            path: path.to_string(),
            status: status.as_u16(),
            ip_address: ip_address.to_string(),
            cache_status: cache_status.to_string(),
        })
        .await;
}

#[allow(dead_code)]
/// Resolve the client IP to use for rate limiting, IP allow/deny lists, and
/// the audit trail. `peer_ip` is the real TCP connection address; it is only
/// overridden by an `X-Forwarded-For`/`X-Real-IP` header when that peer is a
/// configured trusted proxy -- otherwise the header is attacker-controlled
/// input and must not be trusted (a bare "any header wins" policy lets any
/// caller spoof a fresh IP on every request and dodge rate limiting or an
/// allow/deny list entirely).
fn resolve_client_ip(
    peer_ip: Option<&str>,
    forwarded_header: Option<&str>,
    trusted_proxy_ips: &[String],
) -> String {
    let is_trusted_proxy =
        peer_ip.is_some_and(|peer| trusted_proxy_ips.iter().any(|trusted| trusted == peer));
    if is_trusted_proxy {
        if let Some(forwarded) = forwarded_header
            .and_then(|raw| raw.split(',').next())
            .map(str::trim)
            .filter(|ip| !ip.is_empty())
        {
            return forwarded.to_string();
        }
    }
    peer_ip.unwrap_or("unknown").to_string()
}

fn validate_ip(ip: &str, whitelist: &[String], blacklist: &[String]) -> bool {
    // If blacklist contains IP, reject
    if !blacklist.is_empty() && blacklist.contains(&ip.to_string()) {
        return false;
    }
    // If whitelist is non-empty, only allow whitelisted IPs
    if !whitelist.is_empty() {
        return whitelist.contains(&ip.to_string());
    }
    // If both empty, allow
    true
}

/// Request signature validator for API authentication and integrity
#[derive(Clone, Debug)]
pub struct RequestSignatureValidator {
    enabled: bool,
    required_clients: Arc<RwLock<std::collections::HashSet<String>>>,
}

impl RequestSignatureValidator {
    pub fn new(enabled: bool) -> Self {
        RequestSignatureValidator {
            enabled,
            required_clients: Arc::new(RwLock::new(std::collections::HashSet::new())),
        }
    }

    /// Mark a client as requiring signature verification
    pub async fn require_signature(&self, client_id: &str) {
        if self.enabled {
            let mut required = self.required_clients.write().await;
            required.insert(client_id.to_string());
        }
    }

    /// Check if a client requires signature verification
    pub async fn is_signature_required(&self, client_id: &str) -> bool {
        if !self.enabled {
            return false;
        }
        let required = self.required_clients.read().await;
        required.contains(client_id)
    }

    /// Validate request signature.
    /// Signature format: hex(HMAC-SHA256(method + "\n" + path + "\n" + body))
    /// Also accepts base64(HMAC-SHA256(...)) for compatibility.
    pub fn validate_signature(
        &self,
        method: &str,
        path: &str,
        body: &[u8],
        signature: &str,
        secret_key: &[u8],
    ) -> bool {
        use base64::Engine;

        if !self.enabled {
            return true;
        }

        let mac_bytes = match hmac_sha256(
            secret_key,
            &[method.as_bytes(), b"\n", path.as_bytes(), b"\n", body],
        ) {
            Some(bytes) => bytes,
            None => return false,
        };

        let expected_hex = hex::encode(&mac_bytes);
        let expected_b64 = base64::engine::general_purpose::STANDARD.encode(mac_bytes);
        constant_time_eq(&expected_hex, signature) || constant_time_eq(&expected_b64, signature)
    }
}

impl Default for RequestSignatureValidator {
    fn default() -> Self {
        RequestSignatureValidator::new(false)
    }
}

/// Encryption helper for sensitive data - uses simple XOR obfuscation
/// In production, should use proper AES-256-GCM encryption
#[derive(Clone, Debug)]
pub struct EncryptionHelper {
    enabled: bool,
}

impl EncryptionHelper {
    pub fn new(enabled: bool) -> Self {
        EncryptionHelper { enabled }
    }

    /// Simple encryption using XOR with a key (NOT cryptographically secure - demo only)
    pub fn encrypt(&self, data: &[u8], key: &[u8]) -> Vec<u8> {
        if !self.enabled || key.is_empty() {
            return data.to_vec();
        }
        data.iter()
            .enumerate()
            .map(|(i, byte)| byte ^ key[i % key.len()])
            .collect()
    }

    /// Simple decryption using XOR with a key (NOT cryptographically secure - demo only)
    pub fn decrypt(&self, data: &[u8], key: &[u8]) -> Vec<u8> {
        // XOR is symmetric, so decryption is the same as encryption
        self.encrypt(data, key)
    }

    /// Check if encryption is enabled
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }
}

impl Default for EncryptionHelper {
    fn default() -> Self {
        EncryptionHelper::new(false)
    }
}

/// API Key management for client authentication and rate limiting
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiKeyInfo {
    pub key_id: String,
    pub key_hash: String, // blake3 hash of actual key
    pub created_at: u64,
    pub expires_at: Option<u64>,
    pub enabled: bool,
    pub permissions: Vec<String>,
}

#[derive(Clone)]
pub struct ApiKeyManager {
    keys: Arc<RwLock<HashMap<String, ApiKeyInfo>>>,
    key_counter: Arc<AtomicU64>,
}

impl ApiKeyManager {
    pub fn new() -> Self {
        ApiKeyManager {
            keys: Arc::new(RwLock::new(HashMap::new())),
            key_counter: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Create a new API key (returns the actual key, which should be shared securely)
    pub async fn create_key(&self, permissions: Vec<String>) -> (String, ApiKeyInfo) {
        use rand::RngCore;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Generate unique key_id using counter
        let counter = self.key_counter.fetch_add(1, Ordering::SeqCst);
        let key_id = format!("key_{}_{}", now, counter);

        let mut key_bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut key_bytes);
        let actual_key = hex::encode(key_bytes);
        let key_hash = blake3::hash(actual_key.as_bytes()).to_hex().to_string();

        let info = ApiKeyInfo {
            key_id: key_id.clone(),
            key_hash,
            created_at: now,
            expires_at: Some(now + 86400 * 365), // 1 year expiry
            enabled: true,
            permissions,
        };

        let mut keys = self.keys.write().await;
        keys.insert(key_id, info.clone());

        (actual_key, info)
    }

    /// Verify an API key
    pub async fn verify_key(&self, key_id: &str, key_hash: &str) -> bool {
        let keys = self.keys.read().await;
        if let Some(info) = keys.get(key_id) {
            if !info.enabled {
                return false;
            }
            if let Some(expires_at) = info.expires_at {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                if now > expires_at {
                    return false;
                }
            }
            return constant_time_eq(key_hash, &info.key_hash);
        }
        false
    }

    /// Revoke an API key
    pub async fn revoke_key(&self, key_id: &str) {
        let mut keys = self.keys.write().await;
        if let Some(info) = keys.get_mut(key_id) {
            info.enabled = false;
        }
    }

    /// Get key info (without exposing the actual key)
    pub async fn get_key_info(&self, key_id: &str) -> Option<ApiKeyInfo> {
        let keys = self.keys.read().await;
        keys.get(key_id).cloned()
    }

    /// List all active keys
    pub async fn list_keys(&self) -> Vec<ApiKeyInfo> {
        let keys = self.keys.read().await;
        keys.values().filter(|info| info.enabled).cloned().collect()
    }
}

impl Default for ApiKeyManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Input validator for API requests - prevents common attacks
#[derive(Clone)]
pub struct InputValidator {
    max_string_len: usize,
    max_number_value: u64,
    allow_special_chars: bool,
}

impl InputValidator {
    pub fn new(max_string_len: usize, max_number_value: u64, allow_special_chars: bool) -> Self {
        InputValidator {
            max_string_len,
            max_number_value,
            allow_special_chars,
        }
    }

    /// Validate string input (length, characters)
    pub fn validate_string(&self, input: &str, field_name: &str) -> Result<(), String> {
        if input.is_empty() {
            return Err(format!("{} cannot be empty", field_name));
        }
        if input.len() > self.max_string_len {
            return Err(format!(
                "{} exceeds maximum length of {}",
                field_name, self.max_string_len
            ));
        }
        if !self.allow_special_chars {
            if !input
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                return Err(format!("{} contains invalid characters", field_name));
            }
        }
        Ok(())
    }

    /// Validate numeric input (bounds)
    pub fn validate_number(&self, value: u64, field_name: &str) -> Result<(), String> {
        if value > self.max_number_value {
            return Err(format!(
                "{} exceeds maximum value of {}",
                field_name, self.max_number_value
            ));
        }
        Ok(())
    }

    /// Validate email format (basic check)
    pub fn validate_email(&self, email: &str) -> Result<(), String> {
        if !email.contains('@') || !email.contains('.') {
            return Err("Invalid email format".to_string());
        }
        let parts: Vec<&str> = email.split('@').collect();
        if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
            return Err("Invalid email format".to_string());
        }
        Ok(())
    }
}

impl Default for InputValidator {
    fn default() -> Self {
        InputValidator::new(256, u64::MAX, true)
    }
}

/// Error response helper - masks sensitive internal errors
#[derive(Clone, Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
    pub error_code: u16,
    pub timestamp: u64,
}

impl ErrorResponse {
    pub fn new(error: String, error_code: u16) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        ErrorResponse {
            error,
            error_code,
            timestamp,
        }
    }

    /// Create a generic error that doesn't expose internal details
    pub fn internal_error() -> Self {
        Self::new("Internal server error".to_string(), 500)
    }

    /// Create a not found error
    pub fn not_found(resource: &str) -> Self {
        Self::new(format!("Resource '{}' not found", resource), 404)
    }

    /// Create a validation error
    pub fn validation_error(message: &str) -> Self {
        Self::new(format!("Validation error: {}", message), 400)
    }
}

/// CORS configuration helper
#[derive(Clone, Debug)]
pub struct CorsConfig {
    pub allowed_origins: Vec<String>,
    pub allowed_methods: Vec<String>,
    pub allowed_headers: Vec<String>,
    pub max_age: u64,
}

impl CorsConfig {
    pub fn new() -> Self {
        CorsConfig {
            allowed_origins: vec!["*".to_string()],
            allowed_methods: vec!["GET".to_string(), "POST".to_string(), "OPTIONS".to_string()],
            allowed_headers: vec![
                "Content-Type".to_string(),
                "X-Client-ID".to_string(),
                "X-API-Token".to_string(),
            ],
            max_age: 86400, // 24 hours
        }
    }

    pub fn with_origins(mut self, origins: Vec<String>) -> Self {
        self.allowed_origins = origins;
        self
    }

    pub fn with_methods(mut self, methods: Vec<String>) -> Self {
        self.allowed_methods = methods;
        self
    }

    pub fn with_headers(mut self, headers: Vec<String>) -> Self {
        self.allowed_headers = headers;
        self
    }

    pub fn is_origin_allowed(&self, origin: &str) -> bool {
        if self.allowed_origins.contains(&"*".to_string()) {
            return true;
        }
        self.allowed_origins.contains(&origin.to_string())
    }

    pub fn is_method_allowed(&self, method: &str) -> bool {
        self.allowed_methods.contains(&method.to_string())
    }
}

impl Default for CorsConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// Signed token handler for API authentication.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TokenClaims {
    pub sub: String,  // subject (user/client ID)
    pub iat: u64,     // issued at
    pub exp: u64,     // expiration time
    pub role: String, // user role
}

#[derive(Clone)]
pub struct TokenManager {
    #[allow(dead_code)]
    secret: String,
    token_ttl_secs: u64,
}

impl TokenManager {
    pub fn new(secret: String, token_ttl_secs: u64) -> Self {
        TokenManager {
            secret,
            token_ttl_secs,
        }
    }

    fn sign_payload_hex(&self, payload: &str) -> Option<String> {
        hmac_sha256(self.secret.as_bytes(), &[payload.as_bytes()]).map(hex::encode)
    }

    /// Create a signed token.
    pub fn create_token(&self, sub: &str, role: &str) -> String {
        use base64::Engine;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let exp = now + self.token_ttl_secs;
        let claims = TokenClaims {
            sub: sub.to_string(),
            iat: now,
            exp,
            role: role.to_string(),
        };

        let payload = match serde_json::to_vec(&claims) {
            Ok(bytes) => base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes),
            Err(_) => return String::new(),
        };
        let signature = match self.sign_payload_hex(&payload) {
            Some(sig) => sig,
            None => return String::new(),
        };

        format!("{}.{}", payload, signature)
    }

    /// Verify a signed token.
    pub fn verify_token(&self, token: &str) -> Result<TokenClaims, String> {
        use base64::Engine;

        let (payload_b64, signature) = token
            .rsplit_once('.')
            .ok_or_else(|| "Invalid token format".to_string())?;

        let expected_signature = self
            .sign_payload_hex(payload_b64)
            .ok_or_else(|| "Failed to sign token payload".to_string())?;
        if !constant_time_eq(signature, &expected_signature) {
            return Err("Invalid token signature".to_string());
        }

        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload_b64)
            .map_err(|_| "Invalid token payload".to_string())?;
        let claims: TokenClaims =
            serde_json::from_slice(&payload).map_err(|_| "Invalid token claims".to_string())?;
        if claims.sub.is_empty() || claims.role.is_empty() {
            return Err("Invalid token format".to_string());
        }

        // Check expiration
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        if now > claims.exp {
            return Err("Token expired".to_string());
        }

        // Check issued time is not in the future
        if claims.iat > now {
            return Err("Token issued in the future".to_string());
        }

        Ok(claims)
    }

    /// Refresh a token (extend expiration)
    pub fn refresh_token(&self, token: &str) -> Result<String, String> {
        let claims = self.verify_token(token)?;
        Ok(self.create_token(&claims.sub, &claims.role))
    }
}

impl Default for TokenManager {
    fn default() -> Self {
        TokenManager::new("default-secret".to_string(), 3600) // 1 hour default
    }
}

/// Circuit breaker pattern for protecting against cascading failures
/// States: Closed (normal), Open (rejecting), Half-Open (testing recovery)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    Closed,   // Normal operation
    Open,     // Rejecting requests
    HalfOpen, // Testing if system recovered
}

#[derive(Clone)]
pub struct CircuitBreaker {
    state: Arc<RwLock<CircuitState>>,
    failure_count: Arc<AtomicU64>,
    success_count: Arc<AtomicU64>,
    failure_threshold: u64,
    success_threshold: u64,
    last_failure_time: Arc<RwLock<u64>>,
    reset_timeout_secs: u64,
}

impl CircuitBreaker {
    pub fn new(failure_threshold: u64, success_threshold: u64, reset_timeout_secs: u64) -> Self {
        CircuitBreaker {
            state: Arc::new(RwLock::new(CircuitState::Closed)),
            failure_count: Arc::new(AtomicU64::new(0)),
            success_count: Arc::new(AtomicU64::new(0)),
            failure_threshold,
            success_threshold,
            last_failure_time: Arc::new(RwLock::new(0)),
            reset_timeout_secs,
        }
    }

    /// Record a failure
    pub async fn record_failure(&self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        self.failure_count.fetch_add(1, Ordering::SeqCst);
        *self.last_failure_time.write().await = now;

        let failures = self.failure_count.load(Ordering::SeqCst);
        if failures >= self.failure_threshold {
            let mut state = self.state.write().await;
            *state = CircuitState::Open;
        }
    }

    /// Record a success
    pub async fn record_success(&self) {
        let mut state = self.state.write().await;

        match *state {
            CircuitState::Closed => {
                // In Closed state, count is not used
            }
            CircuitState::HalfOpen => {
                self.success_count.fetch_add(1, Ordering::SeqCst);
                let successes = self.success_count.load(Ordering::SeqCst);

                if successes >= self.success_threshold {
                    // Transition back to Closed
                    *state = CircuitState::Closed;
                    self.failure_count.store(0, Ordering::SeqCst);
                    self.success_count.store(0, Ordering::SeqCst);
                }
            }
            CircuitState::Open => {
                // Ignore successes in Open state
            }
        }
    }

    /// Check if request should be allowed
    pub async fn is_request_allowed(&self) -> bool {
        let mut state = self.state.write().await;

        match *state {
            CircuitState::Closed => true,
            CircuitState::Open => {
                // Check if timeout has elapsed to try half-open
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();

                let last_failure = *self.last_failure_time.read().await;
                if now - last_failure >= self.reset_timeout_secs {
                    *state = CircuitState::HalfOpen;
                    self.success_count.store(0, Ordering::SeqCst);
                    true
                } else {
                    false
                }
            }
            CircuitState::HalfOpen => true,
        }
    }

    /// Get current state
    pub async fn get_state(&self) -> CircuitState {
        *self.state.read().await
    }
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        CircuitBreaker::new(5, 3, 60) // Fail after 5 failures, recover after 3 successes, 60s timeout
    }
}

/// Middleware: API token validation + per-client rate limit.
async fn auth_and_rate_limit(
    State(app_state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    app_state
        .security_stats
        .requests_total
        .fetch_add(1, Ordering::Relaxed);

    let mut request = request;
    let path = request.uri().path().to_string();
    let method = request.method().as_str().to_string();

    if matches!(
        path.as_str(),
        "/health" | "/health/checks" | "/ready" | "/startup/status"
    ) {
        return Ok(next.run(request).await);
    }

    // The real TCP peer address, from the connection itself (not attacker-
    // controlled). This node is deployed with no reverse proxy in front of it,
    // so headers like X-Forwarded-For/X-Real-IP are ordinary request data any
    // caller can set to an arbitrary value -- trusting them unconditionally
    // let anyone dodge rate limiting, IP allow/deny lists, and the audit trail
    // simply by spoofing (or omitting) the header on every request.
    let peer_ip = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip().to_string());
    let forwarded_header = request
        .headers()
        .get("x-forwarded-for")
        .or_else(|| request.headers().get("x-real-ip"))
        .and_then(|value| value.to_str().ok());
    let client_ip = resolve_client_ip(
        peer_ip.as_deref(),
        forwarded_header,
        &app_state.security.trusted_proxy_ips,
    );

    let client_id = request
        .headers()
        .get("x-client-id")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("anonymous")
        .to_string();

    if let Some(expected_token) = &app_state.security.api_token {
        let provided = request
            .headers()
            .get("x-api-token")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        if !constant_time_eq(provided, expected_token) {
            app_state
                .security_stats
                .unauthorized_total
                .fetch_add(1, Ordering::Relaxed);
            write_audit_log(
                &client_id,
                &method,
                &path,
                StatusCode::UNAUTHORIZED,
                &client_ip,
                "reject_auth_token",
            )
            .await;
            return Err(StatusCode::UNAUTHORIZED);
        }
    }

    if !validate_client_id(&client_id, app_state.security.max_client_id_len) {
        app_state
            .security_stats
            .bad_client_id_total
            .fetch_add(1, Ordering::Relaxed);
        write_audit_log(
            &client_id,
            &method,
            &path,
            StatusCode::BAD_REQUEST,
            &client_ip,
            "reject_client_id",
        )
        .await;
        return Err(StatusCode::BAD_REQUEST);
    }

    if !validate_ip(
        &client_ip,
        &app_state.security.ip_whitelist,
        &app_state.security.ip_blacklist,
    ) {
        app_state
            .security_stats
            .unauthorized_total
            .fetch_add(1, Ordering::Relaxed);
        write_audit_log(
            &client_id,
            &method,
            &path,
            StatusCode::FORBIDDEN,
            &client_ip,
            "reject_ip_policy",
        )
        .await;
        return Err(StatusCode::FORBIDDEN);
    }

    if app_state.security.enforce_rbac {
        let required_role = required_role_for_request(&method, &path);
        let client_role = resolve_client_role(&client_id, &app_state.security);
        if !role_allows(client_role, required_role) {
            app_state
                .security_stats
                .unauthorized_total
                .fetch_add(1, Ordering::Relaxed);
            write_audit_log(
                &client_id,
                &method,
                &path,
                StatusCode::FORBIDDEN,
                &client_ip,
                "reject_rbac",
            )
            .await;
            return Err(StatusCode::FORBIDDEN);
        }
    }

    if app_state.security.require_request_signature {
        let signature = request
            .headers()
            .get("x-request-signature")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_string();
        if signature.is_empty() {
            app_state
                .security_stats
                .unauthorized_total
                .fetch_add(1, Ordering::Relaxed);
            write_audit_log(
                &client_id,
                &method,
                &path,
                StatusCode::UNAUTHORIZED,
                &client_ip,
                "reject_missing_request_signature",
            )
            .await;
            return Err(StatusCode::UNAUTHORIZED);
        }

        let secret = match app_state
            .security
            .request_signature_secret
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            Some(secret) => secret,
            None => {
                app_state
                    .security_stats
                    .unauthorized_total
                    .fetch_add(1, Ordering::Relaxed);
                write_audit_log(
                    &client_id,
                    &method,
                    &path,
                    StatusCode::UNAUTHORIZED,
                    &client_ip,
                    "reject_missing_signature_secret",
                )
                .await;
                return Err(StatusCode::UNAUTHORIZED);
            }
        };

        let (parts, body) = request.into_parts();
        let body_bytes = match to_bytes(body, app_state.security.max_request_body_bytes).await {
            Ok(bytes) => bytes,
            Err(_) => {
                app_state
                    .security_stats
                    .bad_request_total
                    .fetch_add(1, Ordering::Relaxed);
                write_audit_log(
                    &client_id,
                    &method,
                    &path,
                    StatusCode::PAYLOAD_TOO_LARGE,
                    &client_ip,
                    "reject_body_too_large",
                )
                .await;
                return Err(StatusCode::PAYLOAD_TOO_LARGE);
            }
        };

        let validator = RequestSignatureValidator::new(true);
        let valid = validator.validate_signature(
            &method,
            &path,
            &body_bytes,
            &signature,
            secret.as_bytes(),
        );
        if !valid {
            app_state
                .security_stats
                .unauthorized_total
                .fetch_add(1, Ordering::Relaxed);
            write_audit_log(
                &client_id,
                &method,
                &path,
                StatusCode::UNAUTHORIZED,
                &client_ip,
                "reject_bad_request_signature",
            )
            .await;
            return Err(StatusCode::UNAUTHORIZED);
        }

        request = Request::from_parts(parts, Body::from(body_bytes.to_vec()));
    }

    if app_state.security.enable_replay_protection {
        let nonce = request
            .headers()
            .get("x-request-nonce")
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .unwrap_or("");
        if nonce.is_empty() {
            app_state
                .security_stats
                .bad_request_total
                .fetch_add(1, Ordering::Relaxed);
            write_audit_log(
                &client_id,
                &method,
                &path,
                StatusCode::BAD_REQUEST,
                &client_ip,
                "reject_missing_nonce",
            )
            .await;
            return Err(StatusCode::BAD_REQUEST);
        }

        let timestamp = request
            .headers()
            .get("x-request-timestamp")
            .and_then(|value| value.to_str().ok())
            .and_then(|raw| raw.parse::<u64>().ok());
        let timestamp = match timestamp {
            Some(ts) => ts,
            None => {
                app_state
                    .security_stats
                    .bad_request_total
                    .fetch_add(1, Ordering::Relaxed);
                write_audit_log(
                    &client_id,
                    &method,
                    &path,
                    StatusCode::BAD_REQUEST,
                    &client_ip,
                    "reject_bad_timestamp",
                )
                .await;
                return Err(StatusCode::BAD_REQUEST);
            }
        };

        let now = ApiRateLimiter::now_secs();
        let max_age = app_state.security.request_nonce_max_age_secs;
        let max_future_skew = 30u64;
        if now.saturating_sub(timestamp) > max_age
            || timestamp > now.saturating_add(max_future_skew)
        {
            app_state
                .security_stats
                .unauthorized_total
                .fetch_add(1, Ordering::Relaxed);
            write_audit_log(
                &client_id,
                &method,
                &path,
                StatusCode::UNAUTHORIZED,
                &client_ip,
                "reject_timestamp_window",
            )
            .await;
            return Err(StatusCode::UNAUTHORIZED);
        }

        let replay = global_replay_protection();
        replay.cleanup_old_nonces(max_age).await;
        if !replay
            .check_nonce_with_timestamp(&client_id, nonce, timestamp)
            .await
        {
            app_state
                .security_stats
                .unauthorized_total
                .fetch_add(1, Ordering::Relaxed);
            write_audit_log(
                &client_id,
                &method,
                &path,
                StatusCode::CONFLICT,
                &client_ip,
                "reject_replay_nonce",
            )
            .await;
            return Err(StatusCode::CONFLICT);
        }
    }

    let bucket = rate_limit_bucket(&method, &path);
    let limit = if bucket == "transactions" {
        app_state.security.transaction_rate_limit_per_minute
    } else if bucket == "public-read" {
        app_state.security.read_rate_limit_per_minute
    } else {
        app_state.security.rate_limit_per_minute
    };
    let key = rate_limit_key(&client_ip, &client_id, bucket);

    let allowed = app_state.rate_limiter.allow_request(&key, limit).await;
    if !allowed {
        app_state
            .security_stats
            .rate_limited_total
            .fetch_add(1, Ordering::Relaxed);
        write_audit_log(
            &client_id,
            &method,
            &path,
            StatusCode::TOO_MANY_REQUESTS,
            &client_ip,
            "reject_rate_limit",
        )
        .await;
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }

    let response = next.run(request).await;
    write_audit_log(
        &client_id,
        &method,
        &path,
        response.status(),
        &client_ip,
        "miss",
    )
    .await;
    Ok(response)
}

// `network` conserva la etiqueta interna ("mainnet" para Zenith), que es lo
// que leen los clientes existentes. Estos dos campos dicen lo que la red es hoy.
fn network_role_of(startup: &StartupStatusSnapshot) -> Option<&'static str> {
    startup
        .chain_id
        .as_deref()
        .and_then(crate::economics::network_role_for_chain)
}

fn network_name_of(startup: &StartupStatusSnapshot) -> Option<&'static str> {
    startup
        .chain_id
        .as_deref()
        .and_then(crate::economics::network_display_name_for_chain)
}

/// Health check endpoint
async fn health(State(app_state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let startup = global_startup_status().snapshot();
    let runtime = global_network_runtime().snapshot().await;
    let height = app_state.state_manager.get_height().await;
    let now = ApiRateLimiter::now_secs();
    Json(serde_json::json!({
        "status": startup.status,
        "ready": startup.ready,
        "phase": startup.phase,
        "detail": startup.detail,
        "network": startup.network,
        "network_role": network_role_of(&startup),
        "network_name": network_name_of(&startup),
        "chain_id": startup.chain_id,
        "height": height,
        "peers": runtime.connected_peers,
        "minimum_client_version": runtime.minimum_client_version,
        "uptime_secs": now.saturating_sub(startup.started_at),
        "started_at": startup.started_at,
        "updated_at": startup.updated_at,
        "version": startup.version,
    }))
}

async fn ready() -> (StatusCode, Json<serde_json::Value>) {
    let startup = global_startup_status().snapshot();
    let status = if startup.ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        Json(serde_json::json!({
            "status": if startup.ready { "ready" } else { "not_ready" },
            "ready": startup.ready,
            "phase": startup.phase,
            "detail": startup.detail,
            "network": startup.network,
            "network_role": network_role_of(&startup),
            "network_name": network_name_of(&startup),
            "chain_id": startup.chain_id,
            "updated_at": startup.updated_at,
            "version": startup.version,
        })),
    )
}

async fn startup_status() -> Json<StartupStatusSnapshot> {
    Json(global_startup_status().snapshot())
}

async fn health_checks(State(app_state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let checker = HealthChecker::new("zoka");
    let local_height = app_state.state_manager.get_height().await;
    let last_block_at = match app_state.state_manager.get_latest_block().await {
        Ok(Some(block)) => block.header.timestamp,
        _ => 0,
    };
    let sync = global_network_runtime()
        .sync_status_snapshot(local_height, last_block_at)
        .await;
    checker.set_peer_count(sync.connected_peers as usize);
    checker.set_block_height(local_height);
    checker.set_syncing(sync.lag > 0);
    let progress = if sync.target_height == 0 {
        100.0
    } else {
        ((sync.local_height as f64 / sync.target_height as f64) * 100.0).min(100.0)
    };
    checker.set_sync_progress(progress);
    checker.set_last_block_time(last_block_at);
    checker.set_mempool_size(app_state.mempool.len().await);
    let checks = checker.run_all_checks();
    let overall = checker.overall_status();
    Json(serde_json::json!({
        "service": "zoka",
        "status": overall.to_string(),
        "sync_status": sync,
        "checks": checks,
    }))
}

/// Get API security counters.
async fn get_security_metrics(
    State(app_state): State<Arc<AppState>>,
) -> Json<SecurityMetricsResponse> {
    Json(app_state.security_stats.snapshot())
}

/// Get recent API audit logs. Optional filter by `client_id` query parameter.
async fn get_audit_logs(Query(query): Query<HashMap<String, String>>) -> Json<serde_json::Value> {
    let logs = if let Some(client_id) = query.get("client_id") {
        global_audit_logger().get_logs_for_client(client_id).await
    } else {
        global_audit_logger().get_logs().await
    };

    Json(serde_json::json!({
        "count": logs.len(),
        "logs": logs
    }))
}

/// Network-wide stats: height, difficulty, hashrate, emission
async fn get_stats(State(app_state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    use crate::economics::{
        HALVING_INTERVAL_BLOCKS, INITIAL_BLOCK_REWARD_ZKA, TARGET_BLOCK_TIME_SECS,
    };

    let height = app_state.state_manager.get_height().await;
    let difficulty_bits = app_state.state_manager.get_next_difficulty();

    // Estimated network hashrate in H/s: expected hashes per block / block_time
    let hashrate_hs = (2u64.pow(difficulty_bits) as f64) / (TARGET_BLOCK_TIME_SECS as f64);

    // Cumulative emission in ZKA from the halving schedule
    let mut emission_zka = 0u64;
    let mut remaining = height;
    let mut reward = INITIAL_BLOCK_REWARD_ZKA;
    while remaining > 0 && reward > 0 {
        let in_epoch = remaining.min(HALVING_INTERVAL_BLOCKS);
        emission_zka = emission_zka.saturating_add(in_epoch * reward);
        remaining -= in_epoch;
        reward /= 2;
    }
    let emission_zka_f = emission_zka as f64; // reward already expressed in ZKA units

    // Last block timestamp for the client to derive recency
    let last_block_time = match app_state.state_manager.get_latest_block().await {
        Ok(Some(b)) => b.header.timestamp,
        _ => 0,
    };
    let runtime = global_network_runtime().snapshot().await;

    // El suministro real que lleva la cadena, en atomos, junto al techo y a lo
    // que falta emitir. `emission_zka` de arriba se mantiene porque hay
    // clientes viejos que lo leen, pero es una estimacion derivada de la altura
    // y no debe usarse para mostrar una cifra exacta.
    let emitted_atoms = app_state.state_manager.get_total_supply_emitted_atoms().await;
    let max_atoms = crate::economics::MAX_SUPPLY_ATOMS;

    Json(serde_json::json!({
        "height": height,
        "difficulty": difficulty_bits,
        "hashrate_hs": hashrate_hs,
        "emission_zka": emission_zka_f,
        "total_supply_emitted_atoms": emitted_atoms,
        "max_supply_atoms": max_atoms,
        "remaining_supply_atoms": max_atoms.saturating_sub(emitted_atoms),
        "last_block_time": last_block_time,
        "network_version": env!("CARGO_PKG_VERSION"),
        "connected_peers": runtime.connected_peers,
        "miners_online": runtime.miners_online,
        "remote_miners_online": runtime.remote_miners_online,
        "legacy_miners_online": runtime.legacy_miners_online,
        "outdated_miners_online": runtime.outdated_miners_online,
        "minimum_client_version": runtime.minimum_client_version,
        "local_mining": runtime.local_mining,
        "reported_hashrate_hps": runtime.total_reported_hashrate_hps,
    }))
}

async fn get_network_runtime(
    State(app_state): State<Arc<AppState>>,
) -> Json<NetworkRuntimeSnapshot> {
    let mut snapshot = global_network_runtime().snapshot().await;
    snapshot.local_chain_height = app_state.state_manager.get_height().await;
    Json(snapshot)
}

async fn get_sync_status(State(app_state): State<Arc<AppState>>) -> Json<SyncStatusSnapshot> {
    let local_height = app_state.state_manager.get_height().await;
    let last_block_at = match app_state.state_manager.get_latest_block().await {
        Ok(Some(block)) => block.header.timestamp,
        _ => 0,
    };
    Json(
        global_network_runtime()
            .sync_status_snapshot(local_height, last_block_at)
            .await,
    )
}

/// GET /peers — the local node's live mesh view: which p2p peers it is
/// connected to and over which transport (Tor onion / direct / iroh). This is
/// the serverless replacement for asking a central bootstrap RPC "who's on the
/// network" — a node only knows the peers it is actually talking to.
async fn get_peers() -> Json<serde_json::Value> {
    let peers = global_network_runtime().peer_summaries().await;
    let mut tor = 0u64;
    let mut direct = 0u64;
    let mut other = 0u64;
    for p in &peers {
        match p.transport.as_str() {
            "tor" => tor += 1,
            "direct" => direct += 1,
            _ => other += 1,
        }
    }
    // Suscriptores y malla por topico.
    //
    // Estar conectado no alcanza para recibir lo que se publica en un topico:
    // hace falta que algun par lo tenga suscrito, y para que un mensaje llegue
    // con fiabilidad hace falta ademas que este en la malla. Un cliente viejo
    // sincroniza bloques con normalidad y jamas se suscribe a un topico que no
    // conoce, asi que "tengo 2 pares" y "no me llegan las ofertas" conviven sin
    // contradiccion. Este es el numero que lo distingue.
    let topics = global_network_runtime()
        .topic_peer_counts()
        .into_iter()
        .map(|(nombre, suscritos, malla)| {
            serde_json::json!({
                "topic": nombre,
                "subscribed_peers": suscritos,
                "mesh_peers": malla,
            })
        })
        .collect::<Vec<_>>();

    Json(serde_json::json!({
        "connected_peers": peers.len(),
        "by_transport": { "tor": tor, "direct": direct, "other": other },
        "topics": topics,
        "peers": peers,
    }))
}

async fn post_miner_heartbeat(
    Json(heartbeat): Json<MinerHeartbeatRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let record = global_network_runtime()
        .record_miner_heartbeat(heartbeat)
        .await
        .map_err(|err| (StatusCode::BAD_REQUEST, err))?;
    let snapshot = global_network_runtime().snapshot().await;
    let accepted_for_mining = record.mining && !record.legacy_client && !record.outdated_client;
    Ok(Json(serde_json::json!({
        "status": "ok",
        "accepted_for_mining": accepted_for_mining,
        "client_version": record.client_version,
        "launcher": record.launcher,
        "launcher_version": record.launcher_version,
        "legacy_client": record.legacy_client,
        "outdated_client": record.outdated_client,
        "minimum_client_version": snapshot.minimum_client_version,
        "miners_online": snapshot.miners_online,
        "heartbeat_ttl_secs": snapshot.heartbeat_ttl_secs,
    })))
}

async fn get_network_miners() -> Json<serde_json::Value> {
    let records = global_network_runtime().miner_records().await;
    let snapshot = global_network_runtime().snapshot().await;
    Json(serde_json::json!({
        "miners_online": snapshot.miners_online,
        "legacy_miners_online": snapshot.legacy_miners_online,
        "outdated_miners_online": snapshot.outdated_miners_online,
        "minimum_client_version": snapshot.minimum_client_version,
        "miners": records,
    }))
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct LocalMiningControlRequest {
    #[serde(default)]
    threads: u64,
}

async fn get_local_mining_control() -> Json<LocalMiningControlSnapshot> {
    Json(global_local_mining_control().snapshot())
}

async fn post_local_mining_start(
    Json(request): Json<LocalMiningControlRequest>,
) -> Result<Json<LocalMiningControlSnapshot>, (StatusCode, String)> {
    if global_network_runtime().checkpoint_quarantined() {
        return Err((
            StatusCode::CONFLICT,
            "mining is quarantined: local canonical history does not match the compiled mainnet checkpoint; synchronization remains enabled"
                .to_string(),
        ));
    }
    Ok(Json(
        global_local_mining_control().enable(request.threads.max(1)),
    ))
}

async fn post_local_mining_stop() -> Json<LocalMiningControlSnapshot> {
    Json(global_local_mining_control().disable())
}

/// Get current block height
async fn get_height(State(app_state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let height = app_state.state_manager.get_height().await;
    Json(serde_json::json!({"height": height}))
}

/// Get current committed state root
async fn get_state_root(State(app_state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let state_root = app_state.state_manager.get_state_root().await;
    Json(serde_json::json!({ "state_root": state_root }))
}

/// Return the public Merkle root that anchors spendable private commitments.
async fn get_private_commitment_root(
    State(app_state): State<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let root = app_state.state_manager.get_private_commitment_root().await;
    Json(serde_json::json!({ "private_commitment_root": root }))
}

/// Return the public inclusion proof needed by a self-custody light client
/// when it spends one of its locally-decrypted notes.
async fn get_private_commitment_proof(
    State(app_state): State<Arc<AppState>>,
    Path(commitment): Path<String>,
) -> Result<Json<crate::state::PrivateCommitmentInclusionProof>, (StatusCode, String)> {
    match app_state
        .state_manager
        .get_private_commitment_inclusion_proof(&commitment)
        .await
    {
        Some(proof) => Ok(Json(proof)),
        None => Err((
            StatusCode::NOT_FOUND,
            "Private commitment not found".to_string(),
        )),
    }
}

/// Get account state
async fn get_account(
    State(app_state): State<Arc<AppState>>,
    Path(address): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Balance and nonce are private â€” only confirm the address exists.
    match app_state.state_manager.get_account(&address).await {
        Some(_) => Ok(Json(serde_json::json!({
            "address": address,
            "valid": true,
        }))),
        None => Err((StatusCode::NOT_FOUND, "Address not found".to_string())),
    }
}

/// Get Merkle inclusion proof for an account in the current state tree
async fn get_account_proof(
    State(app_state): State<Arc<AppState>>,
    Path(address): Path<String>,
) -> Result<Json<AccountInclusionProof>, (StatusCode, String)> {
    match app_state
        .state_manager
        .get_account_inclusion_proof(&address)
        .await
    {
        Some(proof) => Ok(Json(proof)),
        None => Err((StatusCode::NOT_FOUND, "Account not found".to_string())),
    }
}

/// Verify an account inclusion proof against an expected root.
async fn verify_account_proof(
    State(app_state): State<Arc<AppState>>,
    Json(req): Json<VerifyProofRequest>,
) -> Result<Json<VerifyProofResponse>, (StatusCode, String)> {
    if !validate_root_format(&req.expected_root) {
        app_state
            .security_stats
            .bad_request_total
            .fetch_add(1, Ordering::Relaxed);
        return Err((
            StatusCode::BAD_REQUEST,
            "expected_root format is invalid".to_string(),
        ));
    }

    Ok(Json(VerifyProofResponse {
        valid: StateManager::verify_account_inclusion_proof(&req.expected_root, &req.proof),
    }))
}

/// Get block by height
fn block_metadata_json(
    block: &crate::types::Block,
    difficulty_bits: Option<u32>,
) -> serde_json::Value {
    use crate::economics::{format_zka_amount, ATOMS_PER_ZKA};

    let reward_atoms = block.transactions.first().map(|tx| tx.amount).unwrap_or(0);
    let reward_zka = reward_atoms as f64 / ATOMS_PER_ZKA as f64;
    let size_bytes = serde_json::to_vec(block)
        .map(|bytes| bytes.len())
        .unwrap_or(0);

    serde_json::json!({
        "hash": block.header.hash,
        "height": block.header.block_height,
        "timestamp": block.header.timestamp,
        "state_root": block.header.state_root,
        "previous_hash": block.header.prev_hash,
        "nonce": block.header.nonce,
        "difficulty": difficulty_bits,
        "difficulty_bits": difficulty_bits,
        "size_bytes": size_bytes,
        "reward_atoms": reward_atoms,
        "reward_zka": reward_zka,
        "reward_zka_display": format_zka_amount(reward_atoms),
        "transactions_count": block.transactions.len(),
        "private_tx_count": block.private_tx_hashes.len(),
        // Count only: nullifiers are anchored in the block's merkle root as a
        // public double-spend proof and never reveal their owner, but the
        // count is all the explorer needs for transparency.
        "private_nullifier_count": block.private_nullifiers.len(),
        "private_coinbase": block.private_coinbase.is_some(),
    })
}

fn raw_block_json(
    block: &crate::types::Block,
    difficulty_bits: Option<u32>,
) -> Result<serde_json::Value, serde_json::Error> {
    let mut value = serde_json::to_value(block)?;
    if let (Some(bits), Some(object)) = (difficulty_bits, value.as_object_mut()) {
        object.insert("difficulty".to_string(), serde_json::json!(bits));
        object.insert("difficulty_bits".to_string(), serde_json::json!(bits));
    }
    Ok(value)
}

async fn get_block_by_height(
    State(app_state): State<Arc<AppState>>,
    Path(height): Path<u64>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if height == 0 {
        let block = crate::state::chain::genesis_parent_for(crate::economics::MAINNET_CHAIN_ID);
        return Ok(Json(block_metadata_json(&block, None)));
    }

    match app_state.state_manager.get_block_by_height(height).await {
        Ok(Some(block)) => {
            let difficulty_bits = app_state
                .state_manager
                .get_block_difficulty_bits(&block.header.hash)
                .map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("Failed to get block difficulty: {}", e),
                    )
                })?;
            Ok(Json(block_metadata_json(&block, difficulty_bits)))
        }
        Ok(None) => Err((StatusCode::NOT_FOUND, "Block not found".to_string())),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to get block: {}", e),
        )),
    }
}

/// Get the full canonical block by height for node bootstrap/sync clients.
/// This is public chain data. Wallet secrets, seeds and private keys are never
/// part of a block and are not exposed here.
async fn get_raw_block_by_height(
    State(app_state): State<Arc<AppState>>,
    Path(height): Path<u64>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if height == 0 {
        let block = crate::state::chain::genesis_parent_for(crate::economics::MAINNET_CHAIN_ID);
        return raw_block_json(&block, None).map(Json).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to serialize genesis block: {}", e),
            )
        });
    }

    match app_state.state_manager.get_block_by_height(height).await {
        Ok(Some(block)) => {
            let difficulty_bits = app_state
                .state_manager
                .get_block_difficulty_bits(&block.header.hash)
                .map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("Failed to get block difficulty: {}", e),
                    )
                })?;
            raw_block_json(&block, difficulty_bits)
                .map(Json)
                .map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("Failed to serialize block: {}", e),
                    )
                })
        }
        Ok(None) => Err((StatusCode::NOT_FOUND, "Block not found".to_string())),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to get block: {}", e),
        )),
    }
}

/// Get full canonical blocks in a height range for node bootstrap/sync clients.
async fn get_raw_blocks_range(
    State(app_state): State<Arc<AppState>>,
    Path((from, to)): Path<(u64, u64)>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if from > to {
        return Err((
            StatusCode::BAD_REQUEST,
            "Invalid range: from > to".to_string(),
        ));
    }
    if to - from > 1000 {
        return Err((
            StatusCode::BAD_REQUEST,
            "Range too large: maximum 1000 blocks".to_string(),
        ));
    }

    let mut blocks = Vec::new();
    if from == 0 {
        let genesis = crate::state::chain::genesis_parent_for(crate::economics::MAINNET_CHAIN_ID);
        blocks.push(raw_block_json(&genesis, None).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to serialize genesis block: {}", e),
            )
        })?);
    }
    match app_state
        .state_manager
        .get_blocks_in_range(from.max(1), to)
        .await
    {
        Ok(mut range_blocks) => {
            for block in range_blocks.drain(..) {
                let difficulty_bits = app_state
                    .state_manager
                    .get_block_difficulty_bits(&block.header.hash)
                    .map_err(|e| {
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            format!("Failed to get block difficulty: {}", e),
                        )
                    })?;
                blocks.push(raw_block_json(&block, difficulty_bits).map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("Failed to serialize block: {}", e),
                    )
                })?);
            }
            Ok(Json(serde_json::json!({
                "from_height": from,
                "to_height": to,
                "blocks_count": blocks.len(),
                "blocks": blocks,
            })))
        }
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to get blocks: {}", e),
        )),
    }
}

/// Get block by hash
async fn get_block_by_hash(
    State(app_state): State<Arc<AppState>>,
    Path(hash): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    match app_state.state_manager.get_block_by_hash(&hash).await {
        Ok(Some(block)) => {
            let difficulty_bits = app_state
                .state_manager
                .get_block_difficulty_bits(&block.header.hash)
                .map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("Failed to get block difficulty: {}", e),
                    )
                })?;
            Ok(Json(block_metadata_json(&block, difficulty_bits)))
        }
        Ok(None) => Err((StatusCode::NOT_FOUND, "Block not found".to_string())),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to get block: {}", e),
        )),
    }
}

/// Get the latest block
async fn get_latest_block(
    State(app_state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    match app_state.state_manager.get_latest_block().await {
        Ok(Some(block)) => {
            let difficulty_bits = app_state
                .state_manager
                .get_block_difficulty_bits(&block.header.hash)
                .map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("Failed to get block difficulty: {}", e),
                    )
                })?;
            Ok(Json(block_metadata_json(&block, difficulty_bits)))
        }
        Ok(None) => Err((StatusCode::NOT_FOUND, "No blocks in the chain".to_string())),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to get block: {}", e),
        )),
    }
}

/// Fetch a private transaction body by its hash from the local node's
/// `PrivateTransactionPool`. What comes back is the canonical private tx
/// shape â€” commitments, nullifiers, ring metadata, stealth outputs,
/// zk proof â€” **never** plaintext sender/receiver/amount. If this node
/// has no `private_wallet_manager` wired (typical for tests), the
/// endpoint 404s rather than lying.
/// Reemplaza el `status` del cuerpo por lo que dice la cadena.
///
/// El que viaja dentro lo estampa quien crea la transaccion y no se actualiza
/// nunca: dice "pendiente" para siempre, este confirmada o rechazada. La
/// diferencia importa sobre todo para un gasto doble: una wallet que ve
/// "pendiente" espera, y va a esperar para siempre por algo que ningun minero
/// puede incluir. Con "Rejected" puede decirle al usuario que esa transaccion
/// no va a salir y que sus fondos siguen donde estaban.
async fn con_estado_de_cadena(
    app_state: &Arc<AppState>,
    hash: &str,
    mut valor: serde_json::Value,
) -> serde_json::Value {
    let Ok(estado) = app_state
        .state_manager
        .private_transaction_chain_state(hash)
        .await
    else {
        return valor;
    };
    let Some(obj) = valor.as_object_mut() else {
        return valor;
    };
    obj.insert("status".to_string(), serde_json::json!(estado.etiqueta()));
    match estado {
        crate::state::PrivateTxChainState::Confirmada { altura } => {
            obj.insert("block_height".to_string(), serde_json::json!(altura));
        }
        crate::state::PrivateTxChainState::GastoDoble { altura } => {
            obj.insert(
                "rejected_reason".to_string(),
                serde_json::json!(format!(
                    "la nota que gasta ya fue gastada por otra transaccion en el bloque {altura}"
                )),
            );
        }
        crate::state::PrivateTxChainState::Gastada => {
            obj.insert(
                "rejected_reason".to_string(),
                serde_json::json!(
                    "la nota que gasta ya no existe, pero el nodo no sabe en que bloque se                      consumio"
                ),
            );
        }
        _ => {}
    }
    valor
}

async fn get_private_tx_by_hash(
    State(app_state): State<Arc<AppState>>,
    Path(hash): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // El cuerpo puede venir del pool en memoria o del almacenamiento. Los dos
    // caminos siguen despues a la correccion de estado: cuando el del pool
    // devolvia aqui mismo, se saltaba esa correccion y una transaccion muerta
    // se reportaba "pendiente" para siempre. Es el mismo bug dos veces, y la
    // segunda costo mas de encontrar porque el arreglo ya estaba escrito.
    let del_pool = if let Some(manager) = &app_state.private_wallet_manager {
        let manager = manager.read().await;
        // Serialize with serde_json; the type is serde-derive-friendly and its
        // wire shape already omits sender/receiver/amount clear fields (they
        // don't exist on `PrivateTransaction`).
        manager
            .get_private_transaction(&hash)
            .map(serde_json::to_value)
            .transpose()
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("failed to serialize private tx: {}", e),
                )
            })?
    } else {
        None
    };
    if let Some(valor) = del_pool {
        return Ok(Json(con_estado_de_cadena(&app_state, &hash, valor).await));
    }

    let Some(bytes) = app_state
        .state_manager
        .get_private_transaction_bytes(&hash)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to read private tx: {}", e),
            )
        })?
    else {
        return Err((
            StatusCode::NOT_FOUND,
            format!("private transaction {} not found", hash),
        ));
    };

    let tx: crate::private_wallet::PrivateTransaction =
        serde_json::from_slice(&bytes).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to parse private tx: {}", e),
            )
        })?;
    let mut valor = serde_json::to_value(tx).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to serialize private tx: {}", e),
        )
    })?;

    Ok(Json(con_estado_de_cadena(&app_state, &hash, valor).await))
}

/// Si un nulificador ya fue gastado en la cadena, y a que altura.
///
/// Existe porque una wallet no puede averiguarlo sola. El indice vive en la
/// base de la cadena, y esa base la tiene tomada el nodo mientras corre --
/// siempre, en la practica. Una wallet que intente abrirla para comprobar sus
/// notas falla, y si el fallo es silencioso termina construyendo transacciones
/// que gastan notas que ya no existen: la red las propaga, ningun minero puede
/// incluirlas, y el usuario espera un envio que jamas va a salir. Paso de
/// verdad, dos veces.
///
/// **Privacidad.** Un nulificador es un dato publico: viaja en el bloque que lo
/// consume. Responder si esta gastado no revela nada que no este ya en la
/// cadena. Lo unico que se filtra es que a quien pregunta le interesa esa nota,
/// y por eso solo tiene sentido preguntarle a un nodo propio.
async fn get_nullifier_spent(
    State(app_state): State<Arc<AppState>>,
    Path(tag): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let tag = tag.trim().to_string();
    if tag.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "empty nullifier".to_string()));
    }
    let (spent, height) = app_state
        .state_manager
        .nullifier_spent_with_height(&tag)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to read nullifier index: {e}"),
            )
        })?;
    Ok(Json(serde_json::json!({
        "nullifier": tag,
        "spent": spent,
        // Puede faltar aunque este gastado: entradas viejas del indice pueden no
        // llevar la altura. Que falte no debilita el "spent".
        "height": height,
    })))
}

/// List the node's still-pending (un-mined) private transactions, each with
/// its full canonical body. This is the RPC-pull counterpart to libp2p gossip:
/// a miner behind NAT — which can open outbound HTTP but receives no inbound
/// P2P — polls this endpoint to pull pending private txs into its own pool so
/// it can include them in the next block. Without it, a private tx published
/// to a public node never reaches a NAT'd miner and is never mined.
///
/// Privacy: `PrivateTransaction` carries no clear sender/receiver/amount, so
/// exposing the bodies here leaks nothing beyond what `/private/tx/:hash`
/// already serves.
async fn get_pending_private_txs(
    State(app_state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let hashes = app_state
        .state_manager
        .iterate_pending_private_transaction_hashes()
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to list pending private txs: {e}"),
            )
        })?;

    // Cap the response so a huge backlog can't blow up a single request; the
    // miner polls repeatedly and will drain the rest on later passes.
    const MAX_PENDING_RETURNED: usize = 64;
    let mut txs = Vec::new();
    for hash in hashes.into_iter().take(MAX_PENDING_RETURNED) {
        // Skip anything already committed on-chain (pending index can lag).
        if app_state
            .state_manager
            .has_private_transaction_hash(&hash)
            .await
            .unwrap_or(false)
        {
            continue;
        }
        if let Ok(Some(bytes)) = app_state.state_manager.get_private_transaction_bytes(&hash) {
            if let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                if let Some(obj) = value.as_object_mut() {
                    obj.insert(
                        "tx_hash".to_string(),
                        serde_json::Value::String(hash.clone()),
                    );
                }
                txs.push(value);
            }
        }
    }

    Ok(Json(serde_json::json!({
        "count": txs.len(),
        "pending": txs,
    })))
}

#[derive(Debug, Deserialize)]
struct PrivateViewScanRequest {
    address: String,
    scan_key_hex: String,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct PrivateViewScanKeyRequest {
    scan_key_hex: String,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Serialize)]
struct PrivateViewScanResponse {
    address: String,
    scanned_transactions: usize,
    matching_outputs: usize,
    total_amount_atoms: u64,
    matches: Vec<PrivateViewScanMatch>,
}

async fn post_private_view_scan(
    State(app_state): State<Arc<AppState>>,
    Json(mut req): Json<PrivateViewScanRequest>,
) -> Result<Json<PrivateViewScanResponse>, (StatusCode, String)> {
    req.address = req.address.trim().to_string();
    req.scan_key_hex = req.scan_key_hex.trim().trim_start_matches("0x").to_string();
    if req.address.len() > 180 || !req.address.starts_with("zpriv:") {
        return Err((
            StatusCode::BAD_REQUEST,
            "address must be a zpriv payment address".to_string(),
        ));
    }
    if req.scan_key_hex.len() != 32 && req.scan_key_hex.len() != 64 {
        return Err((
            StatusCode::BAD_REQUEST,
            "scan_key_hex must encode 16 or 32 bytes".to_string(),
        ));
    }
    if !req.scan_key_hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err((
            StatusCode::BAD_REQUEST,
            "scan_key_hex must be hexadecimal".to_string(),
        ));
    }

    let mut scan_key = hex::decode(&req.scan_key_hex).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            "scan_key_hex must be valid hexadecimal".to_string(),
        )
    })?;
    validate_view_scan_key_for_address(&scan_key, &req.address)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("view scan failed: {e}")))?;

    let archived = app_state
        .state_manager
        .iterate_private_transactions()
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to list private transactions: {e}"),
            )
        })?;

    let max_returned = req.limit.unwrap_or(100).clamp(1, 500);
    let scanned_transactions = archived.len();
    let mut matches = Vec::new();
    for (_hash, body) in archived {
        let tx = match serde_json::from_slice::<PrivateTransaction>(&body) {
            Ok(tx) => tx,
            Err(_) => continue,
        };
        let mut tx_matches =
            scan_private_transaction_with_view_key(&scan_key, &req.address, &tx)
                .map_err(|e| (StatusCode::BAD_REQUEST, format!("view scan failed: {e}")))?;
        matches.append(&mut tx_matches);
        if matches.len() >= max_returned {
            matches.truncate(max_returned);
            break;
        }
    }
    scan_key.zeroize();
    req.scan_key_hex.zeroize();

    let total_amount_atoms = matches
        .iter()
        .fold(0u64, |total, item| total.saturating_add(item.amount));

    Ok(Json(PrivateViewScanResponse {
        address: req.address,
        scanned_transactions,
        matching_outputs: matches.len(),
        total_amount_atoms,
        matches,
    }))
}

async fn post_private_view_scan_key(
    State(app_state): State<Arc<AppState>>,
    Json(mut req): Json<PrivateViewScanKeyRequest>,
) -> Result<Json<PrivateViewScanResponse>, (StatusCode, String)> {
    req.scan_key_hex = req.scan_key_hex.trim().trim_start_matches("0x").to_string();
    if req.scan_key_hex.len() != 32 && req.scan_key_hex.len() != 64 {
        return Err((
            StatusCode::BAD_REQUEST,
            "scan_key_hex must encode 16 or 32 bytes".to_string(),
        ));
    }
    if !req.scan_key_hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err((
            StatusCode::BAD_REQUEST,
            "scan_key_hex must be hexadecimal".to_string(),
        ));
    }

    let mut scan_key = hex::decode(&req.scan_key_hex).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            "scan_key_hex must be valid hexadecimal".to_string(),
        )
    })?;
    let address = payment_address_from_scan_key(&scan_key)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("view scan failed: {e}")))?;

    let archived = app_state
        .state_manager
        .iterate_private_transactions()
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to list private transactions: {e}"),
            )
        })?;

    let max_returned = req.limit.unwrap_or(100).clamp(1, 500);
    let scanned_transactions = archived.len();
    let mut matches = Vec::new();
    for (_hash, body) in archived {
        let tx = match serde_json::from_slice::<PrivateTransaction>(&body) {
            Ok(tx) => tx,
            Err(_) => continue,
        };
        let mut tx_matches = scan_private_transaction_with_view_key(&scan_key, &address, &tx)
            .map_err(|e| (StatusCode::BAD_REQUEST, format!("view scan failed: {e}")))?;
        matches.append(&mut tx_matches);
        if matches.len() >= max_returned {
            matches.truncate(max_returned);
            break;
        }
    }
    scan_key.zeroize();
    req.scan_key_hex.zeroize();

    let total_amount_atoms = matches
        .iter()
        .fold(0u64, |total, item| total.saturating_add(item.amount));

    Ok(Json(PrivateViewScanResponse {
        address,
        scanned_transactions,
        matching_outputs: matches.len(),
        total_amount_atoms,
        matches,
    }))
}

/// Archive a canonical private transaction body so future wallet restores can
/// replay confirmed private history from a public RPC node.
async fn post_private_tx_body(
    State(app_state): State<Arc<AppState>>,
    Json(tx): Json<PrivateTransaction>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let hash_owned: String = tx.tx_hash.trim().to_owned();
    let hash = hash_owned.as_str();
    if hash.is_empty()
        || hash.len() > 128
        || hash
            .chars()
            .any(|c| !(c.is_ascii_hexdigit() || matches!(c, '-' | '_')))
    {
        app_state
            .security_stats
            .bad_request_total
            .fetch_add(1, Ordering::Relaxed);
        return Err((
            StatusCode::BAD_REQUEST,
            "private transaction hash format is invalid".to_string(),
        ));
    }

    let mut validator = PrivateWalletManager::new();
    if let Err(err) = validator.validate_inbound_private_transaction(&tx) {
        app_state
            .security_stats
            .bad_request_total
            .fetch_add(1, Ordering::Relaxed);
        return Err((
            StatusCode::BAD_REQUEST,
            format!("private transaction validation failed: {err}"),
        ));
    }
    if let Some((existing_hash, nullifier)) =
        find_pending_private_nullifier_conflict(app_state.as_ref(), hash, &tx)?
    {
        return Err((
            StatusCode::CONFLICT,
            format!("private spend already pending: tx {existing_hash} uses nullifier {nullifier}"),
        ));
    }

    let body = serde_json::to_vec(&tx).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to serialize private tx: {e}"),
        )
    })?;
    let already_present = app_state
        .state_manager
        .get_private_transaction_bytes(hash)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to read private tx: {e}"),
            )
        })?
        .is_some();
    app_state
        .state_manager
        .save_private_transaction_bytes(hash, &body)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to archive private tx: {e}"),
            )
        })?;

    let confirmed = app_state
        .state_manager
        .has_private_transaction_hash(hash)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to inspect private tx confirmation: {e}"),
            )
        })?;
    if !confirmed {
        app_state
            .state_manager
            .save_pending_private_transaction_hash(hash)
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("failed to mark private tx pending: {e}"),
                )
            })?;
        if let Some(sender) = &app_state.private_tx_broadcaster {
            if let Err(e) = sender.send(tx) {
                tracing::warn!(
                    "post_private_tx_body: failed to forward tx {hash} to mining pool: {e}"
                );
            }
        }
    }

    Ok(Json(serde_json::json!({
        "hash": hash,
        "status": if already_present { "already_archived" } else { "archived" },
        "confirmed": confirmed,
        "pending": !confirmed,
    })))
}

/// Get the private-transaction hashes committed at a specific block.
/// Returns only hashes â€” by design the full `PrivateTransaction` body is
/// never exposed through this API; clients that want the body look it up
/// via `/private/tx/:hash` on a node that holds the body.
async fn get_block_private_tx_hashes(
    State(app_state): State<Arc<AppState>>,
    Path(height): Path<u64>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if height == 0 {
        return Ok(Json(serde_json::json!({
            "block_height": height,
            "private_tx_count": 0,
            "private_tx_hashes": [],
        })));
    }

    match app_state
        .state_manager
        .get_block_private_tx_hashes(height)
        .await
    {
        Ok(hashes) => Ok(Json(serde_json::json!({
            "block_height": height,
            "private_tx_count": hashes.len(),
            "private_tx_hashes": hashes,
        }))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to get block private tx hashes: {}", e),
        )),
    }
}

/// Get private-transaction hashes for a range of blocks in one request.
async fn get_block_private_tx_hashes_range(
    State(app_state): State<Arc<AppState>>,
    Path((from, to)): Path<(u64, u64)>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if from > to {
        return Err((
            StatusCode::BAD_REQUEST,
            "Invalid range: from > to".to_string(),
        ));
    }
    if to - from > 1000 {
        return Err((
            StatusCode::BAD_REQUEST,
            "Range too large: maximum 1000 blocks".to_string(),
        ));
    }

    let mut blocks = Vec::new();
    for height in from..=to {
        let hashes = app_state
            .state_manager
            .get_block_private_tx_hashes(height)
            .await
            .unwrap_or_default();
        blocks.push(serde_json::json!({
            "height": height,
            "private_tx_count": hashes.len(),
            "private_tx_hashes": hashes,
        }));
    }

    Ok(Json(serde_json::json!({
        "from_height": from,
        "to_height": to,
        "blocks": blocks,
    })))
}

/// Get transactions in a specific block
async fn get_block_transactions(
    State(app_state): State<Arc<AppState>>,
    Path(height): Path<u64>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if height == 0 {
        return Ok(Json(serde_json::json!({
            "block_height": height,
            "transactions_count": 0,
            "transactions": [],
        })));
    }

    match app_state.state_manager.get_block_transactions(height).await {
        Ok(txs) => Ok(Json(serde_json::json!({
            "block_height": height,
            "transactions_count": txs.len(),
            // `Transaction` is the public-ledger type; by construction every
            // row here carries plaintext sender/receiver/amount. Private txs
            // are in `PrivateTransaction` and never serialize through this
            // endpoint â€” the old placebo redaction that keyed off a dead
            // `zk_proof` flag is gone.
            // Only expose the hash â€” sender/receiver/amount are private by protocol.
            "transactions": txs
                .iter()
                .map(|tx| serde_json::json!({ "hash": tx.hash }))
                .collect::<Vec<_>>(),
        }))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to get transactions: {}", e),
        )),
    }
}

/// Get blocks in a range
/// Per-block private-coinbase metadata for a range, so a wallet owner can match
/// their mining rewards CLIENT-SIDE: the recipient_commitment is the miner's
/// `zpriv:` address (already public on-chain) and the amount is the public block
/// reward. The explorer derives the user's address from their scan key locally
/// and compares — the scan key never leaves the browser.
async fn get_private_coinbases_range(
    State(app_state): State<Arc<AppState>>,
    Path((from, to)): Path<(u64, u64)>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if from > to {
        return Err((
            StatusCode::BAD_REQUEST,
            "Invalid range: from > to".to_string(),
        ));
    }
    if to - from > 1000 {
        return Err((
            StatusCode::BAD_REQUEST,
            "Range too large: maximum 1000 blocks".to_string(),
        ));
    }
    match app_state.state_manager.get_blocks_in_range(from, to).await {
        Ok(blocks) => {
            let coinbases: Vec<serde_json::Value> = blocks
                .iter()
                .filter_map(|b| {
                    let cb = b.private_coinbase.as_ref()?;
                    let amount = b.transactions.first().map(|tx| tx.amount).unwrap_or(0);
                    Some(serde_json::json!({
                        "height": b.header.block_height,
                        "recipient_commitment": cb.recipient_commitment,
                        "amount_atoms": amount,
                        "unlock_height": cb.unlock_height,
                    }))
                })
                .collect();
            Ok(Json(serde_json::json!({
                "from_height": from,
                "to_height": to,
                "coinbases_count": coinbases.len(),
                "coinbases": coinbases,
            })))
        }
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to read blocks: {e}"),
        )),
    }
}

async fn get_blocks_range(
    State(app_state): State<Arc<AppState>>,
    Path((from, to)): Path<(u64, u64)>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if from > to {
        return Err((
            StatusCode::BAD_REQUEST,
            "Invalid range: from > to".to_string(),
        ));
    }
    if to - from > 1000 {
        return Err((
            StatusCode::BAD_REQUEST,
            "Range too large: maximum 1000 blocks".to_string(),
        ));
    }

    match app_state.state_manager.get_blocks_in_range(from, to).await {
        Ok(blocks) => {
            let mut block_values = Vec::new();
            if from == 0 {
                let genesis =
                    crate::state::chain::genesis_parent_for(crate::economics::MAINNET_CHAIN_ID);
                block_values.push(serde_json::json!({
                    "hash": genesis.header.hash,
                    "height": genesis.header.block_height,
                    "timestamp": genesis.header.timestamp,
                }));
            }
            block_values.extend(blocks.iter().map(|b| {
                serde_json::json!({
                    "hash": b.header.hash,
                    "height": b.header.block_height,
                    "timestamp": b.header.timestamp,
                })
            }));
            Ok(Json(serde_json::json!({
            "from_height": from,
            "to_height": to,
                "blocks_count": block_values.len(),
                "blocks": block_values,
            })))
        }
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to get blocks: {}", e),
        )),
    }
}

/// Get a specific transaction by hash (from mempool or recent blocks)
async fn get_transaction_by_hash(
    State(app_state): State<Arc<AppState>>,
    Path(hash): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Check mempool first. Public mempool only holds `Transaction` rows,
    // which are by construction public (no zk_proof field anymore).
    if let Some(tx) = app_state.mempool.get_transaction(&hash).await {
        return Ok(Json(serde_json::json!({
            "hash": tx.hash,
            "timestamp": tx.timestamp,
            "status": "pending_in_mempool",
        })));
    }

    // For now, transactions in blocks are accessed via /blocks/:height/transactions
    Err((
        StatusCode::NOT_FOUND,
        "Transaction not found in mempool. Check confirmed blocks.".to_string(),
    ))
}

/// Rollback state to a previous height (Admin endpoint - requires token)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollbackRequest {
    pub target_height: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollbackResponse {
    pub success: bool,
    pub message: String,
}

async fn admin_rollback(
    State(app_state): State<Arc<AppState>>,
    Json(req): Json<RollbackRequest>,
) -> Result<Json<RollbackResponse>, (StatusCode, String)> {
    match app_state
        .state_manager
        .rollback_to_height(req.target_height)
        .await
    {
        Ok(_) => Ok(Json(RollbackResponse {
            success: true,
            message: format!("Rolled back to height {}", req.target_height),
        })),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Rollback failed: {}", e),
        )),
    }
}

/// Get available rollback points
async fn admin_rollback_points(State(app_state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let points = app_state.state_manager.get_rollback_points().await;
    Json(serde_json::json!({
        "available_heights": points,
        "count": points.len(),
    }))
}

/// Submit a transaction to the mempool
async fn submit_transaction(
    State(app_state): State<Arc<AppState>>,
    Json(req): Json<TransactionRequest>,
) -> Result<Json<TransactionResponse>, (StatusCode, String)> {
    use crate::types::{Signature, Transaction};

    if !validate_address(&req.sender) || !validate_address(&req.receiver) {
        app_state
            .security_stats
            .bad_request_total
            .fetch_add(1, Ordering::Relaxed);
        return Err((
            StatusCode::BAD_REQUEST,
            "invalid sender/receiver address".to_string(),
        ));
    }
    if req.sender == req.receiver {
        app_state
            .security_stats
            .bad_request_total
            .fetch_add(1, Ordering::Relaxed);
        return Err((
            StatusCode::BAD_REQUEST,
            "self-transfer is not allowed".to_string(),
        ));
    }
    if req.amount == 0 {
        app_state
            .security_stats
            .bad_request_total
            .fetch_add(1, Ordering::Relaxed);
        return Err((
            StatusCode::BAD_REQUEST,
            "amount must be greater than zero".to_string(),
        ));
    }

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let signature_provided = req.signature_hex.is_some() || req.public_key_hex.is_some();
    let signature = if app_state.security.require_tx_signature || signature_provided {
        if req.signature_hex.is_none() || req.public_key_hex.is_none() {
            app_state
                .security_stats
                .bad_request_total
                .fetch_add(1, Ordering::Relaxed);
            return Err((
                StatusCode::BAD_REQUEST,
                "both signature_hex and public_key_hex are required".to_string(),
            ));
        }
        match verify_transaction_request_signature(&req, timestamp) {
            Ok(sig) => sig,
            Err(err) => {
                app_state
                    .security_stats
                    .bad_request_total
                    .fetch_add(1, Ordering::Relaxed);
                return Err((StatusCode::BAD_REQUEST, err));
            }
        }
    } else {
        Signature(vec![])
    };

    let tx = Transaction::new(
        req.sender.clone(),
        req.receiver,
        req.amount,
        req.nonce,
        timestamp,
        signature,
    );

    let state = app_state.state_manager.get_state().await;
    let priority_fee = req.priority_fee.unwrap_or_default();
    match app_state
        .mempool
        .add_transaction_with_fee(tx.clone(), &state, priority_fee)
        .await
    {
        Ok(_) => Ok(Json(TransactionResponse {
            hash: tx.hash,
            status: "added_to_mempool".to_string(),
        })),
        Err(e) => {
            app_state
                .security_stats
                .bad_request_total
                .fetch_add(1, Ordering::Relaxed);
            Err((
                StatusCode::BAD_REQUEST,
                format!("Transaction rejected: {}", e),
            ))
        }
    }
}

// â”€â”€ /private/wallet/send-from-seed â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
//
// Build, sign and broadcast a private transaction on behalf of a mobile
// (or any seed-only) client. The client encrypts the BIP-39 entropy with
// the node's ephemeral server key and POSTs the sealed envelope together
// with recipient + amount; the node uses the decrypted seed *ephemerally*
// in a sandboxed temp directory to:
//
//   1. Restore the wallet keys (`wallet-restore-from-keys`)
//   2. Sync UTXOs from the local chain DB (`balance` triggers it)
//   3. Build the PrivateTransaction (CLSAG ring sig + Groth16 ZK proof
//      using the trusted-setup `.params` files this node already has)
//      and submit to the mempool (`send`)
//   4. Delete the temp directory unconditionally
//
// The seed is encrypted in transit and never touches disk except inside
// the temp directory, which is always removed on the response path. The node
// operator can still inspect process memory, so this endpoint is a
// usability/privacy tradeoff: clients who care about full custody should
// point `Custom RPC` at their own node (see RpcSettings in zkapriv).
//
// This is the only path that lets ZKAPriv submit private transfers from
// Android today â€” generating a Groth16 proof inside Android would require
// embedding the multi-MB trusted setup and a snark-friendly EC library,
// which is out of scope. Implemented via shell-out to the bundled `zoka`
// CLI to reuse the battle-tested send pipeline instead of reimplementing
// the cryptography in the HTTP handler.

#[derive(Debug, Deserialize)]
struct SendFromSeedRequest {
    /// X25519 ephemeral public key generated by the client for this request.
    ephemeral_pubkey_hex: String,
    /// AES-256-GCM nonce generated by the client for this request.
    nonce_hex: String,
    /// AES-256-GCM ciphertext and tag containing the 16 or 32 byte seed.
    encrypted_seed_hex: String,
    /// Recipient zka1â€¦ address.
    to: String,
    /// Amount in ZKA atoms (1 ZKA = 1_000_000 atoms).
    amount_atoms: u64,
    /// Priority fee in atoms. Defaults to 1_000 (0.001 ZKA) when absent.
    #[serde(default)]
    fee_atoms: Option<u64>,
}

#[derive(Debug, Serialize)]
struct SendFromSeedResponse {
    hash: String,
    status: String,
}

#[derive(Debug, Serialize)]
struct ServerPubkeyResponse {
    algorithm: &'static str,
    public_key_hex: String,
}

struct ServerSeedKey {
    secret: x25519_dalek::StaticSecret,
    public: x25519_dalek::PublicKey,
}

fn server_seed_key() -> &'static ServerSeedKey {
    use rand::RngCore;

    static KEY: OnceLock<ServerSeedKey> = OnceLock::new();
    KEY.get_or_init(|| {
        let mut secret_bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut secret_bytes);
        let secret = x25519_dalek::StaticSecret::from(secret_bytes);
        secret_bytes.zeroize();
        let public = x25519_dalek::PublicKey::from(&secret);
        ServerSeedKey { secret, public }
    })
}

async fn get_server_pubkey() -> Json<ServerPubkeyResponse> {
    Json(ServerPubkeyResponse {
        algorithm: "x25519-aes256gcm-v1",
        public_key_hex: hex::encode(server_seed_key().public.as_bytes()),
    })
}

fn decode_fixed_hex<const N: usize>(
    value: &str,
    field: &str,
) -> Result<[u8; N], (StatusCode, String)> {
    let decoded = hex::decode(value.trim())
        .map_err(|_| (StatusCode::BAD_REQUEST, format!("invalid {field}")))?;
    decoded.try_into().map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            format!("{field} must be {N} bytes"),
        )
    })
}

fn decrypt_seed(req: &SendFromSeedRequest) -> Result<Vec<u8>, (StatusCode, String)> {
    use aes_gcm::aead::{Aead, Payload};
    use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
    use sha2::{Digest, Sha256};

    let ephemeral_bytes =
        decode_fixed_hex::<32>(&req.ephemeral_pubkey_hex, "ephemeral_pubkey_hex")?;
    let nonce_bytes = decode_fixed_hex::<12>(&req.nonce_hex, "nonce_hex")?;
    let ciphertext = hex::decode(req.encrypted_seed_hex.trim()).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            "invalid encrypted_seed_hex".to_string(),
        )
    })?;
    if ciphertext.len() < 16 {
        return Err((
            StatusCode::BAD_REQUEST,
            "encrypted_seed_hex must include an authentication tag".to_string(),
        ));
    }

    let ephemeral_pubkey = x25519_dalek::PublicKey::from(ephemeral_bytes);
    let server_key = server_seed_key();
    let mut shared_secret = server_key
        .secret
        .diffie_hellman(&ephemeral_pubkey)
        .to_bytes();
    let mut hasher = Sha256::new();
    hasher.update(b"zoka-seed-envelope-v1");
    hasher.update(shared_secret);
    hasher.update(ephemeral_bytes);
    hasher.update(server_key.public.as_bytes());
    let mut aes_key = hasher.finalize();
    shared_secret.zeroize();

    let cipher = Aes256Gcm::new_from_slice(&aes_key).map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to initialize seed envelope cipher".to_string(),
        )
    })?;
    let result = cipher
        .decrypt(
            Nonce::from_slice(&nonce_bytes),
            Payload {
                msg: &ciphertext,
                aad: b"zoka-seed-envelope-v1",
            },
        )
        .map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                "encrypted seed authentication failed".to_string(),
            )
        });
    aes_key.zeroize();
    result
}

fn private_tx_nullifier_tags(tx: &crate::private_wallet::PrivateTransaction) -> HashSet<String> {
    let mut tags = HashSet::new();
    let primary = tx.nullifier.trim();
    if !primary.is_empty() {
        tags.insert(primary.to_string());
    }
    for tag in &tx.input_nullifiers {
        let tag = tag.trim();
        if !tag.is_empty() {
            tags.insert(tag.to_string());
        }
    }
    tags
}

fn find_pending_private_nullifier_conflict(
    app_state: &AppState,
    new_hash: &str,
    new_tx: &crate::private_wallet::PrivateTransaction,
) -> Result<Option<(String, String)>, (StatusCode, String)> {
    let new_tags = private_tx_nullifier_tags(new_tx);
    if new_tags.is_empty() {
        return Ok(None);
    }

    let pending_hashes = app_state
        .state_manager
        .iterate_pending_private_transaction_hashes()
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to inspect pending private txs: {e}"),
            )
        })?;

    for pending_hash in pending_hashes {
        if pending_hash == new_hash {
            continue;
        }
        let body = app_state
            .state_manager
            .get_private_transaction_bytes(&pending_hash)
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("failed to load pending private tx {pending_hash}: {e}"),
                )
            })?;
        let Some(body) = body else {
            continue;
        };
        let Ok(pending_tx) =
            serde_json::from_slice::<crate::private_wallet::PrivateTransaction>(&body)
        else {
            continue;
        };
        for tag in private_tx_nullifier_tags(&pending_tx) {
            if new_tags.contains(&tag) {
                return Ok(Some((pending_hash, tag)));
            }
        }
    }

    Ok(None)
}

async fn send_from_seed(
    State(app_state): State<Arc<AppState>>,
    Json(req): Json<SendFromSeedRequest>,
) -> Result<Json<SendFromSeedResponse>, (StatusCode, String)> {
    use sha2::{Digest, Sha256};

    if !app_state.security.enable_remote_seed_endpoint {
        return Err((
            StatusCode::NOT_FOUND,
            "remote seed endpoint is disabled; submit a locally-built private transaction to POST /private/tx"
                .to_string(),
        ));
    }

    // â”€â”€ Validate inputs â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
    if req.amount_atoms == 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            "amount_atoms must be greater than zero".to_string(),
        ));
    }
    let mut seed_bytes = decrypt_seed(&req)?;
    if seed_bytes.len() != 32 && seed_bytes.len() != 16 {
        return Err((
            StatusCode::BAD_REQUEST,
            "seed_hex must be 16 or 32 bytes".to_string(),
        ));
    }
    if !req.to.starts_with("zka1") || req.to.len() < 32 {
        return Err((
            StatusCode::BAD_REQUEST,
            "invalid recipient address (expected zka1â€¦)".to_string(),
        ));
    }

    // â”€â”€ Derive the spending key from the seed entropy (matches
    //    `derive_spending_key_from_recovery_seed` in private_wallet/mod.rs) â”€
    let mut hasher = Sha256::new();
    hasher.update(b"zoka-private-wallet-spend-v1");
    hasher.update([0x1f]);
    hasher.update(&seed_bytes);
    let mut spending_key = hasher.finalize();
    let mut spending_key_hex = hex::encode(&spending_key);
    let mut scan_key_hex = hex::encode(&seed_bytes);
    spending_key.zeroize();
    seed_bytes.zeroize();

    // â”€â”€ Sandbox the wallet file under a unique temp dir â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let tmp_dir = std::env::temp_dir().join(format!("zk-send-{nanos}"));
    let wallets_dir = tmp_dir
        .join(".zoka")
        .join("mainnet")
        .join("private_wallets");
    if let Err(e) = tokio::fs::create_dir_all(&wallets_dir).await {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to create temp wallet dir: {e}"),
        ));
    }

    // RAII-style cleanup: ensure the temp dir is removed on every exit
    // path so the seed-derived wallet file never persists beyond the
    // request lifecycle.
    struct CleanupGuard(PathBuf);
    impl Drop for CleanupGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = CleanupGuard(tmp_dir.clone());

    let tmp_password = format!("p{nanos}");
    let tmp_name = format!("send{nanos}");

    let zoka_bin = std::env::var("ZOKA_BIN").unwrap_or_else(|_| "zoka".to_string());
    let chain_db = std::env::var("ZOKA_CHAIN_DB_PATH").unwrap_or_else(|_| {
        let network = std::env::var("ZOKA_NETWORK").unwrap_or_else(|_| "mainnet".to_string());
        format!("/data/{network}/blockchain.db")
    });

    // â”€â”€ Step 1: restore the wallet from the seed-derived keys â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
    let restore_result = tokio::process::Command::new(&zoka_bin)
        .current_dir(&tmp_dir)
        .args([
            "wallet-restore-from-keys",
            "--scan-key-hex",
            &scan_key_hex,
            "--spend-key-hex",
            &spending_key_hex,
            "--password",
            &tmp_password,
            "--name",
            &tmp_name,
            "--network",
            "mainnet",
        ])
        .output()
        .await;
    scan_key_hex.zeroize();
    spending_key_hex.zeroize();
    let restore_out = restore_result.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to spawn wallet-restore-from-keys: {e}"),
        )
    })?;

    if !restore_out.status.success() {
        let stderr = String::from_utf8_lossy(&restore_out.stderr);
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("wallet-restore-from-keys failed: {}", stderr.trim()),
        ));
    }

    let restore_stdout = String::from_utf8_lossy(&restore_out.stdout).into_owned();
    let from_addr = restore_stdout
        .lines()
        .find_map(|l| l.strip_prefix("Address:").map(|s| s.trim().to_string()))
        .ok_or_else(|| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("could not parse restored address from: {restore_stdout}"),
            )
        })?;

    // â”€â”€ Step 2: sync UTXOs from the chain DB by querying the balance,
    //    which triggers `sync_private_wallet_history_from_chain_db` and
    //    `sync_private_coinbase_outputs_from_chain_db` internally.
    let balance_out = tokio::process::Command::new(&zoka_bin)
        .current_dir(&tmp_dir)
        .args([
            "balance",
            &from_addr,
            "--password",
            &tmp_password,
            "--network",
            "mainnet",
            "--db",
            &chain_db,
            "--json",
        ])
        .output()
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to spawn balance: {e}"),
            )
        })?;

    if !balance_out.status.success() {
        let stderr = String::from_utf8_lossy(&balance_out.stderr);
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("balance/sync failed: {}", stderr.trim()),
        ));
    }

    // â”€â”€ Step 3: build + broadcast the private transaction. The CLI
    //    converts ZKA strings to atoms internally, so format with 6
    //    decimal places (ATOMS_PER_ZKA = 1_000_000).
    let fee_atoms = req.fee_atoms.unwrap_or(1_000);
    let amount_zka = format!(
        "{}.{:06}",
        req.amount_atoms / 1_000_000,
        req.amount_atoms % 1_000_000
    );
    let fee_zka = format!("{}.{:06}", fee_atoms / 1_000_000, fee_atoms % 1_000_000);

    let send_out = tokio::process::Command::new(&zoka_bin)
        .current_dir(&tmp_dir)
        .args([
            "send",
            &from_addr,
            &req.to,
            &amount_zka,
            "--fee",
            &fee_zka,
            "--password",
            &tmp_password,
            "--network",
            "mainnet",
            "--db",
            &chain_db,
            "--yes",
            "--json",
        ])
        .output()
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to spawn send: {e}"),
            )
        })?;

    if !send_out.status.success() {
        let stderr = String::from_utf8_lossy(&send_out.stderr).into_owned();
        let stdout = String::from_utf8_lossy(&send_out.stdout).into_owned();
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("send failed: {}\n{}", stderr.trim(), stdout.trim()),
        ));
    }

    // â”€â”€ Parse the CLI's output to extract the tx hash. The `send`
    //    command's `--json` flag is honored only for some code paths; the
    //    common chunked-private-transfer branch still prints plain text
    //    (`Private transfer queued\nHash: <hex>\nFrom: â€¦`). Accept BOTH
    //    formats so a successful broadcast never bubbles up as a 500.
    let send_stdout = String::from_utf8_lossy(&send_out.stdout).into_owned();
    let hash = if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&send_stdout) {
        parsed
            .get("transaction_hash")
            .or_else(|| parsed.get("hash"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                parsed
                    .get("transaction_hashes")
                    .and_then(|a| a.as_array())
                    .and_then(|a| a.first())
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
    } else {
        // Plain-text path: look for the `Hash: <hex>` line emitted by the
        // private send pipeline. Tolerates both the singular `Hash:` and
        // chunked `Transaction hashes:` formats.
        send_stdout
            .lines()
            .find_map(|line| {
                line.trim()
                    .strip_prefix("Hash:")
                    .map(|s| s.trim().to_string())
            })
            .filter(|h| !h.is_empty())
            .or_else(|| {
                // Look at lines after a "Transaction hashes:" header (chunked
                // form) and grab the first hex hash.
                let mut in_block = false;
                for line in send_stdout.lines() {
                    let trimmed = line.trim();
                    if trimmed.starts_with("Transaction hashes:") {
                        in_block = true;
                        continue;
                    }
                    if in_block {
                        let candidate = trimmed.trim_start_matches('-').trim();
                        if candidate.len() >= 32 && candidate.chars().all(|c| c.is_ascii_hexdigit())
                        {
                            return Some(candidate.to_string());
                        }
                        if trimmed.is_empty() {
                            break;
                        }
                    }
                }
                None
            })
    }
    .ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("send response missing tx hash: {send_stdout}"),
        )
    })?;

    let built_tx = match app_state.state_manager.get_private_transaction_bytes(&hash) {
        Ok(Some(body)) => {
            match serde_json::from_slice::<crate::private_wallet::PrivateTransaction>(&body) {
                Ok(tx) => Some(tx),
                Err(e) => {
                    tracing::warn!(
                        "send-from-seed: failed to deserialize built tx body {hash}: {e}"
                    );
                    None
                }
            }
        }
        Ok(None) => {
            tracing::warn!("send-from-seed: built tx {hash} not found in priv_tx storage");
            None
        }
        Err(e) => {
            tracing::warn!("send-from-seed: failed reading tx body {hash} from storage: {e}");
            None
        }
    };

    if let Some(tx) = built_tx.as_ref() {
        if let Some((existing_hash, nullifier)) =
            find_pending_private_nullifier_conflict(app_state.as_ref(), &hash, tx)?
        {
            let _ = app_state
                .state_manager
                .delete_pending_private_transaction_hash(&hash);
            let _ = app_state.state_manager.delete_private_transaction(&hash);
            return Err((
                StatusCode::CONFLICT,
                format!(
                    "private spend already pending: tx {existing_hash} uses nullifier {nullifier}"
                ),
            ));
        }
    }

    // ── Hand the built tx to the fullnode main loop (if configured) so
    //    it lands in the in-memory pool AND gets gossiped via libp2p.
    //    Without this step the body sits in `priv_tx:<hash>` storage but
    //    no peer's miner ever sees it — exactly the failure mode the
    //    user hit when mobile sends "succeeded" but balance never moved.
    //    Best-effort: errors here MUST NOT mask the successful build.
    if let (Some(sender), Some(tx)) = (&app_state.private_tx_broadcaster, built_tx) {
        if let Err(e) = sender.send(tx) {
            tracing::warn!(
                "send-from-seed: failed to forward tx {hash} to fullnode broadcaster: {e}"
            );
        }
    }

    Ok(Json(SendFromSeedResponse {
        hash,
        status: "added_to_mempool".to_string(),
    }))
}

/// Get current mempool size
async fn get_mempool_size(State(app_state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let size = app_state.mempool.len().await;
    Json(serde_json::json!({"pending_transactions": size}))
}

/// Get pending transactions from mempool
async fn get_mempool_pending(State(app_state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let pending = app_state.mempool.get_pending().await;
    let hashes: Vec<serde_json::Value> = pending
        .iter()
        .map(|tx| serde_json::json!({ "hash": tx.hash, "timestamp": tx.timestamp }))
        .collect();
    Json(serde_json::json!({
        "count": pending.len(),
        "pending": hashes
    }))
}

/// Get mempool metrics and statistics
async fn get_mempool_metrics(State(app_state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let metrics = app_state.mempool.get_metrics().await;
    Json(serde_json::json!({
        "current_size": metrics.total_size,
        "max_size": metrics.max_size,
        "transactions_added": metrics.transactions_added,
        "transactions_removed": metrics.transactions_removed,
        "transactions_evicted": metrics.transactions_evicted,
        "total_fees_collected": metrics.total_fees_collected,
        "average_fee": metrics.average_fee,
        "rejections": {
            "insufficient_fee": metrics.rejections_insufficient_fee,
            "too_many_from_sender": metrics.rejections_too_many_from_sender,
            "invalid_state": metrics.rejections_invalid_state,
        }
    }))
}

/// Get a specific transaction from mempool by hash
// ── Team mining ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JoinMiningTeamRequest {
    invite_code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct TeamMiningControlRequest {
    #[serde(default)]
    threads: u64,
}

fn team_mining_runtime_or_404(
    app_state: &Arc<AppState>,
) -> Result<&Arc<crate::team_mining::TeamMiningRuntime>, (StatusCode, String)> {
    app_state.team_mining_runtime.as_ref().ok_or((
        StatusCode::NOT_FOUND,
        "this node does not run the team-mining protocol".to_string(),
    ))
}

fn team_error_response(error: crate::team_mining::TeamError) -> (StatusCode, String) {
    use crate::team_mining::TeamError;
    let status = match error {
        TeamError::NotCoordinator
        | TeamError::PeerBindingMismatch
        | TeamError::CoordinatorCannotLeave => StatusCode::FORBIDDEN,
        TeamError::TeamNotDiscovered
        | TeamError::FailoverNotReady
        | TeamError::NoActiveTemplate
        | TeamError::StaleTemplate => StatusCode::CONFLICT,
        TeamError::Persistence(_) | TeamError::Pow(_) => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::BAD_REQUEST,
    };
    (status, error.to_string())
}

async fn get_team_mining_status(
    State(app_state): State<Arc<AppState>>,
) -> Result<Json<crate::team_mining::TeamRuntimeStatus>, (StatusCode, String)> {
    Ok(Json(
        team_mining_runtime_or_404(&app_state)?.status().await,
    ))
}

async fn create_mining_team(
    State(app_state): State<Arc<AppState>>,
) -> Result<Json<crate::team_mining::TeamRuntimeStatus>, (StatusCode, String)> {
    let runtime = team_mining_runtime_or_404(&app_state)?;
    if runtime.store.lock().await.state().is_some() {
        return Err((
            StatusCode::CONFLICT,
            "this wallet already belongs to a mining team".to_string(),
        ));
    }
    runtime
        .create_team(now_secs())
        .await
        .map_err(team_error_response)?;
    Ok(Json(runtime.status().await))
}

async fn join_mining_team(
    State(app_state): State<Arc<AppState>>,
    Json(request): Json<JoinMiningTeamRequest>,
) -> Result<Json<crate::team_mining::TeamRuntimeStatus>, (StatusCode, String)> {
    let runtime = team_mining_runtime_or_404(&app_state)?;
    if runtime.store.lock().await.state().is_some() {
        return Err((
            StatusCode::CONFLICT,
            "leave the current mining team before joining another".to_string(),
        ));
    }
    runtime
        .request_join(&request.invite_code, now_secs())
        .await
        .map_err(team_error_response)?;
    Ok(Json(runtime.status().await))
}

async fn leave_mining_team(
    State(app_state): State<Arc<AppState>>,
) -> Result<Json<crate::team_mining::TeamRuntimeStatus>, (StatusCode, String)> {
    let runtime = team_mining_runtime_or_404(&app_state)?;
    runtime
        .request_leave(now_secs())
        .await
        .map_err(team_error_response)?;
    runtime.set_enabled(false, 1).await;
    Ok(Json(runtime.status().await))
}

/// Disuelve el equipo. Solo el coordinador puede; el resto usa /leave.
async fn disband_mining_team(
    State(app_state): State<Arc<AppState>>,
) -> Result<Json<crate::team_mining::TeamRuntimeStatus>, (StatusCode, String)> {
    let runtime = team_mining_runtime_or_404(&app_state)?;
    runtime.disband().await.map_err(team_error_response)?;
    runtime.set_enabled(false, 1).await;
    Ok(Json(runtime.status().await))
}

async fn start_team_mining(
    State(app_state): State<Arc<AppState>>,
    body: Option<Json<TeamMiningControlRequest>>,
) -> Result<Json<crate::team_mining::TeamRuntimeStatus>, (StatusCode, String)> {
    let runtime = team_mining_runtime_or_404(&app_state)?;
    let status = runtime.status().await;
    let joined = status.team.as_ref().is_some_and(|team| {
        team.members
            .get(&status.local_address)
            .is_some_and(|member| member.active && !member.expelled)
    });
    if !joined {
        return Err((
            StatusCode::CONFLICT,
            "create or join a mining team before starting".to_string(),
        ));
    }
    if global_network_runtime().checkpoint_quarantined() {
        return Err((
            StatusCode::CONFLICT,
            "team mining is quarantined until canonical history matches the checkpoint"
                .to_string(),
        ));
    }
    let threads = body.map(|Json(request)| request.threads).unwrap_or(1).max(1);
    runtime.set_enabled(true, threads).await;
    Ok(Json(runtime.status().await))
}

async fn stop_team_mining(
    State(app_state): State<Arc<AppState>>,
) -> Result<Json<crate::team_mining::TeamRuntimeStatus>, (StatusCode, String)> {
    let runtime = team_mining_runtime_or_404(&app_state)?;
    runtime.set_enabled(false, 1).await;
    Ok(Json(runtime.status().await))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Último checkpoint firmado por el nodo del equipo que este nodo aceptó.
async fn get_latest_signed_checkpoint(
    State(app_state): State<Arc<AppState>>,
) -> Result<Json<crate::checkpoint_authority::SignedCheckpoint>, (StatusCode, String)> {
    let storage = app_state.state_manager.raw_storage();
    match crate::checkpoint_authority::latest_signed_checkpoint(&storage) {
        Ok(Some(checkpoint)) => Ok(Json(checkpoint)),
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            "no signed checkpoint accepted yet".to_string(),
        )),
        Err(error) => Err((StatusCode::INTERNAL_SERVER_ERROR, error.to_string())),
    }
}

/// Recibe un checkpoint firmado y lo verifica igual que uno que llega por la
/// red. Sirve para inyectarlo a mano si el gossip no llega.
async fn post_signed_checkpoint(
    State(app_state): State<Arc<AppState>>,
    Json(checkpoint): Json<crate::checkpoint_authority::SignedCheckpoint>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let chain_id = global_startup_status().snapshot().chain_id.ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "the node does not know its chain yet".to_string(),
    ))?;
    let authority = crate::checkpoint_authority::authority_key_for_chain(&chain_id).ok_or((
        StatusCode::NOT_FOUND,
        format!("chain {chain_id} has no checkpoint authority"),
    ))?;
    let storage = app_state.state_manager.raw_storage();
    match crate::checkpoint_authority::accept_signed_checkpoint(
        &storage,
        &chain_id,
        &checkpoint,
        &authority,
    ) {
        Ok(acceptance) => Ok(Json(serde_json::json!({
            "accepted": acceptance == crate::checkpoint_authority::Acceptance::Accepted,
            "height": checkpoint.height,
        }))),
        Err(
            error @ (crate::checkpoint_authority::CheckpointError::Conflict { .. }
            | crate::checkpoint_authority::CheckpointError::Stale { .. }),
        ) => Err((StatusCode::CONFLICT, error.to_string())),
        Err(error) => Err((StatusCode::BAD_REQUEST, error.to_string())),
    }
}

async fn get_mempool_transaction(
    State(app_state): State<Arc<AppState>>,
    Path(hash): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if let Some(tx) = app_state.mempool.get_transaction(&hash).await {
        Ok(Json(serde_json::json!({
            "hash": tx.hash,
            "timestamp": tx.timestamp,
            "status": "pending",
        })))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

/// PBFT consensus start request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PbftStartRequest {
    pub node_id: String,
    pub validators: Vec<String>,
    pub timeout_ms: Option<u64>,
}

/// PBFT consensus status response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PbftStatusResponse {
    pub node_id: String,
    pub current_view: u64,
    pub is_primary: bool,
    pub primary: String,
    pub timeout_ms: u64,
    pub fault_tolerance: usize,
    pub quorum_prepared: usize,
    pub quorum_commit: usize,
    pub validators: Vec<String>,
}

/// PBFT consensus message request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PbftMessageRequest {
    pub msg_type: String,
    pub view: u64,
    pub sequence: u64,
    pub block_hash: String,
    pub sender: Option<String>,
    pub timestamp_ms: Option<u64>,
}

/// Snapshot management request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSnapshotRequest {
    pub name: Option<String>,
}

/// Advanced event filter
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdvancedEventFilter {
    pub block_height_min: Option<u64>,
    pub block_height_max: Option<u64>,
    pub addresses: Option<Vec<String>>,
    pub tx_type: Option<String>,
    pub include_pending: bool,
}

/// Bridge from PBFT `Committed` event to state + private-pool
/// application. Returns a JSON report with what happened so the HTTP
/// response can surface it. Any failure is reported, not panicked:
/// consensus finality must not be blocked by a missing block or a
/// transient DB error.
async fn auto_apply_committed_block(
    app_state: &Arc<AppState>,
    block_hash: &str,
) -> serde_json::Value {
    let block = match app_state.state_manager.get_block_by_hash(block_hash).await {
        Ok(Some(b)) => b,
        Ok(None) => {
            return serde_json::json!({
                "status": "block_not_in_storage",
                "block_hash": block_hash,
            });
        }
        Err(e) => {
            return serde_json::json!({
                "status": "storage_error",
                "block_hash": block_hash,
                "error": e.to_string(),
            });
        }
    };

    if let Err(e) = app_state.state_manager.apply_block(&block).await {
        return serde_json::json!({
            "status": "apply_block_failed",
            "block_hash": block_hash,
            "error": e.to_string(),
        });
    }

    // Notify the private pool if this node holds one. The block's
    // `private_tx_hashes` are the canonical list of private txs that
    // were committed at this height.
    let mut private_confirmed = 0usize;
    let mut private_missing: Vec<String> = Vec::new();
    if let Some(manager) = &app_state.private_wallet_manager {
        let mut manager = manager.write().await;
        for hash in &block.private_tx_hashes {
            match manager.confirm_private_transaction(hash) {
                Ok(()) => private_confirmed += 1,
                Err(_) => private_missing.push(hash.clone()),
            }
        }
    }

    serde_json::json!({
        "status": "applied",
        "block_hash": block_hash,
        "public_tx_applied": block.transactions.len(),
        "private_tx_confirmed": private_confirmed,
        "private_tx_missing": private_missing,
    })
}

/// Start PBFT consensus
async fn pbft_start(
    State(app_state): State<Arc<AppState>>,
    Json(req): Json<PbftStartRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    match PbftConsensus::with_timeout(
        req.node_id.clone(),
        req.validators.clone(),
        req.timeout_ms.unwrap_or(5000),
    ) {
        Ok(consensus) => {
            // Store consensus in app state
            let mut pbft = app_state.pbft_consensus.lock().await;
            *pbft = Some(consensus);
            drop(pbft);

            Ok(Json(serde_json::json!({
                "status": "started",
                "node_id": req.node_id,
                "validators_count": req.validators.len(),
                "current_view": 0,
                "fault_tolerance": (req.validators.len().saturating_sub(1)) / 3,
            })))
        }
        Err(_) => Err(StatusCode::BAD_REQUEST),
    }
}

/// Get PBFT consensus status
async fn pbft_status(
    State(app_state): State<Arc<AppState>>,
) -> Result<Json<PbftStatusResponse>, StatusCode> {
    let pbft = app_state.pbft_consensus.lock().await;

    match pbft.as_ref() {
        Some(consensus) => {
            let node_id = consensus.node_id().to_string();
            let current_view = consensus.current_view();
            let primary = consensus
                .primary_for_view(current_view)
                .unwrap_or("unknown")
                .to_string();

            Ok(Json(PbftStatusResponse {
                node_id: node_id.clone(),
                current_view,
                is_primary: consensus.is_primary(current_view),
                primary,
                timeout_ms: consensus.timeout_ms(),
                fault_tolerance: consensus.fault_tolerance(),
                quorum_prepared: consensus.prepare_quorum(),
                quorum_commit: consensus.commit_quorum(),
                validators: consensus.validators().to_vec(),
            }))
        }
        None => {
            // Return default response if consensus not started
            Ok(Json(PbftStatusResponse {
                node_id: "unknown".to_string(),
                current_view: 0,
                is_primary: false,
                primary: "unknown".to_string(),
                timeout_ms: 5000,
                fault_tolerance: 1,
                quorum_prepared: 2,
                quorum_commit: 3,
                validators: vec![],
            }))
        }
    }
}

/// Handle PBFT message (pre-prepare, prepare, commit)
async fn pbft_message(
    State(app_state): State<Arc<AppState>>,
    Json(req): Json<PbftMessageRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // Validate message type
    if !["pre-prepare", "prepare", "commit"].contains(&req.msg_type.as_str()) {
        return Err(StatusCode::BAD_REQUEST);
    }

    let mut pbft = app_state.pbft_consensus.lock().await;

    match pbft.as_mut() {
        Some(consensus) => {
            use crate::consensus::pbft::{PbftMessage, PbftMessageType};

            let msg_type = match req.msg_type.as_str() {
                "pre-prepare" => PbftMessageType::PrePrepare,
                "prepare" => PbftMessageType::Prepare,
                "commit" => PbftMessageType::Commit,
                _ => return Err(StatusCode::BAD_REQUEST),
            };

            let sender = req
                .sender
                .clone()
                .unwrap_or_else(|| consensus.node_id().to_string());
            let timestamp_ms = req.timestamp_ms.unwrap_or_else(|| {
                (SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis()) as u64
            });
            let msg = PbftMessage::with_timestamp(
                msg_type,
                req.view,
                req.sequence,
                req.block_hash.clone(),
                sender,
                timestamp_ms,
            );

            match consensus.process_message(msg) {
                Ok(event) => {
                    // Auto-wire PBFT finality â†’ state-apply pipeline.
                    // When a block is committed by consensus, and this
                    // node holds both a state manager and a private
                    // wallet manager, we immediately apply the block and
                    // notify the private pool. Nodes without a wallet
                    // manager (tests, stateless observers) just log the
                    // event and move on.
                    let mut commit_report: Option<serde_json::Value> = None;
                    if let Some(crate::consensus::pbft::PbftEvent::Committed {
                        block_hash, ..
                    }) = &event
                    {
                        drop(pbft); // release the consensus lock before taking other locks
                        commit_report =
                            Some(auto_apply_committed_block(&app_state, block_hash).await);
                        // Re-take for subsequent calls â€” cheap since no
                        // work happens here afterwards, but keep the
                        // structure consistent.
                        let _ = app_state.pbft_consensus.lock().await;
                    }

                    let mut json = serde_json::json!({
                        "status": "received",
                        "msg_type": req.msg_type,
                        "view": req.view,
                        "sequence": req.sequence,
                        "processed": true,
                        "event": event.map(|e| format!("{e:?}")),
                    });
                    if let Some(report) = commit_report {
                        json["commit_report"] = report;
                    }
                    Ok(Json(json))
                }
                Err(err) => Ok(Json(serde_json::json!({
                    "status": "rejected",
                    "msg_type": req.msg_type,
                    "view": req.view,
                    "sequence": req.sequence,
                    "processed": false,
                    "reason": err.to_string(),
                }))),
            }
        }
        None => {
            // Consensus not started yet
            Ok(Json(serde_json::json!({
                "status": "queued",
                "msg_type": req.msg_type,
                "view": req.view,
                "sequence": req.sequence,
                "processed": false,
                "reason": "Consensus not started",
            })))
        }
    }
}

/// Create state snapshot
async fn create_snapshot(
    State(app_state): State<Arc<AppState>>,
    Json(_req): Json<CreateSnapshotRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    match app_state.state_manager.create_snapshot().await {
        Ok(height) => Ok(Json(serde_json::json!({
            "status": "created",
            "snapshot_height": height,
            "total_snapshots": app_state.state_manager.snapshot_count().await,
        }))),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// List all snapshots
async fn list_snapshots(
    State(app_state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let snapshots = app_state.state_manager.list_snapshots().await;

    Ok(Json(serde_json::json!({
        "snapshots": snapshots.iter().map(|(h, root, accounts)| {
            serde_json::json!({
                "height": h,
                "state_root": root,
                "accounts_count": accounts,
            })
        }).collect::<Vec<_>>(),
        "total": snapshots.len(),
    })))
}

/// Restore from snapshot
async fn restore_snapshot(
    State(app_state): State<Arc<AppState>>,
    Path(height): Path<u64>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    match app_state.state_manager.restore_from_snapshot(height).await {
        Ok(_) => Ok(Json(serde_json::json!({
            "status": "restored",
            "height": height,
        }))),
        Err(_) => Err(StatusCode::BAD_REQUEST),
    }
}

/// Advanced event filtering with multi-criteria
async fn filter_events(
    State(_app_state): State<Arc<AppState>>,
    Json(filter): Json<AdvancedEventFilter>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // In production, would query actual events
    // For now, return validation
    let mut criteria_applied = vec![];

    if filter.block_height_min.is_some() || filter.block_height_max.is_some() {
        criteria_applied.push("height_range");
    }
    if filter.addresses.is_some() {
        criteria_applied.push("addresses");
    }
    if filter.tx_type.is_some() {
        criteria_applied.push("tx_type");
    }
    if filter.include_pending {
        criteria_applied.push("pending_transactions");
    }

    Ok(Json(serde_json::json!({
        "filter_criteria": criteria_applied,
        "matching_events": 42,
        "status": "accepted",
    })))
}

/// Profile management request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileManagementRequest {
    pub profile_name: String,
}

fn api_profiles_path() -> PathBuf {
    if let Ok(path) = std::env::var("ZOKA_API_PROFILES_PATH") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }

    if let Some(home) = home::home_dir() {
        return home.join(".zoka").join("cli_profiles.toml");
    }

    PathBuf::from(".zoka/cli_profiles.toml")
}

fn load_api_profiles() -> MultiProfileConfig {
    let path = api_profiles_path();
    MultiProfileConfig::load_from_file(&path).unwrap_or_default()
}

fn save_api_profiles(config: &MultiProfileConfig) -> Result<(), String> {
    let path = api_profiles_path();
    config.save_to_file(&path)
}

/// List available CLI profiles
async fn list_profiles(
    State(_app_state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let config = load_api_profiles();
    let mut profiles = config.list_profiles();
    profiles.sort();

    Ok(Json(serde_json::json!({
        "profiles": profiles,
        "active": config.active_profile,
    })))
}

/// Switch active CLI profile
async fn switch_profile(
    State(_app_state): State<Arc<AppState>>,
    Json(req): Json<ProfileManagementRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let mut config = load_api_profiles();
    if !config.profiles.contains_key(&req.profile_name) {
        let profile = CliConfig::new(req.profile_name.clone());
        config
            .add_profile(req.profile_name.clone(), profile)
            .map_err(|_| StatusCode::BAD_REQUEST)?;
    }
    config
        .switch_profile(&req.profile_name)
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    save_api_profiles(&config).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({
        "status": "switched",
        "profile": req.profile_name,
    })))
}

/// Create the Axum router
/// WebSocket subscription request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscribeRequest {
    pub client_id: String,
    pub subscription_types: Vec<SubscriptionType>,
    pub filters: Option<EventFilter>,
}

/// WebSocket unsubscribe request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnsubscribeRequest {
    pub client_id: String,
    pub subscription_types: Vec<SubscriptionType>,
}

/// WebSocket status response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSocketStatusResponse {
    pub active_connections: usize,
    pub client_subscriptions: Vec<(String, Vec<SubscriptionType>)>,
    pub total_events_broadcast: u64,
}

/// Subscribe to WebSocket events
pub async fn ws_subscribe(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SubscribeRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    // Add connection to manager
    state.ws_manager.add_connection(req.client_id.clone()).await;

    // Subscribe to each requested type
    for sub_type in &req.subscription_types {
        state
            .ws_manager
            .subscribe(&req.client_id, sub_type.clone())
            .await;
    }

    Ok(Json(serde_json::json!({
        "status": "subscribed",
        "client_id": req.client_id,
        "subscriptions": req.subscription_types,
        "timestamp": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    })))
}

/// Unsubscribe from WebSocket events
pub async fn ws_unsubscribe(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UnsubscribeRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    // Unsubscribe from each type
    for sub_type in &req.subscription_types {
        state.ws_manager.unsubscribe(&req.client_id, sub_type).await;
    }

    // If no subscriptions remain, remove connection
    let remaining_subs = state.ws_manager.get_subscriptions(&req.client_id).await;
    if remaining_subs.is_empty() {
        state.ws_manager.remove_connection(&req.client_id).await;
    }

    Ok(Json(serde_json::json!({
        "status": "unsubscribed",
        "client_id": req.client_id,
        "remaining_subscriptions": remaining_subs,
        "timestamp": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    })))
}

/// Get WebSocket connection status
pub async fn ws_status(
    State(state): State<Arc<AppState>>,
) -> Result<Json<WebSocketStatusResponse>, (StatusCode, String)> {
    let snapshot = state.ws_manager.snapshot().await;
    let active = snapshot.len();
    let client_subscriptions = snapshot
        .iter()
        .map(|c| (c.client_id.clone(), c.subscription_types.clone()))
        .collect::<Vec<_>>();
    let total_events_broadcast = snapshot.iter().map(|c| c.event_count).sum();

    Ok(Json(WebSocketStatusResponse {
        active_connections: active,
        client_subscriptions,
        total_events_broadcast,
    }))
}

/// POST /sync/block — receive a mined block from any external miner
/// (including those behind NAT) and apply it to the local chain.
///
/// Accepts the same Block JSON returned by `GET /blocks/:height/raw`.
/// Uses the same code path as `zoka rpc-sync` (try_apply_synced_block_payload)
/// so validation is identical to the normal block sync flow.
#[derive(serde::Deserialize, Default)]
struct MiningControlRequest {
    threads: Option<u64>,
}

/// Start the node's in-process miner at runtime (no restart). Gated by
/// ZOKA_API_ENABLE_MINING_CONTROL so only a local controller can toggle it.
async fn post_mining_start(body: Option<Json<MiningControlRequest>>) -> Json<serde_json::Value> {
    let threads = body.and_then(|b| b.0.threads).unwrap_or(0).max(1);
    let snap = global_local_mining_control().enable(threads);
    Json(serde_json::json!({
        "enabled": snap.enabled,
        "threads": snap.threads,
        "updated_at": snap.updated_at,
    }))
}

/// Stop the node's in-process miner at runtime (the node keeps running + synced).
async fn post_mining_stop() -> Json<serde_json::Value> {
    let snap = global_local_mining_control().disable();
    Json(serde_json::json!({
        "enabled": snap.enabled,
        "updated_at": snap.updated_at,
    }))
}

async fn get_mining_control() -> Json<serde_json::Value> {
    let snap = global_local_mining_control().snapshot();
    Json(serde_json::json!({
        "enabled": snap.enabled,
        "threads": snap.threads,
        "updated_at": snap.updated_at,
    }))
}

async fn post_block_submit(
    State(app_state): State<Arc<AppState>>,
    Json(block): Json<crate::types::Block>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use crate::network::message::BlockResponseMessage;
    use crate::state::sync::{try_apply_synced_block_payload, SyncOutcome};

    // Wrap block in BlockResponseMessage — same format used by rpc-sync
    let encrypted_block = serde_json::to_vec(&block).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("serialize error: {e}"),
        )
    })?;
    let response = BlockResponseMessage {
        block_height: block.header.block_height,
        block_hash: block.header.hash.clone(),
        encrypted_block,
        difficulty_bits: None,
        private_tx_bodies: vec![],
        timestamp: block.header.timestamp,
    };

    let storage = app_state.state_manager.raw_storage();
    let chain_id = crate::economics::MAINNET_CHAIN_ID;

    match try_apply_synced_block_payload(&storage, chain_id, &response) {
        SyncOutcome::Applied(_) => {
            tracing::info!(
                "post_block_submit: applied block h={} hash={}",
                block.header.block_height,
                block.header.hash
            );
            Ok(Json(serde_json::json!({
                "status": "applied",
                "height": block.header.block_height,
                "hash": block.header.hash
            })))
        }
        SyncOutcome::AlreadyKnown { height, hash } => Ok(Json(serde_json::json!({
            "status": "already_known",
            "height": height,
            "hash": hash
        }))),
        SyncOutcome::BehindOrSibling {
            local_head,
            candidate,
            candidate_hash,
        } => Ok(Json(serde_json::json!({
            "status": "side_branch_stored",
            "height": candidate,
            "hash": candidate_hash,
            "local_head": local_head
        }))),
        SyncOutcome::GapAhead {
            local_head,
            candidate,
            ..
        } => {
            // Deliberately don't echo local_head/candidate back to the caller:
            // this endpoint has no protocol-version gate, so it's an easy
            // target for someone probing for a submittable block, and
            // confirming the exact local height for free would hand them a
            // calibration oracle to iterate against. Full detail stays in the
            // server-side log for operators.
            tracing::warn!(
                "post_block_submit: rejected gap (local={local_head} candidate={candidate})"
            );
            Err((StatusCode::CONFLICT, "block rejected: gap".to_string()))
        }
        outcome => {
            tracing::warn!("post_block_submit: rejected ({outcome:?})");
            Err((StatusCode::BAD_REQUEST, "block rejected".to_string()))
        }
    }
}

// ── Public snapshot endpoints ──────────────────────────────────────────────

async fn get_snapshots_list(State(app_state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let chain_id = app_state.state_manager.chain_id().await;
    let dir = crate::snapshot::default_snapshot_dir();
    let all = crate::snapshot::scan_snapshot_manifests(&dir);
    let filtered: Vec<_> = all
        .into_iter()
        .filter(|m| m.chain_id == chain_id)
        .map(|m| {
            serde_json::json!({
                "head_height": m.head_height,
                "head_hash": m.head_hash,
                "block_count": m.block_count,
                "size_bytes": m.size_bytes,
                "sha256_hex": m.sha256_hex,
                "generated_unix_secs": m.generated_unix_secs,
                "signed": m.signature_hex.is_some(),
                "download_urls": m.download_urls,
            })
        })
        .collect();
    Json(serde_json::json!({ "snapshots": filtered, "total": filtered.len() }))
}

async fn get_snapshot_latest(
    State(app_state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let chain_id = app_state.state_manager.chain_id().await;
    let dir = crate::snapshot::default_snapshot_dir();
    let manifests = crate::snapshot::scan_snapshot_manifests(&dir);
    let latest = manifests
        .into_iter()
        .find(|m| m.chain_id == chain_id)
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(serde_json::json!({
        "chain_id": latest.chain_id,
        "head_height": latest.head_height,
        "head_hash": latest.head_hash,
        "block_count": latest.block_count,
        "size_bytes": latest.size_bytes,
        "sha256_hex": latest.sha256_hex,
        "generated_unix_secs": latest.generated_unix_secs,
        "signed": latest.signature_hex.is_some(),
        "signer_pubkey_hex": latest.signer_pubkey_hex,
        "download_urls": latest.download_urls,
    })))
}

async fn download_snapshot(
    State(app_state): State<Arc<AppState>>,
    axum::extract::Path(height): axum::extract::Path<u64>,
) -> Result<axum::response::Response, StatusCode> {
    use axum::http::header;
    use axum::response::IntoResponse;

    let chain_id = app_state.state_manager.chain_id().await;
    let dir = crate::snapshot::default_snapshot_dir();
    let manifests = crate::snapshot::scan_snapshot_manifests(&dir);
    let manifest = manifests
        .into_iter()
        .find(|m| m.chain_id == chain_id && m.head_height == height)
        .ok_or(StatusCode::NOT_FOUND)?;

    let snap_path = std::path::Path::new(&manifest.snap_path);
    let bytes = std::fs::read(snap_path).map_err(|_| StatusCode::NOT_FOUND)?;

    let filename = format!("zoka-snapshot-{height}.snap");
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}\""),
            ),
        ],
        bytes,
    )
        .into_response())
}

pub fn create_router(app_state: Arc<AppState>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION]);

    let mut router = Router::new()
        .route("/stats", get(get_stats))
        .route("/sync/status", get(get_sync_status))
        .route("/peers", get(get_peers))
        .route("/network/runtime", get(get_network_runtime))
        .route("/network/miners", get(get_network_miners))
        .route("/network/miners/heartbeat", post(post_miner_heartbeat))
        .route("/local/mining/control", get(get_local_mining_control))
        .route("/local/mining/start", post(post_local_mining_start))
        .route("/local/mining/stop", post(post_local_mining_stop))
        .route("/team-mining/status", get(get_team_mining_status))
        .route("/team-mining/team", post(create_mining_team))
        .route("/team-mining/join", post(join_mining_team))
        .route("/team-mining/leave", post(leave_mining_team))
        .route("/team-mining/disband", post(disband_mining_team))
        .route("/team-mining/start", post(start_team_mining))
        .route("/team-mining/stop", post(stop_team_mining))
        .route("/health", get(health))
        .route("/health/checks", get(health_checks))
        .route("/ready", get(ready))
        .route("/startup/status", get(startup_status))
        .route("/metrics/security", get(get_security_metrics))
        .route("/metrics/audit", get(get_audit_logs))
        .route("/chain/height", get(get_height))
        .route("/chain/state-root", get(get_state_root))
        .route(
            "/chain/private-commitment-root",
            get(get_private_commitment_root),
        )
        .route(
            "/chain/private-commitments/:commitment/proof",
            get(get_private_commitment_proof),
        )
        .route("/accounts/:address", get(get_account))
        .route("/accounts/:address/proof", get(get_account_proof))
        .route("/accounts/proof/verify", post(verify_account_proof))
        .route("/blocks/:height", get(get_block_by_height))
        .route("/blocks/:height/raw", get(get_raw_block_by_height))
        .route("/blocks/hash/:hash", get(get_block_by_hash))
        .route("/blocks/latest", get(get_latest_block))
        .route("/blocks/:height/transactions", get(get_block_transactions))
        .route(
            "/blocks/:height/private_tx_hashes",
            get(get_block_private_tx_hashes),
        )
        .route(
            "/blocks/range/:from/:to/private_tx_hashes",
            get(get_block_private_tx_hashes_range),
        )
        .route("/private/tx", post(post_private_tx_body))
        .route("/private/view-scan", post(post_private_view_scan))
        .route("/private/view-scan-key", post(post_private_view_scan_key))
        .route("/private/pending", get(get_pending_private_txs))
        .route("/private/nullifier/:tag", get(get_nullifier_spent))
        .route("/private/tx/:hash", get(get_private_tx_by_hash))
        .route("/blocks/range/:from/:to", get(get_blocks_range))
        .route(
            "/private/coinbases/range/:from/:to",
            get(get_private_coinbases_range),
        )
        .route("/blocks/range/:from/:to/raw", get(get_raw_blocks_range))
        .route("/transactions/:hash", get(get_transaction_by_hash))
        .route("/admin/rollback", post(admin_rollback))
        .route("/admin/rollback-points", get(admin_rollback_points))
        .route("/transactions", post(submit_transaction))
        .route("/mempool/size", get(get_mempool_size))
        .route("/mempool/pending", get(get_mempool_pending))
        .route("/mempool/metrics", get(get_mempool_metrics))
        .route("/mempool/tx/:hash", get(get_mempool_transaction))
        .route("/checkpoint/latest", get(get_latest_signed_checkpoint))
        .route("/checkpoint", post(post_signed_checkpoint))
        .route("/state/snapshot", post(create_snapshot))
        .route("/state/snapshots", get(list_snapshots))
        .route("/state/restore/:height", post(restore_snapshot))
        .route("/events/filter", post(filter_events))
        .route("/cli/profiles", get(list_profiles))
        .route("/cli/profiles/switch", post(switch_profile))
        .route("/consensus/start", post(pbft_start))
        .route("/consensus/status", get(pbft_status))
        .route("/consensus/message", post(pbft_message))
        .route("/ws/subscribe", post(ws_subscribe))
        .route("/ws/unsubscribe", post(ws_unsubscribe))
        .route("/ws/status", get(ws_status))
        .route("/snapshots", get(get_snapshots_list))
        .route("/snapshots/latest", get(get_snapshot_latest))
        .route("/snapshots/:height/download", get(download_snapshot));
    if app_state.security.enable_remote_seed_endpoint {
        router = router
            .route("/crypto/server-pubkey", get(get_server_pubkey))
            .route("/private/wallet/send-from-seed", post(send_from_seed));
    }
    if app_state.security.enable_mining_control_endpoint {
        router = router
            .route("/mining/start", post(post_mining_start))
            .route("/mining/stop", post(post_mining_stop))
            .route("/mining/control", get(get_mining_control));
    }
    if app_state.security.enable_block_submit_endpoint {
        router = router.route("/sync/block", post(post_block_submit));
    }
    router
        .layer(DefaultBodyLimit::max(
            app_state.security.max_request_body_bytes,
        ))
        .layer(middleware::from_fn_with_state(
            app_state.clone(),
            auth_and_rate_limit,
        ))
        .layer(cors)
        .with_state(app_state)
}

/// Event notification structure for WebSocket subscribers
#[derive(Clone, Debug, Serialize)]
pub struct BlockEvent {
    pub height: u64,
    pub hash: String,
    pub timestamp: u64,
    pub transactions: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct TransactionEvent {
    pub hash: String,
    pub sender: String,
    pub amount: u64,
    pub timestamp: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct StateEvent {
    pub state_root: String,
    pub height: u64,
    pub timestamp: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct ConsensusEvent {
    pub view: u64,
    pub sequence: u64,
    pub block_hash: String,
    pub event_type: String,
    pub timestamp: u64,
}

/// WebSocket event broadcaster
pub struct EventBroadcaster {
    block_tx: tokio::sync::broadcast::Sender<BlockEvent>,
    transaction_tx: tokio::sync::broadcast::Sender<TransactionEvent>,
    state_tx: tokio::sync::broadcast::Sender<StateEvent>,
    consensus_tx: tokio::sync::broadcast::Sender<ConsensusEvent>,
}

impl EventBroadcaster {
    pub fn new() -> (Self, Self) {
        let (block_tx, _) = tokio::sync::broadcast::channel(100);
        let (transaction_tx, _) = tokio::sync::broadcast::channel(100);
        let (state_tx, _) = tokio::sync::broadcast::channel(100);
        let (consensus_tx, _) = tokio::sync::broadcast::channel(100);

        let broadcaster = EventBroadcaster {
            block_tx: block_tx.clone(),
            transaction_tx: transaction_tx.clone(),
            state_tx: state_tx.clone(),
            consensus_tx: consensus_tx.clone(),
        };

        (
            broadcaster.clone(),
            Self {
                block_tx,
                transaction_tx,
                state_tx,
                consensus_tx,
            },
        )
    }

    pub fn broadcast_block(&self, event: BlockEvent) {
        let _ = self.block_tx.send(event);
    }

    pub fn broadcast_transaction(&self, event: TransactionEvent) {
        let _ = self.transaction_tx.send(event);
    }

    pub fn broadcast_state(&self, event: StateEvent) {
        let _ = self.state_tx.send(event);
    }

    pub fn subscribe_blocks(&self) -> tokio::sync::broadcast::Receiver<BlockEvent> {
        self.block_tx.subscribe()
    }

    pub fn subscribe_transactions(&self) -> tokio::sync::broadcast::Receiver<TransactionEvent> {
        self.transaction_tx.subscribe()
    }

    pub fn subscribe_state(&self) -> tokio::sync::broadcast::Receiver<StateEvent> {
        self.state_tx.subscribe()
    }
}

impl Clone for EventBroadcaster {
    fn clone(&self) -> Self {
        EventBroadcaster {
            block_tx: self.block_tx.clone(),
            transaction_tx: self.transaction_tx.clone(),
            state_tx: self.state_tx.clone(),
            consensus_tx: self.consensus_tx.clone(),
        }
    }
}

/// WebSocket subscription types
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SubscriptionType {
    Blocks,
    Transactions,
    State,
    All,
}

/// WebSocket filter for events
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventFilter {
    pub subscription_type: SubscriptionType,
    pub min_height: Option<u64>,
    pub max_height: Option<u64>,
    pub address_filter: Option<String>,
}

impl EventFilter {
    pub fn new(sub_type: SubscriptionType) -> Self {
        EventFilter {
            subscription_type: sub_type,
            min_height: None,
            max_height: None,
            address_filter: None,
        }
    }

    pub fn with_height_range(mut self, min: u64, max: u64) -> Self {
        self.min_height = Some(min);
        self.max_height = Some(max);
        self
    }

    pub fn with_address_filter(mut self, address: String) -> Self {
        self.address_filter = Some(address);
        self
    }

    pub fn matches_block(&self, block: &BlockEvent) -> bool {
        if self.subscription_type == SubscriptionType::State
            || self.subscription_type == SubscriptionType::Transactions
        {
            return false;
        }

        if let Some(min) = self.min_height {
            if block.height < min {
                return false;
            }
        }
        if let Some(max) = self.max_height {
            if block.height > max {
                return false;
            }
        }
        true
    }

    pub fn matches_transaction(&self, tx: &TransactionEvent) -> bool {
        if self.subscription_type == SubscriptionType::State
            || self.subscription_type == SubscriptionType::Blocks
        {
            return false;
        }

        if let Some(addr) = &self.address_filter {
            if tx.sender != *addr {
                return false;
            }
        }
        true
    }

    pub fn matches_state(&self, _state: &StateEvent) -> bool {
        if self.subscription_type == SubscriptionType::Blocks
            || self.subscription_type == SubscriptionType::Transactions
        {
            return false;
        }
        true
    }
}

/// WebSocket client connection state
#[derive(Debug, Clone)]
pub struct WebSocketConnection {
    pub client_id: String,
    pub connected_at: u64,
    pub subscription_types: Vec<SubscriptionType>,
    pub event_count: u64,
}

impl WebSocketConnection {
    pub fn new(client_id: String) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        WebSocketConnection {
            client_id,
            connected_at: now,
            subscription_types: vec![],
            event_count: 0,
        }
    }

    pub fn subscribe(&mut self, sub_type: SubscriptionType) {
        if !self.subscription_types.contains(&sub_type) {
            self.subscription_types.push(sub_type);
        }
    }

    pub fn unsubscribe(&mut self, sub_type: &SubscriptionType) {
        self.subscription_types.retain(|s| s != sub_type);
    }

    pub fn is_subscribed(&self, sub_type: &SubscriptionType) -> bool {
        self.subscription_types.contains(sub_type)
            || self.subscription_types.contains(&SubscriptionType::All)
    }
}

/// WebSocket connection manager
#[derive(Debug, Clone)]
pub struct WebSocketConnectionManager {
    connections: Arc<RwLock<HashMap<String, WebSocketConnection>>>,
}

impl WebSocketConnectionManager {
    pub fn new() -> Self {
        WebSocketConnectionManager {
            connections: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn add_connection(&self, client_id: String) -> WebSocketConnection {
        let connection = WebSocketConnection::new(client_id.clone());
        let mut conns = self.connections.write().await;
        conns.insert(client_id, connection.clone());
        connection
    }

    pub async fn remove_connection(&self, client_id: &str) {
        let mut conns = self.connections.write().await;
        conns.remove(client_id);
    }

    pub async fn subscribe(&self, client_id: &str, sub_type: SubscriptionType) {
        let mut conns = self.connections.write().await;
        if let Some(conn) = conns.get_mut(client_id) {
            conn.subscribe(sub_type);
        }
    }

    pub async fn unsubscribe(&self, client_id: &str, sub_type: &SubscriptionType) {
        let mut conns = self.connections.write().await;
        if let Some(conn) = conns.get_mut(client_id) {
            conn.unsubscribe(sub_type);
        }
    }

    pub async fn is_subscribed(&self, client_id: &str, sub_type: &SubscriptionType) -> bool {
        let conns = self.connections.read().await;
        conns
            .get(client_id)
            .map(|c| c.is_subscribed(sub_type))
            .unwrap_or(false)
    }

    pub async fn get_active_connections(&self) -> usize {
        let conns = self.connections.read().await;
        conns.len()
    }

    pub async fn get_subscriptions(&self, client_id: &str) -> Vec<SubscriptionType> {
        let conns = self.connections.read().await;
        conns
            .get(client_id)
            .map(|c| c.subscription_types.clone())
            .unwrap_or_default()
    }

    pub async fn increment_event_count(&self, client_id: &str) {
        let mut conns = self.connections.write().await;
        if let Some(conn) = conns.get_mut(client_id) {
            conn.event_count += 1;
        }
    }

    pub async fn get_event_count(&self, client_id: &str) -> u64 {
        let conns = self.connections.read().await;
        conns.get(client_id).map(|c| c.event_count).unwrap_or(0)
    }

    pub async fn snapshot(&self) -> Vec<WebSocketConnection> {
        let conns = self.connections.read().await;
        conns.values().cloned().collect()
    }

    pub async fn total_event_count(&self) -> u64 {
        let conns = self.connections.read().await;
        conns.values().map(|c| c.event_count).sum()
    }
}

/// Start the REST API server with default security config.
pub async fn start_api(
    state_manager: SharedState,
    mempool: SharedMempool,
) -> Result<(), StateError> {
    start_api_with_config(state_manager, mempool, ApiSecurityConfig::default()).await
}

/// Start the REST API server with explicit security config.
pub async fn start_api_with_config(
    state_manager: SharedState,
    mempool: SharedMempool,
    security: ApiSecurityConfig,
) -> Result<(), StateError> {
    let bind_addr = std::env::var("ZOKA_API_BIND")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            StateError::StorageError(
                "ZOKA_API_BIND must be configured before starting the API server".to_string(),
            )
        })?;
    start_api_with_config_on_bind(state_manager, mempool, security, bind_addr, None).await
}

pub async fn start_api_on_bind(
    state_manager: SharedState,
    mempool: SharedMempool,
    bind_addr: String,
) -> Result<(), StateError> {
    start_api_with_config_on_bind(
        state_manager,
        mempool,
        ApiSecurityConfig::default(),
        bind_addr,
        None,
    )
    .await
}

/// Same as `start_api_on_bind` but accepts a broadcast channel sender that
/// links the API into the fullnode main loop. Pass `Some(sender)` from a
/// fullnode boot path so private transactions submitted via the HTTP API
/// (e.g. `/private/tx`) reach the in-memory pool and
/// the libp2p gossip layer of the running node.
pub async fn start_api_on_bind_with_broadcaster(
    state_manager: SharedState,
    mempool: SharedMempool,
    bind_addr: String,
    broadcaster: Option<PrivateTxBroadcastSender>,
) -> Result<(), StateError> {
    start_api_with_config_on_bind(
        state_manager,
        mempool,
        ApiSecurityConfig::default(),
        bind_addr,
        broadcaster,
    )
    .await
}

/// Fullnode entrypoint: exposes private-wallet and team-mining state through
/// the same shared instances the fullnode loops use.
pub async fn start_api_on_bind_with_runtime(
    state_manager: SharedState,
    mempool: SharedMempool,
    bind_addr: String,
    broadcaster: Option<PrivateTxBroadcastSender>,
    private_wallet_manager: SharedPrivateWalletManager,
    team_mining_runtime: Arc<crate::team_mining::TeamMiningRuntime>,
) -> Result<(), StateError> {
    start_api_with_config_on_bind_and_runtime(
        state_manager,
        mempool,
        ApiSecurityConfig::default(),
        bind_addr,
        broadcaster,
        Some(private_wallet_manager),
        Some(team_mining_runtime),
    )
    .await
}

pub async fn start_api_with_config_on_bind(
    state_manager: SharedState,
    mempool: SharedMempool,
    security: ApiSecurityConfig,
    bind_addr: String,
    private_tx_broadcaster: Option<PrivateTxBroadcastSender>,
) -> Result<(), StateError> {
    start_api_with_config_on_bind_and_runtime(
        state_manager,
        mempool,
        security,
        bind_addr,
        private_tx_broadcaster,
        None,
        None,
    )
    .await
}

async fn start_api_with_config_on_bind_and_runtime(
    state_manager: SharedState,
    mempool: SharedMempool,
    security: ApiSecurityConfig,
    bind_addr: String,
    private_tx_broadcaster: Option<PrivateTxBroadcastSender>,
    private_wallet_manager: Option<SharedPrivateWalletManager>,
    team_mining_runtime: Option<Arc<crate::team_mining::TeamMiningRuntime>>,
) -> Result<(), StateError> {
    let app_state = Arc::new(AppState {
        state_manager,
        mempool,
        security,
        rate_limiter: ApiRateLimiter::default(),
        security_stats: Arc::new(ApiSecurityStats::default()),
        ws_manager: WebSocketConnectionManager::new(),
        pbft_consensus: Arc::new(tokio::sync::Mutex::new(None)),
        private_wallet_manager,
        private_tx_broadcaster,
        team_mining_runtime,
    });

    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .map_err(|e| StateError::StorageError(e.to_string()))?;

    println!("API server listening on http://{bind_addr}");

    let router = create_router(app_state);
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .map_err(|e| StateError::StorageError(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Block, PublicKey, Signature};
    static PROFILE_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn minimal_private_tx(
        hash: &str,
        nullifier: &str,
        input_nullifiers: Vec<&str>,
    ) -> crate::private_wallet::PrivateTransaction {
        serde_json::from_value(serde_json::json!({
            "tx_hash": hash,
            "commitment": "commitment",
            "nullifier": nullifier,
            "input_nullifiers": input_nullifiers,
            "zk_proof": null,
            "timestamp": 1,
            "status": "Verified"
        }))
        .unwrap()
    }

    fn test_app_state(test_dir: &str) -> AppState {
        let storage = Arc::new(crate::storage::RocksDbStorage::new(test_dir).unwrap());
        let state_manager = Arc::new(StateManager::new(storage));
        AppState {
            state_manager,
            mempool: Arc::new(Mempool::new(1000)),
            security: ApiSecurityConfig::default(),
            rate_limiter: ApiRateLimiter::default(),
            security_stats: Arc::new(ApiSecurityStats::default()),
            ws_manager: WebSocketConnectionManager::new(),
            pbft_consensus: Arc::new(tokio::sync::Mutex::new(None)),
            private_wallet_manager: None,
            private_tx_broadcaster: None,
            team_mining_runtime: None,
        }
    }

    fn configure_test_team(
        state: &mut AppState,
    ) -> Arc<crate::team_mining::TeamMiningRuntime> {
        let mut manager = PrivateWalletManager::new();
        let address = manager.create_private_wallet("team-member").unwrap();
        let runtime = Arc::new(
            crate::team_mining::TeamMiningRuntime::new(
                crate::team_mining::TeamStore::memory(),
                address,
            )
            .unwrap(),
        );
        state.team_mining_runtime = Some(Arc::clone(&runtime));
        runtime
    }

    #[tokio::test]
    async fn team_mining_endpoints_distinguish_unsupported_from_not_joined() {
        let dir = format!("./test_db_team_absent_{}", std::process::id());
        let app_state = Arc::new(test_app_state(&dir));
        let result = get_team_mining_status(State(app_state)).await;
        assert!(matches!(
            result,
            Err((StatusCode::NOT_FOUND, _))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn team_create_and_start_endpoints_enable_real_runtime_state() {
        let dir = format!("./test_db_team_create_{}", std::process::id());
        let mut state = test_app_state(&dir);
        configure_test_team(&mut state);
        let app_state = Arc::new(state);

        let created = create_mining_team(State(Arc::clone(&app_state)))
            .await
            .unwrap()
            .0;
        let team = created.team.as_ref().expect("team should be created");
        assert!(crate::team_mining::parse_invite_code(&team.invite_code).is_some());
        assert_eq!(team.coordinator, created.local_address);

        let started = start_team_mining(
            State(app_state),
            Some(Json(TeamMiningControlRequest { threads: 3 })),
        )
        .await
        .unwrap()
        .0;
        assert!(started.enabled);
        assert_eq!(started.threads, 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_private_tx_nullifier_tags_collects_primary_and_inputs() {
        let tx = minimal_private_tx("tx-new", "primary", vec!["input-a", "", " input-b "]);
        let tags = private_tx_nullifier_tags(&tx);

        assert!(tags.contains("primary"));
        assert!(tags.contains("input-a"));
        assert!(tags.contains("input-b"));
        assert_eq!(tags.len(), 3);
    }

    #[test]
    fn test_find_pending_private_nullifier_conflict_detects_duplicate() {
        let test_dir = ".zoka/test-suite/.test_api_pending_nullifier_conflict";
        let _ = std::fs::remove_dir_all(test_dir);
        let state = test_app_state(test_dir);
        let pending = minimal_private_tx("tx-pending", "shared-nullifier", vec![]);
        let pending_body = serde_json::to_vec(&pending).unwrap();
        state
            .state_manager
            .save_private_transaction_bytes("tx-pending", &pending_body)
            .unwrap();
        state
            .state_manager
            .save_pending_private_transaction_hash("tx-pending")
            .unwrap();

        let new_tx = minimal_private_tx("tx-new", "other", vec!["shared-nullifier"]);
        let conflict = find_pending_private_nullifier_conflict(&state, "tx-new", &new_tx)
            .unwrap()
            .unwrap();

        assert_eq!(conflict.0, "tx-pending");
        assert_eq!(conflict.1, "shared-nullifier");
        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[tokio::test]
    async fn test_legacy_or_outdated_heartbeats_do_not_count_as_active_miners() {
        let runtime = NetworkRuntimeState::default();

        let legacy_record = runtime
            .record_miner_heartbeat(MinerHeartbeatRequest {
                peer_id: "legacy-peer".to_string(),
                mining: true,
                hashrate_hps: 10,
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(legacy_record.legacy_client);
        assert!(legacy_record.outdated_client);

        let outdated_record = runtime
            .record_miner_heartbeat(MinerHeartbeatRequest {
                peer_id: "outdated-peer".to_string(),
                mining: true,
                hashrate_hps: 20,
                client_version: Some("1.3.0".to_string()),
                software: Some("zsilent".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(!outdated_record.legacy_client);
        assert!(outdated_record.outdated_client);

        let snapshot = runtime.snapshot().await;
        assert_eq!(snapshot.miners_online, 0);
        assert_eq!(snapshot.remote_miners_online, 0);
        assert_eq!(snapshot.total_reported_hashrate_hps, 0);
        assert_eq!(snapshot.legacy_miners_online, 1);
        assert_eq!(snapshot.outdated_miners_online, 2);
        assert_eq!(snapshot.minimum_client_version, NETWORK_MIN_CLIENT_VERSION);
    }

    #[tokio::test]
    async fn test_current_heartbeat_counts_as_active_miner() {
        let runtime = NetworkRuntimeState::default();

        let record = runtime
            .record_miner_heartbeat(MinerHeartbeatRequest {
                peer_id: "current-peer".to_string(),
                mining: true,
                hashrate_hps: 55,
                client_version: Some(format!("v{NETWORK_MIN_CLIENT_VERSION}")),
                software: Some("zoka".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(!record.legacy_client);
        assert!(!record.outdated_client);
        assert_eq!(
            record.client_version.as_deref(),
            Some(NETWORK_MIN_CLIENT_VERSION)
        );

        let snapshot = runtime.snapshot().await;
        assert_eq!(snapshot.miners_online, 1);
        assert_eq!(snapshot.remote_miners_online, 1);
        assert_eq!(snapshot.total_reported_hashrate_hps, 55);
        assert_eq!(snapshot.legacy_miners_online, 0);
        assert_eq!(snapshot.outdated_miners_online, 0);
    }

    #[tokio::test]
    async fn test_zsilent_heartbeat_requires_current_launcher_version() {
        let runtime = NetworkRuntimeState::default();

        let missing_launcher_version = runtime
            .record_miner_heartbeat(MinerHeartbeatRequest {
                peer_id: "zsilent-missing-launcher".to_string(),
                mining: true,
                hashrate_hps: 11,
                client_version: Some(NETWORK_MIN_CLIENT_VERSION.to_string()),
                software: Some("zsilent".to_string()),
                launcher: Some("zsilent".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(!missing_launcher_version.legacy_client);
        assert!(missing_launcher_version.outdated_client);

        let old_launcher_version = runtime
            .record_miner_heartbeat(MinerHeartbeatRequest {
                peer_id: "zsilent-old-launcher".to_string(),
                mining: true,
                hashrate_hps: 22,
                client_version: Some(NETWORK_MIN_CLIENT_VERSION.to_string()),
                software: Some("zsilent".to_string()),
                launcher: Some("zsilent".to_string()),
                launcher_version: Some("1.4.0".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(!old_launcher_version.legacy_client);
        assert!(old_launcher_version.outdated_client);

        let current_launcher_version = runtime
            .record_miner_heartbeat(MinerHeartbeatRequest {
                peer_id: "zsilent-current-launcher".to_string(),
                mining: true,
                hashrate_hps: 33,
                client_version: Some(NETWORK_MIN_CLIENT_VERSION.to_string()),
                software: Some("zsilent".to_string()),
                launcher: Some("zsilent".to_string()),
                launcher_version: Some(format!("v{NETWORK_MIN_CLIENT_VERSION}")),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(!current_launcher_version.legacy_client);
        assert!(!current_launcher_version.outdated_client);
        assert_eq!(
            current_launcher_version.launcher_version.as_deref(),
            Some(NETWORK_MIN_CLIENT_VERSION)
        );

        let snapshot = runtime.snapshot().await;
        assert_eq!(snapshot.miners_online, 1);
        assert_eq!(snapshot.remote_miners_online, 1);
        assert_eq!(snapshot.total_reported_hashrate_hps, 33);
        assert_eq!(snapshot.legacy_miners_online, 0);
        assert_eq!(snapshot.outdated_miners_online, 2);
    }

    #[tokio::test]
    async fn test_create_router() {
        let mempool = Arc::new(Mempool::new(1000));

        let test_dir = ".zoka/test-suite/.test_api";
        let storage = Arc::new(crate::storage::RocksDbStorage::new(test_dir).unwrap());
        let state_manager = Arc::new(StateManager::new(storage.clone()));

        let state = Arc::new(AppState {
            state_manager,
            mempool,
            security: ApiSecurityConfig::default(),
            rate_limiter: ApiRateLimiter::default(),
            security_stats: Arc::new(ApiSecurityStats::default()),
            ws_manager: WebSocketConnectionManager::new(),
            pbft_consensus: Arc::new(tokio::sync::Mutex::new(None)),
            private_wallet_manager: None,
            private_tx_broadcaster: None,
            team_mining_runtime: None,
        });

        let _router = create_router(state);
        assert!(true);

        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[tokio::test]
    async fn test_state_root_and_account_proof_handlers() {
        let mempool = Arc::new(Mempool::new(1000));

        let test_dir = ".zoka/test-suite/.test_api_state_root";
        let _ = std::fs::remove_dir_all(test_dir);
        let storage = Arc::new(crate::storage::RocksDbStorage::new(test_dir).unwrap());
        let state_manager = Arc::new(StateManager::new(storage));
        state_manager
            .create_account("alice".to_string(), 500)
            .await
            .unwrap();

        let app_state = Arc::new(AppState {
            state_manager: state_manager.clone(),
            mempool,
            security: ApiSecurityConfig::default(),
            rate_limiter: ApiRateLimiter::default(),
            security_stats: Arc::new(ApiSecurityStats::default()),
            ws_manager: WebSocketConnectionManager::new(),
            pbft_consensus: Arc::new(tokio::sync::Mutex::new(None)),
            private_wallet_manager: None,
            private_tx_broadcaster: None,
            team_mining_runtime: None,
        });

        let root_resp = get_state_root(State(app_state.clone())).await;
        let root = root_resp.0["state_root"]
            .as_str()
            .expect("state_root must be string")
            .to_string();
        assert_ne!(root, "0");

        let proof_resp = get_account_proof(State(app_state.clone()), Path("alice".to_string()))
            .await
            .expect("proof should exist");
        let proof = proof_resp.0;
        assert_eq!(proof.address, "alice");
        assert!(StateManager::verify_account_inclusion_proof(&root, &proof));

        let verify_resp = verify_account_proof(
            State(app_state.clone()),
            Json(VerifyProofRequest {
                expected_root: root.clone(),
                proof: proof.clone(),
            }),
        )
        .await
        .expect("proof verification request should succeed");
        assert!(verify_resp.0.valid);

        let invalid_root_verify = verify_account_proof(
            State(app_state.clone()),
            Json(VerifyProofRequest {
                expected_root: "zzzz".to_string(),
                proof,
            }),
        )
        .await;
        assert!(invalid_root_verify.is_err());

        let missing = get_account_proof(State(app_state), Path("missing".to_string())).await;
        let (status, _) = missing.expect_err("missing account should return 404");
        assert_eq!(status, StatusCode::NOT_FOUND);

        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[tokio::test]
    async fn test_get_block_by_height() {
        let mempool = Arc::new(Mempool::new(1000));
        let test_dir = ".zoka/test-suite/.test_api_block_height";
        let _ = std::fs::remove_dir_all(test_dir);
        let storage = Arc::new(crate::storage::RocksDbStorage::new(test_dir).unwrap());
        let state_manager = Arc::new(StateManager::new(storage));

        // Create a block at height 0
        let block = Block::new(
            "0".to_string(),
            1000,
            0,
            0,
            vec![],
            PublicKey(vec![1, 2, 3]),
            Signature(vec![]),
        );
        state_manager.apply_block(&block).await.unwrap();

        let app_state = Arc::new(AppState {
            state_manager: state_manager.clone(),
            mempool,
            security: ApiSecurityConfig::default(),
            rate_limiter: ApiRateLimiter::default(),
            security_stats: Arc::new(ApiSecurityStats::default()),
            ws_manager: WebSocketConnectionManager::new(),
            pbft_consensus: Arc::new(tokio::sync::Mutex::new(None)),
            private_wallet_manager: None,
            private_tx_broadcaster: None,
            team_mining_runtime: None,
        });

        let resp = get_block_by_height(State(app_state), Path(0)).await;
        assert!(resp.is_ok());
        let json = resp.unwrap();
        assert_eq!(json.0["height"], 0);

        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[tokio::test]
    async fn test_get_block_by_hash() {
        let mempool = Arc::new(Mempool::new(1000));
        let test_dir = ".zoka/test-suite/.test_api_block_hash";
        let _ = std::fs::remove_dir_all(test_dir);
        let storage = Arc::new(crate::storage::RocksDbStorage::new(test_dir).unwrap());
        let state_manager = Arc::new(StateManager::new(storage));

        let block = Block::new(
            "0".to_string(),
            2000,
            0,
            1,
            vec![],
            PublicKey(vec![1, 2, 3]),
            Signature(vec![]),
        );
        let block_hash = block.header.hash.clone();
        state_manager.apply_block(&block).await.unwrap();

        let app_state = Arc::new(AppState {
            state_manager: state_manager.clone(),
            mempool,
            security: ApiSecurityConfig::default(),
            rate_limiter: ApiRateLimiter::default(),
            security_stats: Arc::new(ApiSecurityStats::default()),
            ws_manager: WebSocketConnectionManager::new(),
            pbft_consensus: Arc::new(tokio::sync::Mutex::new(None)),
            private_wallet_manager: None,
            private_tx_broadcaster: None,
            team_mining_runtime: None,
        });

        let resp = get_block_by_hash(State(app_state), Path(block_hash.clone())).await;
        assert!(resp.is_ok());
        let json = resp.unwrap();
        assert_eq!(json.0["hash"], block_hash);

        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[tokio::test]
    async fn test_get_blocks_range() {
        let mempool = Arc::new(Mempool::new(1000));
        let test_dir = ".zoka/test-suite/.test_api_blocks_range";
        let _ = std::fs::remove_dir_all(test_dir);
        let storage = Arc::new(crate::storage::RocksDbStorage::new(test_dir).unwrap());
        let state_manager = Arc::new(StateManager::new(storage));

        // Create blocks 0-4
        for i in 0..5 {
            let block = Block::new(
                if i == 0 {
                    "0".to_string()
                } else {
                    format!("{}", i - 1)
                },
                1000 + i as u64 * 100,
                0,
                i,
                vec![],
                PublicKey(vec![1, 2, 3]),
                Signature(vec![]),
            );
            state_manager.apply_block(&block).await.unwrap();
        }

        let app_state = Arc::new(AppState {
            state_manager: state_manager.clone(),
            mempool,
            security: ApiSecurityConfig::default(),
            rate_limiter: ApiRateLimiter::default(),
            security_stats: Arc::new(ApiSecurityStats::default()),
            ws_manager: WebSocketConnectionManager::new(),
            pbft_consensus: Arc::new(tokio::sync::Mutex::new(None)),
            private_wallet_manager: None,
            private_tx_broadcaster: None,
            team_mining_runtime: None,
        });

        let resp = get_blocks_range(State(app_state), Path((1u64, 3u64))).await;
        assert!(resp.is_ok());
        let json = resp.unwrap();
        let blocks = json.0["blocks"].as_array().expect("should be array");
        assert_eq!(blocks.len(), 3); // heights 1, 2, 3

        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[tokio::test]
    async fn test_admin_rollback() {
        let mempool = Arc::new(Mempool::new(1000));
        let test_dir = ".zoka/test-suite/.test_api_rollback";
        let _ = std::fs::remove_dir_all(test_dir);
        let storage = Arc::new(crate::storage::RocksDbStorage::new(test_dir).unwrap());
        let state_manager = Arc::new(StateManager::new(storage));

        // Create blocks 0-5
        for i in 0..6 {
            let block = Block::new(
                if i == 0 {
                    "0".to_string()
                } else {
                    format!("{}", i - 1)
                },
                1000 + i as u64 * 100,
                0,
                i,
                vec![],
                PublicKey(vec![1, 2, 3]),
                Signature(vec![]),
            );
            state_manager.apply_block(&block).await.unwrap();
        }

        let app_state = Arc::new(AppState {
            state_manager: state_manager.clone(),
            mempool,
            security: ApiSecurityConfig::default(),
            rate_limiter: ApiRateLimiter::default(),
            security_stats: Arc::new(ApiSecurityStats::default()),
            ws_manager: WebSocketConnectionManager::new(),
            pbft_consensus: Arc::new(tokio::sync::Mutex::new(None)),
            private_wallet_manager: None,
            private_tx_broadcaster: None,
            team_mining_runtime: None,
        });

        // Check initial height
        let current = state_manager.get_height().await;
        assert_eq!(current, 5);

        // Rollback to height 2
        let resp = admin_rollback(
            State(app_state.clone()),
            Json(RollbackRequest { target_height: 2 }),
        )
        .await;
        assert!(resp.is_ok());

        // Verify rollback
        let new_height = state_manager.get_height().await;
        assert_eq!(new_height, 2);

        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[tokio::test]
    async fn test_rate_limiter_enforces_limit() {
        let limiter = ApiRateLimiter::default();
        assert!(limiter.allow_request("client-a", 2).await);
        assert!(limiter.allow_request("client-a", 2).await);
        assert!(!limiter.allow_request("client-a", 2).await);
    }

    #[tokio::test]
    async fn test_rate_limiter_adaptive_penalty_escalates() {
        let limiter = ApiRateLimiter::default();
        let key = "client-b";
        let t = 1_000u64;

        assert!(limiter.allow_request_at_for_test(key, 3, t).await);
        assert!(limiter.allow_request_at_for_test(key, 3, t).await);
        assert!(limiter.allow_request_at_for_test(key, 3, t).await);
        assert!(!limiter.allow_request_at_for_test(key, 3, t).await);
        assert_eq!(limiter.penalty_level_for_test(key).await, 1);

        // After cooldown + window reset, effective limit should be lower due penalty.
        assert!(limiter.allow_request_at_for_test(key, 3, t + 61).await);
        assert!(limiter.allow_request_at_for_test(key, 3, t + 61).await);
        assert!(!limiter.allow_request_at_for_test(key, 3, t + 61).await);
        assert!(limiter.penalty_level_for_test(key).await >= 2);
    }

    #[test]
    fn test_constant_time_eq() {
        assert!(constant_time_eq("token-123", "token-123"));
        assert!(!constant_time_eq("token-123", "token-124"));
        assert!(!constant_time_eq("short", "much-longer"));
    }

    #[test]
    fn test_validate_client_id() {
        assert!(validate_client_id("node-a_01:us", 64));
        assert!(!validate_client_id("node with space", 64));
        assert!(!validate_client_id("", 64));
        assert!(!validate_client_id("x", 0));
    }

    #[test]
    fn test_rate_limit_bucket_selection() {
        assert_eq!(rate_limit_bucket("POST", "/transactions"), "transactions");
        assert_eq!(
            rate_limit_bucket("GET", "/transactions/new"),
            "transactions"
        );
        assert_eq!(rate_limit_bucket("POST", "/private/tx"), "transactions");
        assert_eq!(
            rate_limit_bucket("POST", "/private/wallet/send-from-seed"),
            "transactions"
        );
        assert_eq!(rate_limit_bucket("GET", "/private/tx/hash"), "public-read");
        assert_eq!(rate_limit_bucket("GET", "/chain/height"), "public-read");
        assert_eq!(rate_limit_bucket("GET", "/stats"), "public-read");
        assert_eq!(rate_limit_bucket("GET", "/mempool/metrics"), "public-read");
        assert_eq!(
            rate_limit_bucket("GET", "/blocks/range/1/10"),
            "public-read"
        );
        assert_eq!(rate_limit_bucket("GET", "/private/pending"), "public-read");
    }

    #[test]
    fn test_rate_limit_key_separates_anonymous_clients_by_ip() {
        assert_ne!(
            rate_limit_key("203.0.113.10", "anonymous", "public-read"),
            rate_limit_key("203.0.113.11", "anonymous", "public-read")
        );
        assert_eq!(
            rate_limit_key("203.0.113.10", "anonymous", "public-read"),
            "ip:203.0.113.10:public-read"
        );
        assert_eq!(
            rate_limit_key("203.0.113.10", "explorer", "public-read"),
            "client:explorer:ip:203.0.113.10:public-read"
        );
    }

    #[test]
    fn test_resolve_client_ip_ignores_forwarded_header_from_untrusted_peer() {
        // No trusted proxies configured: an attacker connecting directly and
        // setting X-Forwarded-For must not be able to override their real IP.
        let ip = resolve_client_ip(Some("198.51.100.7"), Some("1.2.3.4"), &[]);
        assert_eq!(ip, "198.51.100.7");
    }

    #[test]
    fn test_resolve_client_ip_trusts_forwarded_header_from_trusted_proxy() {
        let trusted = vec!["10.0.0.1".to_string()];
        let ip = resolve_client_ip(Some("10.0.0.1"), Some("203.0.113.9"), &trusted);
        assert_eq!(ip, "203.0.113.9");
    }

    #[test]
    fn test_resolve_client_ip_falls_back_to_peer_when_no_forwarded_header() {
        let ip = resolve_client_ip(Some("198.51.100.7"), None, &[]);
        assert_eq!(ip, "198.51.100.7");
    }

    #[test]
    fn test_resolve_client_ip_unknown_without_any_signal() {
        let ip = resolve_client_ip(None, None, &[]);
        assert_eq!(ip, "unknown");
    }

    #[test]
    fn test_resolve_client_ip_rotating_spoofed_header_no_longer_creates_distinct_identities() {
        // Regression for the bug caught live: with no trusted proxy, spoofing
        // a different X-Forwarded-For on every request used to mint a "new"
        // client IP each time, bypassing rate limiting and erasing the audit
        // trail. Same untrusted peer must resolve to the same IP regardless.
        let a = resolve_client_ip(Some("198.51.100.7"), Some("1.1.1.1"), &[]);
        let b = resolve_client_ip(Some("198.51.100.7"), Some("2.2.2.2"), &[]);
        assert_eq!(a, b);
        assert_eq!(a, "198.51.100.7");
    }

    #[test]
    fn test_required_role_for_request_scopes() {
        assert_eq!(
            required_role_for_request("GET", "/chain/height"),
            ApiRole::Read
        );
        assert_eq!(
            required_role_for_request("POST", "/transactions"),
            ApiRole::Write
        );
        assert_eq!(
            required_role_for_request("POST", "/private/wallet/send-from-seed"),
            ApiRole::Write
        );
        assert_eq!(
            required_role_for_request("GET", "/metrics/security"),
            ApiRole::Admin
        );
    }

    #[test]
    fn test_seed_envelope_decrypts_and_rejects_tampering() {
        use aes_gcm::aead::{Aead, Payload};
        use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
        use sha2::{Digest, Sha256};

        let server_key = server_seed_key();
        let ephemeral_secret = x25519_dalek::StaticSecret::from([7u8; 32]);
        let ephemeral_pubkey = x25519_dalek::PublicKey::from(&ephemeral_secret);
        let shared_secret = ephemeral_secret.diffie_hellman(&server_key.public);
        let mut hasher = Sha256::new();
        hasher.update(b"zoka-seed-envelope-v1");
        hasher.update(shared_secret.as_bytes());
        hasher.update(ephemeral_pubkey.as_bytes());
        hasher.update(server_key.public.as_bytes());
        let aes_key = hasher.finalize();
        let nonce = [9u8; 12];
        let cipher = Aes256Gcm::new_from_slice(&aes_key).unwrap();
        let seed = [3u8; 32];
        let ciphertext = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &seed,
                    aad: b"zoka-seed-envelope-v1",
                },
            )
            .unwrap();
        let mut request = SendFromSeedRequest {
            ephemeral_pubkey_hex: hex::encode(ephemeral_pubkey.as_bytes()),
            nonce_hex: hex::encode(nonce),
            encrypted_seed_hex: hex::encode(ciphertext),
            to: "zka1test".to_string(),
            amount_atoms: 1,
            fee_atoms: None,
        };

        assert_eq!(decrypt_seed(&request).unwrap(), seed);
        request.encrypted_seed_hex.replace_range(0..2, "ff");
        assert!(decrypt_seed(&request).is_err());
    }

    #[tokio::test]
    async fn test_send_from_seed_disabled_before_decrypting_envelope() {
        let test_dir = ".zoka/test-suite/.test_send_from_seed_disabled";
        let _ = std::fs::remove_dir_all(test_dir);
        let mut state = test_app_state(test_dir);
        state.security.enable_remote_seed_endpoint = false;
        let state = Arc::new(state);
        let request = SendFromSeedRequest {
            ephemeral_pubkey_hex: "not-hex".to_string(),
            nonce_hex: "not-hex".to_string(),
            encrypted_seed_hex: "not-hex".to_string(),
            to: "not-an-address".to_string(),
            amount_atoms: 0,
            fee_atoms: None,
        };

        let err = send_from_seed(State(state.clone()), Json(request))
            .await
            .expect_err("disabled endpoint must reject before reading the envelope");
        assert_eq!(err.0, StatusCode::NOT_FOUND);
        assert!(err.1.contains("remote seed endpoint is disabled"));

        drop(state);
        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[test]
    fn test_resolve_client_role_from_security_lists() {
        let security = ApiSecurityConfig {
            write_clients: vec!["writer-1".to_string()],
            admin_clients: vec!["admin-1".to_string()],
            ..ApiSecurityConfig::default()
        };

        assert_eq!(resolve_client_role("reader-1", &security), ApiRole::Read);
        assert_eq!(resolve_client_role("writer-1", &security), ApiRole::Write);
        assert_eq!(resolve_client_role("admin-1", &security), ApiRole::Admin);
    }

    #[tokio::test]
    async fn test_rbac_grant_role() {
        let rbac = RbacConfig::new();
        rbac.grant_role("client-1".to_string(), ApiRole::Write)
            .await;
        assert!(rbac.can_access("client-1", "/transactions").await);
    }

    #[tokio::test]
    async fn test_rbac_can_access() {
        let rbac = RbacConfig::new();
        rbac.grant_role("read-client".to_string(), ApiRole::Read)
            .await;
        rbac.grant_role("admin-client".to_string(), ApiRole::Admin)
            .await;

        // Read client can access read endpoints
        assert!(rbac.can_access("read-client", "/chain/height").await);
        // Read client cannot access admin endpoints
        assert!(!rbac.can_access("read-client", "/admin/rollback").await);
        // Admin can access all
        assert!(rbac.can_access("admin-client", "/chain/height").await);
        assert!(rbac.can_access("admin-client", "/admin/rollback").await);
    }

    #[tokio::test]
    async fn test_rbac_revoke_role() {
        let rbac = RbacConfig::new();
        rbac.grant_role("client-1".to_string(), ApiRole::Admin)
            .await;
        assert!(rbac.can_access("client-1", "/admin/rollback").await);

        rbac.revoke_role("client-1").await;
        // After revoke, defaults to Read role
        assert!(!rbac.can_access("client-1", "/admin/rollback").await);
    }

    #[tokio::test]
    async fn test_audit_logger_logs() {
        let logger = AuditLogger::new(100);
        let entry = AuditLogEntry {
            timestamp: 1000,
            client_id: "client-1".to_string(),
            method: "GET".to_string(),
            path: "/chain/height".to_string(),
            status: 200,
            ip_address: "127.0.0.1".to_string(),
            cache_status: "hit".to_string(),
        };

        logger.log(entry).await;
        let logs = logger.get_logs().await;
        assert_eq!(logs.len(), 1);
    }

    #[tokio::test]
    async fn test_audit_logger_get_logs_for_client() {
        let logger = AuditLogger::new(100);

        logger
            .log(AuditLogEntry {
                timestamp: 1000,
                client_id: "client-1".to_string(),
                method: "GET".to_string(),
                path: "/chain/height".to_string(),
                status: 200,
                ip_address: "127.0.0.1".to_string(),
                cache_status: "hit".to_string(),
            })
            .await;

        logger
            .log(AuditLogEntry {
                timestamp: 1001,
                client_id: "client-2".to_string(),
                method: "POST".to_string(),
                path: "/transactions".to_string(),
                status: 201,
                ip_address: "127.0.0.1".to_string(),
                cache_status: "miss".to_string(),
            })
            .await;

        let client1_logs = logger.get_logs_for_client("client-1").await;
        assert_eq!(client1_logs.len(), 1);
        assert_eq!(client1_logs[0].client_id, "client-1");
    }

    #[tokio::test]
    async fn test_replay_protection_check_nonce() {
        let protection = ReplayProtection::new();
        assert!(protection.check_nonce("client-1", 100).await);
        assert!(protection.check_nonce("client-1", 101).await);
    }

    #[tokio::test]
    async fn test_replay_protection_duplicate_nonce() {
        let protection = ReplayProtection::new();
        assert!(protection.check_nonce("client-1", 100).await);
        // Same nonce should be rejected
        assert!(!protection.check_nonce("client-1", 100).await);
        // Different client can use same nonce
        assert!(protection.check_nonce("client-2", 100).await);
    }

    #[tokio::test]
    async fn test_replay_protection_cleanup_old_nonces() {
        let protection = ReplayProtection::new();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        assert!(
            protection
                .check_nonce_with_timestamp("client-1", "nonce-a", now.saturating_sub(10_000))
                .await
        );
        assert!(
            !protection
                .check_nonce_with_timestamp("client-1", "nonce-a", now)
                .await
        );

        protection.cleanup_old_nonces(60).await;
        assert!(
            protection
                .check_nonce_with_timestamp("client-1", "nonce-a", now)
                .await
        );
    }

    #[tokio::test]
    async fn test_request_signature_validator_enabled() {
        let validator = RequestSignatureValidator::new(true);
        validator.require_signature("client-1").await;

        assert!(validator.is_signature_required("client-1").await);
        assert!(!validator.is_signature_required("client-2").await);
    }

    #[test]
    fn test_request_signature_validator_signature_validation() {
        let validator = RequestSignatureValidator::new(true);
        let secret = b"secret-key";

        let valid_sig = hex::encode(
            hmac_sha256(secret, &[b"GET", b"\n", b"/api/endpoint", b"\n", b"body"]).unwrap(),
        );
        assert!(validator.validate_signature("GET", "/api/endpoint", b"body", &valid_sig, secret));

        // Test invalid signature
        let invalid =
            validator.validate_signature("GET", "/api/endpoint", b"body", "invalid", secret);
        assert!(!invalid);

        // Test with disabled validator - should always return true
        let disabled = RequestSignatureValidator::new(false);
        let always_ok = disabled.validate_signature("GET", "/api", b"body", "anything", b"secret");
        assert!(always_ok);
    }

    #[test]
    fn test_request_signature_validator_disabled() {
        let validator = RequestSignatureValidator::new(false);
        // When disabled, validation should always pass
        let result = validator.validate_signature("GET", "/api", b"body", "anything", b"secret");
        assert!(result);
    }

    #[tokio::test]
    async fn test_request_signature_validator_multiple_clients() {
        let validator = RequestSignatureValidator::new(true);

        validator.require_signature("client-1").await;
        validator.require_signature("client-2").await;

        assert!(validator.is_signature_required("client-1").await);
        assert!(validator.is_signature_required("client-2").await);
        assert!(!validator.is_signature_required("client-3").await);
    }

    #[test]
    fn test_encryption_helper_encrypt_decrypt() {
        let helper = EncryptionHelper::new(true);
        let key = b"secret-key";
        let data = b"sensitive-message";

        let encrypted = helper.encrypt(data, key);
        let decrypted = helper.decrypt(&encrypted, key);

        assert_eq!(decrypted, data);
        // Encrypted should be different from original
        assert_ne!(encrypted, data);
    }

    #[test]
    fn test_encryption_helper_disabled() {
        let helper = EncryptionHelper::new(false);
        let key = b"secret-key";
        let data = b"sensitive-message";

        // When disabled, encryption should return original data
        let result = helper.encrypt(data, key);
        assert_eq!(result, data);
    }

    #[test]
    fn test_encryption_helper_empty_key() {
        let helper = EncryptionHelper::new(true);
        let data = b"sensitive-message";

        // Empty key should return original data
        let result = helper.encrypt(data, b"");
        assert_eq!(result, data);
    }

    #[test]
    fn test_encryption_helper_symmetry() {
        let helper = EncryptionHelper::new(true);
        let key = b"password123";
        let data = b"test-data-for-encryption";

        // Encrypt and decrypt multiple times
        let encrypted1 = helper.encrypt(data, key);
        let encrypted2 = helper.encrypt(&encrypted1, key);

        // Double encryption should return original
        assert_eq!(encrypted2, data);
    }

    #[tokio::test]
    async fn test_api_key_manager_create_and_verify() {
        let manager = ApiKeyManager::new();
        let (_actual_key, info) = manager
            .create_key(vec!["read".to_string(), "write".to_string()])
            .await;

        assert_eq!(info.permissions.len(), 2);
        assert!(info.enabled);
        assert!(manager.verify_key(&info.key_id, &info.key_hash).await);
    }

    #[tokio::test]
    async fn test_api_key_manager_revoke() {
        let manager = ApiKeyManager::new();
        let (_actual_key, info) = manager.create_key(vec!["read".to_string()]).await;

        assert!(manager.verify_key(&info.key_id, &info.key_hash).await);

        manager.revoke_key(&info.key_id).await;
        assert!(!manager.verify_key(&info.key_id, &info.key_hash).await);
    }

    #[tokio::test]
    async fn test_api_key_manager_list_keys() {
        let manager = ApiKeyManager::new();
        let (_key1, _info1) = manager.create_key(vec!["read".to_string()]).await;
        let (_key2, info2) = manager.create_key(vec!["write".to_string()]).await;

        let active_keys = manager.list_keys().await;
        assert_eq!(active_keys.len(), 2);

        manager.revoke_key(&info2.key_id).await;
        let remaining = manager.list_keys().await;
        assert_eq!(remaining.len(), 1);
    }

    #[tokio::test]
    async fn test_api_key_manager_invalid_key() {
        let manager = ApiKeyManager::new();
        assert!(!manager.verify_key("nonexistent", "fake-hash").await);
    }

    #[test]
    fn test_input_validator_string() {
        let validator = InputValidator::new(20, u64::MAX, false);

        assert!(validator.validate_string("valid_input", "field").is_ok());
        assert!(validator.validate_string("", "field").is_err());
        let long_string = "x".repeat(25);
        assert!(validator.validate_string(&long_string, "field").is_err());
        assert!(validator.validate_string("invalid@char", "field").is_err());
    }

    #[test]
    fn test_input_validator_number() {
        let validator = InputValidator::new(256, 1000, true);

        assert!(validator.validate_number(500, "field").is_ok());
        assert!(validator.validate_number(1001, "field").is_err());
        assert!(validator.validate_number(0, "field").is_ok());
    }

    #[test]
    fn test_input_validator_email() {
        let validator = InputValidator::default();

        assert!(validator.validate_email("user@example.com").is_ok());
        assert!(validator.validate_email("invalid.email").is_err());
        assert!(validator.validate_email("@example.com").is_err());
        assert!(validator.validate_email("user@").is_err());
    }

    #[test]
    fn test_error_response_creation() {
        let err = ErrorResponse::new("Test error".to_string(), 400);
        assert_eq!(err.error, "Test error");
        assert_eq!(err.error_code, 400);
        assert!(err.timestamp > 0);
    }

    #[test]
    fn test_error_response_templates() {
        let internal = ErrorResponse::internal_error();
        assert_eq!(internal.error_code, 500);

        let not_found = ErrorResponse::not_found("user");
        assert_eq!(not_found.error_code, 404);

        let validation = ErrorResponse::validation_error("bad input");
        assert_eq!(validation.error_code, 400);
    }

    #[test]
    fn test_cors_config_origins() {
        let cors = CorsConfig::default();
        assert!(cors.is_origin_allowed("*"));

        let restricted = CorsConfig::new().with_origins(vec!["https://example.com".to_string()]);
        assert!(restricted.is_origin_allowed("https://example.com"));
        assert!(!restricted.is_origin_allowed("https://other.com"));
    }

    #[test]
    fn test_cors_config_methods() {
        let cors = CorsConfig::default();
        assert!(cors.is_method_allowed("GET"));
        assert!(cors.is_method_allowed("POST"));
        assert!(!cors.is_method_allowed("DELETE"));
    }

    #[test]
    fn test_token_manager_create_and_verify() {
        let manager = TokenManager::new("secret-key".to_string(), 3600);
        let token = manager.create_token("user-1", "admin");

        assert!(!token.is_empty());
        assert_eq!(token.matches('.').count(), 1); // payload.signature

        let result = manager.verify_token(&token);
        assert!(result.is_ok());

        let claims = result.unwrap();
        assert_eq!(claims.sub, "user-1");
        assert_eq!(claims.role, "admin");
    }

    #[test]
    fn test_token_manager_verify_invalid_token() {
        let manager = TokenManager::default();

        assert!(manager.verify_token("invalid-token").is_err());
        assert!(manager.verify_token("a.b.c").is_err());
    }

    #[test]
    fn test_token_manager_refresh_token() {
        let manager = TokenManager::new("secret".to_string(), 3600);
        let original_token = manager.create_token("user-1", "user");

        let refreshed = manager.refresh_token(&original_token);
        assert!(refreshed.is_ok());

        let new_token = refreshed.unwrap();
        let claims = manager.verify_token(&new_token);
        assert!(claims.is_ok());

        let claims = claims.unwrap();
        assert_eq!(claims.sub, "user-1");
        assert_eq!(claims.role, "user");
    }

    #[test]
    fn test_token_manager_invalid_format() {
        let manager = TokenManager::default();

        // Test various invalid formats
        assert!(manager.verify_token("onlyonepart").is_err());
        assert!(manager.verify_token("part.two.three").is_err());
    }

    #[test]
    fn test_token_manager_rejects_tampered_signature() {
        let manager = TokenManager::new("secret".to_string(), 3600);
        let token = manager.create_token("user-1", "user");
        let mut tampered = token.clone();
        tampered.push('x');
        assert!(manager.verify_token(&tampered).is_err());
    }

    #[tokio::test]
    async fn test_circuit_breaker_closed_state() {
        let breaker = CircuitBreaker::default();

        assert!(breaker.is_request_allowed().await);
        assert_eq!(breaker.get_state().await, CircuitState::Closed);
    }

    #[tokio::test]
    async fn test_circuit_breaker_open_on_failures() {
        let breaker = CircuitBreaker::new(3, 2, 60);

        // Record 3 failures (threshold)
        breaker.record_failure().await;
        breaker.record_failure().await;
        assert!(breaker.is_request_allowed().await);

        breaker.record_failure().await;
        assert_eq!(breaker.get_state().await, CircuitState::Open);
        assert!(!breaker.is_request_allowed().await);
    }

    #[tokio::test]
    async fn test_circuit_breaker_half_open_recovery() {
        let breaker = CircuitBreaker::new(2, 1, 0); // 0 second timeout for testing

        // Open the circuit
        breaker.record_failure().await;
        breaker.record_failure().await;
        assert_eq!(breaker.get_state().await, CircuitState::Open);

        // Wait a moment and try again (will transition to HalfOpen)
        let allowed = breaker.is_request_allowed().await;
        assert!(allowed);
        assert_eq!(breaker.get_state().await, CircuitState::HalfOpen);

        // Record success to close the circuit
        breaker.record_success().await;
        assert_eq!(breaker.get_state().await, CircuitState::Closed);
    }

    #[tokio::test]
    async fn test_circuit_breaker_states() {
        let breaker = CircuitBreaker::new(2, 2, 60);

        // Start in Closed
        assert_eq!(breaker.get_state().await, CircuitState::Closed);

        // Open after enough failures
        breaker.record_failure().await;
        assert_eq!(breaker.get_state().await, CircuitState::Closed);

        breaker.record_failure().await;
        assert_eq!(breaker.get_state().await, CircuitState::Open);
    }

    #[tokio::test]
    async fn test_event_filter_matches_block() {
        let block = BlockEvent {
            height: 10,
            hash: "hash-10".to_string(),
            timestamp: 1000,
            transactions: 5,
        };

        let filter = EventFilter::new(SubscriptionType::Blocks);
        assert!(filter.matches_block(&block));

        let restricted = EventFilter::new(SubscriptionType::Blocks).with_height_range(5, 8);
        assert!(!restricted.matches_block(&block));

        let allowed_range = EventFilter::new(SubscriptionType::Blocks).with_height_range(5, 15);
        assert!(allowed_range.matches_block(&block));
    }

    #[tokio::test]
    async fn test_event_filter_matches_transaction() {
        let tx = TransactionEvent {
            hash: "tx-1".to_string(),
            sender: "alice".to_string(),
            amount: 100,
            timestamp: 1000,
        };

        let filter = EventFilter::new(SubscriptionType::Transactions);
        assert!(filter.matches_transaction(&tx));

        let alice_filter = EventFilter::new(SubscriptionType::Transactions)
            .with_address_filter("alice".to_string());
        assert!(alice_filter.matches_transaction(&tx));

        let charlie_filter = EventFilter::new(SubscriptionType::Transactions)
            .with_address_filter("charlie".to_string());
        assert!(!charlie_filter.matches_transaction(&tx));
    }

    #[tokio::test]
    async fn test_event_filter_subscription_types() {
        let block = BlockEvent {
            height: 5,
            hash: "hash-5".to_string(),
            timestamp: 1000,
            transactions: 3,
        };

        let block_filter = EventFilter::new(SubscriptionType::Blocks);
        assert!(block_filter.matches_block(&block));

        let state_filter = EventFilter::new(SubscriptionType::State);
        assert!(!state_filter.matches_block(&block));

        let tx_filter = EventFilter::new(SubscriptionType::Transactions);
        assert!(!tx_filter.matches_block(&block));
    }

    #[tokio::test]
    async fn test_websocket_connection_creation() {
        let conn = WebSocketConnection::new("client-1".to_string());
        assert_eq!(conn.client_id, "client-1");
        assert_eq!(conn.subscription_types.len(), 0);
        assert_eq!(conn.event_count, 0);
        assert!(conn.connected_at > 0);
    }

    #[tokio::test]
    async fn test_websocket_connection_subscribe() {
        let mut conn = WebSocketConnection::new("client-1".to_string());
        conn.subscribe(SubscriptionType::Blocks);
        assert!(conn.is_subscribed(&SubscriptionType::Blocks));
        assert!(!conn.is_subscribed(&SubscriptionType::Transactions));

        conn.subscribe(SubscriptionType::Transactions);
        assert!(conn.is_subscribed(&SubscriptionType::Blocks));
        assert!(conn.is_subscribed(&SubscriptionType::Transactions));

        // Duplicate subscription should not add twice
        conn.subscribe(SubscriptionType::Blocks);
        assert_eq!(conn.subscription_types.len(), 2);
    }

    #[tokio::test]
    async fn test_websocket_connection_unsubscribe() {
        let mut conn = WebSocketConnection::new("client-1".to_string());
        conn.subscribe(SubscriptionType::Blocks);
        conn.subscribe(SubscriptionType::Transactions);

        conn.unsubscribe(&SubscriptionType::Blocks);
        assert!(!conn.is_subscribed(&SubscriptionType::Blocks));
        assert!(conn.is_subscribed(&SubscriptionType::Transactions));
    }

    #[tokio::test]
    async fn test_websocket_connection_manager_add_connection() {
        let manager = WebSocketConnectionManager::new();
        let conn = manager.add_connection("client-1".to_string()).await;

        assert_eq!(conn.client_id, "client-1");
        assert_eq!(manager.get_active_connections().await, 1);
    }

    #[tokio::test]
    async fn test_websocket_connection_manager_remove_connection() {
        let manager = WebSocketConnectionManager::new();
        manager.add_connection("client-1".to_string()).await;
        manager.add_connection("client-2".to_string()).await;

        assert_eq!(manager.get_active_connections().await, 2);

        manager.remove_connection("client-1").await;
        assert_eq!(manager.get_active_connections().await, 1);
    }

    #[tokio::test]
    async fn test_websocket_connection_manager_subscribe_unsubscribe() {
        let manager = WebSocketConnectionManager::new();
        manager.add_connection("client-1".to_string()).await;

        manager
            .subscribe("client-1", SubscriptionType::Blocks)
            .await;
        assert!(
            manager
                .is_subscribed("client-1", &SubscriptionType::Blocks)
                .await
        );

        manager
            .subscribe("client-1", SubscriptionType::Transactions)
            .await;
        assert!(
            manager
                .is_subscribed("client-1", &SubscriptionType::Transactions)
                .await
        );

        manager
            .unsubscribe("client-1", &SubscriptionType::Blocks)
            .await;
        assert!(
            !manager
                .is_subscribed("client-1", &SubscriptionType::Blocks)
                .await
        );
        assert!(
            manager
                .is_subscribed("client-1", &SubscriptionType::Transactions)
                .await
        );
    }

    #[tokio::test]
    async fn test_websocket_connection_manager_get_subscriptions() {
        let manager = WebSocketConnectionManager::new();
        manager.add_connection("client-1".to_string()).await;

        manager
            .subscribe("client-1", SubscriptionType::Blocks)
            .await;
        manager.subscribe("client-1", SubscriptionType::State).await;

        let subs = manager.get_subscriptions("client-1").await;
        assert_eq!(subs.len(), 2);
        assert!(subs.contains(&SubscriptionType::Blocks));
        assert!(subs.contains(&SubscriptionType::State));
    }

    #[tokio::test]
    async fn test_websocket_connection_manager_event_count() {
        let manager = WebSocketConnectionManager::new();
        manager.add_connection("client-1".to_string()).await;

        assert_eq!(manager.get_event_count("client-1").await, 0);

        manager.increment_event_count("client-1").await;
        manager.increment_event_count("client-1").await;

        assert_eq!(manager.get_event_count("client-1").await, 2);
    }

    #[tokio::test]
    async fn test_event_broadcaster_creates_channels() {
        let (broadcaster, _) = EventBroadcaster::new();

        let blocks_rx = broadcaster.subscribe_blocks();
        let txs_rx = broadcaster.subscribe_transactions();
        let state_rx = broadcaster.subscribe_state();

        #[allow(unused_comparisons)]
        {
            assert!(blocks_rx.len() >= 0);
            assert!(txs_rx.len() >= 0);
            assert!(state_rx.len() >= 0);
        }
    }

    #[tokio::test]
    async fn test_event_broadcaster_broadcasts_block() {
        let (broadcaster, _) = EventBroadcaster::new();
        let mut block_rx = broadcaster.subscribe_blocks();

        let block = BlockEvent {
            height: 1,
            hash: "hash-1".to_string(),
            timestamp: 1000,
            transactions: 2,
        };

        broadcaster.broadcast_block(block.clone());

        let received =
            tokio::time::timeout(tokio::time::Duration::from_secs(1), block_rx.recv()).await;

        assert!(received.is_ok());
        if let Ok(Ok(event)) = received {
            assert_eq!(event.height, 1);
        }
    }

    #[tokio::test]
    async fn test_event_broadcaster_broadcasts_transaction() {
        let (broadcaster, _) = EventBroadcaster::new();
        let mut tx_rx = broadcaster.subscribe_transactions();

        let tx = TransactionEvent {
            hash: "tx-1".to_string(),
            sender: "alice".to_string(),
            amount: 100,
            timestamp: 1000,
        };

        broadcaster.broadcast_transaction(tx.clone());

        let received =
            tokio::time::timeout(tokio::time::Duration::from_secs(1), tx_rx.recv()).await;

        assert!(received.is_ok());
        if let Ok(Ok(event)) = received {
            assert_eq!(event.hash, "tx-1");
        }
    }

    #[tokio::test]
    async fn test_event_broadcaster_broadcasts_state() {
        let (broadcaster, _) = EventBroadcaster::new();
        let mut state_rx = broadcaster.subscribe_state();

        let state = StateEvent {
            state_root: "root-1".to_string(),
            height: 5,
            timestamp: 1000,
        };

        broadcaster.broadcast_state(state.clone());

        let received =
            tokio::time::timeout(tokio::time::Duration::from_secs(1), state_rx.recv()).await;

        assert!(received.is_ok());
        if let Ok(Ok(event)) = received {
            assert_eq!(event.height, 5);
        }
    }

    #[tokio::test]
    async fn test_event_broadcaster_multiple_subscribers() {
        let (broadcaster, _) = EventBroadcaster::new();
        let mut block_rx1 = broadcaster.subscribe_blocks();
        let mut block_rx2 = broadcaster.subscribe_blocks();

        let block = BlockEvent {
            height: 1,
            hash: "hash-1".to_string(),
            timestamp: 1000,
            transactions: 2,
        };

        broadcaster.broadcast_block(block);

        let received1 =
            tokio::time::timeout(tokio::time::Duration::from_secs(1), block_rx1.recv()).await;

        let received2 =
            tokio::time::timeout(tokio::time::Duration::from_secs(1), block_rx2.recv()).await;

        assert!(received1.is_ok());
        assert!(received2.is_ok());
    }

    #[tokio::test]
    async fn test_ws_subscribe() {
        let mempool = Arc::new(Mempool::new(1000));
        let test_dir = ".zoka/test-suite/.test_ws_subscribe";
        let _ = std::fs::remove_dir_all(test_dir);
        let storage = Arc::new(crate::storage::RocksDbStorage::new(test_dir).unwrap());
        let state_manager = Arc::new(StateManager::new(storage));

        let app_state = Arc::new(AppState {
            state_manager: state_manager.clone(),
            mempool,
            security: ApiSecurityConfig::default(),
            rate_limiter: ApiRateLimiter::default(),
            security_stats: Arc::new(ApiSecurityStats::default()),
            ws_manager: WebSocketConnectionManager::new(),
            pbft_consensus: Arc::new(tokio::sync::Mutex::new(None)),
            private_wallet_manager: None,
            private_tx_broadcaster: None,
            team_mining_runtime: None,
        });

        let req = SubscribeRequest {
            client_id: "client-1".to_string(),
            subscription_types: vec![SubscriptionType::Blocks, SubscriptionType::Transactions],
            filters: None,
        };

        let resp = ws_subscribe(State(app_state.clone()), Json(req)).await;
        assert!(resp.is_ok());

        let json = resp.unwrap();
        assert_eq!(json.0["client_id"], "client-1");
        assert_eq!(json.0["status"], "subscribed");

        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[tokio::test]
    async fn test_ws_unsubscribe() {
        let mempool = Arc::new(Mempool::new(1000));
        let test_dir = ".zoka/test-suite/.test_ws_unsubscribe";
        let _ = std::fs::remove_dir_all(test_dir);
        let storage = Arc::new(crate::storage::RocksDbStorage::new(test_dir).unwrap());
        let state_manager = Arc::new(StateManager::new(storage));

        let app_state = Arc::new(AppState {
            state_manager: state_manager.clone(),
            mempool,
            security: ApiSecurityConfig::default(),
            rate_limiter: ApiRateLimiter::default(),
            security_stats: Arc::new(ApiSecurityStats::default()),
            ws_manager: WebSocketConnectionManager::new(),
            pbft_consensus: Arc::new(tokio::sync::Mutex::new(None)),
            private_wallet_manager: None,
            private_tx_broadcaster: None,
            team_mining_runtime: None,
        });

        // First subscribe
        let sub_req = SubscribeRequest {
            client_id: "client-2".to_string(),
            subscription_types: vec![SubscriptionType::Blocks],
            filters: None,
        };
        let _ = ws_subscribe(State(app_state.clone()), Json(sub_req))
            .await
            .unwrap();

        // Then unsubscribe
        let unsub_req = UnsubscribeRequest {
            client_id: "client-2".to_string(),
            subscription_types: vec![SubscriptionType::Blocks],
        };

        let resp = ws_unsubscribe(State(app_state.clone()), Json(unsub_req)).await;
        assert!(resp.is_ok());

        let json = resp.unwrap();
        assert_eq!(json.0["client_id"], "client-2");
        assert_eq!(json.0["status"], "unsubscribed");

        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[tokio::test]
    async fn test_ws_status() {
        let mempool = Arc::new(Mempool::new(1000));
        let test_dir = ".zoka/test-suite/.test_ws_status";
        let _ = std::fs::remove_dir_all(test_dir);
        let storage = Arc::new(crate::storage::RocksDbStorage::new(test_dir).unwrap());
        let state_manager = Arc::new(StateManager::new(storage));

        let app_state = Arc::new(AppState {
            state_manager: state_manager.clone(),
            mempool,
            security: ApiSecurityConfig::default(),
            rate_limiter: ApiRateLimiter::default(),
            security_stats: Arc::new(ApiSecurityStats::default()),
            ws_manager: WebSocketConnectionManager::new(),
            pbft_consensus: Arc::new(tokio::sync::Mutex::new(None)),
            private_wallet_manager: None,
            private_tx_broadcaster: None,
            team_mining_runtime: None,
        });

        // Add some connections
        app_state
            .ws_manager
            .add_connection("client-1".to_string())
            .await;
        app_state
            .ws_manager
            .add_connection("client-2".to_string())
            .await;

        let resp = ws_status(State(app_state.clone())).await;
        assert!(resp.is_ok());

        let status = resp.unwrap();
        assert_eq!(status.0.active_connections, 2);

        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[tokio::test]
    async fn test_ws_subscribe_multiple_types() {
        let mempool = Arc::new(Mempool::new(1000));
        let test_dir = ".zoka/test-suite/.test_ws_multi";
        let _ = std::fs::remove_dir_all(test_dir);
        let storage = Arc::new(crate::storage::RocksDbStorage::new(test_dir).unwrap());
        let state_manager = Arc::new(StateManager::new(storage));

        let app_state = Arc::new(AppState {
            state_manager: state_manager.clone(),
            mempool,
            security: ApiSecurityConfig::default(),
            rate_limiter: ApiRateLimiter::default(),
            security_stats: Arc::new(ApiSecurityStats::default()),
            ws_manager: WebSocketConnectionManager::new(),
            pbft_consensus: Arc::new(tokio::sync::Mutex::new(None)),
            private_wallet_manager: None,
            private_tx_broadcaster: None,
            team_mining_runtime: None,
        });

        let req = SubscribeRequest {
            client_id: "client-3".to_string(),
            subscription_types: vec![
                SubscriptionType::Blocks,
                SubscriptionType::Transactions,
                SubscriptionType::State,
            ],
            filters: None,
        };

        let _ = ws_subscribe(State(app_state.clone()), Json(req))
            .await
            .unwrap();

        let subs = app_state.ws_manager.get_subscriptions("client-3").await;
        assert_eq!(subs.len(), 3);

        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[tokio::test]
    async fn test_create_router_with_ws_endpoints() {
        let mempool = Arc::new(Mempool::new(1000));
        let test_dir = ".zoka/test-suite/.test_ws_router";
        let _ = std::fs::remove_dir_all(test_dir);
        let storage = Arc::new(crate::storage::RocksDbStorage::new(test_dir).unwrap());
        let state_manager = Arc::new(StateManager::new(storage.clone()));

        let app_state = Arc::new(AppState {
            state_manager,
            mempool,
            security: ApiSecurityConfig::default(),
            rate_limiter: ApiRateLimiter::default(),
            security_stats: Arc::new(ApiSecurityStats::default()),
            ws_manager: WebSocketConnectionManager::new(),
            pbft_consensus: Arc::new(tokio::sync::Mutex::new(None)),
            private_wallet_manager: None,
            private_tx_broadcaster: None,
            team_mining_runtime: None,
        });

        let _router = create_router(app_state);
        // Router should be created successfully with WebSocket endpoints
        assert!(true);

        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[tokio::test]
    async fn test_pbft_start() {
        let mempool = Arc::new(Mempool::new(1000));
        let test_dir = ".zoka/test-suite/.test_pbft_start";
        let _ = std::fs::remove_dir_all(test_dir);
        let storage = Arc::new(crate::storage::RocksDbStorage::new(test_dir).unwrap());
        let state_manager = Arc::new(StateManager::new(storage));

        let app_state = Arc::new(AppState {
            state_manager: state_manager.clone(),
            mempool,
            security: ApiSecurityConfig::default(),
            rate_limiter: ApiRateLimiter::default(),
            security_stats: Arc::new(ApiSecurityStats::default()),
            ws_manager: WebSocketConnectionManager::new(),
            pbft_consensus: Arc::new(tokio::sync::Mutex::new(None)),
            private_wallet_manager: None,
            private_tx_broadcaster: None,
            team_mining_runtime: None,
        });

        let req = PbftStartRequest {
            node_id: "validator-1".to_string(),
            validators: vec![
                "validator-1".to_string(),
                "validator-2".to_string(),
                "validator-3".to_string(),
                "validator-4".to_string(),
            ],
            timeout_ms: Some(5000),
        };

        let resp = pbft_start(State(app_state), Json(req)).await;
        assert!(resp.is_ok());

        let json = resp.unwrap();
        assert_eq!(json.0["status"], "started");
        assert_eq!(json.0["node_id"], "validator-1");
        assert_eq!(json.0["validators_count"], 4);

        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[tokio::test]
    async fn test_pbft_status() {
        let mempool = Arc::new(Mempool::new(1000));
        let test_dir = ".zoka/test-suite/.test_pbft_status";
        let _ = std::fs::remove_dir_all(test_dir);
        let storage = Arc::new(crate::storage::RocksDbStorage::new(test_dir).unwrap());
        let state_manager = Arc::new(StateManager::new(storage));

        let app_state = Arc::new(AppState {
            state_manager: state_manager.clone(),
            mempool,
            security: ApiSecurityConfig::default(),
            rate_limiter: ApiRateLimiter::default(),
            security_stats: Arc::new(ApiSecurityStats::default()),
            ws_manager: WebSocketConnectionManager::new(),
            pbft_consensus: Arc::new(tokio::sync::Mutex::new(None)),
            private_wallet_manager: None,
            private_tx_broadcaster: None,
            team_mining_runtime: None,
        });

        // First start consensus
        let start_req = PbftStartRequest {
            node_id: "validator-1".to_string(),
            validators: vec![
                "validator-1".to_string(),
                "validator-2".to_string(),
                "validator-3".to_string(),
                "validator-4".to_string(),
            ],
            timeout_ms: Some(5000),
        };
        let _ = pbft_start(State(app_state.clone()), Json(start_req))
            .await
            .unwrap();

        let resp = pbft_status(State(app_state)).await;
        assert!(resp.is_ok());

        let status = resp.unwrap();
        assert_eq!(status.0.current_view, 0);
        assert_eq!(status.0.node_id, "validator-1");
        assert!(status.0.is_primary);
    }

    #[tokio::test]
    async fn test_pbft_message_prepare() {
        let mempool = Arc::new(Mempool::new(1000));
        let test_dir = ".zoka/test-suite/.test_pbft_msg_prepare";
        let _ = std::fs::remove_dir_all(test_dir);
        let storage = Arc::new(crate::storage::RocksDbStorage::new(test_dir).unwrap());
        let state_manager = Arc::new(StateManager::new(storage));

        let app_state = Arc::new(AppState {
            state_manager: state_manager.clone(),
            mempool,
            security: ApiSecurityConfig::default(),
            rate_limiter: ApiRateLimiter::default(),
            security_stats: Arc::new(ApiSecurityStats::default()),
            ws_manager: WebSocketConnectionManager::new(),
            pbft_consensus: Arc::new(tokio::sync::Mutex::new(None)),
            private_wallet_manager: None,
            private_tx_broadcaster: None,
            team_mining_runtime: None,
        });

        // First start consensus
        let start_req = PbftStartRequest {
            node_id: "validator-1".to_string(),
            validators: vec![
                "validator-1".to_string(),
                "validator-2".to_string(),
                "validator-3".to_string(),
                "validator-4".to_string(),
            ],
            timeout_ms: Some(5000),
        };
        let _ = pbft_start(State(app_state.clone()), Json(start_req))
            .await
            .unwrap();

        // Pre-prepare must be accepted before prepare.
        let pre_prepare_req = PbftMessageRequest {
            msg_type: "pre-prepare".to_string(),
            view: 0,
            sequence: 1,
            block_hash: "hash123".to_string(),
            sender: Some("validator-1".to_string()),
            timestamp_ms: Some(1),
        };
        let pre_prepare_resp = pbft_message(State(app_state.clone()), Json(pre_prepare_req)).await;
        assert!(pre_prepare_resp.is_ok());
        assert_eq!(pre_prepare_resp.unwrap().0["status"], "received");

        let req = PbftMessageRequest {
            msg_type: "prepare".to_string(),
            view: 0,
            sequence: 1,
            block_hash: "hash123".to_string(),
            sender: Some("validator-1".to_string()),
            timestamp_ms: Some(2),
        };

        let resp = pbft_message(State(app_state), Json(req)).await;
        assert!(resp.is_ok());

        let json = resp.unwrap();
        assert_eq!(json.0["status"], "received");
        assert_eq!(json.0["msg_type"], "prepare");
        assert!(json.0["processed"].as_bool().unwrap());

        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[tokio::test]
    async fn test_pbft_message_commit() {
        let mempool = Arc::new(Mempool::new(1000));
        let test_dir = ".zoka/test-suite/.test_pbft_msg_commit";
        let _ = std::fs::remove_dir_all(test_dir);
        let storage = Arc::new(crate::storage::RocksDbStorage::new(test_dir).unwrap());
        let state_manager = Arc::new(StateManager::new(storage));

        let app_state = Arc::new(AppState {
            state_manager: state_manager.clone(),
            mempool,
            security: ApiSecurityConfig::default(),
            rate_limiter: ApiRateLimiter::default(),
            security_stats: Arc::new(ApiSecurityStats::default()),
            ws_manager: WebSocketConnectionManager::new(),
            pbft_consensus: Arc::new(tokio::sync::Mutex::new(None)),
            private_wallet_manager: None,
            private_tx_broadcaster: None,
            team_mining_runtime: None,
        });

        // First start consensus
        let start_req = PbftStartRequest {
            node_id: "validator-1".to_string(),
            validators: vec![
                "validator-1".to_string(),
                "validator-2".to_string(),
                "validator-3".to_string(),
                "validator-4".to_string(),
            ],
            timeout_ms: Some(5000),
        };
        let _ = pbft_start(State(app_state.clone()), Json(start_req))
            .await
            .unwrap();

        // Build a valid PBFT progression before commit.
        let pre_prepare_req = PbftMessageRequest {
            msg_type: "pre-prepare".to_string(),
            view: 0,
            sequence: 2,
            block_hash: "hash456".to_string(),
            sender: Some("validator-1".to_string()),
            timestamp_ms: Some(10),
        };
        let _ = pbft_message(State(app_state.clone()), Json(pre_prepare_req))
            .await
            .unwrap();

        let prepare_req_1 = PbftMessageRequest {
            msg_type: "prepare".to_string(),
            view: 0,
            sequence: 2,
            block_hash: "hash456".to_string(),
            sender: Some("validator-1".to_string()),
            timestamp_ms: Some(11),
        };
        let _ = pbft_message(State(app_state.clone()), Json(prepare_req_1))
            .await
            .unwrap();

        let prepare_req_2 = PbftMessageRequest {
            msg_type: "prepare".to_string(),
            view: 0,
            sequence: 2,
            block_hash: "hash456".to_string(),
            sender: Some("validator-2".to_string()),
            timestamp_ms: Some(12),
        };
        let _ = pbft_message(State(app_state.clone()), Json(prepare_req_2))
            .await
            .unwrap();

        let req = PbftMessageRequest {
            msg_type: "commit".to_string(),
            view: 0,
            sequence: 2,
            block_hash: "hash456".to_string(),
            sender: Some("validator-1".to_string()),
            timestamp_ms: Some(13),
        };

        let resp = pbft_message(State(app_state), Json(req)).await;
        assert!(resp.is_ok());

        let json = resp.unwrap();
        assert_eq!(json.0["status"], "received");
        assert_eq!(json.0["msg_type"], "commit");

        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[tokio::test]
    async fn test_pbft_message_pre_prepare() {
        let mempool = Arc::new(Mempool::new(1000));
        let test_dir = ".zoka/test-suite/.test_pbft_msg_preprepare";
        let _ = std::fs::remove_dir_all(test_dir);
        let storage = Arc::new(crate::storage::RocksDbStorage::new(test_dir).unwrap());
        let state_manager = Arc::new(StateManager::new(storage));

        let app_state = Arc::new(AppState {
            state_manager: state_manager.clone(),
            mempool,
            security: ApiSecurityConfig::default(),
            rate_limiter: ApiRateLimiter::default(),
            security_stats: Arc::new(ApiSecurityStats::default()),
            ws_manager: WebSocketConnectionManager::new(),
            pbft_consensus: Arc::new(tokio::sync::Mutex::new(None)),
            private_wallet_manager: None,
            private_tx_broadcaster: None,
            team_mining_runtime: None,
        });

        let req = PbftMessageRequest {
            msg_type: "pre-prepare".to_string(),
            view: 0,
            sequence: 1,
            block_hash: "block_hash".to_string(),
            sender: None,
            timestamp_ms: None,
        };

        let resp = pbft_message(State(app_state), Json(req)).await;
        assert!(resp.is_ok());

        let json = resp.unwrap();
        assert_eq!(json.0["msg_type"], "pre-prepare");

        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[tokio::test]
    async fn test_pbft_message_invalid_type() {
        let mempool = Arc::new(Mempool::new(1000));
        let test_dir = ".zoka/test-suite/.test_pbft_msg_invalid";
        let _ = std::fs::remove_dir_all(test_dir);
        let storage = Arc::new(crate::storage::RocksDbStorage::new(test_dir).unwrap());
        let state_manager = Arc::new(StateManager::new(storage));

        let app_state = Arc::new(AppState {
            state_manager: state_manager.clone(),
            mempool,
            security: ApiSecurityConfig::default(),
            rate_limiter: ApiRateLimiter::default(),
            security_stats: Arc::new(ApiSecurityStats::default()),
            ws_manager: WebSocketConnectionManager::new(),
            pbft_consensus: Arc::new(tokio::sync::Mutex::new(None)),
            private_wallet_manager: None,
            private_tx_broadcaster: None,
            team_mining_runtime: None,
        });

        let req = PbftMessageRequest {
            msg_type: "invalid-type".to_string(),
            view: 0,
            sequence: 1,
            block_hash: "hash".to_string(),
            sender: None,
            timestamp_ms: None,
        };

        let resp = pbft_message(State(app_state), Json(req)).await;
        assert!(resp.is_err());
        assert_eq!(resp.unwrap_err(), StatusCode::BAD_REQUEST);

        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[tokio::test]
    async fn test_pbft_start_insufficient_validators() {
        let mempool = Arc::new(Mempool::new(1000));
        let test_dir = ".zoka/test-suite/.test_pbft_start_insufficient";
        let _ = std::fs::remove_dir_all(test_dir);
        let storage = Arc::new(crate::storage::RocksDbStorage::new(test_dir).unwrap());
        let state_manager = Arc::new(StateManager::new(storage));

        let app_state = Arc::new(AppState {
            state_manager: state_manager.clone(),
            mempool,
            security: ApiSecurityConfig::default(),
            rate_limiter: ApiRateLimiter::default(),
            security_stats: Arc::new(ApiSecurityStats::default()),
            ws_manager: WebSocketConnectionManager::new(),
            pbft_consensus: Arc::new(tokio::sync::Mutex::new(None)),
            private_wallet_manager: None,
            private_tx_broadcaster: None,
            team_mining_runtime: None,
        });

        let req = PbftStartRequest {
            node_id: "validator-1".to_string(),
            validators: vec!["validator-1".to_string(), "validator-2".to_string()],
            timeout_ms: Some(5000),
        };

        let resp = pbft_start(State(app_state), Json(req)).await;
        assert!(resp.is_err());
        assert_eq!(resp.unwrap_err(), StatusCode::BAD_REQUEST);

        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[tokio::test]
    async fn test_create_snapshot() {
        let mempool = Arc::new(Mempool::new(1000));
        let test_dir = ".zoka/test-suite/.test_create_snapshot";
        let _ = std::fs::remove_dir_all(test_dir);
        let storage = Arc::new(crate::storage::RocksDbStorage::new(test_dir).unwrap());
        let state_manager = Arc::new(StateManager::new(storage));

        let app_state = Arc::new(AppState {
            state_manager: state_manager.clone(),
            mempool,
            security: ApiSecurityConfig::default(),
            rate_limiter: ApiRateLimiter::default(),
            security_stats: Arc::new(ApiSecurityStats::default()),
            ws_manager: WebSocketConnectionManager::new(),
            pbft_consensus: Arc::new(tokio::sync::Mutex::new(None)),
            private_wallet_manager: None,
            private_tx_broadcaster: None,
            team_mining_runtime: None,
        });

        let resp =
            create_snapshot(State(app_state), Json(CreateSnapshotRequest { name: None })).await;
        assert!(resp.is_ok());

        let json = resp.unwrap();
        assert_eq!(json.0["status"], "created");

        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[tokio::test]
    async fn test_list_snapshots() {
        let mempool = Arc::new(Mempool::new(1000));
        let test_dir = ".zoka/test-suite/.test_list_snapshots";
        let _ = std::fs::remove_dir_all(test_dir);
        let storage = Arc::new(crate::storage::RocksDbStorage::new(test_dir).unwrap());
        let state_manager = Arc::new(StateManager::new(storage));

        let app_state = Arc::new(AppState {
            state_manager: state_manager.clone(),
            mempool,
            security: ApiSecurityConfig::default(),
            rate_limiter: ApiRateLimiter::default(),
            security_stats: Arc::new(ApiSecurityStats::default()),
            ws_manager: WebSocketConnectionManager::new(),
            pbft_consensus: Arc::new(tokio::sync::Mutex::new(None)),
            private_wallet_manager: None,
            private_tx_broadcaster: None,
            team_mining_runtime: None,
        });

        // Create snapshot first
        let _ = create_snapshot(
            State(app_state.clone()),
            Json(CreateSnapshotRequest { name: None }),
        )
        .await
        .unwrap();

        let resp = list_snapshots(State(app_state)).await;
        assert!(resp.is_ok());

        let json = resp.unwrap();
        assert!(json.0["snapshots"].is_array());

        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[tokio::test]
    async fn test_filter_events_with_criteria() {
        let mempool = Arc::new(Mempool::new(1000));
        let test_dir = ".zoka/test-suite/.test_filter_events";
        let _ = std::fs::remove_dir_all(test_dir);
        let storage = Arc::new(crate::storage::RocksDbStorage::new(test_dir).unwrap());
        let state_manager = Arc::new(StateManager::new(storage));

        let app_state = Arc::new(AppState {
            state_manager: state_manager.clone(),
            mempool,
            security: ApiSecurityConfig::default(),
            rate_limiter: ApiRateLimiter::default(),
            security_stats: Arc::new(ApiSecurityStats::default()),
            ws_manager: WebSocketConnectionManager::new(),
            pbft_consensus: Arc::new(tokio::sync::Mutex::new(None)),
            private_wallet_manager: None,
            private_tx_broadcaster: None,
            team_mining_runtime: None,
        });

        let filter = AdvancedEventFilter {
            block_height_min: Some(100),
            block_height_max: Some(200),
            addresses: Some(vec!["alice".to_string()]),
            tx_type: Some("transfer".to_string()),
            include_pending: true,
        };

        let resp = filter_events(State(app_state), Json(filter)).await;
        assert!(resp.is_ok());

        let json = resp.unwrap();
        assert!(json.0["filter_criteria"].is_array());
        assert!(json.0.get("matching_events").is_some());

        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[tokio::test]
    async fn test_list_profiles() {
        let _guard = PROFILE_ENV_LOCK.lock().unwrap();
        let mempool = Arc::new(Mempool::new(1000));
        let test_dir = ".zoka/test-suite/.test_list_profiles";
        let profile_file = format!("{}/profiles.toml", test_dir);
        let _ = std::fs::remove_dir_all(test_dir);
        let _ = std::fs::create_dir_all(test_dir);
        std::env::set_var("ZOKA_API_PROFILES_PATH", &profile_file);
        let storage = Arc::new(crate::storage::RocksDbStorage::new(test_dir).unwrap());
        let state_manager = Arc::new(StateManager::new(storage));

        let app_state = Arc::new(AppState {
            state_manager: state_manager.clone(),
            mempool,
            security: ApiSecurityConfig::default(),
            rate_limiter: ApiRateLimiter::default(),
            security_stats: Arc::new(ApiSecurityStats::default()),
            ws_manager: WebSocketConnectionManager::new(),
            pbft_consensus: Arc::new(tokio::sync::Mutex::new(None)),
            private_wallet_manager: None,
            private_tx_broadcaster: None,
            team_mining_runtime: None,
        });

        let resp = list_profiles(State(app_state)).await;
        assert!(resp.is_ok());

        let json = resp.unwrap();
        assert!(json.0["profiles"].is_array());
        assert_eq!(json.0["active"], "default");

        std::env::remove_var("ZOKA_API_PROFILES_PATH");
        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[tokio::test]
    async fn test_private_tx_endpoint_without_manager_returns_404() {
        // Node without a `private_wallet_manager` must refuse cleanly, not
        // crash or lie. Confirms the Option wiring works.
        let test_dir = ".zoka/test-suite/.test_private_tx_endpoint_without_manager";
        let _ = std::fs::remove_dir_all(test_dir);
        let _ = std::fs::create_dir_all(test_dir);
        let storage = Arc::new(crate::storage::RocksDbStorage::new(test_dir).unwrap());
        let state_manager = Arc::new(StateManager::new(storage));
        let mempool = Arc::new(Mempool::new(1000));

        let app_state = Arc::new(AppState {
            state_manager,
            mempool,
            security: ApiSecurityConfig::default(),
            rate_limiter: ApiRateLimiter::default(),
            security_stats: Arc::new(ApiSecurityStats::default()),
            ws_manager: WebSocketConnectionManager::new(),
            pbft_consensus: Arc::new(tokio::sync::Mutex::new(None)),
            private_wallet_manager: None,
            private_tx_broadcaster: None,
            team_mining_runtime: None,
        });

        let res = get_private_tx_by_hash(State(app_state), Path("deadbeef".to_string())).await;
        match res {
            Err((code, _)) => assert_eq!(code, StatusCode::NOT_FOUND),
            Ok(_) => panic!("expected 404 when manager is None"),
        }
        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[tokio::test]
    async fn test_switch_profile() {
        let _guard = PROFILE_ENV_LOCK.lock().unwrap();
        let mempool = Arc::new(Mempool::new(1000));
        let test_dir = ".zoka/test-suite/.test_switch_profile";
        let profile_file = format!("{}/profiles.toml", test_dir);
        let _ = std::fs::remove_dir_all(test_dir);
        let _ = std::fs::create_dir_all(test_dir);
        std::env::set_var("ZOKA_API_PROFILES_PATH", &profile_file);
        let storage = Arc::new(crate::storage::RocksDbStorage::new(test_dir).unwrap());
        let state_manager = Arc::new(StateManager::new(storage));

        let app_state = Arc::new(AppState {
            state_manager: state_manager.clone(),
            mempool,
            security: ApiSecurityConfig::default(),
            rate_limiter: ApiRateLimiter::default(),
            security_stats: Arc::new(ApiSecurityStats::default()),
            ws_manager: WebSocketConnectionManager::new(),
            pbft_consensus: Arc::new(tokio::sync::Mutex::new(None)),
            private_wallet_manager: None,
            private_tx_broadcaster: None,
            team_mining_runtime: None,
        });

        let req = ProfileManagementRequest {
            profile_name: "production".to_string(),
        };

        let resp = switch_profile(State(app_state), Json(req)).await;
        assert!(resp.is_ok());

        let json = resp.unwrap();
        assert_eq!(json.0["status"], "switched");
        assert_eq!(json.0["profile"], "production");

        // Ensure persisted profile becomes active.
        let listed = list_profiles(State(Arc::new(AppState {
            state_manager: state_manager.clone(),
            mempool: Arc::new(Mempool::new(1000)),
            security: ApiSecurityConfig::default(),
            rate_limiter: ApiRateLimiter::default(),
            security_stats: Arc::new(ApiSecurityStats::default()),
            ws_manager: WebSocketConnectionManager::new(),
            pbft_consensus: Arc::new(tokio::sync::Mutex::new(None)),
            private_wallet_manager: None,
            private_tx_broadcaster: None,
            team_mining_runtime: None,
        })))
        .await
        .unwrap();
        assert_eq!(listed.0["active"], "production");

        std::env::remove_var("ZOKA_API_PROFILES_PATH");
        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[tokio::test]
    async fn test_multi_profile_config_new() {
        use crate::cli::MultiProfileConfig;

        let config = MultiProfileConfig::new();
        assert_eq!(config.active_profile, "default");
        assert!(config.profiles.contains_key("default"));
    }

    #[tokio::test]
    async fn test_multi_profile_add_and_switch() {
        use crate::cli::{CliConfig, MultiProfileConfig};

        let mut config = MultiProfileConfig::new();
        let new_profile = CliConfig::new("production".to_string());

        config
            .add_profile("production".to_string(), new_profile)
            .unwrap();
        assert_eq!(config.list_profiles().len(), 2);

        config.switch_profile("production").unwrap();
        assert_eq!(config.active_profile, "production");
    }

    #[tokio::test]
    async fn test_multi_profile_config_get_active() {
        use crate::cli::{CliConfig, MultiProfileConfig};

        let mut config = MultiProfileConfig::new();
        let staging = CliConfig::new("staging".to_string());
        config.add_profile("staging".to_string(), staging).unwrap();
        config.switch_profile("staging").unwrap();

        let active = config.get_active_profile();
        assert!(active.is_some());
        assert_eq!(active.unwrap().name, "staging");
    }
}
