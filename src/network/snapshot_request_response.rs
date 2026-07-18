use async_trait::async_trait;
use futures::prelude::*;
use libp2p::{request_response, StreamProtocol};
use serde::{Deserialize, Serialize};
use std::io;

pub const SNAPSHOT_REQUEST_RESPONSE_PROTOCOL: &str = "/zoka/snapshots/1";
const REQUEST_SIZE_MAX: u64 = 4 * 1024;
const RESPONSE_SIZE_MAX: u64 = 64 * 1024;

/// Lightweight snapshot manifest: describes what snapshot this peer can serve
/// without sending the full data.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SnapshotManifest {
    pub chain_id: String,
    pub schema: String,
    pub head_height: u64,
    pub head_hash: String,
    pub cumulative_work: u128,
    pub block_count: u64,
    pub generated_unix_secs: u64,
    pub size_bytes: u64,
    pub sha256_hex: String,
    /// HTTP URL(s) where the full snapshot can be downloaded. Empty if not
    /// publicly accessible via HTTP.
    #[serde(default)]
    pub download_urls: Vec<String>,
    /// iroh EndpointId this peer serves the snapshot from (QUIC + n0 relay
    /// hole-punching, so it works behind NAT with no open port).
    ///
    /// This is what lets the network heal itself without a privileged seed: a
    /// node that has fallen behind asks its peers what they hold, picks the
    /// heaviest chain on offer, and pulls the snapshot straight from whoever
    /// is furthest ahead. Older peers omit the field and are simply skipped as
    /// a pull source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iroh_endpoint_id: Option<String>,
    /// Hex-encoded ed25519 signature over canonical header fields.
    #[serde(default)]
    pub signature_hex: Option<String>,
    /// Hex-encoded ed25519 verifying key of the signer.
    #[serde(default)]
    pub signer_pubkey_hex: Option<String>,
}

/// Ask a peer for a list of available snapshot manifests.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GetSnapshotManifests {
    pub chain_id: String,
    /// If set, only return snapshots at or below this height.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_height: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SnapshotManifestsResponse {
    Manifests { manifests: Vec<SnapshotManifest> },
    NotAvailable { reason: String },
}

#[derive(Clone, Debug, Default)]
pub struct SnapshotRequestResponseCodec;

pub fn protocol_name() -> StreamProtocol {
    StreamProtocol::new(SNAPSHOT_REQUEST_RESPONSE_PROTOCOL)
}

fn json_decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> io::Result<T> {
    serde_json::from_slice(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

fn json_encode<T: Serialize>(value: &T) -> io::Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[async_trait]
impl request_response::Codec for SnapshotRequestResponseCodec {
    type Protocol = StreamProtocol;
    type Request = GetSnapshotManifests;
    type Response = SnapshotManifestsResponse;

    async fn read_request<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
    ) -> io::Result<Self::Request>
    where
        T: AsyncRead + Unpin + Send,
    {
        let mut bytes = Vec::new();
        io.take(REQUEST_SIZE_MAX).read_to_end(&mut bytes).await?;
        json_decode(&bytes)
    }

    async fn read_response<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
    ) -> io::Result<Self::Response>
    where
        T: AsyncRead + Unpin + Send,
    {
        let mut bytes = Vec::new();
        io.take(RESPONSE_SIZE_MAX).read_to_end(&mut bytes).await?;
        json_decode(&bytes)
    }

    async fn write_request<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
        req: Self::Request,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        io.write_all(&json_encode(&req)?).await?;
        io.close().await
    }

    async fn write_response<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
        res: Self::Response,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        io.write_all(&json_encode(&res)?).await?;
        io.close().await
    }
}
