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
        wallet::Address,
    },
};
use crate::{
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
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone)]
pub struct NodeState {
    /// The chain named by the database marker.
    pub profile: Profile,
    pub storage: Arc<RocksDbStorage>,
    pub mempool: Arc<Mutex<stark::Mempool>>,
    pub gate: Arc<Mutex<()>>,
    pub work: Arc<Semaphore>,
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
        stark::state_after(&storage, profile, &parent)?;
        Ok(Self {
            profile,
            storage,
            mempool: Arc::new(Mutex::new(stark::Mempool::default())),
            gate: Arc::new(Mutex::new(())),
            work: Arc::new(Semaphore::new(MAX_CRYPTO_WORKERS)),
        })
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
        .route(
            "/transactions",
            post(submit_transaction).layer(DefaultBodyLimit::max(MAX_TRANSACTION_BYTES)),
        )
        .route("/mining/template", get(mining_template))
        .route(
            "/blocks",
            post(submit_block).layer(DefaultBodyLimit::max(MAX_BLOCK_REQUEST_BYTES)),
        )
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
        let ledger = stark::state_after(&state.storage, state.profile, &tip).map_err(ApiError::internal)?;
        let count = state.mempool.lock().map_err(|_| ApiError::internal("mempool poisoned"))?.len();
        json_response(StatusCode::OK, &serde_json::json!({
            "chain_id":state.profile.chain_id,"pool_version":2,"height":ledger.height,"hash":tip.header.hash,
            "issued_atoms":ledger.issued,"notes":ledger.tree.len(),"mempool_transactions":count,
            "pow_algorithm":state.profile.pow_algorithm(),"xmrig_compatible":state.profile.xmrig_compatible(),
            "max_transaction_bytes":MAX_TRANSACTION_BYTES,"max_block_request_bytes":MAX_BLOCK_REQUEST_BYTES,
            "stratum":super::stratum::status(),
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
        let ledger = stark::state_after(&state.storage, state.profile, &tip).map_err(ApiError::internal)?;
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
            let ledger = stark::state_after(&state.storage, state.profile, &tip).map_err(ApiError::internal)?;
            state.mempool.lock().map_err(|_| ApiError::internal("mempool poisoned"))?.revalidate(&ledger);
        }
        let canonical = state.storage.get_canonical_block_by_height(block.header.block_height)
            .map_err(ApiError::internal)?.is_some_and(|b| b.header.hash == block.header.hash);
        json_response(status, &serde_json::json!({"accepted":true,"hash":block.header.hash,"height":block.header.block_height,
            "canonical":canonical,"already_known":known,"tip_hash":tip.header.hash,"tip_height":tip.header.block_height}))
    }).await
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
            let ledger = stark::state_after(&storage, Profile::REGTEST, &genesis).unwrap();
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
}
