use futures::prelude::*;
use libp2p::{request_response, StreamProtocol};
use serde::{Deserialize, Serialize};
use std::io;

pub const PRIVATE_TX_REQUEST_RESPONSE_PROTOCOL: &str = "/zoka/private-tx/reqresp/1";
const REQUEST_SIZE_MAX: u64 = 16 * 1024;
const RESPONSE_SIZE_MAX: u64 = 2 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PrivateTxBodyRequest {
    pub tx_hash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum PrivateTxBodyResponse {
    Body {
        tx_hash: String,
        body: Vec<u8>,
    },
    NotFound {
        tx_hash: String,
        reason: String,
    },
    RateLimited {
        tx_hash: String,
        retry_after_secs: u64,
    },
}

#[derive(Clone, Debug, Default)]
pub struct PrivateTxRequestResponseCodec;

pub fn protocol_name() -> StreamProtocol {
    StreamProtocol::new(PRIVATE_TX_REQUEST_RESPONSE_PROTOCOL)
}

fn json_decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> io::Result<T> {
    serde_json::from_slice(bytes).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

fn json_encode<T: Serialize>(value: &T) -> io::Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

impl request_response::Codec for PrivateTxRequestResponseCodec {
    type Protocol = StreamProtocol;
    type Request = PrivateTxBodyRequest;
    type Response = PrivateTxBodyResponse;

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
    fn private_tx_body_request_roundtrip() {
        let request = PrivateTxBodyRequest {
            tx_hash: "abcdef".to_string(),
        };
        let encoded = json_encode(&request).expect("encode");
        let decoded: PrivateTxBodyRequest = json_decode(&encoded).expect("decode");
        assert_eq!(decoded, request);
    }

    #[test]
    fn private_tx_body_response_body_roundtrip() {
        let response = PrivateTxBodyResponse::Body {
            tx_hash: "abcdef".to_string(),
            body: vec![1, 2, 3],
        };
        let encoded = json_encode(&response).expect("encode");
        let decoded: PrivateTxBodyResponse = json_decode(&encoded).expect("decode");
        match decoded {
            PrivateTxBodyResponse::Body { tx_hash, body } => {
                assert_eq!(tx_hash, "abcdef");
                assert_eq!(body, vec![1, 2, 3]);
            }
            other => panic!("unexpected response: {:?}", other),
        }
    }

    #[test]
    fn private_tx_body_response_rate_limited_roundtrip() {
        let response = PrivateTxBodyResponse::RateLimited {
            tx_hash: "abcdef".to_string(),
            retry_after_secs: 5,
        };
        let encoded = json_encode(&response).expect("encode");
        let decoded: PrivateTxBodyResponse = json_decode(&encoded).expect("decode");
        match decoded {
            PrivateTxBodyResponse::RateLimited {
                tx_hash,
                retry_after_secs,
            } => {
                assert_eq!(tx_hash, "abcdef");
                assert_eq!(retry_after_secs, 5);
            }
            other => panic!("unexpected response: {:?}", other),
        }
    }
}
