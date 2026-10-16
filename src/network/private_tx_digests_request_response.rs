//! Resúmenes de transacciones privadas en lote, para wallets livianas.
//!
//! Para encontrar sus pagos, una wallet móvil bajaba cada cuerpo de
//! transacción privada de la cadena: en Zenith son 267 y pesan entre 59 y
//! 346 KB, unos 25 MB por Tor (medido el 15/10/2026). Con
//! `/zoka/private-tx-bodies/1` eran menos viajes pero el mismo volumen. Este
//! protocolo manda el resumen ([`PrivateTxDigest`]): las salidas y las marcas
//! de gasto, sin pruebas ni anillo. La wallet decide con su clave qué le toca y
//! pide el cuerpo entero solo de eso.
//!
//! Lo puede servir cualquier nodo y no aprende nada: todas las wallets piden
//! los mismos resúmenes. Ver `docs/HOJA-DE-RUTA-wallet-liviana.md`.

use crate::private_wallet::digest::PrivateTxDigest;
use async_trait::async_trait;
use futures::prelude::*;
use libp2p::{request_response, StreamProtocol};
use serde::{Deserialize, Serialize};
use std::io;

pub const PRIVATE_TX_DIGESTS_REQUEST_RESPONSE_PROTOCOL: &str = "/zoka/private-tx-digests/1";

/// Cuántos resúmenes entran en un pedido.
///
/// Un resumen pesa entre 1 y 10 KB, así que entran muchos más que cuerpos (el
/// lote de cuerpos corta en 100): la cadena entera de Zenith es un solo viaje.
pub const MAX_DIGESTS_PER_REQUEST: usize = 500;

/// 500 hashes de 32 caracteres en JSON, con holgura.
const REQUEST_SIZE_MAX: u64 = 64 * 1024;
/// 500 resúmenes con salidas `zkq1` (1.120 bytes de ciphertext cada una, en
/// hex) entran de sobra; acota la memoria que un par puede hacernos reservar.
const RESPONSE_SIZE_MAX: u64 = 16 * 1024 * 1024;

const FORMATO_JSON_PLANO: u8 = 0;
const FORMATO_JSON_DEFLATE: u8 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PrivateTxDigestsRequest {
    pub tx_hashes: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct PrivateTxDigestsResponse {
    pub digests: Vec<PrivateTxDigest>,
    /// Los que este par no tiene, dichos explícitamente.
    #[serde(default)]
    pub missing: Vec<String>,
    /// Los que no entraron en este pedido por el tope: hay que volver a
    /// pedirlos. Van aparte de `missing` a propósito: el lote de cuerpos los
    /// informaba como faltantes, y "no te los mando ahora" no es "no los tengo".
    #[serde(default)]
    pub deferred: Vec<String>,
}

/// Resultado de pedir resúmenes a los pares conectados.
#[derive(Clone, Debug, Default)]
pub struct PrivateTxDigestsOutcome {
    /// Resúmenes que llegaron, uno por hash pedido.
    pub conseguidos: Vec<PrivateTxDigest>,
    /// Un par dijo explícitamente que no los tiene.
    pub dichos_faltantes: Vec<String>,
    /// Nadie contestó por ellos (par sin el protocolo, plazo agotado, corte):
    /// no se sabe nada y hay que volver a preguntar.
    pub sin_respuesta: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct PrivateTxDigestsRequestResponseCodec;

pub fn protocol_name() -> StreamProtocol {
    StreamProtocol::new(PRIVATE_TX_DIGESTS_REQUEST_RESPONSE_PROTOCOL)
}

fn json_decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> io::Result<T> {
    serde_json::from_slice(bytes).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

fn json_encode<T: Serialize>(value: &T) -> io::Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

fn encode_response(res: &PrivateTxDigestsResponse) -> io::Result<Vec<u8>> {
    use flate2::{write::DeflateEncoder, Compression};
    use std::io::Write;
    let json = json_encode(res)?;
    let mut salida = Vec::with_capacity(json.len() / 3 + 1);
    salida.push(FORMATO_JSON_DEFLATE);
    let mut encoder = DeflateEncoder::new(salida, Compression::default());
    encoder.write_all(&json)?;
    encoder.finish()
}

fn decode_response(bytes: &[u8]) -> io::Result<PrivateTxDigestsResponse> {
    use flate2::read::DeflateDecoder;
    use std::io::Read;
    let (&formato, resto) = bytes
        .split_first()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "respuesta vacia"))?;
    match formato {
        FORMATO_JSON_PLANO => json_decode(resto),
        FORMATO_JSON_DEFLATE => {
            // El tope se aplica también a lo descomprimido: una respuesta chica
            // que se infla sin límite no puede agotarnos la memoria.
            let mut json = Vec::new();
            DeflateDecoder::new(resto)
                .take(RESPONSE_SIZE_MAX * 4)
                .read_to_end(&mut json)?;
            json_decode(&json)
        }
        otro => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("formato de respuesta desconocido: {otro}"),
        )),
    }
}

/// La respuesta a un pedido, con lo que se pase del tope como aplazado.
///
/// `lookup` busca el resumen de un hash; `None` es "no lo tengo".
pub fn answer_request(
    request: &PrivateTxDigestsRequest,
    mut lookup: impl FnMut(&str) -> Option<PrivateTxDigest>,
) -> PrivateTxDigestsResponse {
    let mut response = PrivateTxDigestsResponse::default();
    let mut atendidos = 0usize;
    for hash in &request.tx_hashes {
        let hash = hash.trim();
        if hash.is_empty() {
            continue;
        }
        if atendidos >= MAX_DIGESTS_PER_REQUEST {
            response.deferred.push(hash.to_string());
            continue;
        }
        atendidos += 1;
        match lookup(hash) {
            Some(digest) if digest.tx_hash == hash => response.digests.push(digest),
            _ => response.missing.push(hash.to_string()),
        }
    }
    response
}

#[async_trait]
impl request_response::Codec for PrivateTxDigestsRequestResponseCodec {
    type Protocol = StreamProtocol;
    type Request = PrivateTxDigestsRequest;
    type Response = PrivateTxDigestsResponse;

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
        decode_response(&bytes)
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
        io.write_all(&encode_response(&res)?).await?;
        io.close().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::private_wallet::digest::PrivateOutputDigest;

    fn resumen(hash: &str) -> PrivateTxDigest {
        PrivateTxDigest {
            tx_hash: hash.to_string(),
            spend_tags: vec!["0123456789abcdef".to_string()],
            outputs: vec![PrivateOutputDigest {
                output_id: format!("{hash}-0"),
                output_version: 2,
                ephemeral_pubkey: String::new(),
                kem_ciphertext: "ab".repeat(1120),
                commitment: "c".repeat(64),
                one_time_public_key: "d".repeat(64),
            }],
        }
    }

    #[test]
    fn el_pedido_va_y_vuelve() {
        let request = PrivateTxDigestsRequest {
            tx_hashes: vec!["aa".to_string(), "bb".to_string()],
        };
        let decoded: PrivateTxDigestsRequest =
            json_decode(&json_encode(&request).unwrap()).unwrap();
        assert_eq!(decoded, request);
    }

    #[test]
    fn la_respuesta_sobrevive_la_compresion() {
        let res = PrivateTxDigestsResponse {
            digests: vec![resumen("aa")],
            missing: vec!["bb".to_string()],
            deferred: vec!["cc".to_string()],
        };
        let encoded = encode_response(&res).unwrap();
        assert_eq!(encoded[0], FORMATO_JSON_DEFLATE);
        assert_eq!(decode_response(&encoded).unwrap(), res);
    }

    #[test]
    fn se_entiende_el_json_sin_comprimir_y_sin_campos_nuevos() {
        // Un par que no mande `deferred` (o que no comprima) sigue siendo legible.
        let mut plano = vec![FORMATO_JSON_PLANO];
        plano.extend_from_slice(br#"{"digests":[],"missing":["cc"]}"#);
        let decoded = decode_response(&plano).unwrap();
        assert_eq!(decoded.missing, vec!["cc".to_string()]);
        assert!(decoded.deferred.is_empty());
    }

    #[test]
    fn un_formato_desconocido_se_rechaza() {
        assert!(decode_response(&[99u8, 1, 2, 3]).is_err());
        assert!(decode_response(&[]).is_err());
    }

    #[test]
    fn lo_que_pasa_el_tope_queda_aplazado_y_no_faltante() {
        let hashes: Vec<String> = (0..MAX_DIGESTS_PER_REQUEST + 3)
            .map(|i| format!("{i:032x}"))
            .collect();
        let request = PrivateTxDigestsRequest {
            tx_hashes: hashes.clone(),
        };
        let res = answer_request(&request, |hash| {
            (hash != hashes[1]).then(|| resumen(hash))
        });
        assert_eq!(res.digests.len(), MAX_DIGESTS_PER_REQUEST - 1);
        assert_eq!(res.missing, vec![hashes[1].clone()]);
        assert_eq!(res.deferred, hashes[MAX_DIGESTS_PER_REQUEST..].to_vec());
    }

    #[test]
    fn un_resumen_de_otro_hash_cuenta_como_faltante() {
        let request = PrivateTxDigestsRequest {
            tx_hashes: vec!["aa".to_string()],
        };
        let res = answer_request(&request, |_| Some(resumen("otro")));
        assert!(res.digests.is_empty());
        assert_eq!(res.missing, vec!["aa".to_string()]);
    }

    #[test]
    fn un_lote_de_resumenes_comprime() {
        let res = PrivateTxDigestsResponse {
            digests: (0..100).map(|i| resumen(&format!("{i:032x}"))).collect(),
            ..Default::default()
        };
        let plano = json_encode(&res).unwrap().len();
        let comprimido = encode_response(&res).unwrap().len();
        assert!(comprimido * 2 < plano, "{plano} -> {comprimido}");
    }
}
