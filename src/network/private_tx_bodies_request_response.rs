//! Muchos cuerpos de transaccion privada en una sola peticion.
//!
//! El protocolo de a uno (`/zoka/private-tx/reqresp/1`) sigue existiendo y es
//! el que hay que usar cuando falta un cuerpo suelto. Este es para el caso de
//! la wallet que arranca: medido contra la cadena real, 266 transacciones
//! privadas significaban 266 idas y vueltas por Tor, serializadas, y eso era
//! todo el tiempo de sincronizacion de un telefono.
//!
//! Los cuerpos no se pueden sacar de los bloques. El campo `private_tx_bodies`
//! del bloque existe pero llega vacio: es una funcion dormida hasta que se fije
//! su altura de activacion en consenso. Verificado el 17/08 contra los bloques
//! 423, 16211 y 17771 -- hashes privados si, cuerpos cero. Asi que la unica
//! forma de conseguirlos es pedirlos, y lo unico que se puede mejorar es
//! cuantas veces hay que pedir.
//!
//! Mismo patron que `/zoka/wallet-digests/1`, que ya dio 10,9x sobre la cadena
//! real: lote + deflate. Un cuerpo de transaccion privada es JSON con mucha
//! estructura repetida, asi que comprime bien en conjunto y casi nada de a uno.

use async_trait::async_trait;
use futures::prelude::*;
use libp2p::{request_response, StreamProtocol};
use serde::{Deserialize, Serialize};
use std::io;

pub const PRIVATE_TX_BODIES_REQUEST_RESPONSE_PROTOCOL: &str = "/zoka/private-tx-bodies/1";

/// Cuantos cuerpos entran en un pedido.
///
/// No es un numero de rendimiento sino de cortesia: un pedido gigante obliga al
/// que responde a leer y comprimir mucho de golpe, y sobre Tor un mensaje
/// enorme tarda mas en llegar que dos medianos. Cien alcanza para que 266
/// transacciones sean tres viajes en lugar de 266.
pub const MAX_BODIES_PER_REQUEST: usize = 100;

const REQUEST_SIZE_MAX: u64 = 16 * 1024;
/// Los cuerpos llevan pruebas, asi que un lote pesa. Se acota para que un par
/// no pueda hacer que reservemos memoria sin limite.
const RESPONSE_SIZE_MAX: u64 = 32 * 1024 * 1024;

const FORMATO_JSON_PLANO: u8 = 0;
const FORMATO_JSON_DEFLATE: u8 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PrivateTxBodiesRequest {
    pub tx_hashes: Vec<String>,
}

/// Un cuerpo que si estaba.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PrivateTxBodyEntry {
    pub tx_hash: String,
    pub body: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct PrivateTxBodiesResponse {
    pub bodies: Vec<PrivateTxBodyEntry>,
    /// Los que este par no tiene, dichos explicitamente.
    ///
    /// Van aparte y no como ausencia silenciosa: "no lo tengo" y "no te
    /// entendi" son cosas distintas, y quien pregunta necesita distinguirlas
    /// para saber si vale la pena preguntarle a otro.
    pub missing: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct PrivateTxBodiesRequestResponseCodec;

pub fn protocol_name() -> StreamProtocol {
    StreamProtocol::new(PRIVATE_TX_BODIES_REQUEST_RESPONSE_PROTOCOL)
}

fn json_decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> io::Result<T> {
    serde_json::from_slice(bytes).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

fn json_encode<T: Serialize>(value: &T) -> io::Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

fn encode_response(res: &PrivateTxBodiesResponse) -> io::Result<Vec<u8>> {
    use flate2::{write::DeflateEncoder, Compression};
    use std::io::Write;
    let json = json_encode(res)?;
    let mut salida = Vec::with_capacity(json.len() / 4 + 1);
    salida.push(FORMATO_JSON_DEFLATE);
    let mut encoder = DeflateEncoder::new(salida, Compression::default());
    encoder.write_all(&json)?;
    encoder.finish()
}

fn decode_response(bytes: &[u8]) -> io::Result<PrivateTxBodiesResponse> {
    use flate2::read::DeflateDecoder;
    use std::io::Read;
    let (&formato, resto) = bytes
        .split_first()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "respuesta vacia"))?;
    match formato {
        FORMATO_JSON_PLANO => json_decode(resto),
        FORMATO_JSON_DEFLATE => {
            let mut json = Vec::new();
            DeflateDecoder::new(resto).read_to_end(&mut json)?;
            json_decode(&json)
        }
        otro => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("formato de respuesta desconocido: {otro}"),
        )),
    }
}

#[async_trait]
impl request_response::Codec for PrivateTxBodiesRequestResponseCodec {
    type Protocol = StreamProtocol;
    type Request = PrivateTxBodiesRequest;
    type Response = PrivateTxBodiesResponse;

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

    #[test]
    fn el_pedido_de_muchos_hashes_va_y_vuelve() {
        let request = PrivateTxBodiesRequest {
            tx_hashes: vec!["aa".to_string(), "bb".to_string()],
        };
        let encoded = json_encode(&request).expect("encode");
        let decoded: PrivateTxBodiesRequest = json_decode(&encoded).expect("decode");
        assert_eq!(decoded, request);
    }

    #[test]
    fn los_cuerpos_y_los_faltantes_sobreviven_la_compresion() {
        let res = PrivateTxBodiesResponse {
            bodies: vec![PrivateTxBodyEntry {
                tx_hash: "aa".to_string(),
                body: vec![1, 2, 3, 4],
            }],
            missing: vec!["bb".to_string()],
        };
        let encoded = encode_response(&res).expect("encode");
        assert_eq!(
            encoded[0], FORMATO_JSON_DEFLATE,
            "la respuesta tiene que ir comprimida"
        );
        let decoded = decode_response(&encoded).expect("decode");
        assert_eq!(decoded.bodies.len(), 1);
        assert_eq!(decoded.bodies[0].tx_hash, "aa");
        assert_eq!(decoded.bodies[0].body, vec![1, 2, 3, 4]);
        assert_eq!(decoded.missing, vec!["bb".to_string()]);
    }

    #[test]
    fn se_sigue_entendiendo_el_json_sin_comprimir() {
        // Un par viejo -- o una version futura que decida no comprimir -- tiene
        // que seguir siendo legible. Sin esto, cambiar el formato una vez mas
        // partiria la red en dos.
        let res = PrivateTxBodiesResponse {
            bodies: vec![],
            missing: vec!["cc".to_string()],
        };
        let mut plano = vec![FORMATO_JSON_PLANO];
        plano.extend_from_slice(&json_encode(&res).expect("encode"));
        let decoded = decode_response(&plano).expect("decode");
        assert_eq!(decoded.missing, vec!["cc".to_string()]);
    }

    #[test]
    fn un_formato_desconocido_se_rechaza_en_vez_de_adivinarse() {
        let basura = vec![99u8, 1, 2, 3];
        assert!(decode_response(&basura).is_err());
    }

    #[test]
    fn un_lote_de_cuerpos_parecidos_comprime_mucho() {
        // El caso real: cuerpos de transaccion privada, que son JSON con la
        // misma forma repetida. De a uno la compresion no compra nada; en lote
        // es donde aparece la ganancia, y es la razon de que este protocolo
        // exista.
        let cuerpo = serde_json::json!({
            "tx_hash": "0".repeat(32),
            "inputs": [{"nullifier": "1".repeat(64), "proof": "2".repeat(256)}],
            "outputs": [{"commitment": "3".repeat(64), "ciphertext": "4".repeat(256)}],
        })
        .to_string()
        .into_bytes();
        let res = PrivateTxBodiesResponse {
            bodies: (0..50)
                .map(|i| PrivateTxBodyEntry {
                    tx_hash: format!("{i:032x}"),
                    body: cuerpo.clone(),
                })
                .collect(),
            missing: vec![],
        };
        let plano = json_encode(&res).expect("plano").len();
        let comprimido = encode_response(&res).expect("comprimido").len();
        assert!(
            comprimido * 4 < plano,
            "un lote de cuerpos parecidos deberia comprimir al menos 4x: {plano} -> {comprimido}"
        );
    }
}
