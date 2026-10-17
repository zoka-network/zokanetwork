//! La API de Zoka Quantic del nodo: lo que necesitan una wallet (de
//! escritorio, del teléfono o la de línea de comandos) y un explorador.
//!
//! - `GET /quantic/info`: cadena, punta, raíz del árbol (el ancla de la próxima
//!   transferencia) y tamaño del mempool.
//! - `POST /quantic/tx` con `{"tx_hex": "..."}`: la admite el mempool y sale a
//!   la red.
//! - `GET /quantic/blocks?from=&to=`: las transacciones de cada bloque, en hex.
//! - `GET /quantic/compact?from=&to=`: los bloques compactos de la wallet
//!   liviana (etapa 5, A5), con el `state_root` de cada uno para verificarlos.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;

use super::{decode_block, mempool, runtime, tree_root_after, GENESIS_HEIGHT};
use crate::api::AppState;
use crate::storage::RocksDbStorage;

/// Bloques por pedido: los enteros pesan, los compactos mucho menos.
pub const MAX_BLOCKS_PER_REQUEST: u64 = 50;
pub const MAX_COMPACT_BLOCKS_PER_REQUEST: u64 = 500;

type ApiError = (StatusCode, String);

fn chain_id() -> Result<String, ApiError> {
    crate::api::global_startup_status()
        .snapshot()
        .chain_id
        .filter(|id| crate::economics::is_quantic_chain_id(id))
        .ok_or((StatusCode::NOT_FOUND, "this node does not follow a Zoka Quantic chain".to_string()))
}

fn internal(error: impl std::fmt::Display) -> ApiError {
    (StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}

pub fn routes(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route("/quantic/info", get(get_info))
        .route("/quantic/tx", post(post_tx))
        .route("/quantic/blocks", get(get_blocks))
        .route("/quantic/compact", get(get_compact))
}

async fn get_info(State(app_state): State<Arc<AppState>>) -> Result<Json<serde_json::Value>, ApiError> {
    let chain_id = chain_id()?;
    let storage = app_state.state_manager.raw_storage();
    let height = storage.get_chain_height().map_err(|e| internal(format!("{e:?}")))?;
    let tip = storage.get_last_block_hash().map_err(|e| internal(format!("{e:?}")))?;
    let tree_root = if height >= GENESIS_HEIGHT {
        tree_root_after(&storage, &tip).map_err(internal)?.map(hex::encode)
    } else {
        None
    };
    let mempool_size = mempool::global().lock().map(|pool| pool.len()).unwrap_or(0);
    let network = super::network_for_chain(&chain_id).ok_or_else(|| internal("unknown chain"))?;
    let params = super::consensus_params(network);
    Ok(Json(serde_json::json!({
        "chain_id": chain_id,
        "height": height,
        "tip": tip,
        "tree_root": tree_root,
        "mempool_size": mempool_size,
        "genesis_height": GENESIS_HEIGHT,
        "coinbase_maturity": params.coinbase_maturity,
        "anchor_window": params.anchor_window,
    })))
}

#[derive(Deserialize)]
struct SubmitBody {
    tx_hex: String,
}

async fn post_tx(
    State(app_state): State<Arc<AppState>>,
    Json(body): Json<SubmitBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let chain_id = chain_id()?;
    let bytes = hex::decode(body.tx_hex.trim())
        .map_err(|_| (StatusCode::BAD_REQUEST, "tx_hex is not hex".to_string()))?;
    let storage = app_state.state_manager.raw_storage();
    // La verificación de la prueba lleva unos milisegundos: fuera del hilo de la API.
    let result = tokio::task::spawn_blocking(move || runtime::submit_from_api(&storage, &chain_id, &bytes))
        .await
        .map_err(internal)?;
    match result {
        Ok(txid) => Ok(Json(serde_json::json!({ "accepted": true, "txid": txid }))),
        Err(mempool::MempoolError::AlreadyKnown) => Ok(Json(serde_json::json!({ "accepted": false, "reason": "already known" }))),
        Err(mempool::MempoolError::Storage(error)) => Err(internal(error)),
        Err(error) => Err((StatusCode::BAD_REQUEST, error.to_string())),
    }
}

#[derive(Deserialize)]
struct RangeQuery {
    from: u64,
    to: Option<u64>,
}

fn range(storage: &RocksDbStorage, query: &RangeQuery, limit: u64) -> Result<std::ops::RangeInclusive<u64>, ApiError> {
    let height = storage.get_chain_height().map_err(|e| internal(format!("{e:?}")))?;
    let from = query.from.max(GENESIS_HEIGHT);
    let to = query.to.unwrap_or(height).min(height).min(from.saturating_add(limit - 1));
    Ok(from..=to)
}

async fn get_blocks(
    State(app_state): State<Arc<AppState>>,
    Query(query): Query<RangeQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    chain_id()?;
    let storage = app_state.state_manager.raw_storage();
    let mut blocks = Vec::new();
    for height in range(&storage, &query, MAX_BLOCKS_PER_REQUEST)? {
        let Some(block) = storage.get_canonical_block_by_height(height).map_err(|e| internal(format!("{e:?}")))? else {
            break;
        };
        blocks.push(serde_json::json!({
            "height": height,
            "hash": block.header.hash,
            "prev_hash": block.header.prev_hash,
            "state_root": block.header.state_root,
            "timestamp": block.header.timestamp,
            "transactions": block.quantic_transactions.iter().map(hex::encode).collect::<Vec<_>>(),
        }));
    }
    Ok(Json(serde_json::json!({ "blocks": blocks })))
}

async fn get_compact(
    State(app_state): State<Arc<AppState>>,
    Query(query): Query<RangeQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    chain_id()?;
    let storage = app_state.state_manager.raw_storage();
    let mut blocks = Vec::new();
    for height in range(&storage, &query, MAX_COMPACT_BLOCKS_PER_REQUEST)? {
        let Some(block) = storage.get_canonical_block_by_height(height).map_err(|e| internal(format!("{e:?}")))? else {
            break;
        };
        let decoded = decode_block(&block).map_err(internal)?;
        let compact = zoka_quantic::compact::CompactBlock::from_block(&decoded);
        blocks.push(serde_json::json!({
            "height": height,
            "hash": block.header.hash,
            "prev_hash": block.header.prev_hash,
            "state_root": block.header.state_root,
            "transactions": compact.transactions.iter().map(|tx| hex::encode(tx.to_bytes())).collect::<Vec<_>>(),
        }));
    }
    Ok(Json(serde_json::json!({ "blocks": blocks })))
}
