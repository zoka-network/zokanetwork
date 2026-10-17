//! Lo que un nodo de Zoka Quantic le sirve a una wallet liviana.
//!
//! La API HTTP (`GET /quantic/info`, `GET /quantic/compact`, `POST
//! /quantic/tx`) y el protocolo P2P de los teléfonos (`/zoka-quantic/light/1`)
//! responden con estas mismas funciones: por las dos vías se recibe lo mismo, y
//! la wallet lo verifica igual (ver `wallet_sync`).

use crate::network::quantic_light_request_response::{
    CompactBlockWire, QuanticLightRequest, QuanticLightResponse, MAX_COMPACT_BLOCKS_PER_REQUEST,
};
use crate::storage::RocksDbStorage;

use super::{decode_block, runtime, tree_root_after, GENESIS_HEIGHT};

/// La punta de la cadena que sigue el nodo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChainInfo {
    pub height: u64,
    pub tip: String,
    /// Raíz del árbol de notas después de la punta (el ancla de la próxima
    /// transferencia). `None` antes del génesis.
    pub tree_root: Option<String>,
}

pub fn chain_info(storage: &RocksDbStorage) -> Result<ChainInfo, String> {
    let height = storage.get_chain_height().map_err(|e| format!("{e:?}"))?;
    let tip = storage.get_last_block_hash().map_err(|e| format!("{e:?}"))?;
    let tree_root = if height >= GENESIS_HEIGHT {
        tree_root_after(storage, &tip).map_err(|e| e.to_string())?.map(hex::encode)
    } else {
        None
    };
    Ok(ChainInfo { height, tip, tree_root })
}

/// El bloque compacto de la altura `height` de la cadena canónica; `None` si
/// todavía no existe.
pub fn compact_block_at(storage: &RocksDbStorage, height: u64) -> Result<Option<CompactBlockWire>, String> {
    let Some(block) = storage
        .get_canonical_block_by_height(height)
        .map_err(|e| format!("{e:?}"))?
    else {
        return Ok(None);
    };
    let decoded = decode_block(&block).map_err(|e| e.to_string())?;
    let compact = zoka_quantic::compact::CompactBlock::from_block(&decoded);
    Ok(Some(CompactBlockWire {
        height,
        hash: block.header.hash,
        prev_hash: block.header.prev_hash,
        state_root: block.header.state_root,
        transactions: compact.transactions.iter().map(|tx| hex::encode(tx.to_bytes())).collect(),
    }))
}

/// Bytes de transacciones compactas (en hex) por respuesta. Con bloques casi
/// vacíos entran los 500; con bloques llenos (96 acciones, unos 240 KB en hex)
/// serían más de 100 MB: la respuesta se corta antes y la wallet sigue desde
/// el último bloque que recibió. Comprimido queda en unos 4 MB, que por Tor
/// llegan en decenas de segundos.
pub const MAX_COMPACT_RESPONSE_HEX_BYTES: usize = 8 * 1024 * 1024;

/// Los bloques compactos de `from` a `to` (sin `to`, hasta la punta), como
/// mucho `limit` y hasta [`MAX_COMPACT_RESPONSE_HEX_BYTES`]. Antes del génesis
/// no hay: empieza en él.
pub fn compact_blocks(
    storage: &RocksDbStorage,
    from: u64,
    to: Option<u64>,
    limit: u64,
) -> Result<Vec<CompactBlockWire>, String> {
    compact_blocks_within(storage, from, to, limit, MAX_COMPACT_RESPONSE_HEX_BYTES)
}

/// Lo mismo con otro presupuesto de bytes. El primer bloque va siempre, aunque
/// solo pase el presupuesto (uno lleno son 1,3 MB): así la wallet avanza.
pub fn compact_blocks_within(
    storage: &RocksDbStorage,
    from: u64,
    to: Option<u64>,
    limit: u64,
    max_hex_bytes: usize,
) -> Result<Vec<CompactBlockWire>, String> {
    let height = storage.get_chain_height().map_err(|e| format!("{e:?}"))?;
    let from = from.max(GENESIS_HEIGHT);
    let to = to
        .unwrap_or(height)
        .min(height)
        .min(from.saturating_add(limit.max(1) - 1));
    let mut blocks = Vec::new();
    let mut hex_bytes = 0usize;
    for height in from..=to {
        let Some(block) = compact_block_at(storage, height)? else {
            break;
        };
        let size: usize = block.transactions.iter().map(String::len).sum();
        if !blocks.is_empty() && hex_bytes.saturating_add(size) > max_hex_bytes {
            break;
        }
        hex_bytes = hex_bytes.saturating_add(size);
        blocks.push(block);
    }
    Ok(blocks)
}

/// La respuesta a un pedido de `/zoka-quantic/light/1` de un nodo que sigue
/// `chain_id`.
pub fn answer(storage: &RocksDbStorage, chain_id: &str, request: &QuanticLightRequest) -> QuanticLightResponse {
    let response = match request {
        QuanticLightRequest::Info => chain_info(storage).map(|info| QuanticLightResponse::Info {
            chain_id: chain_id.to_string(),
            height: info.height,
            tip: info.tip,
            tree_root: info.tree_root,
        }),
        QuanticLightRequest::Compact { from, to } => {
            compact_blocks(storage, *from, Some(*to), MAX_COMPACT_BLOCKS_PER_REQUEST)
                .map(|blocks| QuanticLightResponse::Compact { blocks })
        }
        QuanticLightRequest::SubmitTx { tx_hex } => Ok(submit(storage, chain_id, tx_hex)),
    };
    response.unwrap_or_else(|message| QuanticLightResponse::Error { message })
}

/// Una transferencia armada y probada en el teléfono: si el mempool la
/// admite, sale a la red en la próxima vuelta del runtime.
fn submit(storage: &RocksDbStorage, chain_id: &str, tx_hex: &str) -> QuanticLightResponse {
    let rejected = |reason: String| QuanticLightResponse::Submitted {
        accepted: false,
        txid: None,
        reason: Some(reason),
    };
    let Ok(bytes) = hex::decode(tx_hex.trim()) else {
        return rejected("tx_hex is not hex".to_string());
    };
    match runtime::submit_from_api(storage, chain_id, &bytes) {
        Ok(txid) => QuanticLightResponse::Submitted {
            accepted: true,
            txid: Some(txid),
            reason: None,
        },
        Err(error) => rejected(error.to_string()),
    }
}
