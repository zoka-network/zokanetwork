//! Authenticated STARK-chain transport. The pinned ML-DSA server identity signs
//! this connection's TLS exporter, not a reusable greeting or an unbound nonce.
//! TLS must negotiate X25519MLKEM768. Only STARK v2 chains are served, each one
//! named in the descriptor and bound into the signed context.
use std::{sync::Arc, time::Duration};

use iroh::{
    endpoint::{presets::Preset, Connection, QuicTransportConfig, RecvStream, SendStream},
    Endpoint, EndpointAddr, EndpointId, SecretKey,
};
use serde::{Deserialize, Serialize};
use tokio::{sync::Semaphore, task::JoinHandle};
use zeroize::Zeroizing;
use zoka_pq::{
    MlDsa65PublicKey, MlDsa65SecretKey, MlDsa65Signature, ML_DSA_65_PUBLIC_KEY_BYTES,
    ML_DSA_65_SIGNATURE_BYTES,
};

use super::{
    stark::{self, Profile},
    stark_api::NodeState,
};
use crate::{network::pq_tls::RequiredPostQuantumTls, state::chain::apply_block, types::Block};
use stark::protocol::transaction::{Transaction, MAX_TRANSACTION_BYTES};

pub const ALPN: &[u8] = b"/zoka/quantic-stark/2";
pub const MAX_BLOCK_WIRE_BYTES: usize = 2 * stark::protocol::ledger::MAX_BLOCK_BYTES + 1024 * 1024;
const MAGIC: &[u8; 4] = b"ZQS2";
const AUTH_CONTEXT: &[u8] = b"zoka-quantic-stark-server-v2";
const EXPORTER_LABEL: &[u8] = b"EXPORTER-ZOKA-QUANTIC-STARK-v2";
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_CONNECTIONS: usize = 8;
const HEAD: u8 = 1;
const GET_BLOCK: u8 = 2;
const SUBMIT_BLOCK: u8 = 3;
const SUBMIT_TX: u8 = 4;
const GET_HASH: u8 = 5;
const MAX_ERROR_BYTES: usize = 2048;

fn err(value: impl std::fmt::Display) -> String {
    value.to_string()
}

/// Store this descriptor through an authenticated channel. Receiving a key
/// from an untrusted peer is not a substitute for pinning its expected identity.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeDescriptor {
    pub version: u8,
    pub chain_id: String,
    pub endpoint_addr: EndpointAddr,
    pub ml_dsa_public_key: Vec<u8>,
}
impl NodeDescriptor {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 2 || Profile::for_chain(&self.chain_id).is_none() {
            return Err("STARK descriptor network/version".into());
        }
        MlDsa65PublicKey::from_bytes(&self.ml_dsa_public_key).map_err(err)?;
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
        version: 2,
        chain_id: state.profile.chain_id.into(),
        endpoint_addr: endpoint.addr(),
        ml_dsa_public_key: identity.public_key(),
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
                let connected = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
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
        HEAD => Ok(0),
        GET_BLOCK | GET_HASH => Ok(8),
        SUBMIT_BLOCK => Ok(MAX_BLOCK_WIRE_BYTES),
        SUBMIT_TX => Ok(MAX_TRANSACTION_BYTES),
        _ => Err("unknown STARK request".into()),
    }
}

async fn read_request_header(recv: &mut RecvStream) -> Result<(u8, usize), String> {
    let mut header = [0u8; 5];
    recv.read_exact(&mut header).await.map_err(err)?;
    let tag = header[0];
    let len = u32::from_be_bytes(header[1..].try_into().unwrap()) as usize;
    if len > request_limit(tag)? || (matches!(tag, GET_BLOCK | GET_HASH) && len != 8) {
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

async fn serve_requests(
    connection: &Connection,
    mut send: SendStream,
    mut recv: RecvStream,
    state: NodeState,
) -> Result<(), String> {
    loop {
        let request = async {
            let (tag, len) = read_request_header(&mut recv).await?;
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
fn head(state: &NodeState) -> Result<Head, String> {
    let block = tip(state)?;
    Ok(Head {
        chain_id: state.profile.chain_id.into(),
        height: block.header.block_height,
        hash: block.header.hash,
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
            let ledger = stark::state_after(&state.storage, state.profile, &tip(state)?)?;
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
        let ledger = stark::state_after(&state.storage, state.profile, &current)?;
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
    connection: Connection,
    send: SendStream,
    recv: RecvStream,
}

impl Client {
    pub async fn connect(descriptor: &NodeDescriptor, preset: impl Preset) -> Result<Self, String> {
        descriptor.validate()?;
        let endpoint = Endpoint::builder(RequiredPostQuantumTls(preset))
            .transport_config(transport_config())
            .bind()
            .await
            .map_err(err)?;
        let authenticated = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
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
            if public != descriptor.ml_dsa_public_key {
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
                connection,
                send,
                recv,
            }),
            other => {
                endpoint.close().await;
                Err(match other {
                    Ok(Err(reason)) => reason,
                    _ => "STARK authentication timeout".into(),
                })
            }
        }
    }

    pub async fn close(&self) {
        self.connection.close(0u32.into(), b"client done");
        self.endpoint.close().await;
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
            self.send.write_all(&[tag]).await.map_err(err)?;
            self.send
                .write_all(&(body.len() as u32).to_be_bytes())
                .await
                .map_err(err)?;
            self.send.write_all(body).await.map_err(err)?;
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
            Ok::<_, String>((header[0], bytes))
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
        let mut imported = 0;
        for height in low + 1..=last {
            let block = self
                .get_block(height)
                .await?
                .ok_or("STARK remote block unavailable")?;
            let fresh = with_state(state, move |state| import_block_locked(state, &block)).await?;
            if fresh {
                imported += 1;
            }
        }
        Ok(imported)
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
        bad_pin.ml_dsa_public_key = ServerIdentity::from_seed(&[72; 32]).unwrap().public_key();
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
            let ledger = stark::state_after(&state.storage, state.profile, &for_payment)?;
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
}
