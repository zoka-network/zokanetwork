//! Authenticated STARK-chain transport. The pinned ML-DSA server identity signs
//! this connection's TLS exporter, not a reusable greeting or an unbound nonce.
//! TLS must negotiate X25519MLKEM768. Only STARK v2 chains are served, each one
//! named in the descriptor and bound into the signed context.
//!
//! Peer exchange: a node announces only its own endpoint (the id must be the
//! connection's authenticated remote id) and answers `PEERS` with endpoints it
//! has itself dialed and authenticated. Neither makes a learned peer trusted:
//! blocks are validated locally whoever serves them.
//!
//! `HEAD` carries the height of the node's latest accepted checkpoint, so a
//! session fetches (`CHECKPOINT`) or offers (`SUBMIT_CHECKPOINT`) only when
//! the two sides differ. Each node verifies a checkpoint against its chain's
//! compiled authority before keeping it.
//!
//! Address directory (`stark_directory`): `GET_ADDRESS` asks for the record
//! of a short-address digest and `SUBMIT_ADDRESS` offers one, with a budget
//! per connection because each stamp costs a crypto worker to check.
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use iroh::{
    endpoint::{presets::Preset, Connection, QuicTransportConfig, RecvStream, SendStream},
    Endpoint, EndpointAddr, EndpointId, SecretKey, TransportAddr,
};
use serde::{Deserialize, Serialize};
use tokio::{sync::Semaphore, task::JoinHandle};
use zeroize::Zeroizing;
use zoka_pq::{
    MlDsa65PublicKey, MlDsa65SecretKey, MlDsa65Signature, ML_DSA_65_PUBLIC_KEY_BYTES,
    ML_DSA_65_SIGNATURE_BYTES,
};

use super::{
    stark::{self, protocol::wallet::ShortAddress, Profile},
    stark_api::{NodeState, MAX_CHECKPOINT_BYTES},
    stark_directory::{self, AddressRecord, MAX_RECORD_BYTES, SUBMITS_PER_MINUTE},
};
use crate::{
    checkpoint_authority::{Acceptance, SignedCheckpoint},
    network::pq_tls::RequiredPostQuantumTls,
    state::chain::apply_block,
    types::Block,
};
use stark::protocol::transaction::{Transaction, MAX_TRANSACTION_BYTES};

pub const ALPN: &[u8] = b"/zoka/quantic-stark/2";
pub const DESCRIPTOR_VERSION: u8 = 3;
pub const MAX_BLOCK_WIRE_BYTES: usize = 2 * stark::protocol::ledger::MAX_BLOCK_BYTES + 1024 * 1024;
const MAGIC: &[u8; 4] = b"ZQS2";
const AUTH_CONTEXT: &[u8] = b"zoka-quantic-stark-server-v2";
const EXPORTER_LABEL: &[u8] = b"EXPORTER-ZOKA-QUANTIC-STARK-v2";
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
/// Block requests sent before their answers are read (see `fetch_and_import`).
const PIPELINE: usize = 16;
/// Over Tor (an endpoint with no IP socket, `docs/MODO-TOR.md`) building the
/// circuit to an onion alone can take longer than a whole direct handshake.
const TOR_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(90);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
/// Inbound sessions. Each one holds at most one stream, and verification
/// work shares the node's two crypto permits whatever this number is.
const MAX_CONNECTIONS: usize = 64;
const HEAD: u8 = 1;
const GET_BLOCK: u8 = 2;
const SUBMIT_BLOCK: u8 = 3;
const SUBMIT_TX: u8 = 4;
const GET_HASH: u8 = 5;
const PEERS: u8 = 6;
const ANNOUNCE: u8 = 7;
const CHECKPOINT: u8 = 8;
const SUBMIT_CHECKPOINT: u8 = 9;
const GET_ADDRESS: u8 = 10;
const SUBMIT_ADDRESS: u8 = 11;
const MAX_ERROR_BYTES: usize = 2048;
const MAX_ANNOUNCE_BYTES: usize = 4096;
/// Endpoints answered per `PEERS` request.
pub const MAX_SHARED_PEERS: usize = 16;
const MAX_PEERS_RESPONSE_BYTES: usize = MAX_SHARED_PEERS * MAX_ANNOUNCE_BYTES;
/// One `PEERS` answer per connection and interval; earlier repeats get `[]`.
const PEERS_INTERVAL: Duration = Duration::from_secs(60);
/// Transport addresses a descriptor may carry.
const MAX_ADDRESSES: usize = 8;
const KEY_ID_CONTEXT: &str = "ZOKA STARK ML-DSA key id v2";

fn err(value: impl std::fmt::Display) -> String {
    value.to_string()
}

/// The 32-byte pin of an ML-DSA-65 public key. The server sends the key
/// itself during the handshake; a descriptor only has to name it.
pub fn key_id(public: &[u8]) -> String {
    hex::encode(blake3::derive_key(KEY_ID_CONTEXT, public))
}

/// Store this descriptor through an authenticated channel. Receiving a key
/// from an untrusted peer is not a substitute for pinning its expected identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeDescriptor {
    pub version: u8,
    pub chain_id: String,
    pub endpoint_addr: EndpointAddr,
    /// [`key_id`] of the server's ML-DSA-65 public key.
    pub ml_dsa_key_id: String,
}
impl NodeDescriptor {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != DESCRIPTOR_VERSION || Profile::for_chain(&self.chain_id).is_none() {
            return Err("STARK descriptor network/version".into());
        }
        let pin = hex::decode(&self.ml_dsa_key_id).map_err(err)?;
        if pin.len() != 32 || self.ml_dsa_key_id != hex::encode(&pin) {
            return Err("STARK descriptor ML-DSA key id".into());
        }
        // IP, relay or a dialable address of the stream transport (the
        // `.onion:port` of a Tor seed); no other custom transport.
        let addresses = &self.endpoint_addr.addrs;
        if addresses.len() > MAX_ADDRESSES
            || addresses.iter().any(|a| match a {
                TransportAddr::Ip(_) | TransportAddr::Relay(_) => false,
                TransportAddr::Custom(custom) => {
                    crate::network::stream_transport::dial_target(custom).is_none()
                }
                _ => true,
            })
        {
            return Err("STARK descriptor addresses".into());
        }
        Ok(())
    }
}

/// Root seed persistence belongs to the caller's encrypted key store.
/// Separate KDF domains prevent reuse of an ML-DSA seed as an Ed25519 seed.
#[derive(Clone)]
pub struct ServerIdentity {
    transport: SecretKey,
    signature: Arc<MlDsa65SecretKey>,
}
impl ServerIdentity {
    pub fn from_seed(seed: &[u8; 32]) -> Result<Self, String> {
        let classical =
            Zeroizing::new(blake3::derive_key("ZOKA STARK transport identity v2", seed));
        let post_quantum =
            Zeroizing::new(blake3::derive_key("ZOKA STARK ML-DSA identity v2", seed));
        Ok(Self {
            transport: SecretKey::from_bytes(&classical),
            signature: Arc::new(MlDsa65SecretKey::from_seed(post_quantum.as_ref()).map_err(err)?),
        })
    }
    pub fn public_key(&self) -> Vec<u8> {
        self.signature.public_key().to_bytes().to_vec()
    }
    pub fn endpoint_id(&self) -> EndpointId {
        self.transport.public()
    }
    pub fn key_id(&self) -> String {
        key_id(&self.public_key())
    }
}

fn transport_config() -> QuicTransportConfig {
    QuicTransportConfig::builder()
        .max_concurrent_bidi_streams(1u32.into())
        .max_concurrent_uni_streams(0u32.into())
        .stream_receive_window((1024 * 1024u32).into())
        .receive_window((2 * 1024 * 1024u32).into())
        .send_window(2 * 1024 * 1024)
        .max_idle_timeout(Some(
            Duration::from_secs(125)
                .try_into()
                .expect("bounded idle timeout"),
        ))
        .build()
}

fn auth_message(
    connection: &Connection,
    chain_id: &str,
    server: EndpointId,
    client: EndpointId,
    public: &[u8],
) -> Result<Zeroizing<Vec<u8>>, String> {
    let mut context = Vec::new();
    context.extend_from_slice(ALPN);
    context.push(0);
    context.extend_from_slice(chain_id.as_bytes());
    context.push(0);
    context.extend_from_slice(server.as_bytes());
    context.extend_from_slice(client.as_bytes());
    context.extend_from_slice(public);
    let mut exporter = Zeroizing::new([0u8; 64]);
    connection
        .export_keying_material(exporter.as_mut(), EXPORTER_LABEL, &context)
        .map_err(|error| format!("TLS exporter failed: {error:?}"))?;
    let mut message = Zeroizing::new(b"server\0".to_vec());
    message.extend_from_slice(&context);
    message.extend_from_slice(exporter.as_ref());
    Ok(message)
}

pub struct Server {
    pub endpoint: Endpoint,
    pub descriptor: NodeDescriptor,
    task: JoinHandle<()>,
}
impl Server {
    pub async fn close(&self) {
        self.endpoint.close().await;
        self.task.abort();
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub async fn serve(
    state: NodeState,
    identity: ServerIdentity,
    preset: impl Preset,
) -> Result<Server, String> {
    let endpoint = Endpoint::builder(RequiredPostQuantumTls(preset))
        .secret_key(identity.transport.clone())
        .transport_config(transport_config())
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await
        .map_err(err)?;
    let descriptor = NodeDescriptor {
        version: DESCRIPTOR_VERSION,
        chain_id: state.profile.chain_id.into(),
        endpoint_addr: endpoint.addr(),
        ml_dsa_key_id: identity.key_id(),
    };
    let listener = endpoint.clone();
    let connections = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    let task = tokio::spawn(async move {
        while let Some(incoming) = listener.accept().await {
            let Ok(permit) = connections.clone().try_acquire_owned() else {
                incoming.refuse();
                continue;
            };
            let state = state.clone();
            let identity = identity.clone();
            let server_id = listener.id();
            tokio::spawn(async move {
                let _permit = permit;
                let _inbound = state.peers.inbound();
                // A node in Tor mode arrives on the stream transport, whose
                // circuit adds seconds to every round trip.
                let limit = match incoming.remote_addr() {
                    iroh::endpoint::IncomingAddr::Custom(_) => TOR_HANDSHAKE_TIMEOUT,
                    _ => HANDSHAKE_TIMEOUT,
                };
                let connected = tokio::time::timeout(limit, async {
                    let connection = incoming.await.map_err(err)?;
                    let (send, recv) =
                        authenticate_server(&connection, server_id, &identity, &state).await?;
                    Ok::<_, String>((connection, send, recv))
                })
                .await;
                if let Ok(Ok((connection, send, recv))) = connected {
                    let _ = serve_requests(&connection, send, recv, state).await;
                    connection.close(0u32.into(), b"STARK session ended");
                }
            });
        }
    });
    Ok(Server {
        endpoint,
        descriptor,
        task,
    })
}

async fn authenticate_server(
    connection: &Connection,
    server: EndpointId,
    identity: &ServerIdentity,
    state: &NodeState,
) -> Result<(SendStream, RecvStream), String> {
    let (mut send, mut recv) = connection.accept_bi().await.map_err(err)?;
    let mut magic = [0u8; 4];
    recv.read_exact(&mut magic).await.map_err(err)?;
    if &magic != MAGIC {
        return Err("STARK handshake version".into());
    }
    let public = identity.public_key();
    let message = auth_message(
        connection,
        state.profile.chain_id,
        server,
        connection.remote_id(),
        &public,
    )?;
    let key = identity.signature.clone();
    let permit = state.work.clone().acquire_owned().await.map_err(err)?;
    let signature = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        key.sign(message.as_ref(), AUTH_CONTEXT)
            .map(|sig| sig.to_bytes().to_vec())
            .map_err(err)
    })
    .await
    .map_err(err)??;
    send.write_all(&public).await.map_err(err)?;
    send.write_all(&signature).await.map_err(err)?;
    Ok((send, recv))
}

fn request_limit(tag: u8) -> Result<usize, String> {
    match tag {
        HEAD | PEERS | CHECKPOINT => Ok(0),
        GET_BLOCK | GET_HASH => Ok(8),
        SUBMIT_BLOCK => Ok(MAX_BLOCK_WIRE_BYTES),
        SUBMIT_TX => Ok(MAX_TRANSACTION_BYTES),
        ANNOUNCE => Ok(MAX_ANNOUNCE_BYTES),
        SUBMIT_CHECKPOINT => Ok(MAX_CHECKPOINT_BYTES),
        GET_ADDRESS => Ok(32),
        SUBMIT_ADDRESS => Ok(MAX_RECORD_BYTES),
        _ => Err("unknown STARK request".into()),
    }
}

async fn read_request_header(recv: &mut RecvStream) -> Result<(u8, usize), String> {
    let mut header = [0u8; 5];
    recv.read_exact(&mut header).await.map_err(err)?;
    let tag = header[0];
    let len = u32::from_be_bytes(header[1..].try_into().unwrap()) as usize;
    if len > request_limit(tag)?
        || (matches!(tag, GET_BLOCK | GET_HASH) && len != 8)
        || (tag == GET_ADDRESS && len != 32)
    {
        return Err("STARK request length".into());
    }
    Ok((tag, len))
}

async fn write_response(
    send: &mut SendStream,
    response: Result<Vec<u8>, String>,
) -> Result<(), String> {
    let (status, bytes) = match response {
        Ok(bytes) if bytes.len() <= MAX_BLOCK_WIRE_BYTES => (0u8, bytes),
        Ok(_) => (1, b"STARK response size limit".to_vec()),
        Err(reason) => (
            1,
            reason
                .into_bytes()
                .into_iter()
                .take(MAX_ERROR_BYTES)
                .collect(),
        ),
    };
    send.write_all(&[status]).await.map_err(err)?;
    send.write_all(&(bytes.len() as u32).to_be_bytes())
        .await
        .map_err(err)?;
    send.write_all(&bytes).await.map_err(err)
}

/// Peer-exchange and directory limits of one inbound connection.
#[derive(Default)]
struct Exchange {
    announced: bool,
    answered: Option<Instant>,
    /// Start of the current minute and the records submitted in it.
    submitted: Option<(Instant, u32)>,
}

impl Exchange {
    /// Count one `SUBMIT_ADDRESS`, if this minute's budget has room.
    fn submit_allowed(&mut self) -> bool {
        let now = Instant::now();
        let (start, count) = match self.submitted {
            Some((start, count)) if now.duration_since(start) < Duration::from_secs(60) => {
                (start, count)
            }
            _ => (now, 0),
        };
        let allowed = count < SUBMITS_PER_MINUTE;
        self.submitted = Some((start, count + u32::from(allowed)));
        allowed
    }
}

/// The stored record of a short-address digest, or an empty answer. A point
/// read: no crypto permit and no gate.
fn address_answer(state: &NodeState, digest: [u8; 32]) -> Result<Vec<u8>, String> {
    let short = ShortAddress {
        network: state.profile.network,
        digest,
    };
    match stark_directory::lookup(&state.storage, &short).map_err(err)? {
        Some((_, record)) => serde_json::to_vec(&record).map_err(err),
        None => Ok(Vec::new()),
    }
}

/// Caller holds a crypto permit; the directory has its own lock.
fn accept_address(state: &NodeState, body: &[u8]) -> Result<Vec<u8>, String> {
    let record: AddressRecord = serde_json::from_slice(body).map_err(err)?;
    let (_, new) = state
        .directory
        .store(&state.storage, state.profile.network, &record)
        .map_err(err)?;
    Ok(if new { b"accepted".to_vec() } else { b"known".to_vec() })
}

/// Cheap and bounded: answered without a crypto permit or the state gate.
fn peer_request(
    state: &NodeState,
    remote: EndpointId,
    tag: u8,
    body: &[u8],
    exchange: &mut Exchange,
) -> Result<Vec<u8>, String> {
    if tag == ANNOUNCE {
        if std::mem::replace(&mut exchange.announced, true) {
            return Err("one STARK announce per connection".into());
        }
        let descriptor: NodeDescriptor = serde_json::from_slice(body).map_err(err)?;
        descriptor.validate()?;
        if descriptor.chain_id != state.profile.chain_id {
            return Err("STARK announce for another chain".into());
        }
        if descriptor.endpoint_addr.id != remote {
            return Err("STARK announce must name the announcing endpoint".into());
        }
        state.peers.book().learn(descriptor);
        return Ok(Vec::new());
    }
    let fresh = exchange
        .answered
        .is_none_or(|at| at.elapsed() >= PEERS_INTERVAL);
    let peers = if fresh {
        exchange.answered = Some(Instant::now());
        state
            .peers
            .book()
            .share(MAX_SHARED_PEERS, super::now_secs())
    } else {
        Vec::new()
    };
    serde_json::to_vec(&peers).map_err(err)
}

async fn serve_requests(
    connection: &Connection,
    mut send: SendStream,
    mut recv: RecvStream,
    state: NodeState,
) -> Result<(), String> {
    let mut exchange = Exchange::default();
    loop {
        let request = async {
            let (tag, len) = read_request_header(&mut recv).await?;
            if matches!(tag, PEERS | ANNOUNCE) {
                let mut body = vec![0; len];
                recv.read_exact(&mut body).await.map_err(err)?;
                let response =
                    peer_request(&state, connection.remote_id(), tag, &body, &mut exchange);
                return write_response(&mut send, response).await;
            }
            if tag == GET_ADDRESS {
                let mut digest = [0u8; 32];
                recv.read_exact(&mut digest).await.map_err(err)?;
                let shared = state.clone();
                let response = tokio::task::spawn_blocking(move || address_answer(&shared, digest))
                    .await
                    .map_err(err)?;
                return write_response(&mut send, response).await;
            }
            if tag == SUBMIT_ADDRESS {
                let mut body = vec![0; len];
                recv.read_exact(&mut body).await.map_err(err)?;
                if !exchange.submit_allowed() {
                    let limit = format!(
                        "STARK address submissions are limited to {SUBMITS_PER_MINUTE} per minute"
                    );
                    return write_response(&mut send, Err(limit)).await;
                }
                let shared = state.clone();
                let permit = shared.work.clone().acquire_owned().await.map_err(err)?;
                let response = tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    accept_address(&shared, &body)
                })
                .await
                .map_err(err)?;
                return write_response(&mut send, response).await;
            }
            let shared = state.clone();
            let permit = shared.work.clone().acquire_owned().await.map_err(err)?;
            // Read only the small fixed header before admission. Large bodies
            // share the same bounded budget as API requests and proof jobs.
            let mut body = vec![0; len];
            recv.read_exact(&mut body).await.map_err(err)?;
            // The permit lives in the blocking task, so cancelling a timed-out
            // network future cannot start more expensive verifications.
            let response = tokio::task::spawn_blocking(move || {
                let _permit = permit;
                handle_request(&shared, tag, &body)
            })
            .await
            .map_err(err)?;
            write_response(&mut send, response).await
        };
        if tokio::time::timeout(REQUEST_TIMEOUT, request)
            .await
            .map_err(|_| "STARK request timeout")?
            .is_err()
        {
            connection.close(1u32.into(), b"invalid STARK request");
            return Err("STARK request ended".into());
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Head {
    pub chain_id: String,
    pub height: u64,
    pub hash: String,
    /// Height of the latest accepted checkpoint.
    #[serde(default)]
    pub checkpoint: Option<u64>,
}

fn tip(state: &NodeState) -> Result<Block, String> {
    if state.storage.get_chain_height().map_err(err)? == 0 {
        return Ok(state.profile.genesis_parent());
    }
    let hash = state.storage.get_last_block_hash().map_err(err)?;
    state
        .storage
        .get_block(&hash)
        .map_err(err)?
        .ok_or_else(|| "missing STARK tip".into())
}
/// Caller holds the gate.
pub(crate) fn head(state: &NodeState) -> Result<Head, String> {
    let block = tip(state)?;
    Ok(Head {
        chain_id: state.profile.chain_id.into(),
        height: block.header.block_height,
        hash: block.header.hash,
        checkpoint: state.latest_checkpoint()?.map(|cp| cp.height),
    })
}

fn handle_request(state: &NodeState, tag: u8, body: &[u8]) -> Result<Vec<u8>, String> {
    let _gate = state.gate.lock().map_err(|_| "STARK state lock poisoned")?;
    match tag {
        HEAD => serde_json::to_vec(&head(state)?).map_err(err),
        GET_BLOCK => {
            let height =
                u64::from_be_bytes(body.try_into().map_err(|_| "STARK block height length")?);
            match state
                .storage
                .get_canonical_block_by_height(height)
                .map_err(err)?
            {
                Some(block) => serde_json::to_vec(&block).map_err(err),
                None => Ok(Vec::new()),
            }
        }
        GET_HASH => {
            let height =
                u64::from_be_bytes(body.try_into().map_err(|_| "STARK hash height length")?);
            if height == 0 {
                return Ok(state.profile.genesis_parent().header.hash.into_bytes());
            }
            Ok(state
                .storage
                .get_canonical_block_hash_at_height(height)
                .map_err(err)?
                .unwrap_or_default()
                .into_bytes())
        }
        SUBMIT_TX => {
            let tx = Transaction::decode(body).map_err(err)?;
            let txid = tx.txid();
            let ledger = stark::branch(&state.storage, state.profile, &tip(state)?)?;
            state
                .mempool
                .lock()
                .map_err(|_| "STARK mempool lock poisoned")?
                .insert(&ledger, tx)?;
            Ok(txid.to_vec())
        }
        SUBMIT_BLOCK => {
            let block: Block = serde_json::from_slice(body).map_err(err)?;
            import_block_locked(state, &block)?;
            serde_json::to_vec(&head(state)?).map_err(err)
        }
        CHECKPOINT => serde_json::to_vec(&state.latest_checkpoint()?).map_err(err),
        SUBMIT_CHECKPOINT => {
            let checkpoint: SignedCheckpoint = serde_json::from_slice(body).map_err(err)?;
            Ok(match state.accept_checkpoint(&checkpoint)? {
                Acceptance::Accepted => b"accepted".to_vec(),
                Acceptance::AlreadyKnown => b"known".to_vec(),
            })
        }
        _ => Err("unknown STARK request".into()),
    }
}

/// Caller holds the shared mutation gate. A validated side branch is persisted
/// even when it has not yet accumulated more work than the current tip.
fn import_block_locked(state: &NodeState, block: &Block) -> Result<bool, String> {
    use crate::state::chain::ChainError;
    let previous = tip(state)?.header.hash;
    let fresh = match apply_block(&state.storage, state.profile.chain_id, block) {
        Ok(_) | Err(ChainError::SideBranchNotHeavier { .. }) => true,
        Err(ChainError::BlockAlreadyKnown(_)) => false,
        Err(error) => return Err(err(error)),
    };
    let current = tip(state)?;
    if current.header.hash != previous {
        let ledger = stark::branch(&state.storage, state.profile, &current)?;
        state
            .mempool
            .lock()
            .map_err(|_| "STARK mempool lock poisoned")?
            .revalidate(&ledger);
    }
    Ok(fresh)
}

fn client_work() -> Arc<Semaphore> {
    static WORK: std::sync::OnceLock<Arc<Semaphore>> = std::sync::OnceLock::new();
    WORK.get_or_init(|| Arc::new(Semaphore::new(2))).clone()
}

pub struct Client {
    /// From the pinned descriptor: every head must name this chain.
    chain_id: String,
    endpoint: Endpoint,
    /// False when the endpoint is the node's own server endpoint, shared by
    /// every outbound session and closed only with the server.
    owned: bool,
    connection: Connection,
    send: SendStream,
    recv: RecvStream,
}

impl Client {
    /// A one-off client on its own endpoint (mirror sync, broadcast).
    pub async fn connect(descriptor: &NodeDescriptor, preset: impl Preset) -> Result<Self, String> {
        descriptor.validate()?;
        let endpoint = Endpoint::builder(RequiredPostQuantumTls(preset))
            .transport_config(transport_config())
            .bind()
            .await
            .map_err(err)?;
        Self::handshake(endpoint, true, descriptor).await
    }

    /// Dial from a node's server endpoint. The peer then sees this node's own
    /// endpoint id, which is what lets it accept this node's announce.
    pub async fn connect_from(
        endpoint: &Endpoint,
        descriptor: &NodeDescriptor,
    ) -> Result<Self, String> {
        descriptor.validate()?;
        if descriptor.endpoint_addr.id == endpoint.id() {
            return Err("STARK descriptor names this node".into());
        }
        Self::handshake(endpoint.clone(), false, descriptor).await
    }

    async fn handshake(
        endpoint: Endpoint,
        owned: bool,
        descriptor: &NodeDescriptor,
    ) -> Result<Self, String> {
        let limit = if endpoint.bound_sockets().is_empty() {
            TOR_HANDSHAKE_TIMEOUT
        } else {
            HANDSHAKE_TIMEOUT
        };
        let authenticated = tokio::time::timeout(limit, async {
            let connection = endpoint
                .connect(descriptor.endpoint_addr.clone(), ALPN)
                .await
                .map_err(err)?;
            let (mut send, mut recv) = connection.open_bi().await.map_err(err)?;
            send.write_all(MAGIC).await.map_err(err)?;
            let mut public = vec![0u8; ML_DSA_65_PUBLIC_KEY_BYTES];
            let mut signature = vec![0u8; ML_DSA_65_SIGNATURE_BYTES];
            recv.read_exact(&mut public).await.map_err(err)?;
            recv.read_exact(&mut signature).await.map_err(err)?;
            if key_id(&public) != descriptor.ml_dsa_key_id {
                connection.close(1u32.into(), b"STARK pinned key mismatch");
                return Err("STARK pinned ML-DSA key mismatch".into());
            }
            let message = auth_message(
                &connection,
                &descriptor.chain_id,
                connection.remote_id(),
                endpoint.id(),
                &public,
            )?;
            let permit = client_work().acquire_owned().await.map_err(err)?;
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                let key = MlDsa65PublicKey::from_bytes(&public).map_err(err)?;
                let signature = MlDsa65Signature::from_bytes(&signature).map_err(err)?;
                key.verify(message.as_ref(), AUTH_CONTEXT, &signature)
                    .map_err(err)
            })
            .await
            .map_err(err)??;
            Ok::<_, String>((connection, send, recv))
        })
        .await;
        match authenticated {
            Ok(Ok((connection, send, recv))) => Ok(Self {
                chain_id: descriptor.chain_id.clone(),
                endpoint,
                owned,
                connection,
                send,
                recv,
            }),
            other => {
                if owned {
                    endpoint.close().await;
                }
                Err(match other {
                    Ok(Err(reason)) => reason,
                    _ => "STARK authentication timeout".into(),
                })
            }
        }
    }

    pub async fn close(&self) {
        self.connection.close(0u32.into(), b"client done");
        if self.owned {
            self.endpoint.close().await;
        }
    }

    pub fn remote_id(&self) -> EndpointId {
        self.connection.remote_id()
    }

    /// The peer's latest accepted checkpoint. Not verified here: callers keep
    /// it only through `NodeState::accept_checkpoint`.
    pub async fn checkpoint(&mut self) -> Result<Option<SignedCheckpoint>, String> {
        let bytes = self.request(CHECKPOINT, &[], MAX_CHECKPOINT_BYTES).await?;
        let checkpoint: Option<SignedCheckpoint> = serde_json::from_slice(&bytes).map_err(err)?;
        if checkpoint.as_ref().is_some_and(|cp| cp.chain_id != self.chain_id) {
            return Err("STARK checkpoint for another chain".into());
        }
        Ok(checkpoint)
    }

    pub async fn submit_checkpoint(&mut self, checkpoint: &SignedCheckpoint) -> Result<(), String> {
        let bytes = serde_json::to_vec(checkpoint).map_err(err)?;
        self.request(SUBMIT_CHECKPOINT, &bytes, 16).await.map(drop)
    }

    /// Endpoints the peer has authenticated itself. Unverified here: callers
    /// dial them before sharing them further.
    pub async fn peers(&mut self) -> Result<Vec<NodeDescriptor>, String> {
        let bytes = self.request(PEERS, &[], MAX_PEERS_RESPONSE_BYTES).await?;
        let peers: Vec<NodeDescriptor> = serde_json::from_slice(&bytes).map_err(err)?;
        if peers.len() > MAX_SHARED_PEERS {
            return Err("STARK peers response length".into());
        }
        Ok(peers
            .into_iter()
            .filter(|peer| peer.validate().is_ok() && peer.chain_id == self.chain_id)
            .collect())
    }

    /// The peer's record of a short-address digest. Unverified here: callers
    /// keep it only through `Directory::store`, after checking that it is the
    /// address asked for.
    pub async fn get_address(&mut self, digest: &[u8; 32]) -> Result<Option<AddressRecord>, String> {
        let bytes = self.request(GET_ADDRESS, digest, MAX_RECORD_BYTES).await?;
        if bytes.is_empty() {
            return Ok(None);
        }
        serde_json::from_slice(&bytes).map(Some).map_err(err)
    }

    /// Whether the peer kept the record as new.
    pub async fn submit_address(&mut self, record: &AddressRecord) -> Result<bool, String> {
        let bytes = serde_json::to_vec(record).map_err(err)?;
        match self.request(SUBMIT_ADDRESS, &bytes, 16).await?.as_slice() {
            b"accepted" => Ok(true),
            b"known" => Ok(false),
            _ => Err("STARK address acknowledgement".into()),
        }
    }

    /// Announce this node's own server endpoint. Only meaningful on a client
    /// made with [`Client::connect_from`] on that endpoint.
    pub async fn announce(&mut self, own: &NodeDescriptor) -> Result<(), String> {
        let bytes = serde_json::to_vec(own).map_err(err)?;
        self.request(ANNOUNCE, &bytes, 0).await.map(drop)
    }

    async fn request(
        &mut self,
        tag: u8,
        body: &[u8],
        max_response: usize,
    ) -> Result<Vec<u8>, String> {
        if body.len() > request_limit(tag)? {
            return Err("STARK request size limit".into());
        }
        let response = tokio::time::timeout(REQUEST_TIMEOUT, async {
            self.write_request(tag, body).await?;
            self.read_response(max_response).await
        })
        .await;
        match response {
            Ok(Ok((0, bytes))) => Ok(bytes),
            Ok(Ok((_, bytes))) => Err(format!(
                "STARK peer rejected request: {}",
                String::from_utf8_lossy(&bytes)
            )),
            other => {
                self.connection.close(1u32.into(), b"STARK framing failure");
                Err(match other {
                    Ok(Err(reason)) => reason,
                    _ => "STARK request timeout".into(),
                })
            }
        }
    }

    async fn write_request(&mut self, tag: u8, body: &[u8]) -> Result<(), String> {
        self.send.write_all(&[tag]).await.map_err(err)?;
        self.send
            .write_all(&(body.len() as u32).to_be_bytes())
            .await
            .map_err(err)?;
        self.send.write_all(body).await.map_err(err)
    }

    /// The next response on the stream: its status (0 ok, 1 refused) and body.
    async fn read_response(&mut self, max_response: usize) -> Result<(u8, Vec<u8>), String> {
        let mut header = [0u8; 5];
        self.recv.read_exact(&mut header).await.map_err(err)?;
        let len = u32::from_be_bytes(header[1..].try_into().unwrap()) as usize;
        let limit = match header[0] {
            0 => max_response,
            1 => MAX_ERROR_BYTES,
            _ => return Err("STARK response status".into()),
        };
        if len > limit {
            return Err("STARK response size limit".into());
        }
        let mut bytes = vec![0; len];
        self.recv.read_exact(&mut bytes).await.map_err(err)?;
        Ok((header[0], bytes))
    }

    /// Blocks `heights`, asked for up to [`PIPELINE`] at a time before reading
    /// the answers, which the peer sends back in order: one round trip per
    /// window instead of one per block (over Tor, a second or more each).
    /// Imports each as it arrives; any failure closes the connection, since
    /// answers to requests already sent would still be on the stream.
    async fn fetch_and_import(
        &mut self,
        state: &NodeState,
        heights: std::ops::RangeInclusive<u64>,
    ) -> Result<usize, String> {
        let heights: Vec<u64> = heights.collect();
        let mut imported = 0;
        for window in heights.chunks(PIPELINE) {
            let result = tokio::time::timeout(REQUEST_TIMEOUT, async {
                for height in window {
                    self.write_request(GET_BLOCK, &height.to_be_bytes()).await?;
                }
                let mut fresh_blocks = 0;
                for &height in window {
                    let (status, bytes) = self.read_response(MAX_BLOCK_WIRE_BYTES).await?;
                    if status != 0 {
                        return Err(format!(
                            "STARK peer rejected request: {}",
                            String::from_utf8_lossy(&bytes)
                        ));
                    }
                    if bytes.is_empty() {
                        return Err("STARK remote block unavailable".into());
                    }
                    let permit = client_work().acquire_owned().await.map_err(err)?;
                    let block = tokio::task::spawn_blocking(move || {
                        let _permit = permit;
                        let block: Block = serde_json::from_slice(&bytes).map_err(err)?;
                        if block.header.block_height != height {
                            return Err("STARK response block height".into());
                        }
                        Ok::<_, String>(block)
                    })
                    .await
                    .map_err(err)??;
                    if with_state(state, move |state| import_block_locked(state, &block)).await? {
                        fresh_blocks += 1;
                    }
                }
                Ok::<_, String>(fresh_blocks)
            })
            .await;
            match result {
                Ok(Ok(fresh)) => imported += fresh,
                other => {
                    self.connection.close(1u32.into(), b"STARK pipelined fetch failed");
                    return Err(match other {
                        Ok(Err(reason)) => reason,
                        _ => "STARK request timeout".into(),
                    });
                }
            }
        }
        Ok(imported)
    }

    pub async fn head(&mut self) -> Result<Head, String> {
        decode_head(&self.request(HEAD, &[], 1024).await?, &self.chain_id)
    }

    /// Transport authentication is not consensus validation. `pull` verifies
    /// fetched blocks locally; callers using this lower-level method must too.
    pub async fn get_block(&mut self, height: u64) -> Result<Option<Block>, String> {
        let permit = client_work().acquire_owned().await.map_err(err)?;
        let bytes = self
            .request(GET_BLOCK, &height.to_be_bytes(), MAX_BLOCK_WIRE_BYTES)
            .await?;
        if bytes.is_empty() {
            return Ok(None);
        }
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let block: Block = serde_json::from_slice(&bytes).map_err(err)?;
            if block.header.block_height != height {
                return Err("STARK response block height".into());
            }
            Ok(Some(block))
        })
        .await
        .map_err(err)?
    }

    pub async fn block_hash(&mut self, height: u64) -> Result<Option<String>, String> {
        let bytes = self.request(GET_HASH, &height.to_be_bytes(), 64).await?;
        if bytes.is_empty() {
            return Ok(None);
        }
        let hash = String::from_utf8(bytes).map_err(err)?;
        if hash.len() != 64 || hex::decode(&hash).is_err() {
            return Err("STARK block hash encoding".into());
        }
        Ok(Some(hash))
    }

    pub async fn submit_tx(&mut self, tx: &Transaction) -> Result<[u8; 32], String> {
        let bytes = tx.encode().map_err(err)?;
        let id: [u8; 32] = self
            .request(SUBMIT_TX, &bytes, 32)
            .await?
            .try_into()
            .map_err(|_| "STARK tx acknowledgement length")?;
        if id != tx.txid() {
            return Err("STARK tx acknowledgement mismatch".into());
        }
        Ok(id)
    }

    pub async fn submit_block(&mut self, block: &Block) -> Result<Head, String> {
        let bytes = serde_json::to_vec(block).map_err(err)?;
        decode_head(&self.request(SUBMIT_BLOCK, &bytes, 1024).await?, &self.chain_id)
    }

    pub async fn pull(&mut self, state: &NodeState) -> Result<usize, String> {
        self.pull_limited(state, 64).await
    }

    /// Fetch at most `max_blocks` new blocks. Locate the common canonical
    /// ancestor by hash, then skip an already validated remote branch prefix.
    /// The latter lets repeated bounded pulls finish forks longer than a batch.
    pub async fn pull_limited(
        &mut self,
        state: &NodeState,
        max_blocks: usize,
    ) -> Result<usize, String> {
        if max_blocks == 0 || max_blocks > 256 {
            return Err("STARK pull batch limit must be 1..256".into());
        }
        if state.profile.chain_id != self.chain_id {
            return Err("STARK peer serves another chain".into());
        }
        let remote = self.head().await?;
        let local = with_state(state, |state| head(state)).await?;
        if remote.height == 0 || (remote.height == local.height && remote.hash == local.hash) {
            return Ok(0);
        }
        // The usual case, the peer extends this node's tip: one question
        // instead of two binary searches (about 20 round trips at a thousand
        // blocks, which over Tor dominate the sync).
        if local.height > 0 && remote.height > local.height {
            if self.block_hash(local.height).await?.as_deref() == Some(local.hash.as_str()) {
                let last = remote.height.min(local.height.saturating_add(max_blocks as u64));
                return self.fetch_and_import(state, local.height + 1..=last).await;
            }
        }
        let mut low = 0;
        let mut high = local.height.min(remote.height) + 1;
        while low + 1 < high {
            let mid = low + (high - low) / 2;
            let remote_hash = self
                .block_hash(mid)
                .await?
                .ok_or("STARK remote chain changed during locator")?;
            let local_hash = with_state(state, move |state| {
                state
                    .storage
                    .get_canonical_block_hash_at_height(mid)
                    .map_err(err)
            })
            .await?;
            if local_hash.as_deref() == Some(&remote_hash) {
                low = mid;
            } else {
                high = mid;
            }
        }
        // All ancestors of a locally validated side-branch block were also
        // validated. Binary-search that prefix without assuming it is canonical.
        high = remote.height + 1;
        while low + 1 < high {
            let mid = low + (high - low) / 2;
            let hash = self
                .block_hash(mid)
                .await?
                .ok_or("STARK remote chain changed during locator")?;
            let known = with_state(state, move |state| {
                Ok(state.storage.block_exists(&hash).map_err(err)?
                    && state
                        .storage
                        .get_raw(&format!("chain:block_meta:{hash}"))
                        .map_err(err)?
                        .is_some())
            })
            .await?;
            if known {
                low = mid;
            } else {
                high = mid;
            }
        }
        let last = remote.height.min(low.saturating_add(max_blocks as u64));
        if last <= low {
            return Ok(0);
        }
        self.fetch_and_import(state, low + 1..=last).await
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        self.connection.close(0u32.into(), b"client dropped");
    }
}

fn decode_head(bytes: &[u8], chain_id: &str) -> Result<Head, String> {
    let head: Head = serde_json::from_slice(bytes).map_err(err)?;
    if head.chain_id != chain_id
        || head.height > u32::MAX as u64
        || head.hash.len() != 64
        || hex::decode(&head.hash).is_err()
        || head.checkpoint.is_some_and(|h| h == 0 || h > u32::MAX as u64)
    {
        return Err("STARK head network/height/hash".into());
    }
    Ok(head)
}

async fn with_state<T: Send + 'static>(
    state: &NodeState,
    work: impl FnOnce(&NodeState) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let state = state.clone();
    tokio::time::timeout(REQUEST_TIMEOUT, async move {
        let permit = state.work.clone().acquire_owned().await.map_err(err)?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let _gate = state.gate.lock().map_err(|_| "STARK state lock poisoned")?;
            work(&state)
        })
        .await
        .map_err(err)?
    })
    .await
    .map_err(|_| "STARK local validation timeout")?
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::endpoint::presets;
    use stark::protocol::{
        ledger::{ATOMS, PREMINE},
        model::Input,
        transaction::{transfer, Payment},
        wallet::Account,
    };

    const CHAIN: &str = Profile::REGTEST.chain_id;

    fn state() -> (NodeState, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(crate::storage::RocksDbStorage::new(dir.path()).unwrap());
        storage
            .put_raw(
                super::super::stark_api::DB_MARKER,
                CHAIN.as_bytes().to_vec(),
            )
            .unwrap();
        (NodeState::new(storage).unwrap(), dir)
    }
    fn localhost(server: &Server) -> NodeDescriptor {
        let mut descriptor = server.descriptor.clone();
        let socket = server
            .endpoint
            .bound_sockets()
            .into_iter()
            .find(|a| a.is_ipv4())
            .unwrap();
        descriptor.endpoint_addr = EndpointAddr::new(server.endpoint.id()).with_ip_addr(
            std::net::SocketAddr::new(std::net::Ipv4Addr::LOCALHOST.into(), socket.port()),
        );
        descriptor
    }
    async fn mine_on(
        state: &NodeState,
        parent: Block,
        txs: Vec<Transaction>,
        timestamp: u64,
    ) -> Block {
        with_state(state, move |state| {
            let alice = Account::from_seed(&[31; 32], 3, 0).map_err(err)?;
            let block = stark::mine(
                &state.storage,
                state.profile,
                &parent,
                &alice.address(),
                txs,
                timestamp,
            )?;
            import_block_locked(state, &block)?;
            Ok(block)
        })
        .await
        .unwrap()
    }

    /// Tor mode's transport without Tor: a node that has no UDP socket and no
    /// relay syncs from a server that listens only on the stream transport,
    /// with the same pinned ML-DSA identity and hybrid TLS, and gets a mined
    /// block back to it (`docs/MODO-TOR.md`, E2).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_node_syncs_and_submits_over_the_stream_transport_alone() {
        use crate::network::stream_transport::{StreamOnly, StreamTransport, TcpDialer};
        let (remote, _remote_dir) = state();
        let (local, _local_dir) = state();
        let genesis = mine_on(&remote, Profile::REGTEST.genesis_parent(), vec![], 1_700_000_000).await;
        let second = mine_on(&remote, genesis, vec![], 1_700_000_120).await;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = listener.local_addr().unwrap().to_string();
        let transport = StreamTransport::new(Some(&target), Arc::new(TcpDialer)).unwrap();
        let server = serve(
            remote.clone(),
            ServerIdentity::from_seed(&[73; 32]).unwrap(),
            StreamOnly(transport.clone()),
        )
        .await
        .unwrap();
        let _listening = transport.listen(listener);
        // Like `localhost` for IP: the endpoint has not reported its custom
        // address yet when the descriptor is built, so the node states it
        // (`--stream-address` in Tor mode). It validates.
        let mut descriptor = server.descriptor.clone();
        descriptor.endpoint_addr = EndpointAddr::new(server.endpoint.id()).with_addrs([
            TransportAddr::Custom(crate::network::stream_transport::dial_addr(&target).unwrap()),
        ]);
        descriptor.validate().unwrap();
        let encoded = serde_json::to_vec(&descriptor).unwrap();
        let descriptor: NodeDescriptor = serde_json::from_slice(&encoded).unwrap();

        let dialer = StreamTransport::new(None, Arc::new(TcpDialer)).unwrap();
        let mut client = Client::connect(&descriptor, StreamOnly(dialer.clone()))
            .await
            .unwrap();
        assert_eq!(client.head().await.unwrap().height, 2);
        assert_eq!(client.pull(&local).await.unwrap(), 2);
        assert_eq!(
            local.storage.get_last_block_hash().unwrap(),
            second.header.hash
        );
        let third = mine_on(&local, second, vec![], 1_700_000_240).await;
        assert_eq!(
            client.submit_block(&third).await.unwrap().hash,
            third.header.hash
        );
        assert_eq!(
            remote.storage.get_last_block_hash().unwrap(),
            third.header.hash
        );
        assert_eq!(dialer.link_count(), 1);
        server.close().await;
    }

    /// E3 for real: the server listens on loopback behind an onion service of
    /// a local `tor.exe`, and a node with no UDP socket dials its `.onion`
    /// through that Tor's SOCKS port, syncs and hands back a mined block.
    /// Needs `ZOKA_TEST_TOR_EXE` (third_party/tor/.../tor.exe) and the Tor
    /// network: `cargo test --features quantic-pq --lib tor_from_an_onion -- --ignored`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "needs tor.exe (ZOKA_TEST_TOR_EXE) and the Tor network"]
    async fn a_node_syncs_through_tor_from_an_onion_seed() {
        use crate::network::stream_transport::{
            dial_addr, SocksDialer, StreamOnly, StreamTransport, TcpDialer,
        };
        use tokio::io::AsyncBufReadExt;
        let tor = std::env::var("ZOKA_TEST_TOR_EXE").expect("set ZOKA_TEST_TOR_EXE");
        let (remote, _remote_dir) = state();
        let (local, _local_dir) = state();
        let genesis = mine_on(&remote, Profile::REGTEST.genesis_parent(), vec![], 1_700_000_000).await;
        let second = mine_on(&remote, genesis, vec![], 1_700_000_120).await;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let service_port = listener.local_addr().unwrap().port();
        let socks_port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let tor_dir = tempfile::tempdir().unwrap();
        let slashes = |path: std::path::PathBuf| path.display().to_string().replace('\\', "/");
        let hidden = tor_dir.path().join("hs");
        let torrc = tor_dir.path().join("torrc");
        std::fs::write(
            &torrc,
            format!(
                "SocksPort 127.0.0.1:{socks_port}\nDataDirectory {}\nHiddenServiceDir {}\n\
                 HiddenServicePort 9444 127.0.0.1:{service_port}\nLog notice stdout\n",
                slashes(tor_dir.path().join("data")),
                slashes(hidden.clone()),
            ),
        )
        .unwrap();
        let mut child = tokio::process::Command::new(&tor)
            .arg("-f")
            .arg(&torrc)
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut lines = tokio::io::BufReader::new(child.stdout.take().unwrap()).lines();
        tokio::time::timeout(std::time::Duration::from_secs(240), async {
            while let Some(line) = lines.next_line().await.unwrap() {
                if line.contains("Bootstrapped 100%") {
                    break;
                }
            }
        })
        .await
        .expect("Tor did not bootstrap in 4 minutes");
        tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });
        let onion = std::fs::read_to_string(hidden.join("hostname")).unwrap().trim().to_string();
        let target = format!("{onion}:9444");

        let transport = StreamTransport::new(Some(&target), Arc::new(TcpDialer)).unwrap();
        let server = serve(
            remote.clone(),
            ServerIdentity::from_seed(&[74; 32]).unwrap(),
            StreamOnly(transport.clone()),
        )
        .await
        .unwrap();
        let _listening = transport.listen(listener);
        let mut descriptor = server.descriptor.clone();
        descriptor.endpoint_addr = EndpointAddr::new(server.endpoint.id())
            .with_addrs([TransportAddr::Custom(dial_addr(&target).unwrap())]);

        // A new onion service takes a while to be reachable: keep trying.
        let proxy = std::net::SocketAddr::from(([127, 0, 0, 1], socks_port));
        let started = std::time::Instant::now();
        let mut client = loop {
            let dialer = StreamTransport::new(None, Arc::new(SocksDialer { proxy })).unwrap();
            let attempt = tokio::time::timeout(
                std::time::Duration::from_secs(90),
                Client::connect(&descriptor, StreamOnly(dialer)),
            )
            .await;
            match attempt {
                Ok(Ok(client)) => break client,
                other => {
                    assert!(
                        started.elapsed() < std::time::Duration::from_secs(420),
                        "no connection over Tor after 7 minutes: {:?}",
                        other.map(|r| r.err())
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        };
        eprintln!("connected over Tor to {target} after {:?}", started.elapsed());
        assert_eq!(client.head().await.unwrap().height, 2);
        assert_eq!(client.pull(&local).await.unwrap(), 2);
        assert_eq!(local.storage.get_last_block_hash().unwrap(), second.header.hash);
        let third = mine_on(&local, second, vec![], 1_700_000_240).await;
        assert_eq!(client.submit_block(&third).await.unwrap().hash, third.header.hash);
        assert_eq!(remote.storage.get_last_block_hash().unwrap(), third.header.hash);
        eprintln!("synced 2 blocks and submitted 1 over Tor in {:?}", started.elapsed());
        server.close().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pq_pinned_localhost_sync_private_tx_invalid_proof_and_bounded_fork_recovery() {
        let (remote, _remote_dir) = state();
        let (local, _local_dir) = state();
        let genesis = mine_on(&remote, Profile::REGTEST.genesis_parent(), vec![], 1_700_000_000).await;
        let server = serve(
            remote.clone(),
            ServerIdentity::from_seed(&[71; 32]).unwrap(),
            presets::Minimal,
        )
        .await
        .unwrap();
        let descriptor = localhost(&server);
        let encoded = serde_json::to_vec(&descriptor).unwrap();
        let descriptor: NodeDescriptor = serde_json::from_slice(&encoded).unwrap();
        let mut bad_pin = descriptor.clone();
        bad_pin.ml_dsa_key_id = ServerIdentity::from_seed(&[72; 32]).unwrap().key_id();
        let rejected = Client::connect(&bad_pin, presets::Minimal)
            .await
            .err()
            .expect("wrong PQ pin rejected");
        assert!(rejected.contains("pinned ML-DSA"), "{rejected}");
        let mut client = Client::connect(&descriptor, presets::Minimal)
            .await
            .unwrap();
        assert_eq!(client.head().await.unwrap().height, 1);
        assert_eq!(client.pull_limited(&local, 1).await.unwrap(), 1);
        let for_payment = genesis.clone();
        let payment = with_state(&local, move |state| {
            let alice = Account::from_seed(&[31; 32], 3, 0).map_err(err)?;
            let bob = Account::from_seed(&[32; 32], 3, 0).map_err(err)?;
            let ledger = stark::full_ledger(&state.storage, state.profile, &for_payment)?;
            let cb = stark::decode(&for_payment)?.remove(0);
            let (note, _) = alice
                .receive(cb.statement.commitments[0], &cb.envelopes[0])
                .map_err(err)?;
            transfer(
                &alice,
                ledger.tree.root(),
                vec![Input {
                    key: alice.spend.clone(),
                    note,
                    path: ledger.tree.path(0).map_err(err)?,
                }],
                vec![
                    Payment::new(bob.address(), ATOMS),
                    Payment::new(alice.address(), PREMINE - ATOMS - 200),
                ],
                200,
                20,
            )
            .map_err(err)
        })
        .await
        .unwrap();
        let mut forged = payment.clone();
        let index = forged.proof.proof.len() - 1;
        forged.proof.proof[index] ^= 1;
        assert!(client.submit_tx(&forged).await.is_err());
        assert!(remote.mempool.lock().unwrap().select().is_empty());
        assert_eq!(client.submit_tx(&payment).await.unwrap(), payment.txid());
        assert_eq!(remote.mempool.lock().unwrap().select().len(), 1);
        let paid = mine_on(
            &remote,
            genesis.clone(),
            vec![payment.clone()],
            1_700_000_120,
        )
        .await;
        assert_eq!(client.pull_limited(&local, 1).await.unwrap(), 1);
        let local_third = mine_on(&local, paid, vec![], 1_700_000_240).await;
        let side_second = mine_on(&remote, genesis, vec![], 1_700_000_121).await;
        let side_third = mine_on(&remote, side_second, vec![], 1_700_000_241).await;
        let side_fourth = mine_on(&remote, side_third, vec![], 1_700_000_361).await;
        // Each batch persists one validated side-branch block. Later polls
        // continue past that prefix instead of downloading block 2 forever.
        for _ in 0..2 {
            assert_eq!(client.pull_limited(&local, 1).await.unwrap(), 1);
            assert_eq!(
                local.storage.get_last_block_hash().unwrap(),
                local_third.header.hash
            );
        }
        assert_eq!(client.pull_limited(&local, 1).await.unwrap(), 1);
        assert_eq!(
            local.storage.get_last_block_hash().unwrap(),
            side_fourth.header.hash
        );
        assert_eq!(client.pull(&local).await.unwrap(), 0);
        let fifth = mine_on(&local, side_fourth, vec![payment], 1_700_000_481).await;
        assert_eq!(
            client.submit_block(&fifth).await.unwrap().hash,
            fifth.header.hash
        );
        // Retry is idempotent, rather than a transport error after acceptance.
        assert_eq!(client.submit_block(&fifth).await.unwrap().height, 5);
        client.close().await;
        server.close().await;
    }

    async fn raw_hello(
        endpoint: &Endpoint,
        descriptor: &NodeDescriptor,
    ) -> (Connection, SendStream, RecvStream, Vec<u8>, Vec<u8>) {
        let connection = endpoint
            .connect(descriptor.endpoint_addr.clone(), ALPN)
            .await
            .unwrap();
        let (mut send, mut recv) = connection.open_bi().await.unwrap();
        send.write_all(MAGIC).await.unwrap();
        let mut public = vec![0; ML_DSA_65_PUBLIC_KEY_BYTES];
        let mut signature = vec![0; ML_DSA_65_SIGNATURE_BYTES];
        recv.read_exact(&mut public).await.unwrap();
        recv.read_exact(&mut signature).await.unwrap();
        (connection, send, recv, public, signature)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exporter_prevents_replay_between_connections_with_the_same_endpoint_ids() {
        let (state, _dir) = state();
        let server = serve(
            state,
            ServerIdentity::from_seed(&[73; 32]).unwrap(),
            presets::Minimal,
        )
        .await
        .unwrap();
        let descriptor = localhost(&server);
        let endpoint = Endpoint::builder(RequiredPostQuantumTls(presets::Minimal))
            .transport_config(transport_config())
            .bind()
            .await
            .unwrap();
        let (first, _first_send, _first_recv, public, signature) =
            raw_hello(&endpoint, &descriptor).await;
        assert_eq!(
            crate::network::pq_tls::negotiated_group(&first),
            Some(rustls::NamedGroup::X25519MLKEM768)
        );
        let key = MlDsa65PublicKey::from_bytes(&public).unwrap();
        let signature = MlDsa65Signature::from_bytes(&signature).unwrap();
        let original =
            auth_message(&first, CHAIN, server.endpoint.id(), endpoint.id(), &public).unwrap();
        key.verify(original.as_ref(), AUTH_CONTEXT, &signature)
            .unwrap();
        first.close(0u32.into(), b"reconnect");
        let (second, mut send, _recv, again, _) = raw_hello(&endpoint, &descriptor).await;
        assert_eq!(again, public);
        let replay_target =
            auth_message(&second, CHAIN, server.endpoint.id(), endpoint.id(), &public).unwrap();
        assert_ne!(original.as_slice(), replay_target.as_slice());
        assert!(key
            .verify(replay_target.as_ref(), AUTH_CONTEXT, &signature)
            .is_err());
        // Oversized body is rejected from the prefix, before allocating it.
        send.write_all(&[SUBMIT_TX]).await.unwrap();
        send.write_all(&u32::MAX.to_be_bytes()).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), second.closed())
            .await
            .expect("oversized frame closed");
        endpoint.close().await;
        server.close().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn peer_exchange_takes_only_self_announces_and_shares_only_dialed_peers() {
        let (seed_state, _seed_dir) = state();
        let (node_state, _node_dir) = state();
        let seed = serve(
            seed_state.clone(),
            ServerIdentity::from_seed(&[74; 32]).unwrap(),
            presets::Minimal,
        )
        .await
        .unwrap();
        let node = serve(
            node_state,
            ServerIdentity::from_seed(&[75; 32]).unwrap(),
            presets::Minimal,
        )
        .await
        .unwrap();
        seed_state
            .peers
            .book()
            .configure(seed.endpoint.id(), false, &[]);
        let target = localhost(&seed);
        let own = localhost(&node);

        // A client on its own endpoint cannot announce someone else's.
        let mut stranger = Client::connect(&target, presets::Minimal).await.unwrap();
        let refused = stranger.announce(&own).await.unwrap_err();
        assert!(refused.contains("announcing endpoint"), "{refused}");
        assert_eq!(seed_state.peers.book().pending_len(), 0);
        stranger.close().await;

        let mut client = Client::connect_from(&node.endpoint, &target).await.unwrap();
        assert!(client.peers().await.unwrap().is_empty());
        client.announce(&own).await.unwrap();
        let again = client.announce(&own).await.unwrap_err();
        assert!(again.contains("one STARK announce"), "{again}");
        assert!(
            client.peers().await.unwrap().is_empty(),
            "an announce is not shared before the seed dials it"
        );
        let pending = seed_state.peers.book().next_pending().unwrap();
        assert_eq!(pending, own);

        // The seed dials back: the endpoint authenticates with its pinned key.
        let mut back = Client::connect_from(&seed.endpoint, &pending)
            .await
            .unwrap();
        assert_eq!(back.head().await.unwrap().chain_id, CHAIN);
        back.close().await;
        seed_state
            .peers
            .book()
            .verified(pending, super::super::now_secs());
        let mut fresh = Client::connect(&target, presets::Minimal).await.unwrap();
        assert_eq!(fresh.peers().await.unwrap(), vec![own]);
        assert!(
            fresh.peers().await.unwrap().is_empty(),
            "one answer per connection and minute"
        );
        assert!(Client::connect_from(&seed.endpoint, &target).await.is_err());
        fresh.close().await;
        client.close().await;
        node.close().await;
        seed.close().await;
    }

    fn authority() -> (
        crate::hybrid_signature::HybridSigningKey,
        crate::checkpoint_authority::AuthorityKeys,
    ) {
        let key = crate::hybrid_signature::HybridSigningKey::from_parts(
            ed25519_dalek::SigningKey::from_bytes(&[76; 32]),
            &[77; 32],
        )
        .unwrap();
        let public = key.verifying_key();
        let keys = crate::checkpoint_authority::AuthorityKeys {
            ed25519: *public.ed25519(),
            mldsa: Some(public.mldsa().clone()),
        };
        (key, keys)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn checkpoints_travel_only_with_the_authority_signature() {
        let (key, keys) = authority();
        let (mut chain, _dir) = state();
        chain.authority = Some(keys);
        let genesis = mine_on(&chain, Profile::REGTEST.genesis_parent(), vec![], 1_700_000_000).await;
        let server = serve(
            chain.clone(),
            ServerIdentity::from_seed(&[78; 32]).unwrap(),
            presets::Minimal,
        )
        .await
        .unwrap();
        let mut client = Client::connect(&localhost(&server), presets::Minimal)
            .await
            .unwrap();
        assert_eq!(client.head().await.unwrap().checkpoint, None);
        assert_eq!(client.checkpoint().await.unwrap(), None);

        let checkpoint = SignedCheckpoint::sign_hybrid(&key, CHAIN, 1, &genesis.header.hash);
        let mut forged = checkpoint.clone();
        forged.block_hash = "00".repeat(32);
        let refused = client.submit_checkpoint(&forged).await.unwrap_err();
        assert!(refused.contains("signature"), "{refused}");
        client.submit_checkpoint(&checkpoint).await.unwrap();
        client.submit_checkpoint(&checkpoint).await.unwrap();
        assert_eq!(client.head().await.unwrap().checkpoint, Some(1));
        assert_eq!(client.checkpoint().await.unwrap(), Some(checkpoint));
        let conflict = SignedCheckpoint::sign_hybrid(&key, CHAIN, 1, &"cd".repeat(32));
        let refused = client.submit_checkpoint(&conflict).await.unwrap_err();
        assert!(refused.contains("conflicting"), "{refused}");
        client.close().await;
        server.close().await;

        // A node that does not know the chain's authority keeps nothing.
        let (keyless, _keyless_dir) = state();
        let server = serve(
            keyless,
            ServerIdentity::from_seed(&[79; 32]).unwrap(),
            presets::Minimal,
        )
        .await
        .unwrap();
        let mut client = Client::connect(&localhost(&server), presets::Minimal)
            .await
            .unwrap();
        let conflict = SignedCheckpoint::sign_hybrid(&key, CHAIN, 1, &genesis.header.hash);
        let refused = client.submit_checkpoint(&conflict).await.unwrap_err();
        assert!(refused.contains("no checkpoint authority"), "{refused}");
        client.close().await;
        server.close().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_directory_answers_and_takes_records_within_budget() {
        let (node, _dir) = state();
        let server = serve(
            node.clone(),
            ServerIdentity::from_seed(&[80; 32]).unwrap(),
            presets::Minimal,
        )
        .await
        .unwrap();
        let mut client = Client::connect(&localhost(&server), presets::Minimal)
            .await
            .unwrap();
        let bob = Account::from_seed(&[81; 32], 3, 0).unwrap().address();
        let short = bob.short_address().unwrap();
        assert_eq!(client.get_address(&short.digest).await.unwrap(), None);
        let record = AddressRecord::new(&bob).unwrap();
        let mut unstamped = record.clone();
        unstamped.stamp = (0u64..)
            .find(|stamp| !stark_directory::stamp_is_valid(&short, *stamp))
            .unwrap();
        let refused = client.submit_address(&unstamped).await.unwrap_err();
        assert!(refused.contains("work stamp"), "{refused}");
        assert!(client.submit_address(&record).await.unwrap());
        assert!(!client.submit_address(&record).await.unwrap());
        assert_eq!(
            client.get_address(&short.digest).await.unwrap(),
            Some(record.clone())
        );
        // Three submissions so far. The budget refuses the next one past it,
        // and the connection stays open.
        for _ in 3..SUBMITS_PER_MINUTE {
            assert!(!client.submit_address(&record).await.unwrap());
        }
        let limited = client.submit_address(&record).await.unwrap_err();
        assert!(limited.contains("per minute"), "{limited}");
        assert!(client.get_address(&short.digest).await.unwrap().is_some());
        client.close().await;
        server.close().await;
    }
}
