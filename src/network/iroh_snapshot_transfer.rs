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
use std::sync::Arc;

use iroh::endpoint::presets;
use iroh::{Endpoint, EndpointAddr};
use tokio::io::AsyncWriteExt;

/// ALPN for the Zoka snapshot pull protocol. Both ends must present the same
/// bytestring or iroh aborts the connection during the handshake.
pub const ZOKA_SNAPSHOT_ALPN: &[u8] = b"/zoka/snapshot-pull/1";

/// Fixed request tag the client sends to ask for the current snapshot. Kept as
/// a versioned constant so the protocol can grow (e.g. range requests) without
/// a breaking change.
const REQUEST_SNAPSHOT_TAG: &[u8; 4] = b"SNP1";

/// Hard ceiling on an in-memory fetch, guarding [`fetch_snapshot`] against a
/// malicious/oversized length prefix. Large snapshots should stream to disk.
const MAX_IN_MEMORY_SNAPSHOT: u64 = 512 * 1024 * 1024;

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

    let accept_endpoint = endpoint.clone();
    tokio::spawn(async move {
        while let Some(incoming) = accept_endpoint.accept().await {
            let payload = snapshot_bytes.clone();
            tokio::spawn(async move {
                if let Err(err) = serve_one(incoming, payload).await {
                    tracing::warn!(target: "zoka.iroh", error = %err, "snapshot serve connection failed");
                }
            });
        }
    });

    Ok(endpoint)
}

async fn serve_one(
    incoming: iroh::endpoint::Incoming,
    snapshot_bytes: Arc<Vec<u8>>,
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

    let len = snapshot_bytes.len() as u64;
    send.write_all(&len.to_be_bytes()).await?;
    send.write_all(&snapshot_bytes).await?;
    send.finish()?;
    // Wait for the peer to acknowledge receipt (stream closed) so the data is
    // flushed before the connection tears down.
    connection.closed().await;
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
}
