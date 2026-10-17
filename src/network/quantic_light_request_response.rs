//! Zoka Quantic para wallets livianas por la red P2P (`/zoka-quantic/light/1`).
//!
//! Un teléfono no llega a la API HTTP de un nodo (no hay puertos abiertos, va
//! por Tor): pide por este protocolo, a cualquier nodo conectado, lo mismo que
//! `zoka quantic-wallet` pide por HTTP. Tres pedidos:
//!
//! - `info`: cadena, altura, punta y raíz del árbol de notas;
//! - `compact { from, to }`: los bloques compactos de la etapa 5 (A5), que la
//!   wallet verifica contra el `state_root` de cada bloque;
//! - `submit_tx { tx_hex }`: una transferencia ya armada y probada en el
//!   teléfono, que el nodo admite en su mempool y publica.
//!
//! El nodo no aprende qué notas son del teléfono: todas las wallets piden los
//! mismos bloques. Sí ve cuándo una envía una transacción (como cualquier par
//! que la reciba primero); por eso conviene usar Tor.

use async_trait::async_trait;
use futures::prelude::*;
use libp2p::{request_response, StreamProtocol};
use serde::{Deserialize, Serialize};
use std::io;

pub const QUANTIC_LIGHT_PROTOCOL: &str = "/zoka-quantic/light/1";

/// Bloques compactos por pedido. Uno vacío (solo el coinbase) pesa unos
/// 2,5 KB en hex: 500 son poco más de 1 MB.
pub const MAX_COMPACT_BLOCKS_PER_REQUEST: u64 = 500;

/// Un pedido es chico; una transferencia de 64 acciones en hex, la más grande.
const REQUEST_SIZE_MAX: u64 = 1024 * 1024;
/// 500 bloques compactos llenos, con holgura; acota la memoria que un par
/// puede hacernos reservar.
const RESPONSE_SIZE_MAX: u64 = 32 * 1024 * 1024;

const FORMATO_JSON_PLANO: u8 = 0;
const FORMATO_JSON_DEFLATE: u8 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QuanticLightRequest {
    Info,
    Compact { from: u64, to: u64 },
    SubmitTx { tx_hex: String },
}

/// Un bloque compacto como lo da `GET /quantic/compact`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompactBlockWire {
    pub height: u64,
    pub hash: String,
    pub prev_hash: String,
    pub state_root: String,
    pub transactions: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QuanticLightResponse {
    Info {
        chain_id: String,
        height: u64,
        tip: String,
        tree_root: Option<String>,
    },
    Compact {
        blocks: Vec<CompactBlockWire>,
    },
    Submitted {
        accepted: bool,
        txid: Option<String>,
        reason: Option<String>,
    },
    Error {
        message: String,
    },
}

#[derive(Clone, Debug, Default)]
pub struct QuanticLightCodec;

pub fn protocol_name() -> StreamProtocol {
    StreamProtocol::new(QUANTIC_LIGHT_PROTOCOL)
}

fn json_decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> io::Result<T> {
    serde_json::from_slice(bytes).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

fn json_encode<T: Serialize>(value: &T) -> io::Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

/// Comprimido con deflate: el hex de los ciphertexts X-Wing se reduce casi a la mitad.
fn encode_response(response: &QuanticLightResponse) -> io::Result<Vec<u8>> {
    use std::io::Write;
    let json = json_encode(response)?;
    let mut encoder = flate2::write::DeflateEncoder::new(vec![FORMATO_JSON_DEFLATE], flate2::Compression::fast());
    encoder.write_all(&json)?;
    encoder.finish()
}

fn decode_response(bytes: &[u8]) -> io::Result<QuanticLightResponse> {
    use std::io::Read;
    match bytes.split_first() {
        Some((&FORMATO_JSON_PLANO, rest)) => json_decode(rest),
        Some((&FORMATO_JSON_DEFLATE, rest)) => {
            let mut json = Vec::new();
            flate2::read::DeflateDecoder::new(rest)
                .take(RESPONSE_SIZE_MAX * 4)
                .read_to_end(&mut json)?;
            json_decode(&json)
        }
        _ => Err(io::Error::new(io::ErrorKind::InvalidData, "unknown response format")),
    }
}

#[async_trait]
impl request_response::Codec for QuanticLightCodec {
    type Protocol = StreamProtocol;
    type Request = QuanticLightRequest;
    type Response = QuanticLightResponse;

    async fn read_request<T>(&mut self, _: &Self::Protocol, io: &mut T) -> io::Result<Self::Request>
    where
        T: AsyncRead + Unpin + Send,
    {
        let mut bytes = Vec::new();
        io.take(REQUEST_SIZE_MAX).read_to_end(&mut bytes).await?;
        json_decode(&bytes)
    }

    async fn read_response<T>(&mut self, _: &Self::Protocol, io: &mut T) -> io::Result<Self::Response>
    where
        T: AsyncRead + Unpin + Send,
    {
        let mut bytes = Vec::new();
        io.take(RESPONSE_SIZE_MAX).read_to_end(&mut bytes).await?;
        decode_response(&bytes)
    }

    async fn write_request<T>(&mut self, _: &Self::Protocol, io: &mut T, request: Self::Request) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        io.write_all(&json_encode(&request)?).await?;
        io.close().await
    }

    async fn write_response<T>(&mut self, _: &Self::Protocol, io: &mut T, response: Self::Response) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        io.write_all(&encode_response(&response)?).await?;
        io.close().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_and_responses_survive_their_wire_format() {
        for request in [
            QuanticLightRequest::Info,
            QuanticLightRequest::Compact { from: 1, to: 500 },
            QuanticLightRequest::SubmitTx { tx_hex: "ab".repeat(100) },
        ] {
            let bytes = json_encode(&request).unwrap();
            assert_eq!(json_decode::<QuanticLightRequest>(&bytes).unwrap(), request);
        }
        let response = QuanticLightResponse::Compact {
            blocks: vec![CompactBlockWire {
                height: 7,
                hash: "aa".repeat(32),
                prev_hash: "bb".repeat(32),
                state_root: "quantic-v1:...".to_string(),
                transactions: vec!["cd".repeat(1300)],
            }],
        };
        let bytes = encode_response(&response).unwrap();
        assert!(bytes.len() < 1300, "deflate shrinks the hex: {}", bytes.len());
        assert_eq!(decode_response(&bytes).unwrap(), response);
        assert!(decode_response(&[9, 1, 2]).is_err());
    }

    #[test]
    fn the_json_names_are_stable_for_the_phone() {
        let json = serde_json::to_string(&QuanticLightRequest::Compact { from: 1, to: 2 }).unwrap();
        assert_eq!(json, r#"{"kind":"compact","from":1,"to":2}"#);
        let info: QuanticLightResponse = serde_json::from_str(
            r#"{"kind":"info","chain_id":"zoka-quantic-testnet-v1","height":3,"tip":"x","tree_root":null}"#,
        )
        .unwrap();
        assert!(matches!(info, QuanticLightResponse::Info { height: 3, .. }));
    }
}
