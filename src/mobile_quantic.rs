//! Motor del teléfono para Zoka Quantic (ZKAPriv), sobre `crates/zoka-quantic`.
//!
//! Igual que el de Zenith (`mobile.rs`): la app le pasa su semilla y datos
//! públicos de la cadena, el motor arma todo en memoria y las claves nunca
//! salen del teléfono. Todo entra y sale en JSON, para cruzar JNI una vez.
//!
//! - [`quantic_wallet_info_json`]: la dirección de la wallet.
//! - [`quantic_sync_json`]: escanea bloques compactos (los que da
//!   `/zoka-quantic/light/1`, ver `mobile_net::quantic_light_json`) a partir
//!   del estado guardado y devuelve el estado nuevo, el saldo y la historia.
//!   Cada bloque se verifica contra su `state_root` (A5): un nodo no puede
//!   esconder ni cambiar una salida.
//! - [`quantic_build_transfer_json`]: arma y prueba un envío (unos 20 s en el
//!   teléfono de 32 bits); la app lo publica con `submit_tx`.
//! - [`quantic_preload_proving_key_json`]: arma la clave de prueba (40 s en el
//!   teléfono de pruebas). Conviene llamarla en segundo plano al abrir la app.
//!
//! **Reorganizaciones.** La wallet solo avanza. La app guarda dos estados: el
//! actual y uno seguro, [`SAFE_DEPTH`] bloques detrás de la punta, que
//! [`quantic_sync_json`] devuelve cuando un lote lo cruza. Si la sincronización
//! falla con `reorg:`, la app vuelve a escanear desde el seguro.
//!
//! Los estados contienen la clave de vista, que muestra toda la historia de la
//! wallet: la app los guarda cifrados, como la semilla.

use serde::{Deserialize, Serialize};
use std::time::Instant;
use zeroize::{Zeroize, Zeroizing};

use crate::network::quantic_light_request_response::MAX_COMPACT_BLOCKS_PER_REQUEST;
use crate::quantic::protocol::builder::Payment;
use crate::quantic::protocol::note_encryption::MEMO_SIZE;
use crate::quantic::protocol::wallet::QuanticWallet;
use crate::quantic::protocol::{Network, QuanticAddress, QuanticSpendingKey};
use crate::quantic::wallet_sync::{scan_compact_blocks, CompactBlockJson};
use crate::quantic::{consensus_params, runtime};

/// Cuántos bloques detrás de la punta queda el estado seguro (el mismo que usa
/// la wallet de línea de comandos).
pub use crate::quantic::wallet_sync::SAFE_DEPTH;

fn network_named(name: &str) -> Result<Network, String> {
    match name.trim() {
        "quantic-testnet" => Ok(Network::Testnet),
        "quantic-regtest" => Ok(Network::Regtest),
        // La mainnet existe recién cuando su génesis está fijo en el binario.
        "quantic-mainnet" if crate::quantic::fixed_genesis(Network::Mainnet).is_some() => Ok(Network::Mainnet),
        "quantic-mainnet" => Err("Zoka Quantic mainnet has not launched".to_string()),
        other => Err(format!("unknown Quantic network '{other}'")),
    }
}

/// La clave de gasto de la semilla de la app (la misma raíz que usa para
/// Zenith: con la misma frase, otra wallet, sin relación con aquella).
fn spending_key(seed_hex: &mut String, network: Network) -> Result<QuanticSpendingKey, String> {
    let seed = hex::decode(seed_hex.trim().trim_start_matches("0x")).map(Zeroizing::new);
    seed_hex.zeroize();
    let seed = seed.map_err(|_| "seed_hex must be hexadecimal".to_string())?;
    QuanticSpendingKey::from_root(&seed, network, 0).map_err(|e| e.to_string())
}

/// La wallet del estado guardado, o una nueva (antes del génesis) sin él.
fn open_wallet(sk: &QuanticSpendingKey, state_hex: Option<&str>) -> Result<QuanticWallet, String> {
    let params = consensus_params(sk.network());
    let fvk = sk.full_viewing_key();
    let Some(state_hex) = state_hex.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(QuanticWallet::new(fvk, &params));
    };
    let bytes = Zeroizing::new(hex::decode(state_hex).map_err(|_| "state_hex must be hexadecimal".to_string())?);
    let wallet = QuanticWallet::from_state_bytes(&bytes, &params).map_err(|e| e.to_string())?;
    if *wallet.fvk().to_bytes() != *fvk.to_bytes() {
        return Err("the saved state belongs to another wallet".to_string());
    }
    Ok(wallet)
}

fn state_hex(wallet: &QuanticWallet) -> String {
    hex::encode(&*wallet.to_state_bytes())
}

/// Pila del hilo donde corre Halo 2. Armar la clave y la prueba recursa hondo:
/// la pila de un hilo común (1 MB: la principal en Windows, un hilo de Java en
/// Android) se desborda. Es reserva de direcciones, no memoria usada.
const PROVER_STACK_BYTES: usize = 64 * 1024 * 1024;

/// Corre `work` en un hilo propio con [`PROVER_STACK_BYTES`] de pila y espera
/// el resultado, sea cual sea el hilo que llama.
fn on_prover_stack<T: Send + 'static>(work: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    std::thread::Builder::new()
        .name("zoka-quantic-prover".to_string())
        .stack_size(PROVER_STACK_BYTES)
        .spawn(work)
        .map_err(|e| format!("could not start the prover thread: {e}"))?
        .join()
        .map_err(|_| "the prover panicked".to_string())?
}

fn encode<T: Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string(value).map_err(|e| format!("failed to encode the response: {e}"))
}

#[derive(Debug, Deserialize)]
pub struct QuanticWalletRequest {
    pub seed_hex: String,
    pub network: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct QuanticWalletInfo {
    pub network: String,
    pub chain_id: String,
    pub address: String,
}

pub fn quantic_wallet_info_json(request_json: &str) -> Result<String, String> {
    let mut request: QuanticWalletRequest =
        serde_json::from_str(request_json).map_err(|e| format!("invalid wallet request: {e}"))?;
    let network = network_named(&request.network)?;
    let sk = spending_key(&mut request.seed_hex, network)?;
    encode(&QuanticWalletInfo {
        network: request.network.trim().to_string(),
        chain_id: network.chain_id().to_string(),
        address: sk.full_viewing_key().address().encode(),
    })
}

#[derive(Debug, Deserialize)]
pub struct QuanticAddressCheck {
    pub address: String,
    pub network: String,
}

/// Si `address` sirve para pagar en `network`, para avisar antes de armar
/// nada. Devuelve `{"valid":true}` o `{"valid":false,"reason":...}`.
pub fn quantic_check_address_json(request_json: &str) -> Result<String, String> {
    let request: QuanticAddressCheck =
        serde_json::from_str(request_json).map_err(|e| format!("invalid address check: {e}"))?;
    let network = network_named(&request.network)?;
    let result = match QuanticAddress::decode_for(request.address.trim(), network) {
        Ok(_) => serde_json::json!({ "valid": true }),
        Err(error) => serde_json::json!({ "valid": false, "reason": error.to_string() }),
    };
    encode(&result)
}

#[derive(Debug, Deserialize)]
pub struct QuanticSyncRequest {
    pub seed_hex: String,
    pub network: String,
    /// El estado que devolvió la llamada anterior; sin él, desde el génesis.
    #[serde(default)]
    pub state_hex: Option<String>,
    /// El hash del último bloque escaneado, que devolvió la llamada anterior.
    #[serde(default)]
    pub last_hash: Option<String>,
    /// La altura de la punta según la red, para el estado seguro.
    #[serde(default)]
    pub tip_height: Option<u64>,
    /// Los bloques que siguen al último escaneado, en orden.
    pub blocks: Vec<CompactBlockJson>,
}

/// Un estado de la wallet con la altura y el hash del último bloque que vio.
#[derive(Debug, Clone, Serialize)]
pub struct QuanticSavedPoint {
    pub state_hex: String,
    pub next_height: u64,
    pub last_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct QuanticHistoryItem {
    pub txid: String,
    pub height: u64,
    pub coinbase: bool,
    pub received_atoms: u64,
    pub spent_atoms: u64,
    /// Lo que ganó (positivo) o gastó, comisión incluida (negativo), la wallet.
    pub net_atoms: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct QuanticSyncResponse {
    pub current: QuanticSavedPoint,
    /// El último estado de este lote que quedó [`SAFE_DEPTH`] bloques detrás
    /// de la punta; `None` si el lote no pasó por ahí.
    pub safe: Option<QuanticSavedPoint>,
    pub balance_atoms: u64,
    pub immature_atoms: u64,
    pub history: Vec<QuanticHistoryItem>,
    pub blocks_scanned: usize,
    pub scan_ms: u64,
}

pub fn quantic_sync_json(request_json: &str) -> Result<String, String> {
    let mut request: QuanticSyncRequest =
        serde_json::from_str(request_json).map_err(|e| format!("invalid sync request: {e}"))?;
    if request.blocks.len() > MAX_COMPACT_BLOCKS_PER_REQUEST as usize {
        return Err(format!(
            "at most {MAX_COMPACT_BLOCKS_PER_REQUEST} blocks per call, got {}",
            request.blocks.len()
        ));
    }
    let network = network_named(&request.network)?;
    let sk = spending_key(&mut request.seed_hex, network)?;
    let mut wallet = open_wallet(&sk, request.state_hex.as_deref())?;
    let mut last_hash = request.last_hash.clone().filter(|hash| !hash.trim().is_empty());
    let safe_height = request.tip_height.map(|tip| tip.saturating_sub(SAFE_DEPTH));
    let mut safe = None;
    let started = Instant::now();
    scan_compact_blocks(&mut wallet, &mut last_hash, &request.blocks, |wallet, block| {
        if safe_height.is_some_and(|safe_height| block.height <= safe_height) {
            safe = Some(QuanticSavedPoint {
                state_hex: state_hex(wallet),
                next_height: u64::from(wallet.next_height()),
                last_hash: Some(block.hash.clone()),
            });
        }
    })?;
    let scan_ms = started.elapsed().as_millis() as u64;
    let history = wallet
        .history()
        .into_iter()
        .map(|entry| QuanticHistoryItem {
            txid: entry.txid.to_hex(),
            height: u64::from(entry.height),
            coinbase: entry.coinbase,
            received_atoms: entry.received,
            spent_atoms: entry.spent,
            net_atoms: i64::try_from(entry.net()).unwrap_or(if entry.net() < 0 { i64::MIN } else { i64::MAX }),
        })
        .collect();
    encode(&QuanticSyncResponse {
        current: QuanticSavedPoint {
            state_hex: state_hex(&wallet),
            next_height: u64::from(wallet.next_height()),
            last_hash,
        },
        safe,
        balance_atoms: wallet.balance(),
        immature_atoms: wallet.immature_balance(),
        history,
        blocks_scanned: request.blocks.len(),
        scan_ms,
    })
}

#[derive(Debug, Deserialize)]
pub struct QuanticTransferRequest {
    pub seed_hex: String,
    pub network: String,
    /// El estado actual de la wallet, al día con la punta.
    pub state_hex: String,
    pub to: String,
    pub amount_atoms: u64,
    /// Texto para el destinatario (UTF-8, hasta 512 bytes), cifrado con la nota.
    #[serde(default)]
    pub memo_text: Option<String>,
    /// Altura en la que vence si no entró en un bloque; 0, nunca.
    #[serde(default)]
    pub expiry_height: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct QuanticTransferResponse {
    pub tx_hex: String,
    pub txid: String,
    pub fee_atoms: u64,
    pub actions: usize,
    pub build_ms: u64,
}

/// Arma y prueba un envío. Las notas que usa siguen en el estado como no
/// gastadas hasta que un bloque lo incluya: si la app arma otro antes, puede
/// elegir las mismas y el nodo rechaza el segundo.
pub fn quantic_build_transfer_json(request_json: &str) -> Result<String, String> {
    let request_json = request_json.to_string();
    on_prover_stack(move || build_transfer(&request_json))
}

fn build_transfer(request_json: &str) -> Result<String, String> {
    let mut request: QuanticTransferRequest =
        serde_json::from_str(request_json).map_err(|e| format!("invalid transfer request: {e}"))?;
    let network = network_named(&request.network)?;
    let sk = spending_key(&mut request.seed_hex, network)?;
    if request.amount_atoms == 0 {
        return Err("the amount must be more than zero".to_string());
    }
    let to = QuanticAddress::decode_for(request.to.trim(), network).map_err(|e| e.to_string())?;
    let mut payment = Payment::new(to, request.amount_atoms);
    if let Some(text) = request.memo_text.as_deref().filter(|text| !text.is_empty()) {
        let bytes = text.as_bytes();
        if bytes.len() > MEMO_SIZE {
            return Err(format!("the memo has {} bytes; at most {MEMO_SIZE}", bytes.len()));
        }
        payment.memo[..bytes.len()].copy_from_slice(bytes);
    }
    let wallet = open_wallet(&sk, Some(&request.state_hex))?;
    let started = Instant::now();
    let tx = wallet
        .create_transfer(
            &sk,
            vec![payment],
            request.expiry_height,
            runtime::proving_key(),
            &mut crate::quantic::protocol::os_rng(),
        )
        .map_err(|e| e.to_string())?;
    encode(&QuanticTransferResponse {
        tx_hex: hex::encode(tx.to_bytes()),
        txid: tx.txid().to_hex(),
        fee_atoms: tx.fee().unwrap_or(0),
        actions: tx.actions().len(),
        build_ms: started.elapsed().as_millis() as u64,
    })
}

/// Arma la clave de prueba de Halo 2 si todavía no está. La primera vez tarda
/// (40 s en el teléfono de pruebas); después, nada.
pub fn quantic_preload_proving_key_json() -> Result<String, String> {
    on_prover_stack(|| {
        let started = Instant::now();
        let _ = runtime::proving_key();
        encode(&serde_json::json!({ "ms": started.elapsed().as_millis() as u64 }))
    })
}
