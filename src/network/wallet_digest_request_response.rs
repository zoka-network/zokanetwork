//! Resumenes de bloque para wallets: lo justo para escanear, nada mas.
//!
//! Una wallet liviana que quiere saber su saldo tiene que mirar cada bloque de
//! la cadena, y el protocolo de bloques solo sabe servir bloques enteros. Eso
//! la obliga a bajar la cadena completa: medido contra la red real, 4.559 bytes
//! por bloque -- 77 MB para 17.772 bloques, cincuenta y un minutos sobre Tor en
//! un telefono.
//!
//! De esos 4.559 bytes, **3.069 son la prueba de rango del coinbase**: el 56%
//! de cada bloque, que la wallet baja y descarta salvo en los poquisimos
//! bloques donde el coinbase es suyo. Este protocolo manda el resto -- unos
//! 500 bytes por bloque -- y deja que quien reconozca un coinbase propio pida
//! ESE bloque entero por el protocolo normal.
//!
//! **Por que la prueba de rango no puede omitirse siempre.** Entra en el hash
//! que forma el `output_id` de la nota y se verifica al gastarla. Para las
//! notas propias hace falta; para las ajenas es peso muerto. De ahi el reparto:
//! resumen para todos, bloque entero para los pocos que importan.
//!
//! **Privacidad.** Un resumen lleva el destinatario del coinbase, que ya viaja
//! en claro dentro del bloque -- no se revela nada nuevo. Lo que si cambia es
//! que pedir un bloque entero se vuelve una señal: dice que ahi hay algo que a
//! quien pregunta le interesa. Sobre Tor el par no sabe quien pregunta, pero
//! conviene tenerlo presente antes de usar esto contra un par unico y conocido.

use futures::prelude::*;
use libp2p::{request_response, StreamProtocol};
use serde::{Deserialize, Serialize};
use std::io;

pub const WALLET_DIGEST_REQUEST_RESPONSE_PROTOCOL: &str = "/zoka/wallet-digests/1";
const REQUEST_SIZE_MAX: u64 = 4 * 1024;
const RESPONSE_SIZE_MAX: u64 = 16 * 1024 * 1024;

/// Cuantos resumenes se sirven como maximo en una respuesta.
///
/// Acota lo que un par puede hacerle pedir a otro de una sola vez. A ~500 bytes
/// por resumen, mil entran comodos en la respuesta maxima.
pub const MAX_DIGESTS_PER_RESPONSE: u64 = 1000;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GetWalletDigests {
    pub from_height: u64,
    pub to_height: u64,
}

/// Lo que una wallet necesita de un bloque para saber si le interesa.
///
/// Los campos opcionales faltan cuando el bloque no trae coinbase privado.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlockDigest {
    pub block_height: u64,
    pub block_hash: String,
    /// Encadena los resumenes sin cargar el resto del encabezado. Un cliente
    /// liviano puede detectar huecos o una mezcla de ramas entre respuestas.
    /// `default` conserva lectura de respuestas emitidas por el v1 original;
    /// el cliente trata el valor vacio como protocolo legacy y usa bloques.
    #[serde(default)]
    pub prev_hash: String,
    pub timestamp: u64,
    /// Destinatario y monto del coinbase, en claro como ya estan en el bloque.
    /// Es lo que deja reconocer "este es mio" sin bajar nada mas.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coinbase_recipient: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coinbase_amount: Option<u64>,
    /// Identidad y maduracion de la nota del coinbase. Alcanzan para contar el
    /// saldo; para gastarla hace falta el bloque entero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coinbase_output_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coinbase_commitment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coinbase_unlock_height: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub private_tx_hashes: Vec<String>,
    /// Las notas que este bloque consume. Es la fuente autoritativa de gastos:
    /// una nota no puede figurar gastable solo porque no se pudo bajar el
    /// cuerpo de la transaccion que la consumio.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub private_nullifiers: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum WalletDigestsResponse {
    Digests {
        digests: Vec<BlockDigest>,
    },
    /// El par no puede servir ese tramo. Quien pregunta cae al protocolo de
    /// bloques enteros, que es lento pero siempre esta.
    NotFound {
        reason: String,
    },
}

#[derive(Clone, Debug, Default)]
pub struct WalletDigestRequestResponseCodec;

pub fn protocol_name() -> StreamProtocol {
    StreamProtocol::new(WALLET_DIGEST_REQUEST_RESPONSE_PROTOCOL)
}

fn json_decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> io::Result<T> {
    serde_json::from_slice(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

fn json_encode<T: Serialize>(value: &T) -> io::Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Marca de formato al frente de una respuesta.
///
/// Un resumen son unos 700 bytes y casi todos son texto repetido: cuatro
/// hashes en hexadecimal, la direccion del destinatario, y los nombres de los
/// campos JSON repetidos una vez por bloque. Comprimir eso no es una
/// optimizacion marginal -- es quitar de la linea lo que nunca deberia haber
/// viajado.
///
/// El byte va delante para que quien lee sepa que recibio sin adivinar. Un par
/// que solo entienda el formato plano recibe algo que no es JSON valido,
/// responde que no puede, y quien pregunta cae a bloques enteros: lento, pero
/// nunca un saldo equivocado.
const FORMATO_JSON_PLANO: u8 = 0;
const FORMATO_JSON_DEFLATE: u8 = 1;

fn encode_response(res: &WalletDigestsResponse) -> io::Result<Vec<u8>> {
    use flate2::{write::DeflateEncoder, Compression};
    use std::io::Write;

    let json = json_encode(res)?;
    let mut salida = Vec::with_capacity(json.len() / 4 + 1);
    salida.push(FORMATO_JSON_DEFLATE);
    let mut encoder = DeflateEncoder::new(salida, Compression::default());
    encoder.write_all(&json)?;
    encoder.finish()
}

fn decode_response(bytes: &[u8]) -> io::Result<WalletDigestsResponse> {
    let Some((marca, resto)) = bytes.split_first() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "respuesta de resumenes vacia",
        ));
    };
    match *marca {
        FORMATO_JSON_DEFLATE => {
            use flate2::read::DeflateDecoder;
            use std::io::Read;
            let mut json = Vec::new();
            DeflateDecoder::new(resto)
                .take(RESPONSE_SIZE_MAX)
                .read_to_end(&mut json)?;
            json_decode(&json)
        }
        FORMATO_JSON_PLANO => json_decode(resto),
        otro => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("formato de resumenes desconocido: {otro}"),
        )),
    }
}

impl request_response::Codec for WalletDigestRequestResponseCodec {
    type Protocol = StreamProtocol;
    type Request = GetWalletDigests;
    type Response = WalletDigestsResponse;

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

impl BlockDigest {
    /// Arma el resumen de un bloque.
    ///
    /// Vive aca y no en el nodo para que quien responde y quien pregunta tengan
    /// una sola definicion de que lleva un resumen.
    pub fn from_block(block: &crate::types::Block) -> Self {
        let coinbase_tx = block.transactions.first();
        let coinbase = block.private_coinbase.as_ref();
        BlockDigest {
            block_height: block.header.block_height,
            block_hash: block.header.hash.clone(),
            prev_hash: block.header.prev_hash.clone(),
            timestamp: block.header.timestamp,
            coinbase_recipient: coinbase_tx.map(|tx| tx.receiver.clone()),
            coinbase_amount: coinbase_tx.map(|tx| tx.amount),
            coinbase_output_id: coinbase.map(|c| c.output_id.clone()),
            coinbase_commitment: coinbase.map(|c| c.commitment.clone()),
            coinbase_unlock_height: coinbase.map(|c| c.unlock_height),
            private_tx_hashes: block.private_tx_hashes.clone(),
            private_nullifiers: block.private_nullifiers.clone(),
        }
    }
}

#[cfg(test)]
mod tests_compresion {
    use super::*;

    /// Un resumen con la forma y el tamaño de los reales: hashes de 64
    /// caracteres, una direccion larga, y los mismos nombres de campo en cada
    /// bloque. Es justo esa repeticion la que hace que comprimir valga tanto.
    fn resumen(altura: u64) -> BlockDigest {
        BlockDigest {
            block_height: altura,
            block_hash: format!("{:064x}", altura.wrapping_mul(0x9E3779B97F4A7C15)),
            prev_hash: format!("{:064x}", altura.wrapping_sub(1).wrapping_mul(0x9E3779B97F4A7C15)),
            timestamp: 1_786_000_000 + altura,
            coinbase_recipient: Some(
                "zka1DvXCzHCQa9iqecNxL5KVpbuodnGyuh8a35WHYQ6HWYrJTRw88JeBYcB1KBCuXGAvwqyzPydEPzShSkTHXf1vMD1R7mWhu"
                    .to_string(),
            ),
            coinbase_amount: Some(12_000_000),
            coinbase_output_id: Some(format!("{:064x}", altura.wrapping_mul(0xD6E8FEB86659FD93))),
            coinbase_commitment: Some(format!("{:064x}", altura.wrapping_mul(0xA24BAED4963EE407))),
            coinbase_unlock_height: Some(altura + 100),
            private_tx_hashes: Vec::new(),
            private_nullifiers: Vec::new(),
        }
    }

    #[test]
    fn comprimir_los_resumenes_baja_el_costo_de_escanear_sobre_tor() {
        // El caso real: la wallet pide un tramo y lo recibe por un circuito
        // Tor a unos 20 KB/s. Cada byte que no viaja son milisegundos que el
        // usuario no espera mirando una pantalla que parece colgada.
        let respuesta = WalletDigestsResponse::Digests {
            digests: (1..=250).map(resumen).collect(),
        };
        let plano = json_encode(&respuesta).expect("serializa");
        let comprimido = encode_response(&respuesta).expect("comprime");

        println!(
            "250 resumenes: plano {} B ({} por bloque) -> comprimido {} B ({} por bloque), {:.1}x",
            plano.len(),
            plano.len() / 250,
            comprimido.len(),
            comprimido.len() / 250,
            plano.len() as f64 / comprimido.len() as f64
        );
        assert!(
            comprimido.len() * 3 < plano.len(),
            "comprimir tiene que ahorrar al menos tres veces para justificar la              dependencia; plano {} B, comprimido {} B",
            plano.len(),
            comprimido.len()
        );

        // Y lo que llega tiene que ser exactamente lo que se mando: un ahorro
        // que pierde datos no es un ahorro.
        let vuelta = decode_response(&comprimido).expect("descomprime");
        let WalletDigestsResponse::Digests { digests } = vuelta else {
            panic!("tiene que volver como resumenes");
        };
        assert_eq!(digests.len(), 250);
        assert_eq!(digests[0].block_height, 1);
        assert_eq!(digests[249].block_height, 250);
        assert_eq!(digests[120].coinbase_output_id, resumen(121).coinbase_output_id);
    }

    #[test]
    fn el_formato_plano_se_sigue_leyendo() {
        // Un par que todavia mande JSON sin comprimir tiene que poder ser
        // entendido: el byte de formato existe justo para no tener que adivinar.
        let respuesta = WalletDigestsResponse::NotFound {
            reason: "sin cadena".to_string(),
        };
        let mut plano = vec![FORMATO_JSON_PLANO];
        plano.extend_from_slice(&json_encode(&respuesta).expect("serializa"));

        let vuelta = decode_response(&plano).expect("lee el formato plano");
        let WalletDigestsResponse::NotFound { reason } = vuelta else {
            panic!("tiene que volver como NotFound");
        };
        assert_eq!(reason, "sin cadena");
    }

    #[test]
    fn un_formato_desconocido_falla_en_vez_de_inventar() {
        let basura = vec![99u8, 1, 2, 3];
        assert!(decode_response(&basura).is_err());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_v1_legacy_without_prev_hash_still_decodes() {
        let legacy = serde_json::json!({
            "block_height": 7,
            "block_hash": "hash-7",
            "timestamp": 1234
        });
        let digest: BlockDigest = serde_json::from_value(legacy).expect("legacy v1 digest");

        assert_eq!(digest.block_height, 7);
        assert!(digest.prev_hash.is_empty());
    }
}
