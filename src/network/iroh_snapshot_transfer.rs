//! Snapshot delivery over an iroh QUIC connection.
//!
//! This is the piece that joins the two tracks of the transport evolution:
//! the existing snapshot machinery (`crate::snapshot`) produces a verifiable
//! blob of the whole chain, and iroh delivers that blob directly between two
//! endpoints -- with built-in NAT hole-punching, so a fresh node can pull a
//! snapshot from a peer without either side opening a port and without Tor.
//! (Tor onion stays available as the fallback reachability path for the
//! libp2p gossip/consensus layer; this module is purely the bulk transfer.)
//!
//! Wire framing is deliberately trivial: the client opens a bidirectional
//! stream and writes a fixed request tag; the server replies with an 8-byte
//! big-endian length followed by exactly that many snapshot bytes. Callers
//! that expect very large snapshots should prefer [`fetch_snapshot_streaming`]
//! (writes straight to a file) over [`fetch_snapshot`] (buffers in memory).

use std::io;
use std::path::Path;
use std::sync::{Arc, RwLock};

use iroh::endpoint::presets;
use iroh::{Endpoint, EndpointAddr, SecretKey};
use tokio::io::AsyncWriteExt;

/// Shared, swappable snapshot bytes. Lets a long-running seed refresh what it
/// serves as its chain grows (via [`update_provider`]) without rebinding the
/// endpoint or dropping its stable identity.
pub type SnapshotProvider = Arc<RwLock<Arc<Vec<u8>>>>;

/// Wrap already-`Arc`'d snapshot bytes into an updatable provider (no copy).
pub fn provider_from_arc(bytes: Arc<Vec<u8>>) -> SnapshotProvider {
    Arc::new(RwLock::new(bytes))
}

/// Swap in a freshly exported snapshot for future pulls. In-flight transfers
/// keep serving the bytes they already started with.
pub fn update_provider(provider: &SnapshotProvider, bytes: Vec<u8>) {
    if let Ok(mut guard) = provider.write() {
        *guard = Arc::new(bytes);
    }
}

/// ALPN for the Zoka snapshot pull protocol. Both ends must present the same
/// bytestring or iroh aborts the connection during the handshake.
pub const ZOKA_SNAPSHOT_ALPN: &[u8] = b"/zoka/snapshot-pull/1";

/// Fixed request tag the client sends to ask for the current snapshot. Kept as
/// a versioned constant so the protocol can grow (e.g. range requests) without
/// a breaking change.
const REQUEST_SNAPSHOT_TAG: &[u8; 4] = b"SNP1";

/// Request tag: "what is your current chain head height?" — a cheap staleness
/// probe (8-byte reply) a NAT'd follower polls periodically so it can catch up
/// over iroh without a live libp2p link, and without re-pulling the whole
/// snapshot just to learn it's already current.
const REQUEST_HEAD_TAG: &[u8; 4] = b"HED1";

/// Request tag: "send me up to N consecutive blocks starting at height H."
/// Followed by start-height (u64 BE) + count (u32 BE). Lets a follower pull
/// only the blocks it is missing instead of the entire snapshot.
const REQUEST_BLOCKS_TAG: &[u8; 4] = b"BLK1";

/// Hard ceiling on an in-memory fetch, guarding [`fetch_snapshot`] against a
/// malicious/oversized length prefix. Large snapshots should stream to disk.
const MAX_IN_MEMORY_SNAPSHOT: u64 = 512 * 1024 * 1024;

/// Max blocks a single [`REQUEST_BLOCKS_TAG`] request may return, bounding the
/// response size / abuse. A follower that is further behind pulls across
/// several requests.
const MAX_BLOCKS_PER_REQUEST: u32 = 256;

/// Per-block size ceiling for the incremental block fetch, guarding against a
/// malicious length prefix (mirrors [`MAX_IN_MEMORY_SNAPSHOT`] for one block).
const MAX_SINGLE_BLOCK_BYTES: u64 = 32 * 1024 * 1024;

/// Bind an endpoint that serves `snapshot_bytes` to any peer that connects with
/// [`ZOKA_SNAPSHOT_ALPN`], and return it so the caller can publish its
/// [`Endpoint::addr`] to fetchers. The accept loop runs on a spawned task for
/// the lifetime of the returned endpoint; dropping/closing the endpoint stops
/// it.
///
/// `preset` selects reachability: [`presets::N0`] uses n0's public relays for
/// hole-punching across NATs (production), [`presets::Minimal`] is direct-only
/// (tests / same-LAN).
pub async fn serve_snapshot(
    snapshot_bytes: Arc<Vec<u8>>,
    preset: impl presets::Preset,
) -> Result<Endpoint, Box<dyn std::error::Error + Send + Sync>> {
    let endpoint = Endpoint::builder(preset)
        .alpns(vec![ZOKA_SNAPSHOT_ALPN.to_vec()])
        .bind()
        .await?;
    spawn_snapshot_accept_loop(endpoint.clone(), provider_from_arc(snapshot_bytes));
    Ok(endpoint)
}

/// Like [`serve_snapshot`] but binds a fixed [`SecretKey`] so the endpoint's
/// [`iroh::EndpointId`] is stable across restarts. A seed uses this with its
/// persistent key (see [`load_or_generate_secret_key`]) so it can advertise a
/// fixed iroh address in the network manifest -- the iroh analogue of the
/// permanent Tor onion identity.
pub async fn serve_snapshot_with_key(
    snapshot_bytes: Arc<Vec<u8>>,
    secret_key: SecretKey,
    preset: impl presets::Preset,
) -> Result<Endpoint, Box<dyn std::error::Error + Send + Sync>> {
    serve_snapshot_provider(provider_from_arc(snapshot_bytes), secret_key, preset).await
}

/// Serve an updatable [`SnapshotProvider`] under a fixed identity. The seed
/// keeps the provider and calls [`update_provider`] as its chain grows so new
/// pulls get the latest snapshot without rebinding.
pub async fn serve_snapshot_provider(
    provider: SnapshotProvider,
    secret_key: SecretKey,
    preset: impl presets::Preset,
) -> Result<Endpoint, Box<dyn std::error::Error + Send + Sync>> {
    let endpoint = Endpoint::builder(preset)
        .secret_key(secret_key)
        .alpns(vec![ZOKA_SNAPSHOT_ALPN.to_vec()])
        .bind()
        .await?;
    spawn_snapshot_accept_loop(endpoint.clone(), provider);
    Ok(endpoint)
}

/// Spawn the background accept loop that answers every inbound snapshot pull on
/// `endpoint` with the provider's current bytes. Runs until the endpoint is
/// closed/dropped.
fn spawn_snapshot_accept_loop(endpoint: Endpoint, provider: SnapshotProvider) {
    tokio::spawn(async move {
        while let Some(incoming) = endpoint.accept().await {
            let provider = provider.clone();
            tokio::spawn(async move {
                if let Err(err) = serve_one(incoming, provider).await {
                    tracing::warn!(target: "zoka.iroh", error = %err, "snapshot serve connection failed");
                }
            });
        }
    });
}

/// Load a persistent iroh identity from `path` (32 raw secret-key bytes),
/// generating and saving one on first use. The derived [`iroh::EndpointId`] is
/// stable across restarts, so a seed can advertise a fixed iroh address in the
/// network manifest just like its permanent Tor onion key.
pub fn load_or_generate_secret_key(path: &Path) -> io::Result<SecretKey> {
    if let Ok(bytes) = std::fs::read(path) {
        if bytes.len() == 32 {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes);
            return Ok(SecretKey::from_bytes(&arr));
        }
    }
    let secret_key = SecretKey::from_bytes(&rand::random::<[u8; 32]>());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, secret_key.to_bytes())?;
    Ok(secret_key)
}

async fn serve_one(
    incoming: iroh::endpoint::Incoming,
    provider: SnapshotProvider,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let connection = incoming.await?;
    let (mut send, mut recv) = connection.accept_bi().await?;

    // Read + validate the request tag before serving anything.
    let mut tag = [0u8; 4];
    recv.read_exact(&mut tag).await?;
    if &tag != REQUEST_SNAPSHOT_TAG {
        // Unknown request: close the stream without a body.
        send.finish()?;
        return Ok(());
    }

    // Snapshot the current bytes (cheap Arc clone) so an update mid-transfer
    // doesn't tear this stream's payload out from under it.
    let snapshot_bytes = {
        let guard = provider
            .read()
            .map_err(|_| io::Error::other("snapshot provider lock poisoned"))?;
        guard.clone()
    };
    let len = snapshot_bytes.len() as u64;
    send.write_all(&len.to_be_bytes()).await?;
    send.write_all(&snapshot_bytes).await?;
    send.finish()?;
    // Wait for the peer to acknowledge receipt (stream closed) so the data is
    // flushed before the connection tears down.
    connection.closed().await;
    Ok(())
}

/// Live chain data source for the seed side of the pull protocol. Lets a
/// single iroh endpoint answer all three request kinds: the full snapshot
/// (`SNP1`, fresh bootstrap), the current head height (`HED1`, cheap staleness
/// probe), and a range of individual blocks (`BLK1`, incremental catch-up).
///
/// Implemented outside this module (over the node's chain storage) so the
/// network layer stays storage-agnostic.
pub trait ChainServe: Send + Sync {
    /// Current chain head height.
    fn head_height(&self) -> u64;
    /// Full serialized snapshot bytes for `SNP1` fresh bootstrap.
    fn snapshot_bytes(&self) -> Arc<Vec<u8>>;
    /// Serialized bytes of the block at `height`, or `None` if absent.
    fn block_bytes(&self, height: u64) -> Option<Vec<u8>>;
}

/// Like [`serve_snapshot_provider`] but backed by a live [`ChainServe`] so the
/// same endpoint also answers head-height probes and incremental block-range
/// pulls, not just full-snapshot requests. This is what a seed uses so NAT'd
/// followers can stay current over iroh without a live libp2p mesh.
pub async fn serve_chain(
    source: Arc<dyn ChainServe>,
    secret_key: SecretKey,
    preset: impl presets::Preset,
) -> Result<Endpoint, Box<dyn std::error::Error + Send + Sync>> {
    let endpoint = Endpoint::builder(preset)
        .secret_key(secret_key)
        .alpns(vec![ZOKA_SNAPSHOT_ALPN.to_vec()])
        .bind()
        .await?;
    let accept_endpoint = endpoint.clone();
    tokio::spawn(async move {
        while let Some(incoming) = accept_endpoint.accept().await {
            let source = source.clone();
            tokio::spawn(async move {
                if let Err(err) = serve_chain_one(incoming, source).await {
                    tracing::warn!(target: "zoka.iroh", error = %err, "chain serve connection failed");
                }
            });
        }
    });
    Ok(endpoint)
}

async fn serve_chain_one(
    incoming: iroh::endpoint::Incoming,
    source: Arc<dyn ChainServe>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let connection = incoming.await?;
    let (mut send, mut recv) = connection.accept_bi().await?;

    let mut tag = [0u8; 4];
    recv.read_exact(&mut tag).await?;

    if &tag == REQUEST_SNAPSHOT_TAG {
        let bytes = source.snapshot_bytes();
        let len = bytes.len() as u64;
        send.write_all(&len.to_be_bytes()).await?;
        send.write_all(&bytes).await?;
        send.finish()?;
        connection.closed().await;
    } else if &tag == REQUEST_HEAD_TAG {
        let height = source.head_height();
        send.write_all(&height.to_be_bytes()).await?;
        send.finish()?;
        connection.closed().await;
    } else if &tag == REQUEST_BLOCKS_TAG {
        let mut start_bytes = [0u8; 8];
        recv.read_exact(&mut start_bytes).await?;
        let mut count_bytes = [0u8; 4];
        recv.read_exact(&mut count_bytes).await?;
        let start = u64::from_be_bytes(start_bytes);
        let count = u32::from_be_bytes(count_bytes).min(MAX_BLOCKS_PER_REQUEST);

        // Gather consecutive blocks from `start`, stopping at the first gap so
        // the client always receives a contiguous run it can apply in order.
        let mut blocks: Vec<Vec<u8>> = Vec::new();
        for i in 0..count as u64 {
            match source.block_bytes(start + i) {
                Some(b) => blocks.push(b),
                None => break,
            }
        }
        let n = blocks.len() as u32;
        send.write_all(&n.to_be_bytes()).await?;
        for b in &blocks {
            let len = b.len() as u64;
            send.write_all(&len.to_be_bytes()).await?;
            send.write_all(b).await?;
        }
        send.finish()?;
        connection.closed().await;
    } else {
        // Unknown request: close the stream without a body.
        send.finish()?;
    }
    Ok(())
}

/// Connect to `addr`, pull the snapshot, and return it in memory. Fails if the
/// advertised length exceeds [`MAX_IN_MEMORY_SNAPSHOT`]; use
/// [`fetch_snapshot_streaming`] for large chains.
pub async fn fetch_snapshot(
    addr: EndpointAddr,
    preset: impl presets::Preset,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    let (endpoint, connection) = open_pull(addr, preset).await?;
    let (mut send, mut recv) = connection.open_bi().await?;
    send.write_all(REQUEST_SNAPSHOT_TAG).await?;
    send.finish()?;

    let len = read_len(&mut recv).await?;
    if len > MAX_IN_MEMORY_SNAPSHOT {
        return Err(format!(
            "snapshot advertised {len} bytes, over the {MAX_IN_MEMORY_SNAPSHOT}-byte in-memory limit; use streaming fetch"
        )
        .into());
    }
    let mut buf = vec![0u8; len as usize];
    recv.read_exact(&mut buf).await?;
    connection.close(0u32.into(), b"done");
    endpoint.close().await;
    Ok(buf)
}

/// Connect to `addr` and stream the snapshot straight to `out_path`, returning
/// the number of bytes written. Memory use stays bounded regardless of chain
/// size, which is what a real multi-hundred-MB snapshot needs.
pub async fn fetch_snapshot_streaming(
    addr: EndpointAddr,
    preset: impl presets::Preset,
    out_path: &Path,
) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
    let (endpoint, connection) = open_pull(addr, preset).await?;
    let (mut send, mut recv) = connection.open_bi().await?;
    send.write_all(REQUEST_SNAPSHOT_TAG).await?;
    send.finish()?;

    let len = read_len(&mut recv).await?;
    let mut file = tokio::fs::File::create(out_path).await?;
    let mut remaining = len;
    let mut chunk = vec![0u8; 1024 * 1024];
    while remaining > 0 {
        let want = remaining.min(chunk.len() as u64) as usize;
        recv.read_exact(&mut chunk[..want]).await?;
        file.write_all(&chunk[..want]).await?;
        remaining -= want as u64;
    }
    file.flush().await?;
    connection.close(0u32.into(), b"done");
    endpoint.close().await;
    Ok(len)
}

/// Ask a seed for its current chain head height — a cheap staleness probe a
/// follower polls before deciding whether it needs to pull any blocks.
pub async fn fetch_head(
    addr: EndpointAddr,
    preset: impl presets::Preset,
) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
    let (endpoint, connection) = open_pull(addr, preset).await?;
    let (mut send, mut recv) = connection.open_bi().await?;
    send.write_all(REQUEST_HEAD_TAG).await?;
    send.finish()?;

    let mut height_bytes = [0u8; 8];
    recv.read_exact(&mut height_bytes).await?;
    connection.close(0u32.into(), b"done");
    endpoint.close().await;
    Ok(u64::from_be_bytes(height_bytes))
}

/// Pull up to `count` consecutive serialized blocks starting at `start`. The
/// seed returns as many as it holds contiguously from `start` (possibly fewer
/// than requested, e.g. if `start` is past its head); each element is the
/// serialized bytes of one block, ready to apply in order.
pub async fn fetch_block_range(
    addr: EndpointAddr,
    preset: impl presets::Preset,
    start: u64,
    count: u32,
) -> Result<Vec<Vec<u8>>, Box<dyn std::error::Error + Send + Sync>> {
    let (endpoint, connection) = open_pull(addr, preset).await?;
    let (mut send, mut recv) = connection.open_bi().await?;
    send.write_all(REQUEST_BLOCKS_TAG).await?;
    send.write_all(&start.to_be_bytes()).await?;
    let capped = count.min(MAX_BLOCKS_PER_REQUEST);
    send.write_all(&capped.to_be_bytes()).await?;
    send.finish()?;

    let mut n_bytes = [0u8; 4];
    recv.read_exact(&mut n_bytes).await?;
    let n = u32::from_be_bytes(n_bytes).min(MAX_BLOCKS_PER_REQUEST);
    let mut blocks = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let len = read_len(&mut recv).await?;
        if len > MAX_SINGLE_BLOCK_BYTES {
            return Err(format!(
                "block advertised {len} bytes, over the {MAX_SINGLE_BLOCK_BYTES}-byte limit"
            )
            .into());
        }
        let mut buf = vec![0u8; len as usize];
        recv.read_exact(&mut buf).await?;
        blocks.push(buf);
    }
    connection.close(0u32.into(), b"done");
    endpoint.close().await;
    Ok(blocks)
}

async fn open_pull(
    addr: EndpointAddr,
    preset: impl presets::Preset,
) -> Result<(Endpoint, iroh::endpoint::Connection), Box<dyn std::error::Error + Send + Sync>> {
    let endpoint = Endpoint::bind(preset).await?;
    let connection = endpoint.connect(addr, ZOKA_SNAPSHOT_ALPN).await?;
    Ok((endpoint, connection))
}

async fn read_len(
    recv: &mut iroh::endpoint::RecvStream,
) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
    let mut len_bytes = [0u8; 8];
    recv.read_exact(&mut len_bytes)
        .await
        .map_err(|e| io::Error::other(format!("failed to read snapshot length prefix: {e}")))?;
    Ok(u64::from_be_bytes(len_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Proves the core of the combined architecture end to end: one endpoint
    // serves a snapshot blob, another connects over iroh and pulls the exact
    // bytes back. presets::Minimal keeps it direct/local with no relay or
    // internet dependency.
    #[tokio::test]
    async fn iroh_snapshot_round_trips_between_two_endpoints() {
        let payload: Vec<u8> = (0..256_000u32).map(|i| (i % 251) as u8).collect();
        let server = serve_snapshot(Arc::new(payload.clone()), presets::Minimal)
            .await
            .expect("serve endpoint binds");
        let addr = server.addr();

        let fetched = fetch_snapshot(addr, presets::Minimal)
            .await
            .expect("client pulls snapshot");

        assert_eq!(fetched, payload, "pulled snapshot must match served bytes");
        server.close().await;
    }

    // A persistent key file must yield the same identity every time it is
    // loaded -- the property that lets a seed advertise a fixed iroh address.
    #[test]
    fn persistent_key_yields_stable_identity() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("iroh_key");
        let first = load_or_generate_secret_key(&path).expect("generate");
        let second = load_or_generate_secret_key(&path).expect("reload");
        assert_eq!(
            first.public(),
            second.public(),
            "the same key file must reload to the same identity"
        );
    }

    // A server bound to a fixed key exposes that key's stable EndpointId, and a
    // client can still pull the snapshot from it (direct/local via Minimal).
    #[tokio::test]
    async fn served_with_fixed_key_exposes_stable_id_and_serves() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("iroh_key");
        let key = load_or_generate_secret_key(&path).expect("key");
        let expected_id = key.public();

        let payload: Vec<u8> = (0..64_000u32).map(|i| (i % 97) as u8).collect();
        let server = serve_snapshot_with_key(Arc::new(payload.clone()), key, presets::Minimal)
            .await
            .expect("serve");
        assert_eq!(server.id(), expected_id, "endpoint id must match the fixed key");

        let fetched = fetch_snapshot(server.addr(), presets::Minimal)
            .await
            .expect("fetch");
        assert_eq!(fetched, payload);
        server.close().await;
    }

    // Exercises the incremental tail-sync protocol end to end over iroh: a
    // ChainServe answers head-height probes and contiguous block-range pulls,
    // including the "requested past the head" clamp. This is what keeps a NAT'd
    // follower current without a live libp2p link.
    #[tokio::test]
    async fn iroh_head_and_block_range_round_trip() {
        struct MockChain {
            blocks: Vec<Vec<u8>>,
            snapshot: Arc<Vec<u8>>,
        }
        impl ChainServe for MockChain {
            fn head_height(&self) -> u64 {
                self.blocks.len() as u64
            }
            fn snapshot_bytes(&self) -> Arc<Vec<u8>> {
                self.snapshot.clone()
            }
            fn block_bytes(&self, height: u64) -> Option<Vec<u8>> {
                if height == 0 || height > self.blocks.len() as u64 {
                    return None;
                }
                Some(self.blocks[(height - 1) as usize].clone())
            }
        }

        let blocks: Vec<Vec<u8>> = (1..=5u64)
            .map(|h| format!("block-{h}").into_bytes())
            .collect();
        let source: Arc<dyn ChainServe> = Arc::new(MockChain {
            blocks: blocks.clone(),
            snapshot: Arc::new(b"snapshot-bytes".to_vec()),
        });
        let key = SecretKey::from_bytes(&rand::random::<[u8; 32]>());
        let server = serve_chain(source, key, presets::Minimal)
            .await
            .expect("chain serve binds");

        // Head probe.
        let head = fetch_head(server.addr(), presets::Minimal)
            .await
            .expect("head fetch");
        assert_eq!(head, 5, "head must be the number of blocks");

        // Contiguous range in the middle (heights 2,3,4).
        let mid = fetch_block_range(server.addr(), presets::Minimal, 2, 3)
            .await
            .expect("range fetch");
        assert_eq!(mid.len(), 3);
        assert_eq!(mid[0], b"block-2");
        assert_eq!(mid[2], b"block-4");

        // Requesting past the head returns only what exists (heights 4,5).
        let tail = fetch_block_range(server.addr(), presets::Minimal, 4, 10)
            .await
            .expect("tail fetch");
        assert_eq!(tail.len(), 2, "server clamps to available blocks");
        assert_eq!(tail[1], b"block-5");

        server.close().await;
    }
}
