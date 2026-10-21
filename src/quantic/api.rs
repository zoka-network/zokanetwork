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
//! - `GET /quantic/mining`: el minero de este nodo (encendido, hilos,
//!   hashrate, bloques propios y cuándo maduran).
//!
//! Info y bloques compactos salen de [`super::light`], lo mismo que reciben
//! los teléfonos por la red P2P.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;

use super::{light, mempool, runtime, GENESIS_HEIGHT};
use crate::api::AppState;
use crate::storage::RocksDbStorage;

/// Bloques por pedido: los enteros pesan, los compactos mucho menos.
pub const MAX_BLOCKS_PER_REQUEST: u64 = 50;
pub use crate::network::quantic_light_request_response::MAX_COMPACT_BLOCKS_PER_REQUEST;

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
        .route("/quantic/mining", get(get_mining))
}

/// La dificultad del próximo bloque: el objetivo compacto, sus bits de ceros
/// y el hashrate de red que da un bloque cada 120 s con él.
pub struct NextDifficulty {
    pub compact: u32,
    pub bits: u32,
    pub network_hashrate_hps: u64,
}

pub fn next_difficulty(storage: &RocksDbStorage) -> Option<NextDifficulty> {
    let chain_id = chain_id().ok()?;
    let compact = super::compact_target_for_tip(storage, &chain_id).ok()?;
    let target = super::difficulty::from_compact(compact);
    Some(NextDifficulty {
        compact,
        bits: super::difficulty::bits_of_target(&target),
        network_hashrate_hps: super::difficulty::network_hashrate_for_target(&target),
    })
}

async fn get_info(State(app_state): State<Arc<AppState>>) -> Result<Json<serde_json::Value>, ApiError> {
    let chain_id = chain_id()?;
    let storage = app_state.state_manager.raw_storage();
    let light::ChainInfo { height, tip, tree_root } = light::chain_info(&storage).map_err(internal)?;
    let mempool_size = mempool::global().lock().map(|pool| pool.len()).unwrap_or(0);
    let network = super::network_for_chain(&chain_id).ok_or_else(|| internal("unknown chain"))?;
    let params = super::consensus_params(network);
    let difficulty = next_difficulty(&storage);
    Ok(Json(serde_json::json!({
        "chain_id": chain_id,
        "height": height,
        "tip": tip,
        "tree_root": tree_root,
        "mempool_size": mempool_size,
        "genesis_height": GENESIS_HEIGHT,
        "coinbase_maturity": params.coinbase_maturity,
        "anchor_window": params.anchor_window,
        "difficulty_compact": difficulty.as_ref().map(|d| d.compact),
        "difficulty_bits": difficulty.as_ref().map(|d| d.bits),
        "network_hashrate_hps": difficulty.as_ref().map(|d| d.network_hashrate_hps),
        "total_supply_emitted_atoms": storage.get_total_supply_emitted().unwrap_or(0),
        "max_supply_atoms": zoka_quantic::state::MAX_SUPPLY_ATOMS,
    })))
}

async fn get_mining() -> Result<Json<serde_json::Value>, ApiError> {
    chain_id()?;
    serde_json::to_value(runtime::mining_status()).map(Json).map_err(internal)
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
    let blocks = light::compact_blocks(&storage, query.from, query.to, MAX_COMPACT_BLOCKS_PER_REQUEST)
        .map_err(internal)?;
    Ok(Json(serde_json::json!({ "blocks": blocks })))
}
