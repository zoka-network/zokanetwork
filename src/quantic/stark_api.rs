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
    http::{header, HeaderMap, StatusCode},
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
        transaction::{Transaction, MAX_TRANSACTION_BYTES},
        wallet::{Address, ShortAddress},
    },
};
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
        let transactions = stark::decode(&block).map_err(ApiError::internal)?;
        let coinbase = transactions
            .first()
            .filter(|tx| tx.statement.nullifiers.is_empty());
        let reward = coinbase
            .map(|tx| tx.openings.iter().fold(0u64, |sum, o| sum.saturating_add(o.value)))
            .unwrap_or(0);
        let size: usize = block.quantic_transactions.iter().map(Vec::len).sum();
        let nullifiers: usize = transactions.iter().map(|tx| tx.statement.nullifiers.len()).sum();
        json_response(StatusCode::OK, &serde_json::json!({
            "hash":block.header.hash,"height":block.header.block_height,"timestamp":block.header.timestamp,
            "state_root":block.header.state_root,"previous_hash":block.header.prev_hash,"nonce":block.header.nonce,
            "size_bytes":size,"reward_atoms":reward,
            "reward_zka_display":crate::economics::format_zka_amount(reward),
            "transactions_count":transactions.len(),
            "private_tx_count":transactions.len() - usize::from(coinbase.is_some()),
            "private_nullifier_count":nullifiers,"private_coinbase":coinbase.is_some(),
        }))
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
        let status = json(get(&app, "/status").await).await;
        assert_eq!(status["tip_timestamp"], block.header.timestamp);
        assert_eq!(status["peers"]["best_height"], 0);
    }
}
