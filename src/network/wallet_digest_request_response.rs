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

use async_trait::async_trait;
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

#[async_trait]
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
