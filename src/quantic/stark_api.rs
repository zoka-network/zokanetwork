//! Bounded loopback API for a STARK v2 chain (see `stark::Profile`).
//!
//! API, peer synchronization and mining share `NodeState`. Every state mutation
//! takes `work` asynchronously, moves its permit into `spawn_blocking`, then
//! takes `gate` before touching storage or the mempool. Client cancellation
//! therefore cannot leave an unaccounted cryptographic worker running.
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Extension, Path, Query, Request, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::stark::{
    self,
    Profile,
    protocol::{
        ledger::{subsidy, ATOMS, MAX_SUPPLY, PREMINE},
        scan::{ScanBlock, ScanPage, ScanTransaction, MAX_SCAN_BLOCKS},
        transaction::{Transaction, MAX_TRANSACTION_BYTES},
        wallet::{Address, ShortAddress},
    },
};
use super::difficulty;
use super::stark_directory::{self, AddressRecord, Directory, DirectoryError};
use super::stark_peers::{PeerBook, Peers};
use crate::{
    checkpoint_authority::{self, Acceptance, AuthorityKeys, SignedCheckpoint},
    mining::pow,
    state::chain::{apply_block, ChainError},
    storage::RocksDbStorage,
    types::Block,
};

pub const DB_MARKER: &str = "quantic:stark:chain-id";
// Hex-encoded transaction bytes plus bounded header/nullifier JSON overhead.
pub const MAX_BLOCK_REQUEST_BYTES: usize =
    2 * stark::protocol::ledger::MAX_BLOCK_BYTES + 1024 * 1024;
pub const MAX_CRYPTO_WORKERS: usize = 2;
/// A hybrid checkpoint is about 7 KB of JSON (the ML-DSA half is 6,618 hex).
pub const MAX_CHECKPOINT_BYTES: usize = 16 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
/// Summaries per `/blocks/recent` request.
pub const MAX_RECENT_BLOCKS: usize = 25;
/// The public API's own workers: summaries decode blocks, nothing proves.
const PUBLIC_WORKERS: usize = 2;
/// Requests a second across every public client, and the burst above it.
const PUBLIC_RATE: f64 = 20.0;
const PUBLIC_BURST: f64 = 40.0;
/// How long a public request waits for a worker, and runs at most.
const PUBLIC_QUEUE: Duration = Duration::from_secs(5);
const PUBLIC_TIMEOUT: Duration = Duration::from_secs(20);
/// One day of blocks at the ideal pace: what a chart asks for at most.
const MAX_STATS_BLOCKS: u64 = 720;
const HISTORY_KEY: &str = "quantic:stark:network-history";
/// A sample of the network every five minutes, for one day.
pub const HISTORY_EVERY: Duration = Duration::from_secs(300);
const HISTORY_SAMPLES: usize = 288;

#[derive(Clone)]
pub struct NodeState {
    /// The chain named by the database marker.
    pub profile: Profile,
    pub storage: Arc<RocksDbStorage>,
    pub mempool: Arc<Mutex<stark::Mempool>>,
    pub gate: Arc<Mutex<()>>,
    pub work: Arc<Semaphore>,
    /// Peer table and session counts; its lock is independent of `gate`.
    pub peers: Arc<Peers>,
    /// Keys that sign this chain's checkpoints: the compiled ones, or the
    /// environment's on a local test network. None: only work decides.
    pub authority: Option<AuthorityKeys>,
    /// Short-address lookups and relay; its locks are independent of `gate`.
    pub directory: Arc<Directory>,
}

impl NodeState {
    /// Callers initialize or adopt the dedicated v2 database explicitly. This
    /// constructor cannot silently repurpose a public or unmarked database.
    pub fn new(storage: Arc<RocksDbStorage>) -> Result<Self, String> {
        let profile = storage
            .get_raw(DB_MARKER)
            .map_err(|e| e.to_string())?
            .and_then(|marker| String::from_utf8(marker).ok())
            .and_then(|marker| Profile::for_chain(&marker))
            .ok_or("STARK API requires a database marked with a STARK v2 chain id")?;
        let parent = current_tip(profile, &storage).map_err(|e| e.message)?;
        stark::branch(&storage, profile, &parent)?;
        let peers = Peers::new(PeerBook::load(&storage, profile));
        Ok(Self {
            profile,
            storage,
            mempool: Arc::new(Mutex::new(stark::Mempool::default())),
            gate: Arc::new(Mutex::new(())),
            work: Arc::new(Semaphore::new(MAX_CRYPTO_WORKERS)),
            peers,
            authority: checkpoint_authority::authority_keys_for_chain(profile.chain_id),
            directory: Arc::new(Directory::default()),
        })
    }

    /// Verify a checkpoint against this chain's authority and keep it if it is
    /// the newest. `state::chain` then enforces it. Hold `gate`.
    pub fn accept_checkpoint(&self, checkpoint: &SignedCheckpoint) -> Result<Acceptance, String> {
        let keys = self
            .authority
            .as_ref()
            .ok_or_else(|| format!("{} has no checkpoint authority", self.profile.chain_id))?;
        checkpoint_authority::accept_signed_checkpoint(
            &self.storage,
            self.profile.chain_id,
            checkpoint,
            keys,
        )
        .map_err(|e| e.to_string())
    }

    /// The latest accepted checkpoint. Hold `gate`.
    pub fn latest_checkpoint(&self) -> Result<Option<SignedCheckpoint>, String> {
        checkpoint_authority::latest_signed_checkpoint(&self.storage).map_err(|e| e.to_string())
    }

    /// Whether the canonical chain holds `checkpoint` (false while it is
    /// above the tip or on another branch). Hold `gate`.
    pub fn follows(&self, checkpoint: &SignedCheckpoint) -> Result<bool, String> {
        Ok(self
            .storage
            .get_canonical_block_hash_at_height(checkpoint.height)
            .map_err(|e| e.to_string())?
            .is_some_and(|hash| hash == checkpoint.block_hash))
    }

    /// The canonical tip (the genesis parent of an empty chain). Hold `gate`.
    pub fn tip(&self) -> Result<Block, String> {
        current_tip(self.profile, &self.storage).map_err(|e| e.message)
    }
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
}
impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
    fn internal(error: impl std::fmt::Display) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
    }
    fn invalid(error: impl std::fmt::Display) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, error.to_string())
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            axum::Json(serde_json::json!({"error":self.message})),
        )
            .into_response()
    }
}
type ApiResult<T> = Result<T, ApiError>;

#[derive(Clone)]
struct WorkPermit(Arc<Mutex<Option<OwnedSemaphorePermit>>>);

fn loopback_host(host: &str) -> bool {
    let Ok(authority) = host.parse::<axum::http::uri::Authority>() else {
        return false;
    };
    let host = authority
        .host()
        .trim_start_matches('[')
        .trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// Restrict browser requests to the local origin. No CORS permission is added.
/// Raw CLI clients may omit Origin; a supplied Host must still be loopback.
fn local_origin(headers: &HeaderMap) -> bool {
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
    if headers.contains_key(header::HOST) && !host.is_some_and(loopback_host) {
        return false;
    }
    if headers
        .get("sec-fetch-site")
        .is_some_and(|v| v == "cross-site")
    {
        return false;
    }
    if let Some(origin) = headers.get(header::ORIGIN) {
        let Some(host) = host else { return false };
        let Ok(origin) = origin.to_str() else {
            return false;
        };
        let Ok(uri) = origin.parse::<axum::http::Uri>() else {
            return false;
        };
        return uri.scheme_str() == Some("http")
            && uri
                .authority()
                .is_some_and(|a| a.as_str().eq_ignore_ascii_case(host));
    }
    true
}

async fn admit(State(state): State<NodeState>, mut request: Request, next: Next) -> Response {
    if !local_origin(request.headers()) {
        return ApiError::new(
            StatusCode::FORBIDDEN,
            "the STARK API accepts local-origin requests only",
        )
        .into_response();
    }
    let limit = match request.uri().path() {
        "/transactions" => MAX_TRANSACTION_BYTES,
        "/blocks" => MAX_BLOCK_REQUEST_BYTES,
        "/checkpoint" => MAX_CHECKPOINT_BYTES,
        "/address" => stark_directory::MAX_RECORD_BYTES,
        _ => 0,
    };
    if request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .is_some_and(|size| size > limit as u64)
    {
        return ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "request body exceeds the endpoint limit",
        )
        .into_response();
    }
    let Ok(permit) = state.work.clone().try_acquire_owned() else {
        return ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "STARK worker limit reached; retry later",
        )
        .into_response();
    };
    // Admission happens before Bytes/JSON extraction, so concurrent large
    // uploads cannot allocate an unlimited number of full request bodies.
    request
        .extensions_mut()
        .insert(WorkPermit(Arc::new(Mutex::new(Some(permit)))));
    let mut response = match tokio::time::timeout(REQUEST_TIMEOUT, next.run(request)).await {
        Ok(response) => response,
        Err(_) => {
            ApiError::new(StatusCode::REQUEST_TIMEOUT, "request deadline exceeded").into_response()
        }
    };
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        "no-store".parse().expect("static header"),
    );
    response
}

async fn blocking<T: Send + 'static>(
    permit: WorkPermit,
    job: impl FnOnce() -> ApiResult<T> + Send + 'static,
) -> ApiResult<T> {
    let owned = permit
        .0
        .lock()
        .map_err(|_| ApiError::internal("worker permit lock poisoned"))?
        .take()
        .ok_or_else(|| ApiError::internal("worker permit already consumed"))?;
    tokio::task::spawn_blocking(move || {
        let _permit = owned;
        job()
    })
    .await
    .map_err(ApiError::internal)?
}

fn json_response<T: Serialize>(status: StatusCode, value: &T) -> ApiResult<Response> {
    let bytes = serde_json::to_vec(value).map_err(ApiError::internal)?;
    Ok((status, [(header::CONTENT_TYPE, "application/json")], bytes).into_response())
}

fn current_tip(profile: Profile, storage: &RocksDbStorage) -> ApiResult<Block> {
    if storage.get_chain_height().map_err(ApiError::internal)? == 0 {
        return Ok(profile.genesis_parent());
    }
    let hash = storage.get_last_block_hash().map_err(ApiError::internal)?;
    storage
        .get_block(&hash)
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::internal("canonical tip is missing"))
}

fn content_type(headers: &HeaderMap, expected: &str) -> ApiResult<()> {
    let actual = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(str::trim);
    if actual.is_some_and(|v| v.eq_ignore_ascii_case(expected)) {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            format!("expected Content-Type: {expected}"),
        ))
    }
}

pub fn router(state: NodeState) -> Router {
    Router::new()
        .route("/status", get(status))
        .route("/blocks/:height", get(block_at))
        .route("/blocks/:height/summary", get(block_summary))
        .route("/blocks/hash/:hash/summary", get(block_summary_by_hash))
        .route("/blocks/recent", get(recent_blocks))
        .route("/blocks/stats", get(block_stats))
        .route("/scan", get(scan_page))
        .route("/supply", get(supply))
        .route("/supply/total", get(supply_total))
        .route("/network/history", get(network_history))
        .route(
            "/transactions",
            post(submit_transaction).layer(DefaultBodyLimit::max(MAX_TRANSACTION_BYTES)),
        )
        .route("/mining/template", get(mining_template))
        .route(
            "/blocks",
            post(submit_block).layer(DefaultBodyLimit::max(MAX_BLOCK_REQUEST_BYTES)),
        )
        .route(
            "/checkpoint",
            get(latest_checkpoint)
                .post(submit_checkpoint)
                .layer(DefaultBodyLimit::max(MAX_CHECKPOINT_BYTES)),
        )
        .route(
            "/address",
            post(publish_address).layer(DefaultBodyLimit::max(stark_directory::MAX_RECORD_BYTES)),
        )
        .route("/address/:short", get(address_of))
        .layer(middleware::from_fn_with_state(state.clone(), admit))
        .with_state(state)
}

pub async fn serve(state: NodeState, bind: SocketAddr) -> Result<(), String> {
    if !bind.ip().is_loopback() {
        return Err("the STARK API bind address must be loopback".into());
    }
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|e| e.to_string())?;
    axum::serve(listener, router(state))
        .await
        .map_err(|e| e.to_string())
}

/// What the public API admits: every client arrives through the tunnel from
/// loopback, so the rate is global, and the requests take turns on permits of
/// their own (never the wallet's crypto workers).
#[derive(Clone)]
pub struct PublicGate {
    permits: Arc<Semaphore>,
    bucket: Arc<Mutex<(f64, std::time::Instant)>>,
}

impl PublicGate {
    pub fn new() -> Self {
        Self {
            permits: Arc::new(Semaphore::new(PUBLIC_WORKERS)),
            bucket: Arc::new(Mutex::new((PUBLIC_BURST, std::time::Instant::now()))),
        }
    }

    /// One token from the bucket, which refills at `PUBLIC_RATE` a second.
    fn take(&self) -> bool {
        let Ok(mut bucket) = self.bucket.lock() else {
            return false;
        };
        let now = std::time::Instant::now();
        let refilled = bucket.0 + now.duration_since(bucket.1).as_secs_f64() * PUBLIC_RATE;
        *bucket = (refilled.min(PUBLIC_BURST), now);
        if bucket.0 < 1.0 {
            return false;
        }
        bucket.0 -= 1.0;
        true
    }
}

impl Default for PublicGate {
    fn default() -> Self {
        Self::new()
    }
}

fn with_cors(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, HEAD, OPTIONS"),
    );
    response
}

async fn admit_public(State(gate): State<PublicGate>, mut request: Request, next: Next) -> Response {
    if request.method() == axum::http::Method::OPTIONS {
        return with_cors(StatusCode::NO_CONTENT.into_response());
    }
    if request.method() != axum::http::Method::GET && request.method() != axum::http::Method::HEAD {
        return with_cors(
            ApiError::new(StatusCode::METHOD_NOT_ALLOWED, "the public API is read-only").into_response(),
        );
    }
    if !gate.take() {
        let mut response = with_cors(
            ApiError::new(StatusCode::TOO_MANY_REQUESTS, "too many requests; retry in a second")
                .into_response(),
        );
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
        return response;
    }
    let permit = match tokio::time::timeout(PUBLIC_QUEUE, gate.permits.clone().acquire_owned()).await {
        Ok(Ok(permit)) => permit,
        _ => {
            return with_cors(
                ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "the explorer API is busy; retry later")
                    .into_response(),
            )
        }
    };
    request
        .extensions_mut()
        .insert(WorkPermit(Arc::new(Mutex::new(Some(permit)))));
    let mut response = match tokio::time::timeout(PUBLIC_TIMEOUT, next.run(request)).await {
        Ok(response) => response,
        Err(_) => ApiError::new(StatusCode::REQUEST_TIMEOUT, "request deadline exceeded").into_response(),
    };
    if !response.headers().contains_key(header::CACHE_CONTROL) {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=5"));
    }
    with_cors(response)
}

/// The explorer's read-only API (`--public-api`), for a public front such as
/// the tunnel behind api.zokascan.cc: GET only, any origin, its own permits
/// and rate. Nothing here sends, mines, signs or asks the peers.
pub fn public_router(state: NodeState) -> Router {
    Router::new()
        .route("/", get(public_index))
        .route("/status", get(public_status))
        .route("/blocks/recent", get(recent_blocks))
        .route("/blocks/stats", get(block_stats))
        .route("/blocks/:height/summary", get(block_summary))
        .route("/blocks/hash/:hash/summary", get(block_summary_by_hash))
        .route("/scan", get(scan_page))
        .route("/supply", get(supply))
        .route("/supply/total", get(supply_total))
        .route("/network/history", get(network_history))
        .route("/address/:short", get(public_address_of))
        .route("/checkpoint", get(latest_checkpoint))
        .layer(middleware::from_fn_with_state(PublicGate::new(), admit_public))
        .with_state(state)
}

/// The network as the public explorer shows it: the chain and its work, the
/// latest checkpoint and how many peers this node has, without this node's own
/// services (its Stratum, its authority keys, its wallets).
async fn public_status(
    State(state): State<NodeState>,
    Extension(permit): Extension<WorkPermit>,
) -> ApiResult<Response> {
    blocking(permit, move || {
        let _gate = state.gate.lock().map_err(|_| ApiError::internal("node gate poisoned"))?;
        let tip = current_tip(state.profile, &state.storage)?;
        let branch = stark::branch(&state.storage, state.profile, &tip).map_err(ApiError::internal)?;
        let ledger = branch.state();
        let count = state.mempool.lock().map_err(|_| ApiError::internal("mempool poisoned"))?.len();
        let next = child_target(&state, &tip)?;
        let checkpoint = state
            .latest_checkpoint()
            .map_err(ApiError::internal)?
            .map(|cp| serde_json::json!({"height":cp.height,"block_hash":cp.block_hash}));
        let peers = state.peers.status();
        let addresses = stark_directory::count(&state.storage).map_err(directory_error)?;
        json_response(StatusCode::OK, &serde_json::json!({
            "chain_id":state.profile.chain_id,"node_version":env!("CARGO_PKG_VERSION"),"pool_version":2,
            "height":ledger.height,"hash":tip.header.hash,"tip_timestamp":tip.header.timestamp,
            "issued_atoms":ledger.issued,"notes":ledger.notes,"mempool_transactions":count,
            "pow_algorithm":state.profile.pow_algorithm(),
            "target_block_secs":difficulty::IDEAL_BLOCK_SECS,
            "next_difficulty_bits":difficulty::bits_of_target(&next),
            "hashrate_estimate_hps":difficulty::network_hashrate_for_target(&next),
            "checkpoint":checkpoint,
            "peers":{"outbound":peers["outbound"],"inbound":peers["inbound"]},
            "directory":{"addresses":addresses},
        }))
    })
    .await
}

/// A short address from this node's directory only: a public request never
/// makes the node ask its peers.
async fn public_address_of(
    state: State<NodeState>,
    short: Path<String>,
    permit: Extension<WorkPermit>,
) -> ApiResult<Response> {
    address_of(
        state,
        short,
        Query(AddressQuery {
            local: Some("1".into()),
        }),
        permit,
    )
    .await
}

async fn status(
    State(state): State<NodeState>,
    Extension(permit): Extension<WorkPermit>,
) -> ApiResult<Response> {
    blocking(permit, move || {
        let _gate = state.gate.lock().map_err(|_| ApiError::internal("node gate poisoned"))?;
        let tip = current_tip(state.profile, &state.storage)?;
        let branch = stark::branch(&state.storage, state.profile, &tip).map_err(ApiError::internal)?;
        let ledger = branch.state();
        let count = state.mempool.lock().map_err(|_| ApiError::internal("mempool poisoned"))?.len();
        let checkpoint = match state.latest_checkpoint().map_err(ApiError::internal)? {
            Some(cp) => serde_json::json!({"height":cp.height,"block_hash":cp.block_hash,
                "followed":state.follows(&cp).map_err(ApiError::internal)?}),
            None => serde_json::Value::Null,
        };
        let addresses = stark_directory::count(&state.storage).map_err(directory_error)?;
        let published = stark_directory::local(&state.storage, state.profile.network)
            .map_err(directory_error)?
            .len();
        json_response(StatusCode::OK, &serde_json::json!({
            "chain_id":state.profile.chain_id,"pool_version":2,"height":ledger.height,"hash":tip.header.hash,
            "tip_timestamp":tip.header.timestamp,
            "issued_atoms":ledger.issued,"notes":ledger.notes,"mempool_transactions":count,
            "pow_algorithm":state.profile.pow_algorithm(),"xmrig_compatible":state.profile.xmrig_compatible(),
            "max_transaction_bytes":MAX_TRANSACTION_BYTES,"max_block_request_bytes":MAX_BLOCK_REQUEST_BYTES,
            "stratum":super::stratum::status(),"peers":state.peers.status(),
            "checkpoint":checkpoint,"checkpoint_authority":state.authority.is_some(),
            "directory":{"addresses":addresses,"published_here":published},
        }))
    }).await
}

async fn block_at(
    State(state): State<NodeState>,
    Path(height): Path<u64>,
    Extension(permit): Extension<WorkPermit>,
) -> ApiResult<Response> {
    blocking(permit, move || {
        let _gate = state
            .gate
            .lock()
            .map_err(|_| ApiError::internal("node gate poisoned"))?;
        let block = state
            .storage
            .get_canonical_block_by_height(height)
            .map_err(ApiError::internal)?
            .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "unknown block height"))?;
        json_response(StatusCode::OK, &block)
    })
    .await
}

/// A canonical block without its transaction bodies, which carry the proofs:
/// what an explorer lists. The coinbase's outputs are public, so is its total.
async fn block_summary(
    State(state): State<NodeState>,
    Path(height): Path<u64>,
    Extension(permit): Extension<WorkPermit>,
) -> ApiResult<Response> {
    blocking(permit, move || {
        let block = {
            let _gate = state.gate.lock().map_err(|_| ApiError::internal("node gate poisoned"))?;
            state
                .storage
                .get_canonical_block_by_height(height)
                .map_err(ApiError::internal)?
                .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "unknown block height"))?
        };
        summary_response(&state, &block)
    })
    .await
}

/// A stored block by its hash, canonical or on a side branch.
async fn block_summary_by_hash(
    State(state): State<NodeState>,
    Path(hash): Path<String>,
    Extension(permit): Extension<WorkPermit>,
) -> ApiResult<Response> {
    let hash = hash.trim().to_ascii_lowercase();
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "a block hash is 64 hex characters"));
    }
    blocking(permit, move || {
        let block = {
            let _gate = state.gate.lock().map_err(|_| ApiError::internal("node gate poisoned"))?;
            state
                .storage
                .get_block(&hash)
                .map_err(ApiError::internal)?
                .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "unknown block hash"))?
        };
        summary_response(&state, &block)
    })
    .await
}

#[derive(Deserialize)]
struct RecentQuery {
    count: Option<usize>,
}

/// The summaries of the latest canonical blocks, newest first: an explorer's
/// front page in one request.
async fn recent_blocks(
    State(state): State<NodeState>,
    Query(query): Query<RecentQuery>,
    Extension(permit): Extension<WorkPermit>,
) -> ApiResult<Response> {
    let count = query.count.unwrap_or(10).clamp(1, MAX_RECENT_BLOCKS);
    blocking(permit, move || {
        let (tip, blocks) = {
            let _gate = state.gate.lock().map_err(|_| ApiError::internal("node gate poisoned"))?;
            let tip = state.storage.get_chain_height().map_err(ApiError::internal)?;
            let mut blocks = Vec::with_capacity(count);
            for height in (super::GENESIS_HEIGHT..=tip).rev().take(count) {
                if let Some(block) = state
                    .storage
                    .get_canonical_block_by_height(height)
                    .map_err(ApiError::internal)?
                {
                    blocks.push(block);
                }
            }
            (tip, blocks)
        };
        let summaries = blocks
            .iter()
            .map(|block| summary_of(&state, block))
            .collect::<ApiResult<Vec<_>>>()?;
        let mut response = json_response(
            StatusCode::OK,
            &serde_json::json!({"height":tip,"blocks":summaries}),
        )?;
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=5"));
        Ok(response)
    })
    .await
}

/// What an explorer shows of a stored block: its header, the work it had to
/// meet and, for every transaction, its id, shape and size, without the
/// proofs. The coinbase's outputs are public, so is its total.
fn summary_of(state: &NodeState, block: &Block) -> ApiResult<serde_json::Value> {
    let transactions = stark::decode(block).map_err(ApiError::internal)?;
    let coinbase = transactions
        .first()
        .filter(|tx| tx.statement.nullifiers.is_empty());
    let reward = coinbase
        .map(|tx| tx.openings.iter().fold(0u64, |sum, o| sum.saturating_add(o.value)))
        .unwrap_or(0);
    let size: usize = block.quantic_transactions.iter().map(Vec::len).sum();
    let nullifiers: usize = transactions.iter().map(|tx| tx.statement.nullifiers.len()).sum();
    let fees = transactions
        .iter()
        .fold(0u64, |sum, tx| sum.saturating_add(tx.statement.fee));
    let entries: Vec<_> = transactions
        .iter()
        .zip(&block.quantic_transactions)
        .enumerate()
        .map(|(index, (tx, bytes))| {
            let kind = if index == 0 && coinbase.is_some() {
                "coinbase"
            } else {
                "transfer"
            };
            serde_json::json!({"txid":hex::encode(tx.txid()),"kind":kind,
                "inputs":tx.statement.nullifiers.len(),"outputs":tx.statement.commitments.len(),
                "fee_atoms":tx.statement.fee,"size_bytes":bytes.len(),"proof_bytes":tx.proof.proof.len()})
        })
        .collect();
    let height = block.header.block_height;
    let canonical = state
        .storage
        .get_canonical_block_hash_at_height(height)
        .map_err(ApiError::internal)?
        .is_some_and(|hash| hash == block.header.hash);
    let tip = state.storage.get_chain_height().map_err(ApiError::internal)?;
    let target = target_of(state, block)?;
    Ok(serde_json::json!({
        "hash":block.header.hash,"height":height,"timestamp":block.header.timestamp,
        "state_root":block.header.state_root,"previous_hash":block.header.prev_hash,"nonce":block.header.nonce,
        "size_bytes":size,"reward_atoms":reward,
        "reward_zka_display":crate::economics::format_zka_amount(reward),
        "transactions_count":transactions.len(),
        "private_tx_count":transactions.len() - usize::from(coinbase.is_some()),
        "private_nullifier_count":nullifiers,"private_coinbase":coinbase.is_some(),
        "canonical":canonical,
        "confirmations":if canonical { tip.saturating_sub(height) + 1 } else { 0 },
        "difficulty_bits":difficulty::bits_of_target(&target),
        "target_compact":difficulty::to_compact(&target),
        "hashrate_estimate_hps":difficulty::network_hashrate_for_target(&target),
        "fees_atoms":fees,"transactions":entries,
    }))
}

/// A summary that a public front can cache: for ever once the block sits under
/// the latest checkpoint (nodes never reorganize below it), briefly before.
fn summary_response(state: &NodeState, block: &Block) -> ApiResult<Response> {
    let summary = summary_of(state, block)?;
    let settled = summary["canonical"] == true
        && state
            .latest_checkpoint()
            .map_err(ApiError::internal)?
            .is_some_and(|cp| block.header.block_height <= cp.height);
    let mut response = json_response(StatusCode::OK, &summary)?;
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(if settled {
            "public, max-age=31536000, immutable"
        } else {
            "public, max-age=10"
        }),
    );
    Ok(response)
}

/// The target `block` had to meet: [`super::target_for_child`] of its parent,
/// reading block 1 directly instead of walking back to it (a public chain has
/// a single block 1, fixed in the binary).
fn target_of(state: &NodeState, block: &Block) -> ApiResult<num_bigint::BigUint> {
    if block.header.block_height <= super::GENESIS_HEIGHT {
        return child_target(state, &state.profile.genesis_parent());
    }
    let parent = state
        .storage
        .get_block(&block.header.prev_hash)
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::internal("the block's parent is missing"))?;
    child_target(state, &parent)
}

/// The target of the block after `parent`.
fn child_target(state: &NodeState, parent: &Block) -> ApiResult<num_bigint::BigUint> {
    let genesis_time = if parent.header.block_height < super::GENESIS_HEIGHT {
        0
    } else {
        genesis_time(state)?
    };
    target_after(state, parent, genesis_time)
}

fn genesis_time(state: &NodeState) -> ApiResult<u64> {
    Ok(state
        .storage
        .get_canonical_block_by_height(super::GENESIS_HEIGHT)
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::internal("block 1 is missing"))?
        .header
        .timestamp)
}

/// [`child_target`] with block 1's time already read.
fn target_after(state: &NodeState, parent: &Block, genesis_time: u64) -> ApiResult<num_bigint::BigUint> {
    let anchor = if parent.header.block_height < super::GENESIS_HEIGHT { 0 } else { genesis_time };
    super::target_for_child_after(state.profile.chain_id, parent, anchor).map_err(ApiError::internal)
}

fn cached(mut response: Response, policy: &'static str) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static(policy));
    response
}

/// What the public API answers, so that a client finds its way without docs.
async fn public_index(State(state): State<NodeState>) -> ApiResult<Response> {
    let response = json_response(StatusCode::OK, &serde_json::json!({
        "chain_id":state.profile.chain_id,"node_version":env!("CARGO_PKG_VERSION"),
        "endpoints":{
            "/status":"chain tip, supply, difficulty, peers",
            "/blocks/recent?count=N":"the latest N block summaries (N <= 25)",
            "/blocks/{height}/summary":"one canonical block and its transactions, without proofs",
            "/blocks/hash/{hash}/summary":"one stored block by hash",
            "/blocks/stats?count=N&before=H":"time, interval, difficulty and size of up to 720 blocks, for charts",
            "/scan?from=H&count=N":"up to 100 blocks of encrypted outputs (postcard), for view-key scanners",
            "/supply":"issued and maximum supply (JSON)",
            "/supply/total":"issued supply in ZKA (plain text)",
            "/network/history":"this node's peers and the network's work, every 5 minutes for 24 hours",
            "/address/{short}":"the full address behind a short one, if this node knows it",
            "/checkpoint":"the latest checkpoint signed by the network's authority"
        }
    }))?;
    Ok(cached(response, "public, max-age=300"))
}

#[derive(Deserialize)]
struct StatsQuery {
    count: Option<u64>,
    before: Option<u64>,
}

/// Per-block figures for charts, oldest first: time, interval since the
/// parent, the difficulty the block met (and the hashrate it implies),
/// transactions and size. Nothing is decoded: headers and lengths only.
async fn block_stats(
    State(state): State<NodeState>,
    Query(query): Query<StatsQuery>,
    Extension(permit): Extension<WorkPermit>,
) -> ApiResult<Response> {
    let count = query.count.unwrap_or(180).clamp(1, MAX_STATS_BLOCKS);
    blocking(permit, move || {
        let _gate = state.gate.lock().map_err(|_| ApiError::internal("node gate poisoned"))?;
        let tip = state.storage.get_chain_height().map_err(ApiError::internal)?;
        let end = query.before.map_or(tip, |before| before.saturating_sub(1)).min(tip);
        if end < super::GENESIS_HEIGHT {
            let body = serde_json::json!({"height":tip,"blocks":[]});
            return Ok(cached(json_response(StatusCode::OK, &body)?, "public, max-age=30"));
        }
        let start = end.saturating_sub(count - 1).max(super::GENESIS_HEIGHT);
        let genesis = genesis_time(&state)?;
        let mut parent = if start == super::GENESIS_HEIGHT {
            state.profile.genesis_parent()
        } else {
            state
                .storage
                .get_canonical_block_by_height(start - 1)
                .map_err(ApiError::internal)?
                .ok_or_else(|| ApiError::internal("a canonical block is missing"))?
        };
        let mut entries = Vec::with_capacity((end - start + 1) as usize);
        for height in start..=end {
            let block = state
                .storage
                .get_canonical_block_by_height(height)
                .map_err(ApiError::internal)?
                .ok_or_else(|| ApiError::internal("a canonical block is missing"))?;
            let target = target_after(&state, &parent, genesis)?;
            let interval = (height > super::GENESIS_HEIGHT)
                .then(|| block.header.timestamp.saturating_sub(parent.header.timestamp));
            entries.push(serde_json::json!({
                "height":height,"timestamp":block.header.timestamp,"interval_secs":interval,
                "difficulty_bits":difficulty::bits_of_target(&target),
                "hashrate_estimate_hps":difficulty::network_hashrate_for_target(&target),
                "transactions":block.quantic_transactions.len(),
                "size_bytes":block.quantic_transactions.iter().map(Vec::len).sum::<usize>(),
            }));
            parent = block;
        }
        let body = serde_json::json!({"height":tip,"blocks":entries});
        Ok(cached(json_response(StatusCode::OK, &body)?, "public, max-age=30"))
    })
    .await
}

#[derive(Deserialize)]
struct ScanQuery {
    from: Option<u32>,
    count: Option<u32>,
}

/// Every output of up to [`MAX_SCAN_BLOCKS`] canonical blocks from `from`, as a
/// postcard [`ScanPage`]: what a view key opens where it is kept (in a
/// browser, say), so this node never learns whose wallet is being read. A
/// full page under the latest checkpoint never changes.
async fn scan_page(
    State(state): State<NodeState>,
    Query(query): Query<ScanQuery>,
    Extension(permit): Extension<WorkPermit>,
) -> ApiResult<Response> {
    let from = query.from.unwrap_or(1).max(1);
    let count = query.count.unwrap_or(MAX_SCAN_BLOCKS).clamp(1, MAX_SCAN_BLOCKS);
    blocking(permit, move || {
        let (tip, blocks, checkpoint) = {
            let _gate = state.gate.lock().map_err(|_| ApiError::internal("node gate poisoned"))?;
            let tip = state.storage.get_chain_height().map_err(ApiError::internal)?;
            let last = (u64::from(from) + u64::from(count) - 1).min(tip);
            let mut blocks = Vec::new();
            for height in u64::from(from)..=last {
                blocks.push(
                    state
                        .storage
                        .get_canonical_block_by_height(height)
                        .map_err(ApiError::internal)?
                        .ok_or_else(|| ApiError::internal("a canonical block is missing"))?,
                );
            }
            let checkpoint = state.latest_checkpoint().map_err(ApiError::internal)?.map(|cp| cp.height);
            (tip, blocks, checkpoint)
        };
        let page = ScanPage {
            network: state.profile.network,
            tip: u32::try_from(tip).map_err(ApiError::internal)?,
            from,
            blocks: blocks
                .iter()
                .map(|block| {
                    Ok(ScanBlock {
                        height: u32::try_from(block.header.block_height).map_err(ApiError::internal)?,
                        hash: block.header.hash.clone(),
                        timestamp: block.header.timestamp,
                        transactions: stark::decode(block)
                            .map_err(ApiError::internal)?
                            .iter()
                            .map(ScanTransaction::of)
                            .collect(),
                    })
                })
                .collect::<ApiResult<_>>()?,
        };
        let settled = page.blocks.len() == count as usize
            && checkpoint.is_some_and(|cp| u64::from(from) + u64::from(count) - 1 <= cp);
        let body = page.encode().map_err(ApiError::internal)?;
        let mut response = (StatusCode::OK, body).into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/octet-stream"),
        );
        Ok(cached(
            response,
            if settled { "public, max-age=31536000, immutable" } else { "public, max-age=10" },
        ))
    })
    .await
}

fn zka(atoms: u64) -> String {
    format!("{}.{:06}", atoms / ATOMS, atoms % ATOMS)
}

fn issued(state: &NodeState) -> ApiResult<(u64, u64)> {
    let _gate = state.gate.lock().map_err(|_| ApiError::internal("node gate poisoned"))?;
    let tip = current_tip(state.profile, &state.storage)?;
    let branch = stark::branch(&state.storage, state.profile, &tip).map_err(ApiError::internal)?;
    let ledger = branch.state();
    Ok((u64::from(ledger.height), ledger.issued))
}

/// Issued and maximum supply. Block 1 issued the premine; every later block
/// issues its subsidy (fees go back to the miner, they issue nothing).
async fn supply(
    State(state): State<NodeState>,
    Extension(permit): Extension<WorkPermit>,
) -> ApiResult<Response> {
    blocking(permit, move || {
        let (height, issued) = issued(&state)?;
        let next = subsidy(u32::try_from(height + 1).unwrap_or(u32::MAX));
        let body = serde_json::json!({
            "height":height,"issued_atoms":issued,"issued_zka":zka(issued),
            "max_supply_atoms":MAX_SUPPLY,"max_supply_zka":zka(MAX_SUPPLY),
            "premine_atoms":PREMINE,"next_block_subsidy_atoms":next,
            "atoms_per_zka":ATOMS,
        });
        Ok(cached(json_response(StatusCode::OK, &body)?, "public, max-age=30"))
    })
    .await
}

/// The issued supply in ZKA, as plain text, for listing sites.
async fn supply_total(
    State(state): State<NodeState>,
    Extension(permit): Extension<WorkPermit>,
) -> ApiResult<Response> {
    blocking(permit, move || {
        let (_, issued) = issued(&state)?;
        let mut response = (StatusCode::OK, zka(issued)).into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; charset=utf-8"),
        );
        Ok(cached(response, "public, max-age=30"))
    })
    .await
}

/// One look at the network: this node's sessions and the work the next block
/// needs. Kept for [`HISTORY_SAMPLES`] samples in the database, so a restart
/// does not empty the chart. Hold `gate`.
pub fn record_network_sample(state: &NodeState) -> Result<(), String> {
    let tip = state.tip()?;
    let next = child_target(state, &tip).map_err(|e| e.message)?;
    let peers = state.peers.status();
    let mempool = state.mempool.lock().map_err(|_| "mempool poisoned")?.len();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs();
    let mut samples = history(state)?;
    samples.push(serde_json::json!({
        "time":now,"height":tip.header.block_height,
        "outbound":peers["outbound"],"inbound":peers["inbound"],"known":peers["known"],
        "difficulty_bits":difficulty::bits_of_target(&next),
        "hashrate_estimate_hps":difficulty::network_hashrate_for_target(&next),
        "mempool_transactions":mempool,
    }));
    let excess = samples.len().saturating_sub(HISTORY_SAMPLES);
    samples.drain(..excess);
    let bytes = serde_json::to_vec(&samples).map_err(|e| e.to_string())?;
    state.storage.put_raw(HISTORY_KEY, bytes).map_err(|e| e.to_string())
}

fn history(state: &NodeState) -> Result<Vec<serde_json::Value>, String> {
    Ok(state
        .storage
        .get_raw(HISTORY_KEY)
        .map_err(|e| e.to_string())?
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default())
}

/// The samples of [`record_network_sample`], oldest first. Empty on a node
/// that does not serve the public API (only that one samples).
async fn network_history(
    State(state): State<NodeState>,
    Extension(permit): Extension<WorkPermit>,
) -> ApiResult<Response> {
    blocking(permit, move || {
        let samples = history(&state).map_err(ApiError::internal)?;
        let body = serde_json::json!({"every_secs":HISTORY_EVERY.as_secs(),"samples":samples});
        Ok(cached(json_response(StatusCode::OK, &body)?, "public, max-age=60"))
    })
    .await
}

async fn submit_transaction(
    State(state): State<NodeState>,
    Extension(permit): Extension<WorkPermit>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Response> {
    content_type(&headers, "application/octet-stream")?;
    blocking(permit, move || {
        let tx = Transaction::decode(&body).map_err(ApiError::invalid)?;
        let id = hex::encode(tx.txid());
        let _gate = state
            .gate
            .lock()
            .map_err(|_| ApiError::internal("node gate poisoned"))?;
        let tip = current_tip(state.profile, &state.storage)?;
        let ledger = stark::branch(&state.storage, state.profile, &tip).map_err(ApiError::internal)?;
        state
            .mempool
            .lock()
            .map_err(|_| ApiError::internal("mempool poisoned"))?
            .insert(&ledger, tx)
            .map_err(ApiError::invalid)?;
        json_response(
            StatusCode::ACCEPTED,
            &serde_json::json!({"txid":id,"accepted":true}),
        )
    })
    .await
}

#[derive(Deserialize)]
struct TemplateQuery {
    address: String,
}

#[derive(Serialize)]
struct TemplateResponse {
    chain_id: &'static str,
    pow_algorithm: &'static str,
    xmrig_compatible: bool,
    blob_hex: String,
    target_hex_le: String,
    seed_hex: String,
    nonce_offset: usize,
    nonce_bytes: usize,
    block: Block,
}

async fn mining_template(
    State(state): State<NodeState>,
    Extension(permit): Extension<WorkPermit>,
    Query(query): Query<TemplateQuery>,
) -> ApiResult<Response> {
    blocking(permit, move || {
        let profile = state.profile;
        let address = Address::decode(&query.address, profile.network).map_err(ApiError::invalid)?;
        let (parent, txs) = {
            let _gate = state
                .gate
                .lock()
                .map_err(|_| ApiError::internal("node gate poisoned"))?;
            let parent = current_tip(profile, &state.storage)?;
            let txs = state
                .mempool
                .lock()
                .map_err(|_| ApiError::internal("mempool poisoned"))?
                .select();
            (parent, txs)
        };
        // The parent state is immutable. A concurrent new tip may make this a
        // stale template, as with ordinary mining, without changing its rules.
        let block = stark::template(
            &state.storage,
            profile,
            &parent,
            &address,
            txs,
            super::now_secs(),
        )
        .map_err(ApiError::invalid)?;
        let compact = super::compact_target_for_child(&state.storage, profile.chain_id, &parent)
            .map_err(ApiError::internal)?;
        let mut target = super::difficulty::from_compact(compact).to_bytes_le();
        target.resize(32, 0);
        let header = &block.header;
        let blob = pow::build_quantic_pow_input(
            profile.chain_id,
            &header.prev_hash,
            &header.merkle_root,
            &header.state_root,
            header.block_height,
            header.timestamp,
            header.nonce,
        );
        let response = TemplateResponse {
            chain_id: profile.chain_id,
            pow_algorithm: profile.pow_algorithm(),
            xmrig_compatible: profile.xmrig_compatible(),
            blob_hex: hex::encode(blob),
            target_hex_le: hex::encode(target),
            seed_hex: hex::encode(pow::epoch_seed(profile.chain_id, header.block_height)),
            nonce_offset: pow::QUANTIC_POW_NONCE_OFFSET,
            nonce_bytes: 4,
            block,
        };
        json_response(StatusCode::OK, &response)
    })
    .await
}

async fn submit_block(
    State(state): State<NodeState>,
    Extension(permit): Extension<WorkPermit>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Response> {
    content_type(&headers, "application/json")?;
    blocking(permit, move || {
        let block: Block = serde_json::from_slice(&body).map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, e.to_string()))?;
        let _gate = state.gate.lock().map_err(|_| ApiError::internal("node gate poisoned"))?;
        let old_tip = current_tip(state.profile, &state.storage)?;
        let (status, known) = match apply_block(&state.storage, state.profile.chain_id, &block) {
            Ok(_) => (StatusCode::ACCEPTED, false),
            Err(ChainError::SideBranchNotHeavier { .. }) => (StatusCode::ACCEPTED, false),
            Err(ChainError::BlockAlreadyKnown(_)) => (StatusCode::OK, true),
            Err(ChainError::Storage(message)) => return Err(ApiError::internal(message)),
            Err(e) => return Err(ApiError::invalid(e)),
        };
        let tip = current_tip(state.profile, &state.storage)?;
        if tip.header.hash != old_tip.header.hash {
            let ledger = stark::branch(&state.storage, state.profile, &tip).map_err(ApiError::internal)?;
            state.mempool.lock().map_err(|_| ApiError::internal("mempool poisoned"))?.revalidate(&ledger);
        }
        let canonical = state.storage.get_canonical_block_by_height(block.header.block_height)
            .map_err(ApiError::internal)?.is_some_and(|b| b.header.hash == block.header.hash);
        json_response(status, &serde_json::json!({"accepted":true,"hash":block.header.hash,"height":block.header.block_height,
            "canonical":canonical,"already_known":known,"tip_hash":tip.header.hash,"tip_height":tip.header.block_height}))
    }).await
}

async fn latest_checkpoint(
    State(state): State<NodeState>,
    Extension(permit): Extension<WorkPermit>,
) -> ApiResult<Response> {
    blocking(permit, move || {
        let _gate = state.gate.lock().map_err(|_| ApiError::internal("node gate poisoned"))?;
        json_response(StatusCode::OK, &state.latest_checkpoint().map_err(ApiError::internal)?)
    })
    .await
}

/// Verified like one from a peer: this is how a checkpoint signed by hand
/// (`quantic-pq-node checkpoint-sign`) enters the network.
async fn submit_checkpoint(
    State(state): State<NodeState>,
    Extension(permit): Extension<WorkPermit>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Response> {
    content_type(&headers, "application/json")?;
    blocking(permit, move || {
        let checkpoint: SignedCheckpoint = serde_json::from_slice(&body)
            .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, e.to_string()))?;
        let _gate = state.gate.lock().map_err(|_| ApiError::internal("node gate poisoned"))?;
        let accepted = state.accept_checkpoint(&checkpoint).map_err(ApiError::invalid)?;
        json_response(StatusCode::OK, &serde_json::json!({
            "accepted":true,"already_known":accepted == Acceptance::AlreadyKnown,
            "height":checkpoint.height,"block_hash":checkpoint.block_hash,
            "followed":state.follows(&checkpoint).map_err(ApiError::internal)?,
        }))
    })
    .await
}

/// The v1 request (`POST /quantic/address`).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PublishAddress {
    address: String,
    /// Without one the node finds it (seconds), or keeps the one it has.
    stamp: Option<u64>,
}

fn directory_error(error: DirectoryError) -> ApiError {
    match error {
        DirectoryError::Address | DirectoryError::ShortAddress | DirectoryError::Stamp => {
            ApiError::invalid(error)
        }
        DirectoryError::Full => ApiError::new(StatusCode::INSUFFICIENT_STORAGE, error.to_string()),
        DirectoryError::Storage(_) => ApiError::internal(error),
    }
}

/// Publish a full address. The node pushes it to its outbound peers, and
/// again on every new session.
async fn publish_address(
    State(state): State<NodeState>,
    Extension(permit): Extension<WorkPermit>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Response> {
    content_type(&headers, "application/json")?;
    blocking(permit, move || {
        let request: PublishAddress = serde_json::from_slice(&body)
            .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, e.to_string()))?;
        let network = state.profile.network;
        let full = stark_directory::decode_full(&request.address, network).map_err(directory_error)?;
        let short = full.short_address().map_err(ApiError::internal)?;
        let record = match request.stamp {
            Some(stamp) => AddressRecord {
                address: full.encode().map_err(ApiError::internal)?,
                stamp,
            },
            None => match stark_directory::lookup(&state.storage, &short).map_err(directory_error)? {
                Some((_, stored)) => stored,
                None => AddressRecord::new(&full).map_err(directory_error)?,
            },
        };
        let (short, new) = state
            .directory
            .store(&state.storage, network, &record)
            .map_err(directory_error)?;
        stark_directory::mark_local(&state.storage, &short).map_err(directory_error)?;
        json_response(
            StatusCode::OK,
            &serde_json::json!({"short_address":short.encode(),"new":new}),
        )
    })
    .await
}

#[derive(Deserialize)]
struct AddressQuery {
    /// `1` or `true`: only this node's directory, without asking its peers
    /// (whether a wallet's own address is published yet).
    local: Option<String>,
}

/// The full address of a short one: from this node's directory, or asked of
/// its outbound peers for up to `LOOKUP_TIMEOUT`.
async fn address_of(
    State(state): State<NodeState>,
    Path(short): Path<String>,
    Query(query): Query<AddressQuery>,
    Extension(permit): Extension<WorkPermit>,
) -> ApiResult<Response> {
    let local_only = matches!(query.local.as_deref(), Some("1" | "true"));
    // Waiting on peers holds no crypto worker.
    drop(
        permit
            .0
            .lock()
            .map_err(|_| ApiError::internal("worker permit lock poisoned"))?
            .take(),
    );
    let short = ShortAddress::decode(short.trim(), state.profile.network)
        .map_err(|_| ApiError::invalid("not a short v2 address of this chain"))?;
    let deadline = std::time::Instant::now() + stark_directory::LOOKUP_TIMEOUT;
    let mut stored = state.directory.stored();
    let mut asked = false;
    loop {
        let storage = state.storage.clone();
        let found = tokio::task::spawn_blocking(move || stark_directory::lookup(&storage, &short))
            .await
            .map_err(ApiError::internal)?
            .map_err(directory_error)?;
        if let Some((full, _)) = found {
            return json_response(
                StatusCode::OK,
                &serde_json::json!({"short_address":short.encode(),
                    "address":full.encode().map_err(ApiError::internal)?}),
            );
        }
        if !asked {
            asked = true;
            if local_only {
                return Err(ApiError::new(
                    StatusCode::NOT_FOUND,
                    "this node does not know that short address",
                ));
            }
            if state.peers.book().outbound_len() == 0 {
                return Err(ApiError::new(
                    StatusCode::NOT_FOUND,
                    "this node does not know that short address and has no outbound peers to ask",
                ));
            }
            if !state.directory.want(&short, deadline) {
                return Err(ApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "too many address lookups are waiting; retry later",
                ));
            }
        }
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        match tokio::time::timeout(left, stored.changed()).await {
            Ok(Ok(())) => {}
            _ => {
                return Err(ApiError::new(
                    StatusCode::NOT_FOUND,
                    "neither this node nor its peers know that short address",
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use serde_json::Value;
    use stark::protocol::{
        ledger::{ATOMS, PREMINE},
        model::Input,
        transaction::{transfer, Payment},
        wallet::Account,
    };
    use tower::ServiceExt;

    const CHAIN: &str = Profile::REGTEST.chain_id;

    fn test_state() -> (tempfile::TempDir, NodeState) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(RocksDbStorage::new(dir.path()).unwrap());
        storage
            .put_raw(DB_MARKER, CHAIN.as_bytes().to_vec())
            .unwrap();
        let state = NodeState::new(storage).unwrap();
        (dir, state)
    }

    async fn json(response: Response) -> Value {
        serde_json::from_slice(
            &to_bytes(response.into_body(), MAX_BLOCK_REQUEST_BYTES)
                .await
                .unwrap(),
        )
        .unwrap()
    }

    async fn get(app: &Router, uri: &str) -> Response {
        app.clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn post(app: &Router, uri: &str, kind: &str, body: Vec<u8>) -> Response {
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header(header::CONTENT_TYPE, kind)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn status_limits_media_types_origin_and_busy_admission() {
        let (_dir, state) = test_state();
        let app = router(state.clone());
        let response = get(&app, "/status").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let info = json(response).await;
        assert_eq!(info["chain_id"], CHAIN);
        assert_eq!(info["height"], 0);
        assert_eq!(info["pow_algorithm"], "zoka-cpu-fallback-v1");
        assert_eq!(info["xmrig_compatible"], false);
        assert_eq!(get(&app, "/blocks/1").await.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            get(&app, "/mining/template?address=bad").await.status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            post(&app, "/transactions", "application/json", vec![])
                .await
                .status(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        );
        assert_eq!(
            post(
                &app,
                "/transactions",
                "application/octet-stream",
                b"bad".to_vec()
            )
            .await
            .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            post(&app, "/blocks", "application/json", b"bad".to_vec())
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
        // Declared size is rejected before reading. Actual streamed/undeclared
        // bytes still pass through DefaultBodyLimit and cannot bypass it.
        for (path, maximum) in [
            ("/transactions", MAX_TRANSACTION_BYTES),
            ("/blocks", MAX_BLOCK_REQUEST_BYTES),
        ] {
            let request = Request::builder()
                .method("POST")
                .uri(path)
                .header(header::CONTENT_LENGTH, (maximum + 1).to_string())
                .body(Body::empty())
                .unwrap();
            assert_eq!(
                app.clone().oneshot(request).await.unwrap().status(),
                StatusCode::PAYLOAD_TOO_LARGE
            );
        }
        assert_eq!(
            post(
                &app,
                "/transactions",
                "application/octet-stream",
                vec![0; MAX_TRANSACTION_BYTES + 1]
            )
            .await
            .status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        for (name, value) in [
            ("host", "attacker.example:1234"),
            ("sec-fetch-site", "cross-site"),
            ("origin", "https://attacker.example"),
        ] {
            let request = Request::builder()
                .uri("/status")
                .header(name, value)
                .body(Body::empty())
                .unwrap();
            assert_eq!(
                app.clone().oneshot(request).await.unwrap().status(),
                StatusCode::FORBIDDEN
            );
        }
        let same_origin = Request::builder()
            .uri("/status")
            .header("host", "127.0.0.1:1234")
            .header("origin", "http://127.0.0.1:1234")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.clone().oneshot(same_origin).await.unwrap().status(),
            StatusCode::OK
        );
        let occupied = state
            .work
            .clone()
            .acquire_many_owned(MAX_CRYPTO_WORKERS as u32)
            .await
            .unwrap();
        assert_eq!(
            get(&app, "/status").await.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        drop(occupied);
        assert_eq!(get(&app, "/status").await.status(), StatusCode::OK);
        // Non-loopback addresses are refused before any TCP listener is opened.
        assert!(serve(state, "0.0.0.0:0".parse().unwrap()).await.is_err());
    }

    #[test]
    fn constructor_does_not_adopt_an_unmarked_database() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(RocksDbStorage::new(dir.path()).unwrap());
        assert!(NodeState::new(storage.clone()).is_err());
        assert!(storage.get_raw(DB_MARKER).unwrap().is_none());
        storage
            .put_raw(DB_MARKER, b"another-chain".to_vec())
            .unwrap();
        assert!(NodeState::new(storage).is_err());
    }

    #[tokio::test]
    async fn cancelling_a_request_does_not_release_a_running_worker() {
        let workers = Arc::new(Semaphore::new(1));
        let permit = WorkPermit(Arc::new(Mutex::new(Some(
            workers.clone().acquire_owned().await.unwrap(),
        ))));
        let started = Arc::new(tokio::sync::Notify::new());
        let signal = started.clone();
        let (finish, release) = std::sync::mpsc::channel();
        let task = tokio::spawn(async move {
            blocking(permit, move || {
                signal.notify_one();
                release.recv().unwrap();
                Ok(())
            })
            .await
        });
        started.notified().await;
        task.abort();
        let _ = task.await;
        assert_eq!(workers.available_permits(), 0);
        finish.send(()).unwrap();
        let released = tokio::time::timeout(Duration::from_secs(2), workers.acquire())
            .await
            .unwrap()
            .unwrap();
        drop(released);
        assert_eq!(workers.available_permits(), 1);
    }

    async fn solved_template(app: &Router, state: &NodeState, address: &Address) -> Block {
        let response = get(
            app,
            &format!("/mining/template?address={}", address.encode().unwrap()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let mut data = json(response).await;
        assert_eq!(data["pow_algorithm"], "zoka-cpu-fallback-v1");
        assert_eq!(data["xmrig_compatible"], false);
        assert_eq!(data["nonce_offset"], pow::QUANTIC_POW_NONCE_OFFSET);
        assert_eq!(data["target_hex_le"].as_str().unwrap().len(), 64);
        let mut block: Block = serde_json::from_value(data["block"].take()).unwrap();
        let h = &block.header;
        assert_eq!(
            data["blob_hex"],
            hex::encode(pow::build_quantic_pow_input(
                CHAIN,
                &h.prev_hash,
                &h.merkle_root,
                &h.state_root,
                h.block_height,
                h.timestamp,
                h.nonce
            ))
        );
        let storage = state.storage.clone();
        tokio::task::spawn_blocking(move || {
            let parent = current_tip(Profile::REGTEST, &storage).unwrap();
            let target =
                crate::quantic::compact_target_for_child(&storage, CHAIN, &parent).unwrap();
            crate::quantic::solve_pow(CHAIN, &mut block, target).unwrap();
            block
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn local_api_mines_and_applies_a_real_private_payment() {
        let (_dir, state) = test_state();
        let app = router(state.clone());
        let alice = Account::from_seed(&[71; 32], 3, 0).unwrap();
        let bob = Account::from_seed(&[72; 32], 3, 0).unwrap();
        let genesis = solved_template(&app, &state, &alice.address()).await;
        let encoded_genesis = serde_json::to_vec(&genesis).unwrap();
        let response = post(&app, "/blocks", "application/json", encoded_genesis.clone()).await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert_eq!(json(response).await["canonical"], true);
        let response = post(&app, "/blocks", "application/json", encoded_genesis).await;
        assert_eq!(response.status(), StatusCode::OK);
        let known = json(response).await;
        assert_eq!(known["already_known"], true);
        assert_eq!(known["canonical"], true);
        let restored: Block =
            serde_json::from_value(json(get(&app, "/blocks/1").await).await).unwrap();
        assert_eq!(restored.header.hash, genesis.header.hash);
        let storage = state.storage.clone();
        let recipient = bob.address();
        let payment = tokio::task::spawn_blocking(move || {
            let ledger = stark::full_ledger(&storage, Profile::REGTEST, &genesis).unwrap();
            let reward = stark::decode(&genesis).unwrap().remove(0);
            let (note, _) = alice
                .receive(reward.statement.commitments[0], &reward.envelopes[0])
                .unwrap();
            transfer(
                &alice,
                ledger.tree.root(),
                vec![Input {
                    key: alice.spend.clone(),
                    note,
                    path: ledger.tree.path(0).unwrap(),
                }],
                vec![
                    Payment::new(recipient, ATOMS),
                    Payment::new(alice.address(), PREMINE - ATOMS - 200),
                ],
                200,
                10,
            )
            .unwrap()
        })
        .await
        .unwrap();
        let encoded_payment = payment.encode().unwrap();
        assert_eq!(
            post(
                &app,
                "/transactions",
                "application/octet-stream",
                encoded_payment.clone()
            )
            .await
            .status(),
            StatusCode::ACCEPTED
        );
        assert_eq!(state.mempool.lock().unwrap().len(), 1);
        assert_eq!(
            post(
                &app,
                "/transactions",
                "application/octet-stream",
                encoded_payment.clone()
            )
            .await
            .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        let paid = solved_template(&app, &state, &bob.address()).await;
        assert_eq!(paid.quantic_transactions.len(), 2);
        let response = post(
            &app,
            "/blocks",
            "application/json",
            serde_json::to_vec(&paid).unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert!(state.mempool.lock().unwrap().is_empty());
        assert_eq!(json(get(&app, "/status").await).await["height"], 2);
        assert_eq!(
            post(
                &app,
                "/transactions",
                "application/octet-stream",
                encoded_payment
            )
            .await
            .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        let (received, _) = bob
            .receive(payment.statement.commitments[0], &payment.envelopes[0])
            .unwrap();
        assert_eq!(received.value, ATOMS);
    }

    #[tokio::test]
    async fn an_address_is_published_and_resolved_by_its_short_form() {
        let (_dir, state) = test_state();
        let app = router(state.clone());
        let bob = Account::from_seed(&[61; 32], 3, 0).unwrap().address();
        let full = bob.encode().unwrap();
        let short = bob.short().unwrap();
        // Unknown, and no outbound peer to ask: answered at once.
        let response = get(&app, &format!("/address/{short}")).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let body = serde_json::json!({ "address": full }).to_string().into_bytes();
        let published = json(post(&app, "/address", "application/json", body.clone()).await).await;
        assert_eq!(published["short_address"], short);
        assert_eq!(published["new"], true);
        // Publishing again keeps the stored stamp.
        let again = json(post(&app, "/address", "application/json", body).await).await;
        assert_eq!(again["new"], false);
        let resolved = json(get(&app, &format!("/address/{short}")).await).await;
        assert_eq!(resolved["address"], full);
        assert_eq!(resolved["short_address"], short);
        let status = json(get(&app, "/status").await).await;
        assert_eq!(status["directory"]["addresses"], 1);
        assert_eq!(status["directory"]["published_here"], 1);
        // `?local=1` answers from this node alone, at once.
        let local = get(&app, &format!("/address/{short}?local=1")).await;
        assert_eq!(local.status(), StatusCode::OK);
        let nobody = Account::from_seed(&[62; 32], 3, 0).unwrap().address().short().unwrap();
        let response = get(&app, &format!("/address/{nobody}?local=1")).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        // Only full addresses are published, and only short ones resolve.
        let body = serde_json::json!({ "address": short }).to_string().into_bytes();
        let response = post(&app, "/address", "application/json", body).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let error = json(response).await["error"].as_str().unwrap().to_string();
        assert!(error.contains("publish the full one"), "{error}");
        let response = get(&app, &format!("/address/{full}")).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let testnet = Account::from_seed(&[61; 32], 2, 0).unwrap().address();
        let response = get(&app, &format!("/address/{}", testnet.short().unwrap())).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn a_block_summary_names_its_public_reward_without_the_proofs() {
        let (_dir, state) = test_state();
        let app = router(state.clone());
        assert_eq!(get(&app, "/blocks/1/summary").await.status(), StatusCode::NOT_FOUND);
        let alice = Account::from_seed(&[63; 32], 3, 0).unwrap().address();
        let (block, _) = super::super::stark_node::mine_once(&state, alice).await.unwrap();
        let summary = json(get(&app, "/blocks/1/summary").await).await;
        assert_eq!(summary["hash"], block.header.hash);
        assert_eq!(summary["height"], 1);
        assert_eq!(summary["reward_atoms"], PREMINE);
        assert_eq!(summary["transactions_count"], 1);
        assert_eq!(summary["private_tx_count"], 0);
        assert_eq!(summary["private_coinbase"], true);
        assert!(summary["size_bytes"].as_u64().unwrap() > 0);
        // What an explorer lists of each transaction, and the block's work.
        assert_eq!(summary["canonical"], true);
        assert_eq!(summary["confirmations"], 1);
        assert_eq!(
            summary["difficulty_bits"],
            crate::economics::QUANTIC_REGTEST_DIFFICULTY_BITS
        );
        assert_eq!(summary["fees_atoms"], 0);
        let coinbase = &summary["transactions"][0];
        assert_eq!(summary["transactions"].as_array().unwrap().len(), 1);
        assert_eq!(coinbase["kind"], "coinbase");
        assert_eq!(coinbase["inputs"], 0);
        assert!(coinbase["outputs"].as_u64().unwrap() >= 1);
        assert_eq!(coinbase["txid"].as_str().unwrap().len(), 64);
        assert_eq!(coinbase["size_bytes"], summary["size_bytes"]);
        let by_hash = json(get(&app, &format!("/blocks/hash/{}/summary", block.header.hash)).await).await;
        assert_eq!(by_hash, summary);
        assert_eq!(
            get(&app, "/blocks/hash/xyz/summary").await.status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            get(&app, &format!("/blocks/hash/{}/summary", "0".repeat(64))).await.status(),
            StatusCode::NOT_FOUND
        );
        let recent = json(get(&app, "/blocks/recent?count=5").await).await;
        assert_eq!(recent["height"], 1);
        assert_eq!(recent["blocks"].as_array().unwrap().len(), 1);
        assert_eq!(recent["blocks"][0]["hash"], block.header.hash);
        let status = json(get(&app, "/status").await).await;
        assert_eq!(status["tip_timestamp"], block.header.timestamp);
        assert_eq!(status["peers"]["best_height"], 0);
    }

    async fn get_from(app: &Router, uri: &str, method: &str) -> Response {
        app.clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header(header::HOST, "api.zokascan.cc")
                    .header(header::ORIGIN, "https://zokascan.cc")
                    .header("sec-fetch-site", "cross-site")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn the_public_api_reads_the_chain_for_any_origin_and_nothing_else() {
        let (_dir, state) = test_state();
        let alice = Account::from_seed(&[64; 32], 3, 0).unwrap().address();
        super::super::stark_node::mine_once(&state, alice).await.unwrap();
        let public = public_router(state.clone());

        let response = get_from(&public, "/status", "GET").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "public, max-age=5");
        let status = json(response).await;
        assert_eq!(status["chain_id"], CHAIN);
        assert_eq!(status["height"], 1);
        assert_eq!(status["node_version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(
            status["next_difficulty_bits"],
            crate::economics::QUANTIC_REGTEST_DIFFICULTY_BITS
        );
        // Not this node's own services.
        assert!(status.get("stratum").is_none());
        assert!(status.get("checkpoint_authority").is_none());

        let summary = get_from(&public, "/blocks/1/summary", "GET").await;
        assert_eq!(summary.status(), StatusCode::OK);
        assert_eq!(summary.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
        assert_eq!(json(get_from(&public, "/blocks/recent", "GET").await).await["height"], 1);
        assert_eq!(
            get_from(&public, "/address/zqregtest2abc", "GET").await.status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );

        // Nothing that writes, mines, signs or hands out whole blocks.
        for (uri, method) in [
            ("/transactions", "POST"),
            ("/blocks", "POST"),
            ("/checkpoint", "POST"),
            ("/address", "POST"),
        ] {
            assert_eq!(
                get_from(&public, uri, method).await.status(),
                StatusCode::METHOD_NOT_ALLOWED,
                "{method} {uri}"
            );
        }
        for uri in ["/mining/template", "/blocks/1"] {
            assert_eq!(get_from(&public, uri, "GET").await.status(), StatusCode::NOT_FOUND, "{uri}");
        }
        let preflight = get_from(&public, "/status", "OPTIONS").await;
        assert_eq!(preflight.status(), StatusCode::NO_CONTENT);
        assert_eq!(preflight.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");

        // The local API still refuses the same request.
        assert_eq!(
            get_from(&router(state), "/status", "GET").await.status(),
            StatusCode::FORBIDDEN
        );

        // The rate is shared: the bucket runs dry and then refills.
        let gate = PublicGate::new();
        for _ in 0..PUBLIC_BURST as usize {
            assert!(gate.take());
        }
        assert!(!gate.take());
        std::thread::sleep(Duration::from_millis(200));
        assert!(gate.take());
    }

    #[tokio::test]
    async fn the_public_api_serves_what_a_view_key_and_a_chart_need() {
        let (_dir, state) = test_state();
        let alice = Account::from_seed(&[64; 32], 3, 0).unwrap();
        for _ in 0..3 {
            super::super::stark_node::mine_once(&state, alice.address()).await.unwrap();
        }
        let public = public_router(state.clone());

        // The view key finds every coin of its wallet in the scan pages, and
        // only there: the node is never told which wallet is being read.
        let response = get_from(&public, "/scan?from=1&count=2", "GET").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/octet-stream");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "public, max-age=10");
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let first = ScanPage::decode(&bytes).unwrap();
        assert_eq!((first.from, first.tip, first.blocks.len()), (1, 3, 2));
        let mut viewer = zoka_pq_pool::scan::Viewer::new(alice.view_key().unwrap());
        viewer.add_page(&first).unwrap();
        let bytes = to_bytes(
            get_from(&public, "/scan?from=3", "GET").await.into_body(),
            usize::MAX,
        )
        .await
        .unwrap();
        viewer.add_page(&ScanPage::decode(&bytes).unwrap()).unwrap();
        let seen = viewer.balance();
        let supply = json(get_from(&public, "/supply", "GET").await).await;
        assert_eq!(supply["height"], 3);
        assert_eq!(seen.received_atoms, supply["issued_atoms"].as_u64().unwrap());
        assert_eq!(supply["max_supply_atoms"], MAX_SUPPLY);
        // On regtest a coinbase joins the note tree two blocks later: those of
        // blocks 2 and 3 still wait, block 1's counts at once.
        assert_eq!(seen.immature_atoms, subsidy(2) + subsidy(3));
        let total = get_from(&public, "/supply/total", "GET").await;
        assert_eq!(total.headers()[header::CONTENT_TYPE], "text/plain; charset=utf-8");
        let text = to_bytes(total.into_body(), 64).await.unwrap();
        assert_eq!(std::str::from_utf8(&text).unwrap(), zka(supply["issued_atoms"].as_u64().unwrap()));

        let stats = json(get_from(&public, "/blocks/stats?count=2", "GET").await).await;
        let blocks = stats["blocks"].as_array().unwrap();
        assert_eq!((blocks.len(), &blocks[0]["height"], &blocks[1]["height"]), (2, &serde_json::json!(2), &serde_json::json!(3)));
        assert!(blocks[1]["interval_secs"].is_u64());
        assert_eq!(blocks[1]["transactions"], 1);
        let first = json(get_from(&public, "/blocks/stats?before=2", "GET").await).await;
        assert_eq!(first["blocks"][0]["height"], 1);
        assert!(first["blocks"][0]["interval_secs"].is_null());

        assert_eq!(json(get_from(&public, "/network/history", "GET").await).await["samples"], serde_json::json!([]));
        {
            let _gate = state.gate.lock().unwrap();
            record_network_sample(&state).unwrap();
        }
        let history = json(get_from(&public, "/network/history", "GET").await).await;
        assert_eq!(history["samples"][0]["height"], 3);
        assert!(json(get_from(&public, "/", "GET").await).await["endpoints"]["/scan?from=H&count=N"].is_string());
    }
}
