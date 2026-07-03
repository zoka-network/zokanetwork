use async_trait::async_trait;
use futures::prelude::*;
use libp2p::{request_response, StreamProtocol};
use serde::{Deserialize, Serialize};
use std::io;

use crate::types::BlockHeader;

pub const HEADER_REQUEST_RESPONSE_PROTOCOL: &str = "/zoka/headers/1";
const REQUEST_SIZE_MAX: u64 = 32 * 1024;
const RESPONSE_SIZE_MAX: u64 = 8 * 1024 * 1024;

/// A locator is a list of block hashes the requester already knows, ordered
/// from most recent to oldest with exponentially increasing gaps. The responder
/// returns headers starting from the first hash in the locator that it
/// recognises on its canonical chain.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GetHeaders {
    /// Sparse list of known hashes (tip → genesis direction, exponential gaps).
    pub locator: Vec<String>,
    /// Maximum number of headers to return.
    #[serde(default = "default_max")]
    pub max: u32,
    /// If set, stop at this hash (inclusive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_hash: Option<String>,
}

fn default_max() -> u32 {
    512
}

/// A single header plus the chain-state fields needed for PoW validation and
/// chain-work comparison, which are not part of `BlockHeader` itself.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HeaderSummary {
    pub header: BlockHeader,
    /// Difficulty target that was active when this block was produced.
    pub difficulty_bits: u32,
    /// Cumulative PoW work on the chain up to and including this block.
    pub cumulative_work: u128,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum HeadersResponse {
    Headers {
        headers: Vec<HeaderSummary>,
        /// Hash of the common ancestor the responder found in the locator,
        /// or an empty string if the locator had no match (full sync needed).
        common_ancestor: String,
    },
    NotFound {
        reason: String,
    },
}

#[derive(Clone, Debug, Default)]
pub struct HeaderRequestResponseCodec;

pub fn protocol_name() -> StreamProtocol {
    StreamProtocol::new(HEADER_REQUEST_RESPONSE_PROTOCOL)
}

fn json_decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> io::Result<T> {
    serde_json::from_slice(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

fn json_encode<T: Serialize>(value: &T) -> io::Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[async_trait]
impl request_response::Codec for HeaderRequestResponseCodec {
    type Protocol = StreamProtocol;
    type Request = GetHeaders;
    type Response = HeadersResponse;

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

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_header(height: u64, hash: &str, prev_hash: &str) -> HeaderSummary {
        use crate::types::{PublicKey, Signature};
        HeaderSummary {
            header: BlockHeader {
                hash: hash.to_string(),
                prev_hash: prev_hash.to_string(),
                merkle_root: "mr".to_string(),
                state_root: "sr".to_string(),
                timestamp: height * 30,
                nonce: height,
                block_height: height,
                validator: PublicKey(b"v".to_vec()),
                seal: Signature(b"s".to_vec()),
            },
            difficulty_bits: 20,
            cumulative_work: height as u128 * 1_000_000,
        }
    }

    #[test]
    fn get_headers_default_max_roundtrip() {
        let req = GetHeaders {
            locator: vec![
                "hash-5".to_string(),
                "hash-3".to_string(),
                "hash-1".to_string(),
            ],
            max: default_max(),
            stop_hash: None,
        };
        let encoded = json_encode(&req).expect("encode");
        let decoded: GetHeaders = json_decode(&encoded).expect("decode");
        assert_eq!(decoded.locator.len(), 3);
        assert_eq!(decoded.max, 512);
        assert!(decoded.stop_hash.is_none());
    }

    #[test]
    fn headers_response_roundtrip() {
        let headers = vec![
            dummy_header(1, "hash-1", "genesis"),
            dummy_header(2, "hash-2", "hash-1"),
        ];
        let resp = HeadersResponse::Headers {
            headers,
            common_ancestor: "genesis".to_string(),
        };
        let encoded = json_encode(&resp).expect("encode");
        let decoded: HeadersResponse = json_decode(&encoded).expect("decode");
        match decoded {
            HeadersResponse::Headers {
                headers,
                common_ancestor,
            } => {
                assert_eq!(headers.len(), 2);
                assert_eq!(headers[1].header.block_height, 2);
                assert_eq!(common_ancestor, "genesis");
            }
            other => panic!("unexpected: {:?}", other),
        }
    }
}
