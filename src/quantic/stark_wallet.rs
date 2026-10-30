//! `quantic-pq-wallet`: STARK v2 wallets behind the JSON contract of the v1
//! `quantic-wallet`, so ZSilent keeps its calls and models.
//!
//! Wallets are `<dir>/wallets/<name>.json` in the `quantic-pq` format, so the
//! explicit-path tools open them too. Balance, history and payments read a
//! mirror database (`<dir>/wallet-mirror`) that each call first brings up to
//! the wallet's own node over a direct connection (`--peer`, the file the
//! node writes with `--descriptor-out`). The node's loopback API (`--api`)
//! resolves short addresses, takes payments and publishes the wallet's
//! address. Unlike `quantic-pq wallet-create`, `new` prints the recovery
//! phrase once, as the v1 wallet did: ZSilent shows it to be written down.
//!
//! Every reply is one JSON line (`--json`) after anything the node code may
//! print while syncing, so a caller reads the last line.

use std::{
    fs,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use bip39::{Language, Mnemonic};
use rand::RngCore;
use serde_json::{json, Value};
use zeroize::Zeroizing;

use super::{
    stark::{
        protocol::wallet::{Address, ShortAddress},
        Profile,
    },
    stark_api::NodeState,
    stark_cli::{self, error, flag, option, required, Scan, WalletFile},
    stark_directory::AddressRecord,
    stark_net::NodeDescriptor,
    stark_node,
};
use crate::economics::{format_zka_amount, parse_zka_amount};

const USAGE: &str = "usage: zoka quantic-pq-wallet <new|generate-phrase|restore|address|publish-address|list|balance|history|estimate-fee|send|show-phrase|show-view-key|change-password|backup|import-backup> --network testnet|regtest --dir DIR [--name WALLET | --address ITS_ADDRESS] [--password-env VAR | --password-stdin] [--peer NODE_DESCRIPTOR] [--api http://127.0.0.1:9140] [history: --limit N] [restore: --phrase-env VAR or the phrase on stdin] [estimate-fee and send: --amount ZKA] [send: --to FULL_OR_SHORT [--memo TEXT]] [change-password: --new-password-env VAR] [backup: --out FILE] [import-backup: --file FILE] [--json]";
const DEFAULT_API: &str = "http://127.0.0.1:9140";
/// Blocks one call imports into the mirror before it answers.
const MIRROR_BUDGET: usize = 4096;
const MAX_DESCRIPTOR_BYTES: usize = 8 * 1024;
const MAX_API_REPLY: usize = 1024 * 1024;
const MAX_NAME: usize = 64;

/// The wallets of one chain under `--dir`.
struct Store {
    dir: PathBuf,
    profile: Profile,
}

impl Store {
    fn wallets(&self) -> PathBuf {
        self.dir.join("wallets")
    }

    fn path(&self, name: &str) -> Result<PathBuf, String> {
        let valid = !name.is_empty()
            && name.len() <= MAX_NAME
            && !name.starts_with('.')
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
        if !valid {
            return Err(format!(
                "invalid wallet name {name:?}: up to {MAX_NAME} letters, digits, '-', '_' or '.'"
            ));
        }
        Ok(self.wallets().join(format!("{name}.json")))
    }

    /// Name and address of every wallet of this chain, by name.
    fn list(&self) -> Vec<(String, String)> {
        let Ok(entries) = fs::read_dir(self.wallets()) else {
            return Vec::new();
        };
        let mut wallets: Vec<_> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let path = entry.path();
                let name = path
                    .file_name()?
                    .to_str()?
                    .strip_suffix(".json")?
                    .to_string();
                let (file, profile) = stark_cli::read_wallet(&path).ok()?;
                (profile == self.profile).then_some((name, file.address))
            })
            .collect();
        wallets.sort();
        wallets
    }

    /// `--name`, or the wallet whose full or short address is `--address`.
    fn select(&self, args: &[String]) -> Result<(String, PathBuf), String> {
        let name = match (option(args, "--name"), option(args, "--address")) {
            (Some(name), _) => name.to_string(),
            (None, Some(address)) => {
                let address = address.trim();
                self.list()
                    .into_iter()
                    .find(|(_, own)| {
                        own == address
                            || Address::decode(own, self.profile.network)
                                .and_then(|full| full.short())
                                .is_ok_and(|short| short == address)
                    })
                    .map(|(name, _)| name)
                    .ok_or("no Zoka Quantic v2 wallet here has that address")?
            }
            (None, None) => "default".to_string(),
        };
        let path = self.path(&name)?;
        if !path.exists() {
            return Err(format!("no wallet named {name}"));
        }
        Ok((name, path))
    }
}

fn password(args: &[String]) -> Result<Option<Zeroizing<String>>, String> {
    let value = if let Some(variable) = option(args, "--password-env") {
        Zeroizing::new(std::env::var(variable).map_err(|_| format!("{variable} is not set"))?)
    } else if flag(args, "--password-stdin") {
        stark_cli::secret_line("password")?
    } else if let Some(given) = option(args, "--password") {
        Zeroizing::new(given.to_string())
    } else {
        return Ok(None);
    };
    if value.is_empty() {
        return Err("the password cannot be empty".into());
    }
    Ok(Some(value))
}

fn required_password(args: &[String]) -> Result<Zeroizing<String>, String> {
    password(args)?
        .ok_or_else(|| "this action needs the wallet password (--password-env VAR)".into())
}

fn short_of(address: &str, profile: Profile) -> String {
    Address::decode(address, profile.network)
        .and_then(|full| full.short())
        .unwrap_or_default()
}

fn phrase_of(seed: &[u8; 32]) -> Result<Zeroizing<String>, String> {
    let mnemonic = Mnemonic::from_entropy_in(Language::English, seed).map_err(error)?;
    Ok(Zeroizing::new(mnemonic.to_string()))
}

fn seed_of(phrase: &str) -> Result<Zeroizing<[u8; 32]>, String> {
    let mnemonic = Mnemonic::parse_in_normalized(Language::English, phrase.trim())
        .map_err(|_| "invalid recovery phrase".to_string())?;
    let entropy = Zeroizing::new(mnemonic.to_entropy());
    let seed: [u8; 32] = entropy
        .as_slice()
        .try_into()
        .map_err(|_| "Zoka Quantic v2 needs a 24-word / 256-bit recovery phrase".to_string())?;
    Ok(Zeroizing::new(seed))
}

fn fresh_seed() -> Result<Zeroizing<[u8; 32]>, String> {
    let mut seed = Zeroizing::new([0u8; 32]);
    rand::rngs::OsRng
        .try_fill_bytes(seed.as_mut())
        .map_err(error)?;
    Ok(seed)
}

/// Blocking work off the async runtime, on a thread with a large stack:
/// proving goes deep.
async fn blocking<T: Send + 'static>(
    job: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(move || {
        std::thread::Builder::new()
            .name("quantic-pq-wallet".into())
            .stack_size(64 * 1024 * 1024)
            .spawn(job)
            .map_err(error)?
            .join()
            .map_err(|_| "the wallet thread panicked".to_string())?
    })
    .await
    .map_err(error)?
}

/// The wallet mirror, brought up to the wallet's node when `--peer` names
/// it. Returns the mirror and why it may lag, if it does: an unreachable or
/// still catching-up node does not stop a balance from answering.
async fn mirror(store: &Store, args: &[String]) -> Result<(NodeState, Option<String>), String> {
    let path = store.dir.join("wallet-mirror");
    let profile = store.profile;
    let state = blocking(move || {
        let (db, _) = stark_cli::open_db(&path, Some(profile))?;
        NodeState::new(Arc::new(db))
    })
    .await?;
    let Some(peer) = option(args, "--peer") else {
        return Ok((state, Some("no --peer node descriptor".into())));
    };
    let descriptor: NodeDescriptor = serde_json::from_slice(&stark_cli::read_bounded(
        Path::new(peer),
        MAX_DESCRIPTOR_BYTES,
    )?)
    .map_err(error)?;
    let lag = match stark_node::follow(&state, &descriptor, MIRROR_BUDGET).await {
        Ok((_, true)) => None,
        Ok((head, false)) => Some(format!(
            "the mirror is at height {} and the node is ahead",
            head.height
        )),
        Err(reason) => Some(reason),
    };
    Ok((state, lag))
}

/// One request to the node's API, which listens on loopback only.
fn api_call(
    base: &str,
    method: &str,
    path: &str,
    body: Option<(&str, &[u8])>,
    timeout: Duration,
) -> Result<(u16, Vec<u8>), String> {
    let authority = base
        .strip_prefix("http://")
        .map(|rest| rest.trim_end_matches('/'))
        .filter(|rest| !rest.contains('/'))
        .ok_or("--api must look like http://127.0.0.1:9140")?;
    let address: SocketAddr = authority
        .parse()
        .map_err(|_| "--api must name an IP address and a port".to_string())?;
    if !address.ip().is_loopback() {
        return Err("--api must be a loopback address".into());
    }
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(5))
        .map_err(|e| format!("node API {authority}: {e}"))?;
    stream.set_read_timeout(Some(timeout)).map_err(error)?;
    stream.set_write_timeout(Some(timeout)).map_err(error)?;
    let (kind, bytes) = body.unwrap_or(("application/json", &[]));
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n\r\n",
        bytes.len()
    );
    stream
        .write_all(head.as_bytes())
        .and_then(|_| stream.write_all(bytes))
        .map_err(|e| format!("node API {authority}: {e}"))?;
    let mut reply = Vec::new();
    (&mut stream)
        .take(MAX_API_REPLY as u64 + 1)
        .read_to_end(&mut reply)
        .map_err(|e| format!("node API {authority}: {e}"))?;
    if reply.len() > MAX_API_REPLY {
        return Err("node API reply too large".into());
    }
    let split = reply
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("malformed node API reply")?;
    let head = String::from_utf8_lossy(&reply[..split]).to_ascii_lowercase();
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .ok_or("malformed node API status")?;
    let body = &reply[split + 4..];
    let chunked = head
        .lines()
        .any(|line| line.starts_with("transfer-encoding:") && line.contains("chunked"));
    Ok((
        status,
        if chunked {
            dechunk(body)?
        } else {
            body.to_vec()
        },
    ))
}

fn dechunk(mut body: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let end = body
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or("malformed chunked reply")?;
        let size = std::str::from_utf8(&body[..end])
            .ok()
            .and_then(|line| usize::from_str_radix(line.split(';').next()?.trim(), 16).ok())
            .ok_or("malformed chunk size")?;
        body = &body[end + 2..];
        if size == 0 {
            return Ok(out);
        }
        if body.len() < size + 2 {
            return Err("truncated chunked reply".into());
        }
        out.extend_from_slice(&body[..size]);
        body = &body[size + 2..];
    }
}

/// A JSON reply from the node API. A busy node answers 503 while both of its
/// crypto workers verify: a few retries cover that.
fn api_json(
    base: &str,
    method: &str,
    path: &str,
    body: Option<(&str, &[u8])>,
    timeout: Duration,
) -> Result<(u16, Value), String> {
    for attempt in 0..5 {
        let (status, bytes) = api_call(base, method, path, body, timeout)?;
        if status == 503 && attempt < 4 {
            std::thread::sleep(Duration::from_secs(1));
            continue;
        }
        let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        return Ok((status, value));
    }
    Err("the node API stayed busy".into())
}

fn api_ok(status: u16, value: Value) -> Result<Value, String> {
    if (200..300).contains(&status) {
        return Ok(value);
    }
    let message = value
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("no reason given")
        .to_string();
    Err(format!("node API {status}: {message}"))
}

/// A full address, or the full address the node finds for a short one. The
/// node cannot answer another address: the short one is its digest.
fn resolve(text: &str, profile: Profile, api: &str) -> Result<Address, String> {
    let text = text.trim();
    if let Ok(full) = Address::decode(text, profile.network) {
        return Ok(full);
    }
    let short = ShortAddress::decode(text, profile.network)
        .map_err(|_| format!("--to is not a {} v2 address, full or short", profile.name))?;
    let (status, reply) = api_json(
        api,
        "GET",
        &format!("/address/{}", short.encode()),
        None,
        Duration::from_secs(40),
    )?;
    let reply = api_ok(status, reply)?;
    let full = Address::decode(
        reply["address"].as_str().unwrap_or_default(),
        profile.network,
    )
    .map_err(|_| "the node answered something that is not an address".to_string())?;
    if !short.matches(&full) {
        return Err("the node answered another address for that short address".into());
    }
    Ok(full)
}

/// Balance figures of one scan, with a live pending payment's inputs
/// reserved.
struct Funds {
    spendable: u64,
    immature: u64,
    reserved: u64,
    max_sendable: u64,
    notes: usize,
    pending: bool,
}

fn funds(scan: &Scan, wallet: &Path, password: &str, profile: Profile) -> Result<Funds, String> {
    let reservation = stark_cli::pending(wallet, password, profile)?
        .filter(|tx| stark_cli::live_pending(tx, &scan.state));
    let mut figures = Funds {
        spendable: 0,
        immature: 0,
        reserved: 0,
        max_sendable: 0,
        notes: scan.notes.len(),
        pending: reservation.is_some(),
    };
    for owned in &scan.notes {
        let total = if owned.index.is_none() {
            &mut figures.immature
        } else if reservation
            .as_ref()
            .is_some_and(|tx| tx.statement.nullifiers.contains(&owned.nullifier))
        {
            &mut figures.reserved
        } else {
            &mut figures.spendable
        };
        *total = total
            .checked_add(owned.note.value)
            .ok_or("balance overflow")?;
    }
    // One pending payment per wallet: nothing more goes out until it does.
    if reservation.is_none() {
        figures.max_sendable = stark_cli::max_sendable(&scan.notes);
    }
    Ok(figures)
}

const PENDING_REASON: &str =
    "a payment of this wallet is still pending: wait until it is mined or expires";

/// Run one action and return its JSON reply.
async fn act(action: &str, args: &[String]) -> Result<Value, String> {
    let profile = Profile::named(required(args, "--network")?)?;
    let store = Store {
        dir: PathBuf::from(required(args, "--dir")?),
        profile,
    };
    let api = option(args, "--api").unwrap_or(DEFAULT_API).to_string();
    match action {
        "new" | "restore" => {
            let name = option(args, "--name").unwrap_or("default").to_string();
            let path = store.path(&name)?;
            if path.exists() {
                return Err(format!("a wallet named {name} already exists"));
            }
            let password = required_password(args)?;
            let seed = if action == "new" {
                fresh_seed()?
            } else {
                let phrase = match (
                    option(args, "--phrase-env").or_else(|| option(args, "--seed-env")),
                    option(args, "--phrase"),
                ) {
                    (Some(variable), _) => Zeroizing::new(
                        std::env::var(variable).map_err(|_| format!("{variable} is not set"))?,
                    ),
                    (None, Some(phrase)) => Zeroizing::new(phrase.to_string()),
                    (None, None) => stark_cli::secret_line("recovery phrase")?,
                };
                seed_of(&phrase)?
            };
            let address = blocking({
                let seed = seed.clone();
                move || stark_cli::create_wallet(&path, profile, &seed, &password)
            })
            .await?;
            let mut reply =
                json!({"name":name,"address":address,"short_address":short_of(&address, profile)});
            if action == "new" {
                reply["recovery_phrase"] = phrase_of(&seed)?.as_str().into();
            }
            Ok(reply)
        }
        "generate-phrase" => {
            let phrase = phrase_of(&*fresh_seed()?)?;
            Ok(json!({"words":24,"recovery_phrase":phrase.as_str()}))
        }
        "list" => {
            let password = password(args)?;
            let wallets = store.list();
            let names: Vec<_> = wallets.iter().map(|(name, _)| name.clone()).collect();
            let details = blocking(move || {
                Ok(wallets
                    .into_iter()
                    .map(|(name, address)| {
                        let unlocked = password.as_ref().is_some_and(|password| {
                            store
                                .path(&name)
                                .is_ok_and(|path| stark_cli::open_wallet(&path, password).is_ok())
                        });
                        json!({"name":name,"address":address,
                            "short_address":short_of(&address, profile),"unlocked":unlocked})
                    })
                    .collect::<Vec<_>>())
            })
            .await?;
            Ok(json!({"network":profile.chain_id,"wallets":names,"details":details}))
        }
        "address" => {
            let (name, path) = store.select(args)?;
            let (file, _) = stark_cli::read_wallet(&path)?;
            Ok(
                json!({"name":name,"short_address":short_of(&file.address, profile),"address":file.address}),
            )
        }
        "publish-address" => {
            let (name, path) = store.select(args)?;
            let (file, _) = stark_cli::read_wallet(&path)?;
            let full = Address::decode(&file.address, profile.network).map_err(error)?;
            let short = full.short().map_err(error)?;
            let node = blocking(move || {
                // The stamp is a few seconds of Argon2id here, not on the node.
                let record = AddressRecord::new(&full).map_err(error)?;
                let body =
                    serde_json::to_vec(&json!({"address":record.address,"stamp":record.stamp}))
                        .map_err(error)?;
                let (status, reply) = api_json(
                    &api,
                    "POST",
                    "/address",
                    Some(("application/json", &body)),
                    Duration::from_secs(60),
                )?;
                api_ok(status, reply)
            })
            .await?;
            Ok(json!({"name":name,"short_address":short,"node":node}))
        }
        "backup" => {
            let (name, path) = store.select(args)?;
            let out = PathBuf::from(required(args, "--out")?);
            let password = password(args)?;
            blocking(move || {
                if let Some(password) = password {
                    stark_cli::open_wallet(&path, &password)?;
                }
                let bytes = stark_cli::read_bounded(&path, stark_cli::MAX_WALLET_BYTES)?;
                stark_cli::write_new(&out, &bytes)?;
                Ok(json!({"name":name,"backup":out.display().to_string()}))
            })
            .await
        }
        "import-backup" => {
            let name = option(args, "--name").unwrap_or("restored").to_string();
            let path = store.path(&name)?;
            if path.exists() {
                return Err(format!("a wallet named {name} already exists"));
            }
            let file = PathBuf::from(required(args, "--file")?);
            let password = required_password(args)?;
            blocking(move || {
                let (wallet, found) = stark_cli::read_wallet(&file)?;
                if found != profile {
                    return Err(format!("that wallet belongs to {}", found.chain_id));
                }
                stark_cli::open_wallet(&file, &password)?;
                let bytes = stark_cli::read_bounded(&file, stark_cli::MAX_WALLET_BYTES)?;
                stark_cli::write_new(&path, &bytes)?;
                Ok(json!({"name":name,"address":wallet.address}))
            })
            .await
        }
        "show-phrase" => {
            let (name, path) = store.select(args)?;
            let password = required_password(args)?;
            let seed =
                blocking(move || Ok(stark_cli::open_wallet_seed(&path, &password)?.2)).await?;
            Ok(json!({"name":name,"recovery_phrase":phrase_of(&seed)?.as_str()}))
        }
        "show-view-key" => {
            let (name, path) = store.select(args)?;
            let password = required_password(args)?;
            let key = blocking(move || {
                let (account, _) = stark_cli::open_wallet(&path, &password)?;
                Ok(Zeroizing::new(account.view_key().map_err(error)?.encode()))
            })
            .await?;
            Ok(json!({"name":name,"view_key":key.as_str(),
                "note":"whoever holds this key sees every payment and the balance of this wallet; it cannot spend"}))
        }
        "change-password" => {
            let (name, path) = store.select(args)?;
            let password = required_password(args)?;
            let new_password = Zeroizing::new(match option(args, "--new-password-env") {
                Some(variable) => {
                    std::env::var(variable).map_err(|_| format!("{variable} is not set"))?
                }
                None => required(args, "--new-password")?.to_string(),
            });
            if new_password.is_empty() {
                return Err("the new password cannot be empty".into());
            }
            blocking(move || change_password(&path, profile, &password, &new_password)).await?;
            Ok(json!({"name":name,"changed":true}))
        }
        "balance" | "history" | "estimate-fee" => {
            let (name, path) = store.select(args)?;
            let password = required_password(args)?;
            let amount = match action {
                "estimate-fee" => Some(parse_zka_amount(required(args, "--amount")?)?),
                _ => None,
            };
            let limit: usize = option(args, "--limit")
                .map(|limit| {
                    limit
                        .parse()
                        .map_err(|_| "--limit must be a number".to_string())
                })
                .transpose()?
                .unwrap_or(usize::MAX);
            let (state, lag) = mirror(&store, args).await?;
            let action = action.to_string();
            blocking(move || {
                let (account, _) = stark_cli::open_wallet(&path, &password)?;
                let scan = stark_cli::scan(&state.storage, profile, &account)?;
                let figures = funds(&scan, &path, &password, profile)?;
                let height = u64::from(scan.state.height);
                Ok(match action.as_str() {
                    "balance" => json!({
                        "name":name,"height":height,"wallet_height":height,
                        "node_behind":lag.is_some(),"sync_error":lag,
                        "spendable_atoms":figures.spendable,"immature_atoms":figures.immature,
                        "reserved_atoms":figures.reserved,
                        "spendable":format_zka_amount(figures.spendable),
                        "immature":format_zka_amount(figures.immature),
                        "max_sendable_atoms":figures.max_sendable,"notes":figures.notes,
                        "pending_payment":figures.pending,
                    }),
                    "history" => {
                        let count = scan.history.len();
                        let entries: Vec<_> = scan
                            .history
                            .iter()
                            .rev()
                            .take(limit)
                            .map(|entry| {
                                let net = i128::from(entry.received) - i128::from(entry.spent);
                                let amount = u64::try_from(net.unsigned_abs()).unwrap_or(u64::MAX);
                                let kind = if entry.coinbase {
                                    "coinbase"
                                } else if entry.spent > 0 {
                                    "sent"
                                } else {
                                    "received"
                                };
                                json!({
                                    "txid":hex::encode(entry.txid),"height":entry.height,"kind":kind,
                                    "received_atoms":entry.received,"spent_atoms":entry.spent,
                                    "net_atoms":net.to_string(),"amount_atoms":amount,
                                    "amount":format_zka_amount(amount),"timestamp":entry.timestamp,
                                    "confirmations":height.saturating_sub(entry.height) + 1,
                                })
                            })
                            .collect();
                        json!({"name":name,"height":height,"node_behind":lag.is_some(),"count":count,"transactions":entries})
                    }
                    _ => {
                        let amount = amount.expect("estimate-fee parsed its amount");
                        let mut reply = json!({
                            "name":name,"height":height,"amount_atoms":amount,
                            "max_sendable_atoms":figures.max_sendable,"spendable_atoms":figures.spendable,
                        });
                        let plan = if figures.pending {
                            Err(PENDING_REASON.to_string())
                        } else {
                            stark_cli::select_inputs(scan.notes, amount)
                        };
                        match plan {
                            Ok((inputs, total, fee)) => {
                                reply["enough"] = true.into();
                                reply["fee_atoms"] = fee.into();
                                reply["fee"] = format_zka_amount(fee).into();
                                reply["inputs"] = inputs.len().into();
                                reply["change_atoms"] = (total - amount - fee).into();
                            }
                            Err(reason) => {
                                reply["enough"] = false.into();
                                reply["reason"] = reason.into();
                            }
                        }
                        reply
                    }
                })
            })
            .await
        }
        "send" => {
            let (name, path) = store.select(args)?;
            let password = required_password(args)?;
            let amount = parse_zka_amount(required(args, "--amount")?)?;
            if amount == 0 {
                return Err("--amount must be positive".into());
            }
            let memo = option(args, "--memo")
                .filter(|memo| !memo.is_empty())
                .map(str::to_owned);
            let to = {
                let (text, api) = (required(args, "--to")?.to_string(), api.clone());
                blocking(move || resolve(&text, profile, &api))
                    .await
                    .map_err(|e| format!("--to: {e}"))?
            };
            let (state, lag) = mirror(&store, args).await?;
            if let Some(lag) = lag {
                return Err(format!(
                    "the wallet's node is not in sync ({lag}); send when it is"
                ));
            }
            let wallet = fs::canonicalize(&path).map_err(error)?;
            blocking(move || {
                let _lock = stark_cli::wallet_lock(&wallet)?;
                let (account, _) = stark_cli::open_wallet(&wallet, &password)?;
                let (tx, fee, _) = stark_cli::pay(
                    &state.storage,
                    profile,
                    &wallet,
                    &account,
                    &password,
                    to,
                    amount,
                    memo.as_deref(),
                )?;
                let height = stark_cli::tip(&state.storage, profile)?.header.block_height;
                let bytes = tx.encode().map_err(error)?;
                let sent = api_json(
                    &api,
                    "POST",
                    "/transactions",
                    Some(("application/octet-stream", &bytes)),
                    Duration::from_secs(120),
                );
                match sent {
                    Ok((status, reply)) if (200..300).contains(&status) => Ok(json!({
                        "name":name,"txid":hex::encode(tx.txid()),"fee_atoms":fee,"height":height,"node":reply,
                    })),
                    // The node refused it: free the inputs for another try.
                    Ok((status, reply)) => {
                        let _ = fs::remove_file(stark_cli::pending_path(&wallet));
                        api_ok(status, reply)
                    }
                    // Unknown outcome: the payment may be in the node's
                    // mempool, so its inputs stay reserved until it expires.
                    Err(reason) => Err(format!("{reason}; the payment stays reserved until it is mined or expires")),
                }
            })
            .await
        }
        _ => Err(USAGE.into()),
    }
}

/// Re-encrypt the wallet, and its pending payment if there is one, with a
/// new password. Each file is replaced only once its new copy is written.
fn change_password(path: &Path, profile: Profile, old: &str, new: &str) -> Result<(), String> {
    let _lock = stark_cli::wallet_lock(path)?;
    let (account, _, seed) = stark_cli::open_wallet_seed(path, old)?;
    let (file, found) = stark_cli::read_wallet(path)?;
    if found != profile {
        return Err(format!("that wallet belongs to {}", found.chain_id));
    }
    let wallet = WalletFile {
        schema: file.schema,
        chain_id: file.chain_id,
        address: account.address().encode().map_err(error)?,
        encrypted_seed_hex: hex::encode(
            crate::wallet::storage::encrypt_data(&seed[..], new).map_err(error)?,
        ),
    };
    let mut replaced = vec![(
        path.to_path_buf(),
        serde_json::to_vec_pretty(&wallet).map_err(error)?,
    )];
    if let Some(tx) = stark_cli::pending(path, old, profile)? {
        let bytes = tx.encode().map_err(error)?;
        let pending = crate::wallet::storage::encrypt_data(&bytes, new).map_err(error)?;
        replaced.push((stark_cli::pending_path(path), pending));
    }
    let staged: Vec<_> = replaced
        .into_iter()
        .map(|(target, bytes)| {
            let mut name = target.as_os_str().to_os_string();
            name.push(".new");
            let staged = PathBuf::from(name);
            let _ = fs::remove_file(&staged);
            stark_cli::write_new(&staged, &bytes).map(|_| (staged, target))
        })
        .collect::<Result<_, String>>()?;
    for (staged, target) in staged {
        fs::rename(&staged, &target).map_err(error)?;
    }
    Ok(())
}

fn human(action: &str, reply: &Value) -> String {
    let field = |key: &str| reply[key].as_str().unwrap_or_default().to_string();
    match action {
        "new" => format!(
            "Wallet {} created.\nRecovery phrase (write it down offline; it is shown only once):\n{}\nAddress (the one you share):\n{}",
            field("name"),
            field("recovery_phrase"),
            field("short_address")
        ),
        "generate-phrase" => field("recovery_phrase"),
        "restore" | "address" | "publish-address" => field("short_address"),
        "balance" => {
            let mut text = format!(
                "Spendable: {} ZKA\nImmature: {} ZKA\nHeight: {}",
                field("spendable"),
                field("immature"),
                reply["height"]
            );
            if let Some(reason) = reply["sync_error"].as_str() {
                text.push_str(&format!("\nThe wallet may be behind: {reason}"));
            }
            text
        }
        "list" => reply["wallets"]
            .as_array()
            .map(|names| {
                names
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default(),
        "show-phrase" => field("recovery_phrase"),
        "show-view-key" => format!("{}\n{}", field("view_key"), field("note")),
        "send" => format!("Sent. Transaction {}", field("txid")),
        _ => reply.to_string(),
    }
}

pub async fn run(args: &[String]) -> Result<(), String> {
    let Some((action, rest)) = args.split_first() else {
        return Err(USAGE.into());
    };
    if matches!(action.as_str(), "help" | "--help" | "-h") {
        println!("{USAGE}");
        return Ok(());
    }
    let reply = act(action, rest).await?;
    if flag(rest, "--json") {
        println!("{reply}");
    } else if action == "address" && flag(rest, "--full") {
        println!("{}", reply["address"].as_str().unwrap_or_default());
    } else {
        println!("{}", human(action, &reply));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quantic::{
        stark::protocol::ledger::{conventional_fee, ATOMS, PREMINE},
        stark_api,
        stark_net::{self, ServerIdentity},
    };
    use iroh::endpoint::presets;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| s.to_string()).collect()
    }

    fn base(dir: &Path) -> Vec<String> {
        args(&[
            "--network",
            "regtest",
            "--dir",
            dir.to_str().unwrap(),
            "--password",
            "a test password",
        ])
    }

    fn with(dir: &Path, more: &[&str]) -> Vec<String> {
        let mut all = base(dir);
        all.extend(args(more));
        all
    }

    #[tokio::test]
    async fn a_wallet_is_created_shown_restored_and_rekeyed() {
        let dir = tempfile::tempdir().unwrap();
        let created = act("new", &with(dir.path(), &["--name", "alice"]))
            .await
            .unwrap();
        let phrase = created["recovery_phrase"].as_str().unwrap().to_string();
        assert_eq!(phrase.split_whitespace().count(), 24);
        let address = created["address"].as_str().unwrap().to_string();
        assert!(address.starts_with("zqregtest2"));
        let short = created["short_address"].as_str().unwrap().to_string();
        assert!(act("new", &with(dir.path(), &["--name", "alice"]))
            .await
            .is_err());

        // The same phrase restores the same wallet under another name.
        let restored = act(
            "restore",
            &with(dir.path(), &["--name", "copy", "--phrase", &phrase]),
        )
        .await
        .unwrap();
        assert_eq!(restored["address"], address.as_str());
        let shown = act("show-phrase", &with(dir.path(), &["--address", &short]))
            .await
            .unwrap();
        assert_eq!(shown["recovery_phrase"], phrase.as_str());
        // Its view key names the same wallet and opens nothing else.
        let viewed = act("show-view-key", &with(dir.path(), &["--name", "copy"]))
            .await
            .unwrap();
        let key =
            zoka_pq_pool::wallet::ViewKey::decode(viewed["view_key"].as_str().unwrap(), 3).unwrap();
        assert_eq!(key.address().encode().unwrap(), address);
        let listed = act("list", &base(dir.path())).await.unwrap();
        assert_eq!(listed["wallets"], json!(["alice", "copy"]));
        assert_eq!(listed["details"][0]["unlocked"], true);

        // A new password opens it; the old one no longer does.
        std::env::set_var("ZOKA_TEST_NEW_PASSWORD", "another password");
        act(
            "change-password",
            &with(
                dir.path(),
                &[
                    "--name",
                    "alice",
                    "--new-password-env",
                    "ZOKA_TEST_NEW_PASSWORD",
                ],
            ),
        )
        .await
        .unwrap();
        assert!(act("show-phrase", &with(dir.path(), &["--name", "alice"]))
            .await
            .is_err());
        let mut rekeyed = args(&[
            "--network",
            "regtest",
            "--dir",
            dir.path().to_str().unwrap(),
        ]);
        rekeyed.extend(args(&["--password", "another password", "--name", "alice"]));
        assert_eq!(
            act("show-phrase", &rekeyed).await.unwrap()["recovery_phrase"],
            phrase.as_str()
        );

        // A backup is the encrypted file; importing it checks the password.
        let out = dir.path().join("alice.backup.json");
        act(
            "backup",
            &rekeyed
                .iter()
                .cloned()
                .chain(args(&["--out", out.to_str().unwrap()]))
                .collect::<Vec<_>>(),
        )
        .await
        .unwrap();
        assert!(act(
            "import-backup",
            &with(
                dir.path(),
                &["--name", "again", "--file", out.to_str().unwrap()]
            )
        )
        .await
        .is_err());
        let mut import = rekeyed.clone();
        import.retain(|a| a != "alice" && a != "--name");
        import.extend(args(&["--name", "again", "--file", out.to_str().unwrap()]));
        assert_eq!(
            act("import-backup", &import).await.unwrap()["address"],
            address.as_str()
        );
        assert!(act("new", &with(dir.path(), &["--name", "../escape"]))
            .await
            .is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn balance_and_history_follow_the_wallet_node() {
        let dir = tempfile::tempdir().unwrap();
        let created = act("new", &with(dir.path(), &["--name", "miner"]))
            .await
            .unwrap();
        let address = Address::decode(created["address"].as_str().unwrap(), 3).unwrap();

        // The wallet's node, with one block mined to the wallet.
        let storage =
            Arc::new(crate::storage::RocksDbStorage::new(dir.path().join("node")).unwrap());
        storage
            .put_raw(
                stark_api::DB_MARKER,
                Profile::REGTEST.chain_id.as_bytes().to_vec(),
            )
            .unwrap();
        let node = NodeState::new(storage).unwrap();
        // Block 1 carries the premine, spendable at once; block 2 a coinbase
        // that matures two blocks later.
        let (first, _) = stark_node::mine_once(&node, address.clone()).await.unwrap();
        let (second, _) = stark_node::mine_once(&node, address).await.unwrap();
        let server = stark_net::serve(
            node.clone(),
            ServerIdentity::from_seed(&[90; 32]).unwrap(),
            presets::Minimal,
        )
        .await
        .unwrap();
        let mut descriptor = server.descriptor.clone();
        let socket = server
            .endpoint
            .bound_sockets()
            .into_iter()
            .find(|a| a.is_ipv4())
            .unwrap();
        descriptor.endpoint_addr = iroh::EndpointAddr::new(server.endpoint.id()).with_ip_addr(
            std::net::SocketAddr::new(std::net::Ipv4Addr::LOCALHOST.into(), socket.port()),
        );
        let peer = dir.path().join("node.peer.json");
        fs::write(&peer, serde_json::to_vec(&descriptor).unwrap()).unwrap();

        // Without the node the mirror answers what it has, and says so.
        let alone = act("balance", &with(dir.path(), &["--name", "miner"]))
            .await
            .unwrap();
        assert_eq!(alone["node_behind"], true);
        assert_eq!(alone["height"], 0);

        let synced = with(
            dir.path(),
            &["--name", "miner", "--peer", peer.to_str().unwrap()],
        );
        let balance = act("balance", &synced).await.unwrap();
        assert_eq!(balance["node_behind"], false, "{balance}");
        assert_eq!(balance["height"], 2);
        assert_eq!(balance["spendable_atoms"], PREMINE, "{balance}");
        let immature = balance["immature_atoms"].as_u64().unwrap();
        assert!(immature > 0, "{balance}");
        let fee = conventional_fee(1, 2);
        assert_eq!(balance["max_sendable_atoms"], PREMINE - fee);

        // Newest first, with each block's own time.
        let history = act("history", &synced).await.unwrap();
        assert_eq!(history["count"], 2);
        let entries = &history["transactions"];
        assert_eq!(entries[0]["kind"], "coinbase");
        assert_eq!(entries[0]["height"], 2);
        assert_eq!(entries[0]["amount_atoms"], immature);
        assert_eq!(entries[0]["timestamp"], second.header.timestamp);
        assert_eq!(entries[1]["height"], 1);
        assert_eq!(entries[1]["amount_atoms"], PREMINE);
        assert_eq!(entries[1]["timestamp"], first.header.timestamp);
        assert_eq!(entries[1]["confirmations"], 2);
        let limited = act(
            "history",
            &[synced.clone(), args(&["--limit", "1"])].concat(),
        )
        .await
        .unwrap();
        assert_eq!(limited["transactions"].as_array().unwrap().len(), 1);

        let estimate = act(
            "estimate-fee",
            &[synced.clone(), args(&["--amount", "1"])].concat(),
        )
        .await
        .unwrap();
        assert_eq!(estimate["enough"], true, "{estimate}");
        assert_eq!(estimate["fee_atoms"], fee);
        assert_eq!(estimate["inputs"], 1);
        assert_eq!(estimate["change_atoms"], PREMINE - ATOMS - fee);
        let too_much = act(
            "estimate-fee",
            &[synced.clone(), args(&["--amount", "6000000"])].concat(),
        )
        .await
        .unwrap();
        assert_eq!(too_much["enough"], false, "{too_much}");
        server.close().await;
    }

    #[test]
    fn the_api_client_reads_plain_and_chunked_replies() {
        assert_eq!(
            dechunk(b"4\r\nwiki\r\n5\r\npedia\r\n0\r\n\r\n").unwrap(),
            b"wikipedia"
        );
        assert!(dechunk(b"9\r\nshort\r\n").is_err());
        assert!(api_call(
            "http://192.168.1.1:9140",
            "GET",
            "/status",
            None,
            Duration::from_secs(1)
        )
        .is_err());
        assert!(api_call(
            "https://127.0.0.1:9140",
            "GET",
            "/status",
            None,
            Duration::from_secs(1)
        )
        .is_err());
    }
}
