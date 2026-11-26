//! Servidor JSON-RPC 2.0 de la billetera, para exchanges
//! (`zoka quantic-pq-wallet-rpc`).
//!
//! Abre una billetera del motor liviano, la mantiene al día con el nodo del
//! exchange (`--api`) y atiende JSON-RPC 2.0 por HTTP en `--rpc-bind`
//! (127.0.0.1:18090 por defecto; otra interfaz solo con `--allow-remote`).
//! Cada pedido lleva `Authorization: Bearer <token>`; el token sale de la
//! variable que nombra `--rpc-token-env` y tiene 32 caracteres o más.
//!
//! Depósitos: cada cliente recibe una dirección integrada
//! (`make_integrated_address`), y `get_deposits` da cada nota recibida con su
//! código, sus confirmaciones y si un checkpoint ya la fijó. Retiros:
//! `transfer` con un `request_id` del exchange; repetirlo devuelve el mismo
//! retiro en vez de pagar dos veces. Los montos van en átomos (1 ZKA =
//! 1.000.000), como enteros; los códigos de cliente, como texto, porque pasan
//! de 2^53 y JavaScript los redondearía.
//!
//! La billetera queda bloqueada mientras el servidor corre: la CLI no la
//! puede usar a la vez.

use std::{
    collections::BTreeMap,
    fs::{self, File},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use zeroize::Zeroizing;

use super::{
    api_json, api_ok, blocking, error, flag, option, parse_address, required, required_password,
    resolve, stark_cli, Address, IntegratedAddress, Light, Profile, Store, DEFAULT_API,
};
use crate::quantic::stark::protocol::integrated::tag_memo;

pub const USAGE: &str = "usage: zoka quantic-pq-wallet-rpc --network testnet|regtest --dir DIR [--name WALLET] --password-env VAR --rpc-token-env VAR [--api http://127.0.0.1:9140] [--rpc-bind 127.0.0.1:18090] [--allow-remote]";
const DEFAULT_BIND: &str = "127.0.0.1:18090";
const MIN_TOKEN_CHARS: usize = 32;
const SYNC_EVERY: Duration = Duration::from_secs(10);
const MAX_BODY: usize = 64 * 1024;
const MAX_REQUEST_ID: usize = 64;
const MAX_BATCH: usize = 50;

// Códigos de error propios (JSON-RPC deja -32000 a -32099 para el servidor).
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const INTERNAL: i64 = -32603;
const NOT_SYNCED: i64 = -32001;
const NOT_ENOUGH: i64 = -32002;
const BAD_ADDRESS: i64 = -32003;
const REQUEST_REUSED: i64 = -32004;
const NODE_REFUSED: i64 = -32005;

type RpcResult = Result<Value, (i64, String)>;

/// Un retiro pedido por el exchange, guardado antes de mandarlo.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Withdrawal {
    destination: String,
    amount_atoms: u64,
    memo: String,
    txid: String,
    fee_atoms: u64,
    tag: Option<String>,
    /// La altura de la billetera cuando se armó.
    built_height: u32,
    /// `submitted` si el nodo lo tomó, `rejected` si lo rechazó, `unknown`
    /// si no se supo (sigue reservado hasta minarse o vencer).
    state: String,
    reason: Option<String>,
}

/// La billetera abierta, lo que el servidor sabe del nodo y los retiros.
pub(crate) struct Wallet {
    light: Light,
    profile: Profile,
    api: String,
    lag: Option<String>,
    withdrawals_path: PathBuf,
    withdrawals: BTreeMap<String, Withdrawal>,
    /// Mientras exista, la CLI no puede abrir la misma billetera.
    _lock: File,
}

/// `<name>.json.rpc-withdrawals`, al lado de la billetera.
fn withdrawals_path(wallet: &Path) -> PathBuf {
    let mut path = wallet.as_os_str().to_os_string();
    path.push(".rpc-withdrawals");
    PathBuf::from(path)
}

fn hex_digest(words: &[u32]) -> String {
    hex::encode(words.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<u8>>())
}

fn invalid(message: impl Into<String>) -> (i64, String) {
    (INVALID_PARAMS, message.into())
}

fn internal(message: impl std::fmt::Display) -> (i64, String) {
    (INTERNAL, message.to_string())
}

/// Un número entero que puede venir como número o como texto.
fn u64_param(params: &Value, name: &str) -> Result<Option<u64>, (i64, String)> {
    match &params[name] {
        Value::Null => Ok(None),
        Value::Number(n) => n
            .as_u64()
            .map(Some)
            .ok_or_else(|| invalid(format!("{name} must be a whole number"))),
        Value::String(s) => s
            .parse::<u64>()
            .map(Some)
            .map_err(|_| invalid(format!("{name} must be a whole number"))),
        _ => Err(invalid(format!("{name} must be a whole number"))),
    }
}

fn str_param<'a>(params: &'a Value, name: &str) -> Result<Option<&'a str>, (i64, String)> {
    match &params[name] {
        Value::Null => Ok(None),
        Value::String(s) => Ok(Some(s.as_str())),
        _ => Err(invalid(format!("{name} must be a string"))),
    }
}

fn required_str<'a>(params: &'a Value, name: &str) -> Result<&'a str, (i64, String)> {
    str_param(params, name)?.ok_or_else(|| invalid(format!("{name} is required")))
}

impl Wallet {
    /// Abre la billetera `name` de `dir`, la bloquea y lee hasta el nodo.
    fn open(
        store: &Store,
        args: &[String],
        password: &str,
        api: String,
    ) -> Result<Self, String> {
        let (_, path) = store.select(args)?;
        let wallet = fs::canonicalize(&path).map_err(error)?;
        let lock = stark_cli::wallet_lock(&wallet)?;
        let light = Light::open(&wallet, password, store.profile)?;
        let withdrawals_path = withdrawals_path(&wallet);
        let withdrawals = match fs::read(&withdrawals_path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| format!("{} is damaged: {e}", withdrawals_path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(format!("cannot read {}: {e}", withdrawals_path.display())),
        };
        let mut opened = Self {
            light,
            profile: store.profile,
            api,
            lag: None,
            withdrawals_path,
            withdrawals,
            _lock: lock,
        };
        opened.sync();
        Ok(opened)
    }

    /// Lee los bloques nuevos, confirma el checkpoint del nodo y guarda.
    pub(crate) fn sync(&mut self) {
        let before = self.light.wallet.height();
        self.lag = self.light.sync(&self.api);
        if self.lag.is_none() {
            // El nodo es el del exchange, que ya verificó las firmas del
            // checkpoint: la billetera solo comprueba que leyó ese bloque.
            if let Ok((200, status)) = api_json(&self.api, "GET", "/status", None, Duration::from_secs(20)) {
                let height = status["checkpoint"]["height"].as_u64().unwrap_or(0);
                let hash = status["checkpoint"]["block_hash"].as_str().unwrap_or_default();
                if let (Ok(height), false) = (u32::try_from(height), hash.is_empty()) {
                    if let Err(reason) = self.light.wallet.confirm_checkpoint(height, hash) {
                        self.lag = Some(reason.to_string());
                    }
                }
            }
        }
        if self.light.wallet.height() != before || self.light.retire_pending {
            let _ = self.light.save();
        }
    }

    fn save_withdrawals(&self) -> Result<(), (i64, String)> {
        let bytes = serde_json::to_vec_pretty(&self.withdrawals).map_err(internal)?;
        let mut temporary = self.withdrawals_path.as_os_str().to_os_string();
        temporary.push(".new");
        let temporary = PathBuf::from(temporary);
        fs::write(&temporary, bytes).map_err(internal)?;
        fs::rename(&temporary, &self.withdrawals_path).map_err(internal)
    }

    fn ready(&self) -> Result<(), (i64, String)> {
        if let Some(lag) = &self.lag {
            return Err((NOT_SYNCED, format!("the wallet is not in sync with its node: {lag}")));
        }
        if !self.light.wallet.in_sync() {
            return Err((NOT_SYNCED, "the wallet is still reading the chain".into()));
        }
        Ok(())
    }

    fn short(&self) -> Result<String, (i64, String)> {
        self.light.accounts[0].address().short().map_err(internal)
    }

    pub(crate) fn call(&mut self, method: &str, params: &Value) -> RpcResult {
        match method {
            "get_info" => Ok(self.info()),
            "get_balance" => Ok(self.balance()),
            "get_address" => {
                let full = self.light.accounts[0].address();
                Ok(json!({"address":self.short()?,"full_address":full.encode().map_err(internal)?}))
            }
            "make_integrated_address" => {
                let tag = match u64_param(params, "tag")? {
                    Some(tag) => tag,
                    None => rand::rngs::OsRng.next_u64(),
                };
                let short = self.light.accounts[0].address().short_address().map_err(internal)?;
                Ok(json!({"integrated_address":IntegratedAddress::new(short, tag).encode(),
                    "tag":tag.to_string(),"address":short.encode()}))
            }
            "split_integrated_address" => {
                let text = required_str(params, "integrated_address")?;
                let integrated = IntegratedAddress::decode(text.trim(), self.profile.network)
                    .map_err(|_| (BAD_ADDRESS, "not an integrated address of this network".to_string()))?;
                Ok(json!({"address":integrated.short.encode(),"tag":integrated.tag.to_string()}))
            }
            "validate_address" => {
                let text = required_str(params, "address")?;
                Ok(parse_address(text, self.profile).unwrap_or_else(|| json!({"valid":false})))
            }
            "get_deposits" => self.deposits(params),
            "get_transfer_by_txid" => self.by_txid(params),
            "estimate_fee" => {
                let amount = u64_param(params, "amount_atoms")?
                    .filter(|a| *a > 0)
                    .ok_or_else(|| invalid("amount_atoms is required and positive"))?;
                Ok(match self.light.wallet.quote_total(amount) {
                    Ok((fee, inputs, _)) => json!({"enough":true,"fee_atoms":fee,"inputs":inputs}),
                    Err(reason) => json!({"enough":false,"reason":reason.to_string()}),
                })
            }
            "transfer" => self.transfer(params),
            "get_withdrawal" => {
                let id = required_str(params, "request_id")?;
                let found = self
                    .withdrawals
                    .get(id)
                    .ok_or_else(|| invalid(format!("no withdrawal with request_id {id}")))?;
                Ok(self.withdrawal_status(id, found))
            }
            "get_withdrawals" => Ok(json!({"withdrawals":self
                .withdrawals
                .iter()
                .map(|(id, w)| self.withdrawal_status(id, w))
                .collect::<Vec<_>>()})),
            _ => Err((METHOD_NOT_FOUND, format!("no method {method}"))),
        }
    }

    fn info(&self) -> Value {
        let wallet = &self.light.wallet;
        json!({
            "network":self.profile.chain_id,"wallet_height":wallet.height(),"node_tip":wallet.tip(),
            "synced":self.lag.is_none() && wallet.in_sync(),"lag":self.lag,
            "checkpoint_height":wallet.verified(),"version":env!("CARGO_PKG_VERSION"),
        })
    }

    fn balance(&self) -> Value {
        let b = self.light.wallet.balance();
        json!({
            "height":b.height,"spendable_atoms":b.spendable_atoms,"immature_atoms":b.immature_atoms,
            "reserved_atoms":b.reserved_atoms,
            "total_atoms":b.spendable_atoms.saturating_add(b.immature_atoms).saturating_add(b.reserved_atoms),
            "max_sendable_atoms":b.max_sendable_atoms,"notes":b.notes,
            "pending_withdrawals":self.light.wallet.pendings().len(),
        })
    }

    fn deposit_json(&self, owned: &crate::quantic::stark::protocol::light::OwnedNote) -> Value {
        let wallet = &self.light.wallet;
        json!({
            "deposit_id":hex_digest(&owned.commitment.0),"txid":hex::encode(owned.txid),
            "height":owned.height,"amount_atoms":owned.note.value,
            "tag":owned.tag.map(|t| t.to_string()),
            "confirmations":wallet.height().saturating_sub(owned.height) + 1,
            "finalized":owned.height <= wallet.verified(),
            "coinbase":owned.coinbase,"spent":wallet.is_spent(owned),
        })
    }

    /// Cada nota recibida que no es vuelto de un retiro propio. Sin
    /// `include_coinbase`, tampoco las recompensas de minería.
    fn deposits(&self, params: &Value) -> RpcResult {
        let min = u64_param(params, "min_height")?.unwrap_or(0);
        let max = u64_param(params, "max_height")?.unwrap_or(u64::MAX);
        let tag = u64_param(params, "tag")?;
        let coinbase = params["include_coinbase"].as_bool().unwrap_or(false);
        let deposits: Vec<Value> = self
            .light
            .wallet
            .notes()
            .iter()
            .filter(|n| !n.change && (coinbase || !n.coinbase))
            .filter(|n| (min..=max).contains(&u64::from(n.height)))
            .filter(|n| tag.is_none() || n.tag == tag)
            .map(|n| self.deposit_json(n))
            .collect();
        Ok(json!({"wallet_height":self.light.wallet.height(),"deposits":deposits}))
    }

    fn by_txid(&self, params: &Value) -> RpcResult {
        let txid = required_str(params, "txid")?.trim().to_ascii_lowercase();
        let deposits: Vec<Value> = self
            .light
            .wallet
            .notes()
            .iter()
            .filter(|n| !n.change && hex::encode(n.txid) == txid)
            .map(|n| self.deposit_json(n))
            .collect();
        let withdrawal = self
            .withdrawals
            .iter()
            .find(|(_, w)| w.txid == txid)
            .map(|(id, w)| self.withdrawal_status(id, w));
        Ok(json!({"txid":txid,"deposits":deposits,"withdrawal":withdrawal}))
    }

    /// Lo guardado del retiro y dónde está ahora: `pending`, `confirmed`
    /// (con su altura), `rejected` o `expired`.
    fn withdrawal_status(&self, id: &str, w: &Withdrawal) -> Value {
        let wallet = &self.light.wallet;
        let txid = hex::decode(&w.txid).ok().and_then(|b| <[u8; 32]>::try_from(b).ok());
        let mined = txid.and_then(|t| wallet.history().iter().find(|m| m.txid == t && m.spent > 0));
        let pending = txid.is_some_and(|t| wallet.pendings().iter().any(|p| p.txid == t));
        let (status, height) = match (mined, pending, w.state.as_str()) {
            (Some(m), _, _) => ("confirmed", Some(m.height)),
            (None, true, _) => ("pending", None),
            (None, false, "rejected") => ("rejected", None),
            (None, false, _) => ("expired", None),
        };
        json!({
            "request_id":id,"txid":w.txid,"destination":w.destination,"amount_atoms":w.amount_atoms,
            "fee_atoms":w.fee_atoms,"tag":w.tag,"memo":w.memo,"status":status,"height":height,
            "confirmations":height.map(|h| wallet.height().saturating_sub(h) + 1),
            "finalized":height.is_some_and(|h| h <= wallet.verified()),"reason":w.reason,
        })
    }

    fn transfer(&mut self, params: &Value) -> RpcResult {
        let id = required_str(params, "request_id")?;
        if id.is_empty()
            || id.len() > MAX_REQUEST_ID
            || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
        {
            return Err(invalid(format!(
                "request_id: 1 to {MAX_REQUEST_ID} letters, digits or -_.:"
            )));
        }
        let destination = required_str(params, "destination")?.trim().to_string();
        let amount = u64_param(params, "amount_atoms")?
            .filter(|a| *a > 0)
            .ok_or_else(|| invalid("amount_atoms is required and positive"))?;
        let memo = str_param(params, "memo")?.unwrap_or_default().to_string();
        // The same request again: the same withdrawal, never a second one.
        if let Some(done) = self.withdrawals.get(id) {
            if done.destination == destination && done.amount_atoms == amount && done.memo == memo {
                return Ok(self.withdrawal_status(id, done));
            }
            return Err((
                REQUEST_REUSED,
                format!("request_id {id} was used for another withdrawal"),
            ));
        }
        self.ready()?;
        let (to, tag): (Address, Option<u64>) = resolve(&destination, self.profile, &self.api)
            .map_err(|e| (BAD_ADDRESS, e))?;
        let text = match tag {
            Some(tag) => tag_memo(tag, &memo).map_err(|e| invalid(e.to_string()))?,
            None => memo.clone(),
        };
        let plan = self
            .light
            .wallet
            .plan_for(&self.light.accounts, to, amount, text.as_bytes())
            .map_err(|e| (NOT_ENOUGH, e.to_string()))?;
        let fee = plan.fee;
        let tx = plan.prove_for(&self.light.accounts).map_err(internal)?;
        self.light.wallet.reserve(&tx).map_err(internal)?;
        self.light.save().map_err(internal)?;
        // Written before it leaves: a cut leaves the request on record, and
        // its notes reserved until it is mined or expires.
        let mut record = Withdrawal {
            destination,
            amount_atoms: amount,
            memo,
            txid: hex::encode(tx.txid()),
            fee_atoms: fee,
            tag: tag.map(|t| t.to_string()),
            built_height: self.light.wallet.height(),
            state: "unknown".into(),
            reason: None,
        };
        self.withdrawals.insert(id.to_string(), record.clone());
        self.save_withdrawals()?;
        let bytes = tx.encode().map_err(internal)?;
        let sent = api_json(
            &self.api,
            "POST",
            "/transactions",
            Some(("application/octet-stream", &bytes)),
            Duration::from_secs(120),
        );
        match sent {
            Ok((status, _)) if (200..300).contains(&status) => record.state = "submitted".into(),
            Ok((status, reply)) => {
                // The node refused it: its notes are free for another one.
                self.light.wallet.release_txid(&tx.txid());
                self.light.save().map_err(internal)?;
                record.state = "rejected".into();
                record.reason = Some(match api_ok(status, reply) {
                    Err(reason) => reason,
                    Ok(_) => format!("HTTP {status}"),
                });
            }
            Err(reason) => record.reason = Some(reason),
        }
        self.withdrawals.insert(id.to_string(), record.clone());
        self.save_withdrawals()?;
        if record.state == "rejected" {
            return Err((
                NODE_REFUSED,
                format!(
                    "the node refused the withdrawal: {}",
                    record.reason.clone().unwrap_or_default()
                ),
            ));
        }
        Ok(self.withdrawal_status(id, &record))
    }
}

/// Lo que comparten los pedidos y la lectura periódica.
pub(crate) struct Shared {
    pub(crate) wallet: Mutex<Wallet>,
    token: Zeroizing<String>,
}

impl Shared {
    pub(crate) fn new(wallet: Wallet, token: Zeroizing<String>) -> Result<Self, String> {
        if token.chars().count() < MIN_TOKEN_CHARS {
            return Err(format!("the RPC token needs {MIN_TOKEN_CHARS} characters or more"));
        }
        Ok(Self {
            wallet: Mutex::new(wallet),
            token,
        })
    }

    fn authorized(&self, headers: &HeaderMap) -> bool {
        let Some(given) = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
        else {
            return false;
        };
        let (a, b) = (given.as_bytes(), self.token.as_bytes());
        // Same time whatever byte differs.
        a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
    }
}

fn reply(id: &Value, result: RpcResult) -> Value {
    match result {
        Ok(value) => json!({"jsonrpc":"2.0","id":id,"result":value}),
        Err((code, message)) => {
            json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
        }
    }
}

/// Un pedido: `None` si era una notificación (sin `id`), que no se contesta.
fn one(shared: &Shared, request: &Value) -> Option<Value> {
    let id = request.get("id").cloned();
    let Some(method) = request["method"].as_str().filter(|_| request["jsonrpc"] == "2.0") else {
        return Some(reply(
            &id.unwrap_or(Value::Null),
            Err((INVALID_REQUEST, "a JSON-RPC 2.0 request has jsonrpc \"2.0\" and a method".into())),
        ));
    };
    let params = match &request["params"] {
        Value::Null => json!({}),
        Value::Object(_) => request["params"].clone(),
        _ => {
            return id.map(|id| reply(&id, Err(invalid("params must be an object"))));
        }
    };
    let result = match shared.wallet.lock() {
        Ok(mut wallet) => wallet.call(method, &params),
        Err(_) => Err(internal("the wallet lock was poisoned: restart the server")),
    };
    id.map(|id| reply(&id, result))
}

async fn handle(State(shared): State<Arc<Shared>>, headers: HeaderMap, body: String) -> Response {
    if !shared.authorized(&headers) {
        return (StatusCode::UNAUTHORIZED, "missing or wrong Bearer token").into_response();
    }
    let request: Value = match serde_json::from_str(&body) {
        Ok(value) => value,
        Err(e) => {
            return Json(reply(&Value::Null, Err((PARSE_ERROR, e.to_string())))).into_response()
        }
    };
    let answered = blocking(move || {
        Ok(match request {
            Value::Array(batch) if batch.is_empty() || batch.len() > MAX_BATCH => Some(reply(
                &Value::Null,
                Err((INVALID_REQUEST, format!("a batch has 1 to {MAX_BATCH} requests"))),
            )),
            Value::Array(batch) => {
                let replies: Vec<Value> = batch.iter().filter_map(|r| one(&shared, r)).collect();
                (!replies.is_empty()).then(|| Value::Array(replies))
            }
            single => one(&shared, &single),
        })
    })
    .await;
    match answered {
        Ok(Some(value)) => Json(value).into_response(),
        Ok(None) => StatusCode::NO_CONTENT.into_response(),
        Err(reason) => Json(reply(&Value::Null, Err(internal(reason)))).into_response(),
    }
}

pub(crate) fn router(shared: Arc<Shared>) -> Router {
    Router::new()
        .route("/json_rpc", post(handle))
        .route("/", post(handle))
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .with_state(shared)
}

/// Lee los bloques nuevos cada [`SYNC_EVERY`] mientras el servidor corre.
pub(crate) fn spawn_sync(shared: Arc<Shared>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut every = tokio::time::interval(SYNC_EVERY);
        loop {
            every.tick().await;
            let shared = shared.clone();
            let _ = blocking(move || {
                if let Ok(mut wallet) = shared.wallet.lock() {
                    wallet.sync();
                }
                Ok(())
            })
            .await;
        }
    })
}

pub async fn run(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| matches!(a.as_str(), "help" | "--help" | "-h")) {
        println!("{USAGE}");
        return Ok(());
    }
    let profile = Profile::named(required(args, "--network")?)?;
    let store = Store {
        dir: PathBuf::from(required(args, "--dir")?),
        profile,
    };
    let bind: SocketAddr = option(args, "--rpc-bind")
        .unwrap_or(DEFAULT_BIND)
        .parse()
        .map_err(|_| "--rpc-bind must be an IP and a port, like 127.0.0.1:18090")?;
    if !bind.ip().is_loopback() && !flag(args, "--allow-remote") {
        return Err(format!(
            "{bind} is reachable from other machines: add --allow-remote if that is intended, behind a firewall"
        ));
    }
    let variable = required(args, "--rpc-token-env")?;
    let token = Zeroizing::new(
        std::env::var(variable).map_err(|_| format!("{variable} is not set"))?,
    );
    let password = required_password(args)?;
    let api = option(args, "--api").unwrap_or(DEFAULT_API).to_string();
    let wallet = {
        let args = args.to_vec();
        blocking(move || Wallet::open(&store, &args, &password, api)).await?
    };
    let shared = Arc::new(Shared::new(wallet, token)?);
    spawn_sync(shared.clone());
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|e| format!("cannot listen on {bind}: {e}"))?;
    println!("Zoka wallet JSON-RPC 2.0 listening on http://{bind}/json_rpc");
    axum::serve(listener, router(shared))
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quantic::{
        stark::protocol::ledger::{conventional_fee, ATOMS},
        stark_api, stark_node,
        stark_wallet::act,
    };

    const TOKEN: &str = "an exchange token of 32 characters or more";

    fn args(dir: &Path, more: &[&str]) -> Vec<String> {
        let mut all: Vec<String> = [
            "--network",
            "regtest",
            "--dir",
            dir.to_str().unwrap(),
            "--password",
            "a test password",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        all.extend(more.iter().map(|s| s.to_string()));
        all
    }

    fn call(shared: &Shared, method: &str, params: Value) -> Value {
        one(
            shared,
            &json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}),
        )
        .unwrap()
    }

    fn sync(shared: &Shared) {
        shared.wallet.lock().unwrap().sync();
    }

    /// One HTTP request by hand: the status line and the body.
    async fn post(address: SocketAddr, token: Option<&str>, body: &str) -> (String, String) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        let auth = token
            .map(|t| format!("Authorization: Bearer {t}\r\n"))
            .unwrap_or_default();
        let request = format!(
            "POST /json_rpc HTTP/1.1\r\nHost: localhost\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).await.unwrap();
        let status = answer.lines().next().unwrap_or_default().to_string();
        let body = answer.split("\r\n\r\n").nth(1).unwrap_or_default().to_string();
        (status, body)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_exchange_takes_a_tagged_deposit_and_pays_a_withdrawal_once() {
        let dir = tempfile::tempdir().unwrap();
        let customer = act("new", &args(dir.path(), &["--name", "customer"])).await.unwrap();
        let customer_address =
            Address::decode(customer["address"].as_str().unwrap(), 3).unwrap();
        // Withdrawals go to its full address: it never published the short one.
        let customer_full = customer["address"].as_str().unwrap().to_string();
        act("new", &args(dir.path(), &["--name", "exchange"])).await.unwrap();

        // The exchange's node: the premine and a block go to the customer.
        let storage = Arc::new(
            crate::storage::RocksDbStorage::new(dir.path().join("node")).unwrap(),
        );
        storage
            .put_raw(
                stark_api::DB_MARKER,
                Profile::REGTEST.chain_id.as_bytes().to_vec(),
            )
            .unwrap();
        let node = stark_api::NodeState::new(storage).unwrap();
        stark_node::mine_once(&node, customer_address.clone()).await.unwrap();
        stark_node::mine_once(&node, customer_address.clone()).await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = format!("http://{}", listener.local_addr().unwrap());
        let served = node.clone();
        let api_server =
            tokio::spawn(async move { axum::serve(listener, stark_api::router(served)).await });

        // The exchange publishes its address once, so payers resolve it.
        act("publish-address", &args(dir.path(), &["--name", "exchange", "--api", &api]))
            .await
            .unwrap();
        let store = Store {
            dir: dir.path().to_path_buf(),
            profile: Profile::REGTEST,
        };
        let wallet = {
            let (args, api) = (args(dir.path(), &["--name", "exchange"]), api.clone());
            blocking(move || Wallet::open(&store, &args, "a test password", api))
                .await
                .unwrap()
        };
        assert!(Shared::new(
            Wallet::open(
                &Store {
                    dir: dir.path().to_path_buf(),
                    profile: Profile::REGTEST
                },
                &args(dir.path(), &["--name", "customer"]),
                "a test password",
                api.clone()
            )
            .unwrap(),
            Zeroizing::new("short".into())
        )
        .is_err(), "a short token is refused");
        let shared = Arc::new(Shared::new(wallet, Zeroizing::new(TOKEN.into())).unwrap());
        let info = call(&shared, "get_info", json!({}))["result"].clone();
        assert_eq!((info["synced"].as_bool(), info["wallet_height"].as_u64()), (Some(true), Some(2)), "{info}");

        // A deposit address for customer 9007199254740993 (above 2^53).
        let made = call(&shared, "make_integrated_address", json!({"tag":"9007199254740993"}))["result"].clone();
        let integrated = made["integrated_address"].as_str().unwrap().to_string();
        assert_eq!(made["tag"], "9007199254740993");
        let split = call(&shared, "split_integrated_address", json!({"integrated_address":integrated}))["result"].clone();
        assert_eq!(split["tag"], "9007199254740993");
        assert_eq!(call(&shared, "validate_address", json!({"address":integrated}))["result"]["kind"], "integrated");
        assert_eq!(call(&shared, "validate_address", json!({"address":"nope"}))["result"]["valid"], false);

        // The customer pays it with the CLI, and it is mined.
        let sent = act(
            "send",
            &args(dir.path(), &["--name", "customer", "--api", &api, "--to", &integrated, "--amount", "3", "--memo", "my first deposit"]),
        )
        .await
        .unwrap();
        assert_eq!(sent["tag"], "9007199254740993", "{sent}");
        stark_node::mine_once(&node, customer_address.clone()).await.unwrap();
        {
            let shared = shared.clone();
            blocking(move || {
                sync(&shared);
                Ok(())
            })
            .await
            .unwrap();
        }
        let found = call(&shared, "get_deposits", json!({}))["result"].clone();
        let deposits = found["deposits"].as_array().unwrap();
        assert_eq!(deposits.len(), 1, "{found}");
        assert_eq!(deposits[0]["tag"], "9007199254740993");
        assert_eq!(deposits[0]["amount_atoms"], 3 * ATOMS);
        assert_eq!(deposits[0]["txid"], sent["txid"]);
        assert_eq!(deposits[0]["confirmations"], 1);
        assert_eq!(deposits[0]["coinbase"], false);
        let tagged = call(&shared, "get_deposits", json!({"tag":"9007199254740993"}))["result"].clone();
        assert_eq!(tagged["deposits"].as_array().unwrap().len(), 1);
        let other = call(&shared, "get_deposits", json!({"tag":7}))["result"].clone();
        assert!(other["deposits"].as_array().unwrap().is_empty());
        let by_txid = call(&shared, "get_transfer_by_txid", json!({"txid":sent["txid"]}))["result"].clone();
        assert_eq!(by_txid["deposits"].as_array().unwrap().len(), 1);

        // A withdrawal back to the customer, asked twice: paid once.
        let balance = call(&shared, "get_balance", json!({}))["result"].clone();
        assert_eq!(balance["spendable_atoms"], 3 * ATOMS, "{balance}");
        let withdrawal = json!({"request_id":"w-1","destination":customer_full,"amount_atoms":ATOMS});
        let first = {
            let (shared, withdrawal) = (shared.clone(), withdrawal.clone());
            blocking(move || Ok(call(&shared, "transfer", withdrawal))).await.unwrap()
        };
        let first = first["result"].clone();
        assert_eq!(first["status"], "pending", "{first}");
        assert_eq!(first["fee_atoms"], conventional_fee(1, 2));
        let again = call(&shared, "transfer", withdrawal.clone())["result"].clone();
        assert_eq!(again["txid"], first["txid"], "the same request is the same withdrawal");
        assert_eq!(shared.wallet.lock().unwrap().light.wallet.pendings().len(), 1);
        let reused = call(
            &shared,
            "transfer",
            json!({"request_id":"w-1","destination":customer_full,"amount_atoms":2 * ATOMS}),
        );
        assert_eq!(reused["error"]["code"], REQUEST_REUSED);

        // Mined: confirmed, with its height.
        stark_node::mine_once(&node, customer_address.clone()).await.unwrap();
        {
            let shared = shared.clone();
            blocking(move || {
                sync(&shared);
                Ok(())
            })
            .await
            .unwrap();
        }
        let done = call(&shared, "get_withdrawal", json!({"request_id":"w-1"}))["result"].clone();
        assert_eq!((done["status"].as_str(), done["height"].as_u64()), (Some("confirmed"), Some(4)), "{done}");
        // Its change is not a deposit.
        let found = call(&shared, "get_deposits", json!({}))["result"].clone();
        assert_eq!(found["deposits"].as_array().unwrap().len(), 1, "{found}");
        assert_eq!(call(&shared, "no_such_method", json!({}))["error"]["code"], METHOD_NOT_FOUND);

        // Over HTTP: no token or a wrong one is refused; the right one answers.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let rpc_server = tokio::spawn(async move { axum::serve(listener, router(shared)).await });
        let body = r#"{"jsonrpc":"2.0","id":"a","method":"get_info"}"#;
        assert!(post(address, None, body).await.0.contains("401"));
        assert!(post(address, Some("a wrong token of 32 characters or more!"), body).await.0.contains("401"));
        let (status, reply) = post(address, Some(TOKEN), body).await;
        assert!(status.contains("200"), "{status}");
        let reply: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!((reply["id"].as_str(), reply["result"]["synced"].as_bool()), (Some("a"), Some(true)), "{reply}");
        let batch = r#"[{"jsonrpc":"2.0","id":1,"method":"get_balance"},{"jsonrpc":"2.0","method":"get_info"}]"#;
        let (_, reply) = post(address, Some(TOKEN), batch).await;
        let reply: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(reply.as_array().unwrap().len(), 1, "a notification gets no reply: {reply}");
        rpc_server.abort();
        api_server.abort();
    }
}
