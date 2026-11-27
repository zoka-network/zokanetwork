//! `quantic-pq-wallet`: STARK v2 wallets behind the JSON contract of the v1
//! `quantic-wallet`, so ZSilent keeps its calls and models.
//!
//! Wallets are `<dir>/wallets/<name>.json` in the `quantic-pq` format, so the
//! explicit-path tools open them too. Balance, history and payments run on
//! the light engine of ZKAPriv and the web wallet ([`LightWallet`]): its
//! state, sealed with a key of the seed, sits beside the wallet file
//! (`<name>.json.light`), and each call reads only the blocks that are new
//! since the last one, as `/scan2` pages of the wallet's own node, without
//! the proofs. Until 25/11/2026 each call rebuilt the whole ledger from a
//! mirror of the chain (`<dir>/wallet-mirror`, no longer used): about 30 s
//! and 2 GB read per call at height 9,741, growing with the chain. The
//! node's loopback API (`--api`) also resolves short addresses, takes
//! payments and publishes the wallet's address. `--peer` is still accepted
//! and ignored. Unlike `quantic-pq wallet-create`, `new` prints the recovery
//! phrase once, as the v1 wallet did: ZSilent shows it to be written down.
//!
//! Every reply is one JSON line (`--json`) after anything the node code may
//! print while syncing, so a caller reads the last line.

use std::{
    fs,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    time::Duration,
};

use aes_gcm::{
    aead::{Aead, Payload},
    Aes256Gcm, KeyInit, Nonce,
};
use bip39::{Language, Mnemonic};
use rand::RngCore;
use serde_json::{json, Value};
use zeroize::Zeroizing;

use super::{
    stark::{
        protocol::{
            integrated::{tag_memo, IntegratedAddress},
            light::LightWallet,
            scan::{ScanPage2, MAX_SCAN_BLOCKS, MAX_SCAN_PAGE_BYTES},
            transaction::Transaction,
            wallet::{Account, Address, ShortAddress},
        },
        Profile,
    },
    stark_cli::{self, error, flag, option, required, WalletFile},
    stark_directory::AddressRecord,
};
use crate::economics::{format_zka_amount, parse_zka_amount};

const USAGE: &str = "usage: zoka quantic-pq-wallet <new|generate-phrase|restore|address|addresses|new-address|publish-address|integrated-address|parse-address|list|balance|history|estimate-fee|send|show-phrase|show-view-key|change-password|backup|import-backup> --network testnet|regtest --dir DIR [--name WALLET | --address ITS_ADDRESS] [--password-env VAR | --password-stdin] [--api http://127.0.0.1:9140] [history: --limit N] [show-view-key: --incoming] [restore: --phrase-env VAR or the phrase on stdin] [estimate-fee and send: --amount ZKA] [send: --to FULL_SHORT_OR_INTEGRATED [--memo TEXT]] [integrated-address: --tag N] [parse-address: --address TEXT] [change-password: --new-password-env VAR] [backup: --out FILE] [import-backup: --file FILE] [--json]";
const DEFAULT_API: &str = "http://127.0.0.1:9140";
const MAX_API_REPLY: usize = 1024 * 1024;
const MAX_NAME: usize = 64;
/// Pages of `/scan2` one call reads before it answers (100 blocks each): a
/// wallet that starts from block 1 reads 40,000 blocks in its first call,
/// and the rest in the next ones.
const LIGHT_PAGE_BUDGET: usize = 400;
/// What a sealed light state may weigh: its ledger grows with the chain's
/// notes (about 100 bytes each).
const MAX_LIGHT_STATE_BYTES: usize = 1 << 30;
const LIGHT_MAGIC: &[u8; 8] = b"ZQLIGHT1";
const LIGHT_KEY_CONTEXT: &str = "ZOKA quantic light wallet state v1";

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
        .map_err(|_| "Zoka Quantic needs a 24-word / 256-bit recovery phrase".to_string())?;
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

/// A wallet opened on the light engine: its addresses, its state, and where
/// and with what the state is sealed. The caller holds the wallet lock.
struct Light {
    wallet: LightWallet,
    accounts: Vec<Account>,
    path: PathBuf,
    key: Zeroizing<[u8; 32]>,
    chain_id: &'static str,
    /// A payment the earlier engine left reserved (`<name>.json.stark-pending`),
    /// to hold again once the state has read the chain.
    pending_payment: Option<Transaction>,
    wallet_path: PathBuf,
    /// The earlier engine's file is set aside once the state that took its
    /// payment over is written.
    retire_pending: bool,
}

impl Light {
    /// Open the wallet with its password and its state, or a new state that
    /// reads the chain from block 1 if there is none or it cannot be read
    /// (another chain, another format, damaged): it is only a cache.
    fn open(wallet: &Path, password: &str, profile: Profile) -> Result<Self, String> {
        let (_, _, seed) = stark_cli::open_wallet_seed(wallet, password)?;
        let (file, _) = stark_cli::read_wallet(wallet)?;
        let key = light_key(&seed, profile);
        let path = light_path(wallet);
        let mut light = match read_light(&path, &key, profile)
            .ok()
            .flatten()
            .filter(|state| state.network() == profile.network)
        {
            Some(state) => state,
            None => LightWallet::new(profile.network, 1).map_err(error)?,
        };
        // The file only grows its addresses; a state that read for more (a
        // file put back from an older backup) keeps reading for those.
        let count = file.addresses.max(light.addresses()).max(1);
        if count > light.addresses() {
            light.set_addresses(count).map_err(error)?;
        }
        let accounts = (0..count)
            .map(|index| Account::from_seed(&seed, profile.network, index).map_err(error))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            wallet: light,
            accounts,
            path,
            key,
            chain_id: profile.chain_id,
            pending_payment: stark_cli::pending(wallet, password, profile).ok().flatten(),
            wallet_path: wallet.to_path_buf(),
            retire_pending: false,
        })
    }

    /// Bring the state up to the node of `api`, page by page from where it
    /// stopped. Returns why it may lag: an unreachable or still catching-up
    /// node does not stop a balance from answering.
    fn sync(&mut self, api: &str) -> Option<String> {
        let lag = self.read_pages(api);
        if lag.is_none() {
            self.adopt_pending();
        }
        lag
    }

    /// The payment the earlier engine reserved, held again if it can still
    /// be mined (the engine checks it against the ledger it read), and its
    /// file set aside as `.adopted` so this happens once.
    fn adopt_pending(&mut self) {
        let Some(tx) = self.pending_payment.take() else {
            return;
        };
        if self.wallet.pending().is_none() {
            let _ = self.wallet.reserve(&tx);
        }
        self.retire_pending = true;
    }

    fn read_pages(&mut self, api: &str) -> Option<String> {
        for _ in 0..LIGHT_PAGE_BUDGET {
            let path = format!("/scan2?from={}&count={MAX_SCAN_BLOCKS}", self.wallet.next());
            let bytes = match api_bytes(api, &path, MAX_SCAN_PAGE_BYTES) {
                Ok(bytes) => bytes,
                Err(reason) => return Some(reason),
            };
            let page = match ScanPage2::decode(&bytes) {
                Ok(page) => page,
                Err(_) => return Some("the node sent an unreadable scan page".into()),
            };
            if page.network != self.wallet.network() {
                return Some("the node at --api runs another chain".into());
            }
            let empty = page.blocks.is_empty();
            if let Err(reason) = self.wallet.add_page_for(&self.accounts, &page) {
                return Some(format!("the node's scan page was refused: {reason}"));
            }
            if empty || self.wallet.in_sync() {
                return None;
            }
        }
        Some(format!(
            "the wallet read up to height {} and the node is at {}",
            self.wallet.height(),
            self.wallet.tip()
        ))
    }

    /// Seal and write the state, replacing the last one only once the new
    /// one is whole.
    fn save(&self) -> Result<(), String> {
        let plain = Zeroizing::new(self.wallet.encode().map_err(error)?);
        let cipher = Aes256Gcm::new_from_slice(self.key.as_ref()).map_err(error)?;
        let mut nonce = [0u8; 12];
        rand::rngs::OsRng.try_fill_bytes(&mut nonce).map_err(error)?;
        let sealed = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plain.as_slice(),
                    aad: &light_aad(self.chain_id),
                },
            )
            .map_err(|_| "the wallet state could not be sealed".to_string())?;
        let mut bytes = Vec::with_capacity(LIGHT_MAGIC.len() + nonce.len() + sealed.len());
        bytes.extend_from_slice(LIGHT_MAGIC);
        bytes.extend_from_slice(&nonce);
        bytes.extend_from_slice(&sealed);
        let mut temporary = self.path.as_os_str().to_os_string();
        temporary.push(".new");
        let temporary = PathBuf::from(temporary);
        fs::write(&temporary, &bytes).map_err(error)?;
        fs::rename(&temporary, &self.path).map_err(error)?;
        if self.retire_pending {
            let old = stark_cli::pending_path(&self.wallet_path);
            let mut done = old.as_os_str().to_os_string();
            done.push(".adopted");
            let _ = fs::rename(&old, PathBuf::from(done));
        }
        Ok(())
    }
}

/// `<name>.json.light`, beside the wallet.
fn light_path(wallet: &Path) -> PathBuf {
    let mut path = wallet.as_os_str().to_os_string();
    path.push(".light");
    PathBuf::from(path)
}

/// The state's key: of the seed, so only who opens the wallet reads it, and
/// a new password leaves it as it is.
fn light_key(seed: &[u8; 32], profile: Profile) -> Zeroizing<[u8; 32]> {
    let mut material = Zeroizing::new(Vec::with_capacity(32 + profile.chain_id.len()));
    material.extend_from_slice(seed);
    material.extend_from_slice(profile.chain_id.as_bytes());
    Zeroizing::new(blake3::derive_key(LIGHT_KEY_CONTEXT, &material))
}

fn light_aad(chain_id: &str) -> Vec<u8> {
    [LIGHT_MAGIC.as_slice(), chain_id.as_bytes()].concat()
}

/// The state at `path`: `None` if there is none, an error if it cannot be
/// opened with `key`.
fn read_light(path: &Path, key: &[u8; 32], profile: Profile) -> Result<Option<LightWallet>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let bytes = stark_cli::read_bounded(path, MAX_LIGHT_STATE_BYTES)?;
    let body = bytes
        .strip_prefix(LIGHT_MAGIC.as_slice())
        .filter(|body| body.len() > 12)
        .ok_or("not a sealed wallet state")?;
    let (nonce, sealed) = body.split_at(12);
    let cipher = Aes256Gcm::new_from_slice(key).map_err(error)?;
    let plain = Zeroizing::new(
        cipher
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: sealed,
                    aad: &light_aad(profile.chain_id),
                },
            )
            .map_err(|_| "the wallet state does not open with this wallet".to_string())?,
    );
    LightWallet::decode(&plain).map(Some).map_err(error)
}

/// One request to the node's API, which listens on loopback only.
fn api_call(
    base: &str,
    method: &str,
    path: &str,
    body: Option<(&str, &[u8])>,
    timeout: Duration,
) -> Result<(u16, Vec<u8>), String> {
    api_call_limited(base, method, path, body, timeout, MAX_API_REPLY)
}

/// The body of a `GET` that answers bytes (a scan page), up to `limit`. A
/// busy node answers 503 while both of its workers verify: a few retries
/// cover that.
fn api_bytes(base: &str, path: &str, limit: usize) -> Result<Vec<u8>, String> {
    for attempt in 0..5 {
        let (status, bytes) =
            api_call_limited(base, "GET", path, None, Duration::from_secs(120), limit)?;
        match status {
            200 => return Ok(bytes),
            503 if attempt < 4 => std::thread::sleep(Duration::from_secs(1)),
            _ => {
                let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                return api_ok(status, value).map(|_| Vec::new());
            }
        }
    }
    Err("the node API stayed busy".into())
}

fn api_call_limited(
    base: &str,
    method: &str,
    path: &str,
    body: Option<(&str, &[u8])>,
    timeout: Duration,
    limit: usize,
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
    // The head and the chunk lines on top of the body.
    let cap = limit.saturating_add(64 * 1024);
    (&mut stream)
        .take(cap as u64 + 1)
        .read_to_end(&mut reply)
        .map_err(|e| format!("node API {authority}: {e}"))?;
    if reply.len() > cap {
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

/// Publish `full` on the node of `api` (`POST /address`), so that its short
/// form finds it. The stamp is a few seconds of Argon2id here, not on the node.
fn publish(api: &str, full: Address) -> Result<Value, String> {
    let record = AddressRecord::new(&full).map_err(error)?;
    let body =
        serde_json::to_vec(&json!({"address":record.address,"stamp":record.stamp})).map_err(error)?;
    let (status, reply) = api_json(
        api,
        "POST",
        "/address",
        Some(("application/json", &body)),
        Duration::from_secs(60),
    )?;
    api_ok(status, reply)
}

/// A full address, or the full address the node finds for a short one. The
/// node cannot answer another address: the short one is its digest. An
/// integrated address is a short one with a customer tag, which comes back
/// too: the payment carries it in its memo.
fn resolve(text: &str, profile: Profile, api: &str) -> Result<(Address, Option<u64>), String> {
    let text = text.trim();
    if let Ok(full) = Address::decode(text, profile.network) {
        return Ok((full, None));
    }
    let (short, tag) = match IntegratedAddress::decode(text, profile.network) {
        Ok(integrated) => (integrated.short, Some(integrated.tag)),
        Err(_) => (
            ShortAddress::decode(text, profile.network).map_err(|_| {
                format!(
                    "--to is not a {} v2 address: full, short or integrated",
                    profile.name
                )
            })?,
            None,
        ),
    };
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
    Ok((full, tag))
}

/// What kind of address `text` is, without a node: full, short or
/// integrated (with its tag). `None` if it is none of them on `profile`.
fn parse_address(text: &str, profile: Profile) -> Option<Value> {
    let text = text.trim();
    if let Ok(full) = Address::decode(text, profile.network) {
        return Some(json!({"valid":true,"kind":"full","short_address":full.short().ok(),"tag":null}));
    }
    if let Ok(short) = ShortAddress::decode(text, profile.network) {
        return Some(json!({"valid":true,"kind":"short","short_address":short.encode(),"tag":null}));
    }
    IntegratedAddress::decode(text, profile.network).ok().map(|integrated| {
        json!({"valid":true,"kind":"integrated","short_address":integrated.short.encode(),
            "tag":integrated.tag.to_string()})
    })
}

/// Run one action and return its JSON reply.
async fn act(action: &str, args: &[String]) -> Result<Value, String> {
    let profile = Profile::named(required(args, "--network")?)?;
    // Needs no wallet: checks an address someone typed.
    if action == "parse-address" {
        let text = required(args, "--address")?;
        return Ok(parse_address(text, profile).unwrap_or_else(|| {
            json!({"valid":false,"network":profile.chain_id})
        }));
    }
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
        // A deposit address for one customer: the wallet's main address and
        // the customer's tag (`--tag N`, or a random one). Payers resolve its
        // short part from a node, so the wallet's address must be published.
        "integrated-address" => {
            let (name, path) = store.select(args)?;
            let (file, _) = stark_cli::read_wallet(&path)?;
            let full = Address::decode(&file.address, profile.network).map_err(error)?;
            let tag = match option(args, "--tag") {
                Some(text) => text
                    .parse::<u64>()
                    .map_err(|_| "--tag must be a whole number from 0 to 18446744073709551615")?,
                None => rand::rngs::OsRng.next_u64(),
            };
            let short = full.short_address().map_err(error)?;
            Ok(json!({"name":name,"integrated_address":IntegratedAddress::new(short, tag).encode(),
                "tag":tag.to_string(),"short_address":short.encode(),
                "note":"payers find this wallet's address from its short form: publish it once with publish-address"}))
        }
        // Point 4 of the 10/11 review: more receive addresses of the same
        // seed. Whoever pays two of them cannot tell they paid one wallet.
        "addresses" => {
            let (name, path) = store.select(args)?;
            let password = required_password(args)?;
            let accounts = blocking(move || {
                let (accounts, _) = stark_cli::open_wallet_accounts(&path, &password)?;
                accounts
                    .iter()
                    .map(|account| {
                        let address = account.address();
                        Ok(json!({"index":account.index,"address":address.encode().map_err(error)?,
                            "short_address":address.short().map_err(error)?}))
                    })
                    .collect::<Result<Vec<_>, String>>()
            })
            .await?;
            Ok(json!({"name":name,"addresses":accounts}))
        }
        "new-address" => {
            let (name, path) = store.select(args)?;
            let password = required_password(args)?;
            let (index, address) = blocking(move || {
                let _lock = stark_cli::wallet_lock(&path)?;
                stark_cli::add_address(&path, &password)
            })
            .await?;
            let full = address.encode().map_err(error)?;
            let short = address.short().map_err(error)?;
            // Published right away, so its short form works; a node that is
            // off leaves it for `publish-address --index` later.
            let published = blocking(move || publish(&api, address)).await;
            Ok(json!({"name":name,"index":index,"address":full,"short_address":short,
                "published":published.is_ok(),"publish_error":published.err(),
                "note":"payments to this address are found from now on; give it out after creating it"}))
        }
        "publish-address" => {
            let (name, path) = store.select(args)?;
            // `--index N`: another of the wallet's addresses (it needs the
            // password to derive it); without it, the main one, from the file.
            let full = match option(args, "--index") {
                None => {
                    let (file, _) = stark_cli::read_wallet(&path)?;
                    Address::decode(&file.address, profile.network).map_err(error)?
                }
                Some(index) => {
                    let index: usize = index.parse().map_err(|_| "--index must be a number")?;
                    let password = required_password(args)?;
                    let (mut accounts, _) = blocking(move || {
                        stark_cli::open_wallet_accounts(&path, &password)
                    })
                    .await?;
                    if index >= accounts.len() {
                        return Err(format!("the wallet has {} addresses", accounts.len()));
                    }
                    accounts.swap_remove(index).address()
                }
            };
            let short = full.short().map_err(error)?;
            let node = blocking(move || publish(&api, full)).await?;
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
            // `--incoming`: the half of the key that reads what arrives.
            let incoming = flag(args, "--incoming");
            let key = blocking(move || {
                let (account, _) = stark_cli::open_wallet(&path, &password)?;
                Ok(Zeroizing::new(if incoming {
                    account.incoming_view_key().map_err(error)?.encode()
                } else {
                    account.view_key().map_err(error)?.encode()
                }))
            })
            .await?;
            Ok(if incoming {
                json!({"name":name,"view_key":key.as_str(),"kind":"incoming",
                    "note":"whoever holds this key sees what this wallet receives, with amounts and memos; it does not see what the wallet spends or its balance, and it cannot spend"})
            } else {
                json!({"name":name,"view_key":key.as_str(),"kind":"full",
                    "note":"whoever holds this key sees every payment and the balance of this wallet; it cannot spend"})
            })
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
            let wallet = fs::canonicalize(&path).map_err(error)?;
            let action = action.to_string();
            blocking(move || {
                let _lock = stark_cli::wallet_lock(&wallet)?;
                let mut light = Light::open(&wallet, &password, profile)?;
                let lag = light.sync(&api);
                light.save()?;
                let figures = light.wallet.balance();
                let height = u64::from(light.wallet.height());
                Ok(match action.as_str() {
                    "balance" => json!({
                        "name":name,"height":height,"wallet_height":height,"tip":figures.tip,
                        "node_behind":lag.is_some(),"sync_error":lag,
                        "spendable_atoms":figures.spendable_atoms,
                        "immature_atoms":figures.immature_atoms,
                        "reserved_atoms":figures.reserved_atoms,
                        "spendable":format_zka_amount(figures.spendable_atoms),
                        "immature":format_zka_amount(figures.immature_atoms),
                        "max_sendable_atoms":figures.max_sendable_atoms,"notes":figures.notes,
                        "pending_payment":figures.pending,
                    }),
                    "history" => {
                        let history = light.wallet.history();
                        let count = history.len();
                        let entries: Vec<_> = history
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
                                    "confirmations":height.saturating_sub(u64::from(entry.height)) + 1,
                                    "memo":entry.memo,"tag":entry.tag().map(|tag| tag.to_string()),
                                })
                            })
                            .collect();
                        json!({"name":name,"height":height,"node_behind":lag.is_some(),"count":count,"transactions":entries})
                    }
                    _ => {
                        let amount = amount.expect("estimate-fee parsed its amount");
                        let mut reply = json!({
                            "name":name,"height":height,"amount_atoms":amount,
                            "max_sendable_atoms":figures.max_sendable_atoms,
                            "spendable_atoms":figures.spendable_atoms,
                        });
                        // From the notes no pending payment holds.
                        match light.wallet.quote_total(amount) {
                            Ok((fee, inputs, total)) => {
                                reply["enough"] = true.into();
                                reply["fee_atoms"] = fee.into();
                                reply["fee"] = format_zka_amount(fee).into();
                                reply["inputs"] = inputs.into();
                                reply["change_atoms"] = (total - amount - fee).into();
                            }
                            Err(reason) => {
                                reply["enough"] = false.into();
                                reply["reason"] = reason.to_string().into();
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
            let (to, tag) = {
                let (text, api) = (required(args, "--to")?.to_string(), api.clone());
                blocking(move || resolve(&text, profile, &api))
                    .await
                    .map_err(|e| format!("--to: {e}"))?
            };
            // An integrated address: its tag goes first in the memo, and the
            // payer's own memo, if any, on the next line.
            let memo = match tag {
                Some(tag) => Some(tag_memo(tag, memo.as_deref().unwrap_or_default()).map_err(error)?),
                None => memo,
            };
            let wallet = fs::canonicalize(&path).map_err(error)?;
            blocking(move || {
                let _lock = stark_cli::wallet_lock(&wallet)?;
                let mut light = Light::open(&wallet, &password, profile)?;
                let lag = light.sync(&api);
                light.save()?;
                if let Some(lag) = lag {
                    return Err(format!(
                        "the wallet's node is not in sync ({lag}); send when it is"
                    ));
                }
                let memo = memo.unwrap_or_default();
                let plan = light
                    .wallet
                    .plan_for(&light.accounts, to, amount, memo.as_bytes())
                    .map_err(error)?;
                let fee = plan.fee;
                let tx = plan.prove_for(&light.accounts).map_err(error)?;
                // Held before it leaves: a cut leaves its notes reserved, and
                // the engine frees them when it is mined or expires.
                light.wallet.reserve(&tx).map_err(error)?;
                light.save()?;
                let height = u64::from(light.wallet.height());
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
                        "tag":tag.map(|tag| tag.to_string()),
                    })),
                    // The node refused it: free the inputs for another try.
                    Ok((status, reply)) => {
                        light.wallet.release();
                        light.save()?;
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
        addresses: file.addresses,
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
        "restore" | "address" | "publish-address" | "new-address" => field("short_address"),
        "integrated-address" => format!(
            "{}\nCustomer tag: {}",
            field("integrated_address"),
            field("tag")
        ),
        "parse-address" => match (reply["valid"].as_bool(), reply["kind"].as_str()) {
            (Some(true), Some(kind)) => match reply["tag"].as_str() {
                Some(tag) => format!("Valid {kind} address, customer tag {tag}"),
                None => format!("Valid {kind} address"),
            },
            _ => "Not an address of this network".to_string(),
        },
        "addresses" => reply["addresses"]
            .as_array()
            .map(|list| {
                list.iter()
                    .map(|entry| entry["short_address"].as_str().unwrap_or_default().to_string())
                    .collect::<Vec<_>>()
                    .join("
")
            })
            .unwrap_or_default(),
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

/// El servidor JSON-RPC 2.0 para exchanges (`zoka quantic-pq-wallet-rpc`).
pub mod rpc;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quantic::{
        stark::protocol::ledger::{conventional_fee, ATOMS, PREMINE},
        stark_api, stark_node,
    };
    use std::sync::Arc;

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

    /// Point 4: more receive addresses of the same seed, written to the
    /// wallet file; the main one stays, and there are at most eight.
    #[tokio::test]
    async fn a_wallet_gets_more_addresses_and_keeps_its_main_one() {
        let dir = tempfile::tempdir().unwrap();
        let created = act("new", &with(dir.path(), &["--name", "alice"]))
            .await
            .unwrap();
        let main = created["address"].as_str().unwrap().to_string();
        let file = dir.path().join("wallets").join("alice.json");
        let before = fs::read_to_string(&file).unwrap();
        assert!(!before.contains("addresses"), "one address is written as before");

        // No node at this API: the address is made, and left to publish later.
        let closed = ["--name", "alice", "--api", "http://127.0.0.1:9"];
        let second = act("new-address", &with(dir.path(), &closed))
            .await
            .unwrap();
        assert_eq!(second["index"], 1);
        assert_eq!(second["published"], false);
        assert_ne!(second["address"], main.as_str());
        let listed = act("addresses", &with(dir.path(), &["--name", "alice"]))
            .await
            .unwrap();
        let list = listed["addresses"].as_array().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0]["address"], main.as_str());
        assert_eq!(list[1]["address"], second["address"]);
        let shown = act("address", &with(dir.path(), &["--name", "alice"]))
            .await
            .unwrap();
        assert_eq!(shown["address"], main.as_str());
        // The phrase restores the main one; the others are the same seed's.
        let (accounts, _) = stark_cli::open_wallet_accounts(&file, "a test password").unwrap();
        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[1].address().encode().unwrap(), second["address"].as_str().unwrap());

        // Up to eight; the others without publishing (a few seconds each).
        for _ in 2..crate::quantic::stark::protocol::light::MAX_ADDRESSES {
            stark_cli::add_address(&file, "a test password").unwrap();
        }
        assert!(stark_cli::add_address(&file, "a test password").is_err());
        assert!(act("new-address", &with(dir.path(), &closed)).await.is_err());
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
        assert_eq!(viewed["kind"], "full");
        // `--incoming` gives the half that reads what arrives: another text,
        // the same wallet, and not a key the full one's readers accept.
        let incoming = act(
            "show-view-key",
            &with(dir.path(), &["--name", "copy", "--incoming"]),
        )
        .await
        .unwrap();
        assert_eq!(incoming["kind"], "incoming");
        let text = incoming["view_key"].as_str().unwrap();
        let half = zoka_pq_pool::wallet::IncomingViewKey::decode(text, 3).unwrap();
        assert_eq!(half.address().encode().unwrap(), address);
        assert_eq!(key.incoming().unwrap().encode(), text);
        assert!(zoka_pq_pool::wallet::ViewKey::decode(text, 3).is_err());
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
    async fn balance_history_and_a_payment_run_on_the_light_engine() {
        let dir = tempfile::tempdir().unwrap();
        let created = act("new", &with(dir.path(), &["--name", "miner"]))
            .await
            .unwrap();
        let address = Address::decode(created["address"].as_str().unwrap(), 3).unwrap();
        let friend = act("new", &with(dir.path(), &["--name", "friend"]))
            .await
            .unwrap();

        // The wallet's node, with one block mined to the wallet.
        let storage =
            Arc::new(crate::storage::RocksDbStorage::new(dir.path().join("node")).unwrap());
        storage
            .put_raw(
                stark_api::DB_MARKER,
                Profile::REGTEST.chain_id.as_bytes().to_vec(),
            )
            .unwrap();
        let node = stark_api::NodeState::new(storage).unwrap();
        // Block 1 carries the premine, spendable at once; block 2 a coinbase
        // that matures two blocks later.
        let (first, _) = stark_node::mine_once(&node, address.clone()).await.unwrap();
        let (second, _) = stark_node::mine_once(&node, address.clone()).await.unwrap();
        // Its loopback API, as ZSilent's node serves it.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = format!("http://{}", listener.local_addr().unwrap());
        let served = node.clone();
        let server = tokio::spawn(async move {
            axum::serve(listener, stark_api::router(served)).await
        });

        // Without the node the wallet answers what it has, and says so.
        let alone = act(
            "balance",
            &with(dir.path(), &["--name", "miner", "--api", "http://127.0.0.1:9"]),
        )
        .await
        .unwrap();
        assert_eq!(alone["node_behind"], true);
        assert_eq!(alone["height"], 0);

        let synced = with(dir.path(), &["--name", "miner", "--api", &api]);
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

        // The state stays beside the wallet, sealed: the next call reads only
        // the blocks that are new.
        let sealed = dir.path().join("wallets").join("miner.json.light");
        assert!(fs::read(&sealed).unwrap().starts_with(LIGHT_MAGIC));
        stark_node::mine_once(&node, address.clone()).await.unwrap();
        let later = act("balance", &synced).await.unwrap();
        assert_eq!(later["height"], 3, "{later}");

        // A payment: proved here, held, taken by the node.
        let to = friend["address"].as_str().unwrap();
        let sent = act(
            "send",
            &[
                synced.clone(),
                args(&["--to", to, "--amount", "5", "--memo", "gracias"]),
            ]
            .concat(),
        )
        .await
        .unwrap();
        assert_eq!(sent["fee_atoms"], fee, "{sent}");
        let held = act("balance", &synced).await.unwrap();
        assert_eq!(held["pending_payment"], true, "{held}");
        assert_eq!(held["reserved_atoms"], PREMINE, "{held}");
        let second_payment = act(
            "estimate-fee",
            &[synced.clone(), args(&["--amount", "1"])].concat(),
        )
        .await
        .unwrap();
        assert_eq!(second_payment["enough"], false, "{second_payment}");

        // Mined: the friend has it, and the miner its change.
        stark_node::mine_once(&node, address.clone()).await.unwrap();
        let theirs = act(
            "balance",
            &with(dir.path(), &["--name", "friend", "--api", &api]),
        )
        .await
        .unwrap();
        assert_eq!(theirs["spendable_atoms"], 5 * ATOMS, "{theirs}");
        let mine = act("balance", &synced).await.unwrap();
        assert_eq!(mine["pending_payment"], false, "{mine}");
        assert_eq!(mine["reserved_atoms"], 0, "{mine}");
        assert!(
            mine["spendable_atoms"].as_u64().unwrap() >= PREMINE - 5 * ATOMS - fee,
            "{mine}"
        );
        let history = act("history", &synced).await.unwrap();
        assert_eq!(history["transactions"][0]["kind"], "sent", "{history}");
        assert_eq!(history["transactions"][0]["txid"], sent["txid"]);

        // A state that does not open (another key, damaged) is only a cache:
        // the wallet reads the chain again and answers the same.
        fs::write(&sealed, b"ZQLIGHT1 not a state").unwrap();
        let again = act("balance", &synced).await.unwrap();
        assert_eq!(again["spendable_atoms"], mine["spendable_atoms"], "{again}");
        server.abort();
    }

    /// The network upgrade of the narrow proof, rehearsed on a node: once the
    /// chain reaches the activation height, the same wallet proves its next
    /// payment with version 4 by itself, and the node takes and mines it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn after_the_activation_height_payments_prove_with_version_4() {
        use crate::quantic::stark::protocol::{
            ledger::{proof_version_for, REGTEST_PROOF_V4_HEIGHT},
            PROOF_V4,
        };
        let dir = tempfile::tempdir().unwrap();
        let created = act("new", &with(dir.path(), &["--name", "miner"]))
            .await
            .unwrap();
        let address = Address::decode(created["address"].as_str().unwrap(), 3).unwrap();
        let friend = act("new", &with(dir.path(), &["--name", "friend"]))
            .await
            .unwrap();
        let storage =
            Arc::new(crate::storage::RocksDbStorage::new(dir.path().join("node")).unwrap());
        storage
            .put_raw(
                stark_api::DB_MARKER,
                Profile::REGTEST.chain_id.as_bytes().to_vec(),
            )
            .unwrap();
        let node = stark_api::NodeState::new(storage).unwrap();
        // Up to the block before the switch: the next one proves with v4.
        for _ in 1..REGTEST_PROOF_V4_HEIGHT {
            stark_node::mine_once(&node, address.clone()).await.unwrap();
        }
        assert_eq!(proof_version_for(3, REGTEST_PROOF_V4_HEIGHT), PROOF_V4);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = format!("http://{}", listener.local_addr().unwrap());
        let served = node.clone();
        let server = tokio::spawn(async move {
            axum::serve(listener, stark_api::router(served)).await
        });
        let synced = with(dir.path(), &["--name", "miner", "--api", &api]);
        let to = friend["address"].as_str().unwrap();
        let sent = act(
            "send",
            &[synced.clone(), args(&["--to", to, "--amount", "7"])].concat(),
        )
        .await
        .unwrap();
        let txid: [u8; 32] = hex::decode(sent["txid"].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap();
        let proof = node
            .mempool
            .lock()
            .unwrap()
            .get(&txid)
            .expect("the node took the payment")
            .proof
            .clone();
        assert_eq!(proof.version, PROOF_V4);
        assert!(proof.proof.len() < 1_000_000, "{} bytes", proof.proof.len());
        stark_node::mine_once(&node, address.clone()).await.unwrap();
        let theirs = act(
            "balance",
            &with(dir.path(), &["--name", "friend", "--api", &api]),
        )
        .await
        .unwrap();
        assert_eq!(theirs["spendable_atoms"], 7 * ATOMS, "{theirs}");
        assert_eq!(theirs["height"], REGTEST_PROOF_V4_HEIGHT, "{theirs}");
        server.abort();
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

    #[test]
    fn parse_address_tells_full_short_and_integrated_apart() {
        let account = Account::from_seed(&[41; 32], Profile::REGTEST.network, 0).unwrap();
        let full = account.address();
        let short = full.short_address().unwrap();
        let integrated = IntegratedAddress::new(short, 9_007_199_254_740_993).encode();
        let check = |text: &str| parse_address(text, Profile::REGTEST);
        assert_eq!(check(&full.encode().unwrap()).unwrap()["kind"], "full");
        assert_eq!(check(&short.encode()).unwrap()["kind"], "short");
        let parsed = check(&integrated).unwrap();
        assert_eq!(parsed["kind"], "integrated");
        // Above 2^53: a string, so JavaScript does not round it.
        assert_eq!(parsed["tag"], "9007199254740993");
        assert_eq!(parsed["short_address"], short.encode().as_str());
        assert!(check("zqregtest2notanaddress").is_none());
        assert!(check(&integrated.replacen("zqregtesti2", "zqtesti2", 1)).is_none());
    }
}
