use async_trait::async_trait;
use futures::prelude::*;
use libp2p::{request_response, StreamProtocol};
use serde::{Deserialize, Serialize};
use std::io;

use crate::network::message::BlockResponseMessage;

pub const BLOCK_REQUEST_RESPONSE_PROTOCOL: &str = "/zoka/block/reqresp/1";
const REQUEST_SIZE_MAX: u64 = 64 * 1024;
const RESPONSE_SIZE_MAX: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlockBodyRequest {
    pub block_height: u64,
    #[serde(default)]
    pub block_hash: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum BlockBodyResponse {
    Found(BlockResponseMessage),
    NotFound {
        block_height: u64,
        #[serde(default)]
        block_hash: Option<String>,
        reason: String,
    },
    Rejected {
        block_height: u64,
        #[serde(default)]
        block_hash: Option<String>,
        reason: String,
    },
}

#[derive(Clone, Debug, Default)]
pub struct BlockRequestResponseCodec;

pub fn protocol_name() -> StreamProtocol {
    StreamProtocol::new(BLOCK_REQUEST_RESPONSE_PROTOCOL)
}

fn json_decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> io::Result<T> {
    serde_json::from_slice(bytes).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

fn json_encode<T: Serialize>(value: &T) -> io::Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

#[async_trait]
impl request_response::Codec for BlockRequestResponseCodec {
    type Protocol = StreamProtocol;
    type Request = BlockBodyRequest;
    type Response = BlockBodyResponse;

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

    #[test]
    fn block_body_request_roundtrip() {
        let request = BlockBodyRequest {
            block_height: 42,
            block_hash: Some("hash-42".to_string()),
        };
        let encoded = json_encode(&request).expect("encode");
        let decoded: BlockBodyRequest = json_decode(&encoded).expect("decode");
        assert_eq!(decoded, request);
    }

    #[test]
    fn block_body_response_not_found_roundtrip() {
        let response = BlockBodyResponse::NotFound {
            block_height: 42,
            block_hash: Some("hash-42".to_string()),
            reason: "missing".to_string(),
        };
        let encoded = json_encode(&response).expect("encode");
        let decoded: BlockBodyResponse = json_decode(&encoded).expect("decode");
        match decoded {
            BlockBodyResponse::NotFound {
                block_height,
                block_hash,
                reason,
            } => {
                assert_eq!(block_height, 42);
                assert_eq!(block_hash.as_deref(), Some("hash-42"));
                assert_eq!(reason, "missing");
            }
            other => panic!("unexpected response: {:?}", other),
        }
    }

    #[test]
    fn block_body_response_rejected_roundtrip() {
        let response = BlockBodyResponse::Rejected {
            block_height: 42,
            block_hash: Some("hash-42".to_string()),
            reason: "storage unavailable".to_string(),
        };
        let encoded = json_encode(&response).expect("encode");
        let decoded: BlockBodyResponse = json_decode(&encoded).expect("decode");
        match decoded {
            BlockBodyResponse::Rejected {
                block_height,
                block_hash,
                reason,
            } => {
                assert_eq!(block_height, 42);
                assert_eq!(block_hash.as_deref(), Some("hash-42"));
                assert_eq!(reason, "storage unavailable");
            }
            other => panic!("unexpected response: {:?}", other),
        }
    }
}
