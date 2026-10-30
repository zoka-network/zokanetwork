//! Explicit-path node, mirror synchronization and broadcast tools for the STARK
//! v2 chains. The database marker names the chain; `--network` picks it for a
//! new database.
//!
//! A serving node keeps a session with every pinned peer (`--peer` files and
//! the manifest's seeds) and with up to eight peers learned through the
//! exchange in `stark_net`. All of them are dialed from the node's own server
//! endpoint.
//!
//! Checkpoints: every session first brings the newer of the two sides' latest
//! checkpoints to the other, then moves blocks, so a fresh checkpoint already
//! constrains what the pull imports. With `--authority DIR` the node is the
//! chain's checkpoint authority (the team's PC) and signs them itself.
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use iroh::{endpoint::presets, Endpoint, EndpointAddr, EndpointId};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use tokio::{sync::watch, task::JoinSet};
use zeroize::Zeroizing;

use super::{
    stark::{self, Profile},
    stark_api::{self, NodeState},
    stark_cli::{open_db, read_bounded, write_new},
    stark_directory::{
        self, AddressRecord, LOOKUPS_PER_CYCLE, PUSHES_PER_MINUTE, REPUBLISH_EVERY,
    },
    stark_net::{self, Client, Head, NodeDescriptor, ServerIdentity},
    stark_peers::{Manifest, PeerBook, MAX_MANIFEST_BYTES},
    stark_stratum::StarkBackend,
};
use crate::{
    checkpoint_authority::{
        self, Acceptance, AuthorityKeys, CheckpointSigner, SignedCheckpoint, AUTHORITY_KEY_FILE,
        AUTHORITY_MLDSA_KEY_FILE,
    },
    hybrid_signature::{load_mldsa_seed, HybridSigningKey},
    state::chain::{apply_block, ChainError},
    types::Block,
};
use stark::protocol::{
    transaction::{Transaction, MAX_TRANSACTION_BYTES},
    wallet::{Address, ShortAddress},
};

const IDENTITY_SCHEMA: &str = "zoka-quantic-stark-node-identity-v2";
const MAX_IDENTITY_BYTES: usize = 8 * 1024;
const MAX_DESCRIPTOR_BYTES: usize = 64 * 1024;
const PEER_BATCH: usize = 64;
const MAX_PEERS: usize = 8;
/// Outbound sessions with learned peers, besides the pinned ones.
const LEARNED_TARGET: usize = 8;
const EXCHANGE_INTERVAL: Duration = Duration::from_secs(600);
const MANAGER_TICK: Duration = Duration::from_secs(10);
const SAVE_INTERVAL: Duration = Duration::from_secs(300);
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
const RETRY_MIN: Duration = Duration::from_secs(5);
const RETRY_MAX: Duration = Duration::from_secs(300);
const AUTHORITY_TICK: Duration = Duration::from_secs(10);

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[derive(Default)]
struct Options(BTreeMap<String, Vec<String>>);
impl Options {
    fn parse(args: &[String], allowed: &[&str]) -> Result<Self, String> {
        let mut result = Self::default();
        let mut args = args.iter();
        while let Some(item) = args.next() {
            let (name, value) = match item.split_once('=') {
                Some((name, value)) => (name, value),
                None => (
                    item.as_str(),
                    args.next()
                        .ok_or_else(|| format!("missing value for {item}"))?
                        .as_str(),
                ),
            };
            if !allowed.contains(&name) || value.is_empty() || value.starts_with("--") {
                return Err(format!("invalid option {name}"));
            }
            let values = result.0.entry(name.to_owned()).or_default();
            if name != "--peer" && !values.is_empty() {
                return Err(format!("duplicate option {name}"));
            }
            values.push(value.to_owned());
        }
        if result.peers().len() > MAX_PEERS {
            return Err("at most eight --peer descriptors are allowed".into());
        }
        Ok(result)
    }
    fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name).and_then(|v| v.first()).map(String::as_str)
    }
    fn required(&self, name: &str) -> Result<&str, String> {
        self.get(name).ok_or_else(|| format!("missing {name}"))
    }
    fn peers(&self) -> &[String] {
        self.0.get("--peer").map(Vec::as_slice).unwrap_or(&[])
    }
    fn network(&self) -> Result<Option<Profile>, String> {
        self.get("--network").map(Profile::named).transpose()
    }
    /// On by default on the public testnet, where nobody has to open a port:
    /// relays carry what hole punching cannot. Regtest stays direct.
    fn relay(&self, profile: Profile) -> Result<bool, String> {
        match self.get("--relay") {
            None => Ok(profile == Profile::TESTNET),
            Some("none") => Ok(false),
            Some("N0") => Ok(true),
            _ => Err("--relay must be none or N0".into()),
        }
    }
    /// `builtin` (the manifest compiled in for a public chain), `none`, or
    /// the path of a manifest file.
    fn seeds(&self, profile: Profile) -> Result<Vec<NodeDescriptor>, String> {
        let manifest = match self.get("--seeds").unwrap_or("builtin") {
            "builtin" => Manifest::builtin(profile)?,
            "none" => None,
            path => Some(Manifest::parse(
                &read_bounded(Path::new(path), MAX_MANIFEST_BYTES)?,
                profile,
            )?),
        };
        manifest.map_or(Ok(Vec::new()), |manifest| manifest.seeds())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct IdentityFile {
    schema: String,
    chain_id: String,
    encrypted_seed_hex: String,
}

fn identity_file(
    path: &Path,
    profile: Profile,
    password: &str,
) -> Result<ServerIdentity, String> {
    if password.is_empty() {
        return Err("ZOKA_PQ_PASSWORD cannot be empty".into());
    }
    if path.exists() {
        let identity: IdentityFile =
            serde_json::from_slice(&read_bounded(path, MAX_IDENTITY_BYTES)?).map_err(err)?;
        if identity.schema != IDENTITY_SCHEMA {
            return Err("not a STARK v2 node identity file".into());
        }
        if identity.chain_id != profile.chain_id {
            return Err(format!(
                "this node identity belongs to {}, not {}",
                identity.chain_id, profile.chain_id
            ));
        }
        let encrypted = hex::decode(identity.encrypted_seed_hex).map_err(err)?;
        // Eight magic bytes, 16-byte salt, 12-byte nonce, 32-byte seed and
        // 16-byte GCM tag. The shared legacy decryptor also supports XOR;
        // reject truncated magic-bearing values before it can take that path.
        if encrypted.len() != 84 || !encrypted.starts_with(b"ZOKAENC1") {
            return Err("node identity requires authenticated encryption".into());
        }
        let plain = Zeroizing::new(
            crate::wallet::storage::decrypt_data(&encrypted, password).map_err(err)?,
        );
        let seed: &[u8; 32] = plain
            .as_slice()
            .try_into()
            .map_err(|_| "node identity seed length")?;
        ServerIdentity::from_seed(seed)
    } else {
        let mut seed = Zeroizing::new([0u8; 32]);
        rand::rngs::OsRng.fill_bytes(seed.as_mut());
        let identity = ServerIdentity::from_seed(&seed)?;
        let encrypted =
            crate::wallet::storage::encrypt_data(seed.as_ref(), password).map_err(err)?;
        let file = IdentityFile {
            schema: IDENTITY_SCHEMA.into(),
            chain_id: profile.chain_id.into(),
            encrypted_seed_hex: hex::encode(encrypted),
        };
        write_new(path, &serde_json::to_vec_pretty(&file).map_err(err)?)?;
        Ok(identity)
    }
}

fn identity_chain(path: &Path) -> Result<Profile, String> {
    let file: IdentityFile =
        serde_json::from_slice(&read_bounded(path, MAX_IDENTITY_BYTES)?).map_err(err)?;
    Profile::for_chain(&file.chain_id).ok_or_else(|| "node identity names an unknown chain".into())
}

fn descriptor(path: &Path) -> Result<NodeDescriptor, String> {
    let value: NodeDescriptor =
        serde_json::from_slice(&read_bounded(path, MAX_DESCRIPTOR_BYTES)?).map_err(err)?;
    value.validate()?;
    Ok(value)
}

fn tip(state: &NodeState) -> Result<Block, String> {
    if state.storage.get_chain_height().map_err(err)? == 0 {
        return Ok(state.profile.genesis_parent());
    }
    let hash = state.storage.get_last_block_hash().map_err(err)?;
    state
        .storage
        .get_block(&hash)
        .map_err(err)?
        .ok_or_else(|| "canonical STARK tip missing".into())
}

async fn with_state<T: Send + 'static>(
    state: &NodeState,
    action: impl FnOnce(&NodeState) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let permit = state.work.clone().acquire_owned().await.map_err(err)?;
    let state = state.clone();
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let _gate = state.gate.lock().map_err(|_| "node gate poisoned")?;
        action(&state)
    })
    .await
    .map_err(err)?
}

/// Caller holds the gate. Returns whether `block` became the canonical tip.
pub(crate) fn import_mined_locked(state: &NodeState, block: &Block) -> Result<bool, String> {
    let previous = tip(state)?;
    match apply_block(&state.storage, state.profile.chain_id, block) {
        Ok(_)
        | Err(ChainError::SideBranchNotHeavier { .. })
        | Err(ChainError::BlockAlreadyKnown(_)) => {}
        Err(error) => return Err(err(error)),
    }
    let current = tip(state)?;
    if previous.header.hash != current.header.hash {
        let ledger = stark::branch(&state.storage, state.profile, &current)?;
        state
            .mempool
            .lock()
            .map_err(|_| "mempool poisoned")?
            .revalidate(&ledger);
    }
    Ok(current.header.hash == block.header.hash)
}

pub(crate) async fn mine_once(state: &NodeState, to: Address) -> Result<(Block, bool), String> {
    let permit = state.work.clone().acquire_owned().await.map_err(err)?;
    let state = state.clone();
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let (parent, transactions) = {
            let _gate = state.gate.lock().map_err(|_| "node gate poisoned")?;
            (
                tip(&state)?,
                state
                    .mempool
                    .lock()
                    .map_err(|_| "mempool poisoned")?
                    .select(),
            )
        };
        // An arriving block may change the canonical tip while this proof is
        // built. Consensus still validates and stores the resulting side branch.
        let block = stark::mine(
            &state.storage,
            state.profile,
            &parent,
            &to,
            transactions,
            super::now_secs(),
        )?;
        let _gate = state.gate.lock().map_err(|_| "node gate poisoned")?;
        let canonical = import_mined_locked(&state, &block)?;
        Ok((block, canonical))
    })
    .await
    .map_err(err)?
}

async fn connect(peer: &NodeDescriptor, relay: bool) -> Result<Client, String> {
    if relay {
        Client::connect(peer, presets::N0).await
    } else {
        Client::connect(peer, presets::Minimal).await
    }
}

/// `--peer`, or else the chain's seeds, first reachable wins. The client has
/// its own endpoint: these commands serve nothing.
async fn connect_any(options: &Options, profile: Profile) -> Result<Client, String> {
    let relay = options.relay(profile)?;
    let candidates = match options.peers() {
        [] => options.seeds(profile)?,
        [one] => vec![descriptor(Path::new(one))?],
        _ => return Err("this command takes at most one --peer descriptor file".into()),
    };
    if candidates.is_empty() {
        return Err(format!(
            "no --peer given and {} lists no seed yet",
            profile.chain_id
        ));
    }
    same_chain(&candidates, profile)?;
    let mut last = String::new();
    for peer in &candidates {
        match connect(peer, relay).await {
            Ok(client) => return Ok(client),
            Err(reason) => last = reason,
        }
    }
    Err(format!("no peer reachable: {last}"))
}

async fn head(state: &NodeState) -> Result<Head, String> {
    with_state(state, stark_net::head).await
}

/// Bring the newer of the two latest checkpoints to the side that lacks it.
/// Runs before blocks move, so a fresh checkpoint already constrains the pull.
/// A checkpoint this node rejected is not fetched again from this session.
async fn exchange_checkpoint(
    client: &mut Client,
    state: &NodeState,
    offer: bool,
    refused: &mut Option<u64>,
) -> Result<Head, String> {
    let remote = client.head().await?;
    let local = with_state(state, |state| state.latest_checkpoint()).await?;
    let local_height = local.as_ref().map(|cp| cp.height);
    if remote.checkpoint > local_height && remote.checkpoint != *refused {
        if let Some(checkpoint) = client.checkpoint().await? {
            let height = checkpoint.height;
            let hash = checkpoint.block_hash.clone();
            match with_state(state, move |state| state.accept_checkpoint(&checkpoint)).await {
                Ok(Acceptance::Accepted) => println!(
                    "{}",
                    serde_json::json!({"checkpoint_accepted":height,"hash":hash,"from":client.remote_id().to_string()})
                ),
                Ok(Acceptance::AlreadyKnown) => {}
                Err(reason) => {
                    *refused = Some(height);
                    eprintln!("STARK checkpoint {height} from a peer refused: {reason}");
                }
            }
        }
    } else if offer && local_height > remote.checkpoint {
        let checkpoint = local.expect("a local checkpoint is higher");
        match client.submit_checkpoint(&checkpoint).await {
            Ok(()) => {}
            Err(reason) if rejected(&reason) => eprintln!("STARK checkpoint offer: {reason}"),
            Err(reason) => return Err(reason),
        }
    }
    Ok(remote)
}

#[derive(Default)]
struct PushCursor {
    last_height: u64,
    last_hash: String,
}

async fn canonical_hash(state: &NodeState, height: u64) -> Result<Option<String>, String> {
    with_state(state, move |state| {
        if height == 0 {
            return Ok(Some(state.profile.genesis_parent().header.hash));
        }
        state
            .storage
            .get_canonical_block_hash_at_height(height)
            .map_err(err)
    })
    .await
}

async fn push_blocks(
    client: &mut Client,
    state: &NodeState,
    cursor: &mut Option<PushCursor>,
    batch: usize,
) -> Result<(), String> {
    let remote = client.head().await?;
    let local = head(state).await?;
    if local.height <= remote.height {
        *cursor = None;
        return Ok(());
    }
    let resume = match cursor.as_ref() {
        Some(saved) => {
            canonical_hash(state, saved.last_height).await?.as_deref() == Some(&saved.last_hash)
        }
        None => false,
    };
    if !resume {
        let mut low = 0;
        let mut high = remote.height.min(local.height) + 1;
        while low + 1 < high {
            let mid = low + (high - low) / 2;
            let theirs = client
                .block_hash(mid)
                .await?
                .ok_or("peer chain changed during locator")?;
            if canonical_hash(state, mid).await?.as_deref() == Some(&theirs) {
                low = mid;
            } else {
                high = mid;
            }
        }
        *cursor = Some(PushCursor {
            last_height: low,
            last_hash: canonical_hash(state, low)
                .await?
                .ok_or("local locator missing")?,
        });
    }
    let cursor = cursor.as_mut().expect("cursor initialized");
    let end = local
        .height
        .min(cursor.last_height.saturating_add(batch as u64));
    for height in cursor.last_height + 1..=end {
        let block = with_state(state, move |state| {
            state
                .storage
                .get_canonical_block_by_height(height)
                .map_err(err)?
                .ok_or_else(|| "local block changed during push".into())
        })
        .await?;
        if block.header.prev_hash != cursor.last_hash {
            return Err("local branch changed during push".into());
        }
        client.submit_block(&block).await?;
        cursor.last_height = height;
        cursor.last_hash = block.header.hash;
    }
    Ok(())
}

async fn peer_cycle(
    client: &mut Client,
    state: &NodeState,
    cursor: &mut Option<PushCursor>,
    sent: &mut BTreeSet<[u8; 32]>,
) -> Result<(), String> {
    peer_cycle_limited(client, state, cursor, sent, PEER_BATCH).await
}

async fn peer_cycle_limited(
    client: &mut Client,
    state: &NodeState,
    cursor: &mut Option<PushCursor>,
    sent: &mut BTreeSet<[u8; 32]>,
    batch: usize,
) -> Result<(), String> {
    if batch == 0 || batch > PEER_BATCH {
        return Err("peer cycle batch must be 1..64".into());
    }
    client.pull_limited(state, batch).await?;
    push_blocks(client, state, cursor, batch).await?;
    // A partially sent fork has not necessarily become the peer's canonical
    // chain yet. Its mempool cannot validate our anchors until tips agree.
    // Keep this connection/cursor so forks longer than one batch make progress.
    if client.head().await?.hash != head(state).await?.hash {
        return Ok(());
    }
    let transactions = with_state(state, |state| {
        Ok(state
            .mempool
            .lock()
            .map_err(|_| "mempool poisoned")?
            .select())
    })
    .await?;
    let current: BTreeSet<_> = transactions.iter().map(Transaction::txid).collect();
    sent.retain(|id| current.contains(id));
    for transaction in transactions {
        let id = transaction.txid();
        if !sent.contains(&id) {
            match client.submit_tx(&transaction).await {
                Ok(_) => {
                    sent.insert(id);
                }
                Err(reason) if reason.contains("already in mempool") => {
                    sent.insert(id);
                }
                Err(reason) if reason.starts_with("STARK peer rejected request:") => {
                    // A valid peer may have mined or filled its pool between
                    // HEAD and this submission. Preserve fork progress.
                    eprintln!("STARK transaction relay: {reason}");
                }
                Err(reason) => return Err(reason),
            }
        }
    }
    Ok(())
}

/// What one outbound session did with the address directory.
#[derive(Default)]
struct AddressExchange {
    /// Records pushed; own ones leave this set every `REPUBLISH_EVERY`.
    sent: BTreeSet<[u8; 32]>,
    /// Wanted short addresses asked for in this round. A round ends when
    /// every one was asked, so each is asked again while it is wanted.
    asked: BTreeSet<[u8; 32]>,
    republish: Option<tokio::time::Instant>,
    /// Start of the current minute and the records pushed in it.
    window: Option<(tokio::time::Instant, u32)>,
}

impl AddressExchange {
    /// Count one push, if this minute's budget has room.
    fn push_allowed(&mut self, now: tokio::time::Instant) -> bool {
        let (start, count) = match self.window {
            Some((start, count)) if now.duration_since(start) < Duration::from_secs(60) => {
                (start, count)
            }
            _ => (now, 0),
        };
        let allowed = count < PUSHES_PER_MINUTE;
        self.window = Some((start, count + u32::from(allowed)));
        allowed
    }
}

/// Check the stamp with a crypto permit, outside the gate, and keep the
/// record. Returns whether it was new.
async fn store_record(state: &NodeState, record: AddressRecord) -> Result<bool, String> {
    let permit = state.work.clone().acquire_owned().await.map_err(err)?;
    let state = state.clone();
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        state
            .directory
            .store(&state.storage, state.profile.network, &record)
            .map(|(_, new)| new)
            .map_err(err)
    })
    .await
    .map_err(err)?
}

async fn stored_record(
    state: &NodeState,
    digest: [u8; 32],
) -> Result<Option<AddressRecord>, String> {
    let storage = state.storage.clone();
    let short = ShortAddress {
        network: state.profile.network,
        digest,
    };
    tokio::task::spawn_blocking(move || stark_directory::lookup(&storage, &short))
        .await
        .map_err(err)?
        .map(|found| found.map(|(_, record)| record))
        .map_err(err)
}

/// Ask the peer for the short addresses an API caller waits for, then push
/// this node's own records and the ones it stored lately, within budget.
async fn exchange_addresses(
    client: &mut Client,
    state: &NodeState,
    exchange: &mut AddressExchange,
) -> Result<(), String> {
    let network = state.profile.network;
    let wanted = state.directory.wanted();
    exchange.asked.retain(|digest| wanted.contains(digest));
    if wanted.iter().all(|digest| exchange.asked.contains(digest)) {
        exchange.asked.clear();
    }
    let ask: Vec<_> = wanted
        .into_iter()
        .filter(|digest| !exchange.asked.contains(digest))
        .take(LOOKUPS_PER_CYCLE)
        .collect();
    for digest in ask {
        exchange.asked.insert(digest);
        let record = match client.get_address(&digest).await {
            Ok(Some(record)) => record,
            Ok(None) => continue,
            Err(reason) if rejected(&reason) => {
                eprintln!("STARK address lookup: {reason}");
                continue;
            }
            Err(reason) => return Err(reason),
        };
        let from = client.remote_id().fmt_short().to_string();
        let answers = stark_directory::decode_full(&record.address, network)
            .ok()
            .and_then(|full| full.short_address().ok())
            .is_some_and(|short| short.digest == digest);
        if !answers {
            eprintln!("STARK address lookup: {from} answered with another address");
            continue;
        }
        if let Err(reason) = store_record(state, record).await {
            eprintln!("STARK address from {from}: {reason}");
        }
    }

    let storage = state.storage.clone();
    let own = tokio::task::spawn_blocking(move || stark_directory::local(&storage, network))
        .await
        .map_err(err)?
        .map_err(err)?;
    let now = tokio::time::Instant::now();
    if exchange.republish.is_none_or(|at| at <= now) {
        exchange.republish = Some(now + REPUBLISH_EVERY);
        for short in &own {
            exchange.sent.remove(&short.digest);
        }
    }
    let mut pending: Vec<_> = own.iter().map(|short| short.digest).collect();
    pending.extend(state.directory.relayed());
    let current: BTreeSet<_> = pending.iter().copied().collect();
    exchange.sent.retain(|digest| current.contains(digest));
    let mut seen = BTreeSet::new();
    pending.retain(|digest| !exchange.sent.contains(digest) && seen.insert(*digest));
    for digest in pending {
        if !exchange.push_allowed(now) {
            break;
        }
        exchange.sent.insert(digest);
        let Some(record) = stored_record(state, digest).await? else {
            continue;
        };
        match client.submit_address(&record).await {
            Ok(_) => {}
            Err(reason) if rejected(&reason) => eprintln!("STARK address push: {reason}"),
            Err(reason) => return Err(reason),
        }
    }
    Ok(())
}

async fn stopped(stop: &mut watch::Receiver<bool>) {
    if *stop.borrow() {
        return;
    }
    let _ = stop.changed().await;
}

/// What every outbound session shares: the node state, its server endpoint
/// (dials come from it, so peers see this node's endpoint id) and the
/// descriptor this node announces, if any.
#[derive(Clone)]
struct Network {
    state: NodeState,
    endpoint: Endpoint,
    own: Option<NodeDescriptor>,
}

fn rejected(reason: &str) -> bool {
    reason.starts_with("STARK peer rejected request:")
}

/// One authenticated outbound session: announce, then sync cycles with a
/// peer exchange every ten minutes. Ends with the peer's first transport
/// failure, or with `Ok` at shutdown.
async fn session(
    net: &Network,
    client: &mut Client,
    stop: &mut watch::Receiver<bool>,
) -> Result<(), String> {
    if let Some(own) = &net.own {
        match client.announce(own).await {
            Err(reason) if !rejected(&reason) => return Err(reason),
            Err(reason) => eprintln!("STARK announce: {reason}"),
            Ok(()) => {}
        }
    }
    let id = client.remote_id();
    let mut cursor = None;
    let mut sent = BTreeSet::new();
    let mut refused = None;
    let mut addresses = AddressExchange::default();
    let mut asked = net.state.directory.asked();
    let mut exchange = tokio::time::Instant::now();
    loop {
        // Addresses go before blocks: a lookup should not wait for a sync.
        tokio::select! {
            _ = stopped(stop) => return Ok(()),
            result = async {
                let remote = exchange_checkpoint(client, &net.state, true, &mut refused).await?;
                net.state.peers.book().observe(id, remote.height);
                exchange_addresses(client, &net.state, &mut addresses).await?;
                peer_cycle(client, &net.state, &mut cursor, &mut sent).await
            } => result?,
        }
        if exchange <= tokio::time::Instant::now() {
            exchange = tokio::time::Instant::now() + EXCHANGE_INTERVAL;
            let peers = tokio::select! {
                _ = stopped(stop) => return Ok(()),
                result = client.peers() => result,
            };
            match peers {
                Ok(peers) => {
                    let mut book = net.state.peers.book();
                    for peer in peers {
                        book.learn(peer);
                    }
                }
                Err(reason) if rejected(&reason) => eprintln!("STARK peer exchange: {reason}"),
                Err(reason) => return Err(reason),
            }
        }
        tokio::select! {
            _ = stopped(stop) => return Ok(()),
            _ = tokio::time::sleep(Duration::from_secs(5)) => {}
            Ok(()) = asked.changed() => {}
        }
    }
}

/// A `--peer` or manifest seed: redialed for as long as the node runs, with
/// a delay that doubles up to five minutes while it stays unreachable.
async fn pinned_loop(net: Network, peer: NodeDescriptor, mut stop: watch::Receiver<bool>) {
    let id = peer.endpoint_addr.id;
    let mut wait = RETRY_MIN;
    loop {
        let connected = tokio::select! { _ = stopped(&mut stop) => return, result = Client::connect_from(&net.endpoint, &peer) => result };
        match connected {
            Ok(mut client) => {
                wait = RETRY_MIN;
                net.state.peers.book().connected(id);
                let result = session(&net, &mut client, &mut stop).await;
                net.state.peers.book().disconnected(id);
                client.close().await;
                match result {
                    Ok(()) => return,
                    Err(reason) => eprintln!("STARK peer {}: {reason}; retrying", id.fmt_short()),
                }
            }
            Err(reason) => eprintln!(
                "STARK peer {} connection: {reason}; retrying in {} s",
                id.fmt_short(),
                wait.as_secs()
            ),
        }
        tokio::select! { _ = stopped(&mut stop) => return, _ = tokio::time::sleep(wait) => {} }
        wait = (wait * 2).min(RETRY_MAX);
    }
}

/// A session with a learned peer. Reports whether it authenticated, so the
/// manager can back off unreachable endpoints.
async fn learned(
    net: Network,
    peer: NodeDescriptor,
    mut stop: watch::Receiver<bool>,
) -> (EndpointId, bool) {
    let id = peer.endpoint_addr.id;
    let connected = tokio::select! { _ = stopped(&mut stop) => return (id, true), result = Client::connect_from(&net.endpoint, &peer) => result };
    let Ok(mut client) = connected else {
        return (id, false);
    };
    {
        let mut book = net.state.peers.book();
        book.verified(peer, super::now_secs());
        book.connected(id);
    }
    if let Err(reason) = session(&net, &mut client, &mut stop).await {
        eprintln!("STARK learned peer {}: {reason}", id.fmt_short());
    }
    net.state.peers.book().disconnected(id);
    client.close().await;
    (id, true)
}

/// Dial an announced or shared endpoint once. It joins the book only if it
/// authenticates with its pinned key and serves this chain.
async fn probe(net: Network, peer: NodeDescriptor) {
    let checked = tokio::time::timeout(PROBE_TIMEOUT, async {
        let mut client = Client::connect_from(&net.endpoint, &peer).await?;
        let head = client.head().await;
        client.close().await;
        head
    })
    .await;
    if let Ok(Ok(_)) = checked {
        net.state.peers.book().verified(peer, super::now_secs());
    }
}

fn save_book(state: &NodeState) {
    let snapshot = state.peers.book().snapshot();
    if let Err(reason) = PeerBook::save(snapshot, &state.storage) {
        eprintln!("STARK peer table: {reason}");
    }
}

/// Keeps up to `LEARNED_TARGET` sessions with learned peers, dials one
/// pending endpoint per tick and saves the book every five minutes.
async fn manager_loop(net: Network, mut stop: watch::Receiver<bool>) {
    let mut sessions = JoinSet::new();
    let mut busy = BTreeSet::new();
    let mut probing: Option<tokio::task::JoinHandle<()>> = None;
    let mut saved = tokio::time::Instant::now();
    let mut tick = tokio::time::interval(MANAGER_TICK);
    loop {
        tokio::select! {
            _ = stopped(&mut stop) => break,
            Some(done) = sessions.join_next() => {
                if let Ok((id, connected)) = done {
                    busy.remove(&id);
                    let mut book = net.state.peers.book();
                    if connected {
                        book.ended(id, super::now_secs());
                    } else {
                        book.failed(id, super::now_secs());
                    }
                }
                continue;
            }
            _ = tick.tick() => {}
        }
        let now = super::now_secs();
        while sessions.len() < LEARNED_TARGET {
            let Some(peer) = net.state.peers.book().candidate(now, &busy) else {
                break;
            };
            busy.insert(peer.endpoint_addr.id);
            sessions.spawn(learned(net.clone(), peer, stop.clone()));
        }
        if probing.as_ref().is_none_or(|task| task.is_finished()) {
            let next = net.state.peers.book().next_pending();
            probing = next.map(|peer| tokio::spawn(probe(net.clone(), peer)));
        }
        if saved.elapsed() >= SAVE_INTERVAL {
            save_book(&net.state);
            saved = tokio::time::Instant::now();
        }
    }
    if let Some(task) = probing {
        task.abort();
    }
    while sessions.join_next().await.is_some() {}
    save_book(&net.state);
}

async fn mining_loop(
    state: NodeState,
    address: Address,
    interval: Duration,
    mut stop: watch::Receiver<bool>,
) {
    loop {
        let result = tokio::select! { _ = stopped(&mut stop) => return, result = mine_once(&state, address.clone()) => result };
        match result {
            Ok((block, canonical)) => println!(
                "{}",
                serde_json::json!({"mined":block.header.hash,"height":block.header.block_height,"canonical":canonical})
            ),
            Err(reason) => eprintln!("STARK mining: {reason}"),
        }
        tokio::select! { _ = stopped(&mut stop) => return, _ = tokio::time::sleep(interval) => {} }
    }
}

/// The team's checkpoint keys from `dir` (the files `zoka checkpoint-keygen`
/// creates). Both halves must be the ones compiled for the chain: a node
/// would reject anything else, so a mismatch is an error, not a warning.
fn load_authority(dir: &Path, keys: &AuthorityKeys) -> Result<CheckpointSigner, String> {
    let ed25519 = checkpoint_authority::load_signing_key(&dir.join(AUTHORITY_KEY_FILE))?;
    if ed25519.verifying_key() != keys.ed25519 {
        return Err("the Ed25519 authority key is not the one compiled for this chain".into());
    }
    let seed = load_mldsa_seed(&dir.join(AUTHORITY_MLDSA_KEY_FILE))?;
    let hybrid = HybridSigningKey::from_parts(ed25519.clone(), seed.as_ref()).map_err(err)?;
    match &keys.mldsa {
        Some(expected) if hybrid.verifying_key().mldsa().to_bytes() == expected.to_bytes() => {}
        _ => return Err("the ML-DSA authority key is not the one compiled for this chain".into()),
    }
    let mut signer = CheckpointSigner::ed25519_only(ed25519);
    signer.hybrid = Some(hybrid);
    signer.legacy_v1 = false;
    Ok(signer)
}

/// The block to sign now, if any: every `AUTHORITY_SIGN_INTERVAL` blocks,
/// `AUTHORITY_CONFIRMATIONS` below the tip, only while the network confirms
/// the tip (`Peers::confirm_tip`) and the chain holds the latest checkpoint.
/// One that corrects the branch (signed by hand) is not signed over again
/// higher up: signing resumes once the node reorganizes. Hold `gate`.
fn sign_due(state: &NodeState, signer: &CheckpointSigner) -> Result<Option<SignedCheckpoint>, String> {
    let tip = state.storage.get_chain_height().map_err(err)?;
    if !state.peers.confirm_tip(tip) {
        return Ok(None);
    }
    let latest = state.latest_checkpoint()?;
    if let Some(latest) = &latest {
        if !state.follows(latest)? {
            return Ok(None);
        }
    }
    let Some(height) =
        checkpoint_authority::next_height_to_sign(tip, latest.as_ref().map(|cp| cp.height))
    else {
        return Ok(None);
    };
    let keys = state.authority.as_ref().ok_or("no checkpoint authority")?;
    let hash = state
        .storage
        .get_canonical_block_hash_at_height(height)
        .map_err(err)?
        .ok_or("canonical block to sign is missing")?;
    let checkpoint = signer
        .sign_all(keys, state.profile.chain_id, height, &hash)
        .into_iter()
        .next()
        .ok_or("nothing signed")?;
    state.accept_checkpoint(&checkpoint)?;
    Ok(Some(checkpoint))
}

async fn authority_loop(state: NodeState, signer: Arc<CheckpointSigner>, mut stop: watch::Receiver<bool>) {
    loop {
        tokio::select! { _ = stopped(&mut stop) => return, _ = tokio::time::sleep(AUTHORITY_TICK) => {} }
        let signer = signer.clone();
        match with_state(&state, move |state| sign_due(state, &signer)).await {
            Ok(Some(cp)) => println!(
                "{}",
                serde_json::json!({"checkpoint_signed":cp.height,"hash":cp.block_hash})
            ),
            Ok(None) => {}
            Err(reason) => eprintln!("STARK checkpoint signing: {reason}"),
        }
    }
}

/// The explorer's chart of the network: a sample every `HISTORY_EVERY` while
/// the public API is served (the first after one interval, once peers are up).
async fn history_loop(state: NodeState, mut stop: watch::Receiver<bool>) {
    loop {
        tokio::select! { _ = stopped(&mut stop) => return, _ = tokio::time::sleep(stark_api::HISTORY_EVERY) => {} }
        if let Err(reason) = with_state(&state, stark_api::record_network_sample).await {
            eprintln!("STARK network history: {reason}");
        }
    }
}

async fn new_state(path: PathBuf, wanted: Option<Profile>) -> Result<NodeState, String> {
    tokio::task::spawn_blocking(move || NodeState::new(Arc::new(open_db(&path, wanted)?.0)))
        .await
        .map_err(err)?
}

async fn serve(options: Options) -> Result<(), String> {
    let db = PathBuf::from(options.required("--db")?);
    let identity_path = PathBuf::from(options.required("--identity")?);
    let bind: std::net::SocketAddr = options
        .get("--api")
        .unwrap_or("127.0.0.1:9140")
        .parse()
        .map_err(err)?;
    if !bind.ip().is_loopback() {
        return Err("--api must bind a loopback address".into());
    }
    // The explorer's read-only API. ZOKA_PQ_PUBLIC_API lets the node a wallet
    // starts (ZSilent) serve it without a flag of its own.
    let public_bind = match options
        .get("--public-api")
        .map(str::to_string)
        .or_else(|| std::env::var("ZOKA_PQ_PUBLIC_API").ok().filter(|v| !v.trim().is_empty()))
    {
        Some(text) => {
            let public: std::net::SocketAddr = text.trim().parse().map_err(|_| {
                format!("--public-api (or ZOKA_PQ_PUBLIC_API) must be host:port, not {text}")
            })?;
            if !public.ip().is_loopback() {
                return Err(
                    "--public-api must bind a loopback address; a tunnel publishes it".into(),
                );
            }
            if public.port() == bind.port() {
                return Err("--public-api needs a port of its own, not the --api one".into());
            }
            Some(public)
        }
        None => None,
    };
    let interval: u64 = options
        .get("--interval")
        .unwrap_or("120")
        .parse()
        .map_err(err)?;
    if !(1..=86_400).contains(&interval) {
        return Err("--interval must be 1..86400 seconds".into());
    }
    let network = options.network()?;
    let authority_dir = options.get("--authority").map(PathBuf::from);
    let announce = match options.get("--announce") {
        None | Some("yes") => true,
        Some("no") => false,
        _ => return Err("--announce must be yes or no".into()),
    };
    let peers = options
        .peers()
        .iter()
        .map(|v| descriptor(Path::new(v)))
        .collect::<Result<Vec<_>, _>>()?;
    let password = Zeroizing::new(
        std::env::var("ZOKA_PQ_PASSWORD")
            .map_err(|_| "set ZOKA_PQ_PASSWORD for the encrypted node identity")?,
    );
    let state = new_state(db, network).await?;
    let profile = state.profile;
    let relay = options.relay(profile)?;
    let seeds = options.seeds(profile)?;
    same_chain(&peers, profile)?;
    let miner = options
        .get("--mine-to")
        .map(|v| Address::decode(v, profile.network).map_err(err))
        .transpose()?;
    let stratum: Option<std::net::SocketAddr> = options
        .get("--stratum")
        .map(|v| v.parse().map_err(err))
        .transpose()?;
    // RandomX chains are mined by XMRig through the Stratum; there --mine-to
    // only pays logins that are not addresses. The built-in CPU miner, which
    // solves one template to the end, is for the regtest PoW.
    let stratum = match (stratum, profile.xmrig_compatible()) {
        (Some(_), false) => {
            return Err(format!(
                "--stratum needs RandomX; {} uses the CPU test PoW (use --network testnet)",
                profile.chain_id
            ))
        }
        (None, true) if miner.is_some() => {
            return Err(format!(
                "{} is mined with XMRig: add --stratum HOST:PORT (--mine-to then pays logins that are not addresses)",
                profile.chain_id
            ))
        }
        (Some(bind), true) => Some((
            bind,
            Arc::new(StarkBackend::new(state.clone(), miner.clone())?),
        )),
        (None, _) => None,
    };
    let signer = match authority_dir {
        Some(dir) => {
            let keys = state.authority.clone().ok_or_else(|| {
                format!(
                    "{} has no compiled checkpoint authority (a local test network sets {} and {})",
                    profile.chain_id,
                    checkpoint_authority::AUTHORITY_PUBKEY_ENV,
                    checkpoint_authority::AUTHORITY_MLDSA_PUBKEY_ENV
                )
            })?;
            Some(Arc::new(
                tokio::task::spawn_blocking(move || load_authority(&dir, &keys))
                    .await
                    .map_err(err)??,
            ))
        }
        None => None,
    };
    let identity =
        tokio::task::spawn_blocking(move || identity_file(&identity_path, profile, &password))
            .await
            .map_err(err)??;
    let listener = tokio::net::TcpListener::bind(bind).await.map_err(err)?;
    let actual_bind = listener.local_addr().map_err(err)?;
    let public_listener = match public_bind {
        Some(public) => Some(tokio::net::TcpListener::bind(public).await.map_err(err)?),
        None => None,
    };
    let public_actual = match &public_listener {
        Some(listener) => Some(listener.local_addr().map_err(err)?),
        None => None,
    };
    let mut server = if relay {
        stark_net::serve(state.clone(), identity, presets::N0).await?
    } else {
        stark_net::serve(state.clone(), identity, presets::Minimal).await?
    };
    if relay
        && tokio::time::timeout(Duration::from_secs(45), server.endpoint.online())
            .await
            .is_err()
    {
        server.close().await;
        return Err("N0 relay did not become available before the startup deadline".into());
    }
    server.descriptor.endpoint_addr = server.endpoint.addr();
    // Minimal has no discovery/relay. Publish explicit local sockets even when
    // asynchronous endpoint address discovery has not emitted an update yet.
    for mut address in server.endpoint.bound_sockets() {
        if address.ip().is_unspecified() {
            address.set_ip(if address.is_ipv4() {
                std::net::Ipv4Addr::LOCALHOST.into()
            } else {
                std::net::Ipv6Addr::LOCALHOST.into()
            });
        }
        server.descriptor.endpoint_addr = server
            .descriptor
            .endpoint_addr
            .clone()
            .with_ip_addr(address);
    }
    if let Some(path) = options.get("--descriptor-out") {
        if let Err(reason) = serde_json::to_vec_pretty(&server.descriptor)
            .map_err(err)
            .and_then(|bytes| write_new(Path::new(path), &bytes))
        {
            server.close().await;
            return Err(reason);
        }
    }
    // A seed that lists itself does not dial itself; repeated pins collapse.
    let own_id = server.endpoint.id();
    let mut pinned: Vec<NodeDescriptor> = Vec::new();
    for peer in peers.into_iter().chain(seeds) {
        let id = peer.endpoint_addr.id;
        if id != own_id && !pinned.iter().any(|p| p.endpoint_addr.id == id) {
            pinned.push(peer);
        }
    }
    state.peers.book().configure(own_id, relay, &pinned);
    // With relays, address lookup resolves the id: announce nothing else.
    let own = announce.then(|| {
        let mut own = server.descriptor.clone();
        if relay {
            own.endpoint_addr = EndpointAddr::new(own_id);
        }
        own
    });
    let net = Network {
        state: state.clone(),
        endpoint: server.endpoint.clone(),
        own,
    };
    // The descriptor contains public transport and authentication keys only.
    println!(
        "{}",
        serde_json::json!({"chain_id":profile.chain_id,"api":format!("http://{actual_bind}"),
        "pow_algorithm":profile.pow_algorithm(),"xmrig_compatible":profile.xmrig_compatible(),
        "stratum":options.get("--stratum"),"relay":relay,"pinned_peers":pinned.len(),
        "public_api":public_actual.map(|addr| format!("http://{addr}")),
        "announce":announce,"checkpoint_authority":signer.is_some(),"descriptor":server.descriptor})
    );
    let (shutdown, stop) = watch::channel(false);
    let mut api_stop = stop.clone();
    let api_router = stark_api::router(state.clone());
    let mut api = tokio::spawn(async move {
        axum::serve(listener, api_router)
            .with_graceful_shutdown(async move {
                stopped(&mut api_stop).await;
            })
            .await
            .map_err(err)
    });
    let mut workers = Vec::new();
    if let Some(listener) = public_listener {
        let public_router = stark_api::public_router(state.clone());
        let mut public_stop = stop.clone();
        workers.push(tokio::spawn(async move {
            let served = axum::serve(listener, public_router)
                .with_graceful_shutdown(async move {
                    stopped(&mut public_stop).await;
                })
                .await;
            if let Err(reason) = served {
                eprintln!("STARK public API: {reason}");
            }
        }));
        workers.push(tokio::spawn(history_loop(state.clone(), stop.clone())));
    }
    for peer in pinned {
        workers.push(tokio::spawn(pinned_loop(net.clone(), peer, stop.clone())));
    }
    workers.push(tokio::spawn(manager_loop(net, stop.clone())));
    if let Some(signer) = signer {
        workers.push(tokio::spawn(authority_loop(state.clone(), signer, stop.clone())));
    }
    // The accept loop never ends by itself: it is aborted at shutdown.
    let mut stratum_task = None;
    if let Some((bind, backend)) = stratum {
        super::stratum::mark_configured(bind);
        stratum_task = Some(tokio::spawn(async move {
            if let Err(reason) = super::stratum::serve_backend(bind, backend).await {
                super::stratum::mark_failed(&reason);
                eprintln!("{reason}");
            }
        }));
    } else if let Some(address) = miner {
        workers.push(tokio::spawn(mining_loop(
            state.clone(),
            address,
            Duration::from_secs(interval),
            stop,
        )));
    }
    let mut api_finished = false;
    let result = tokio::select! {
        signal = tokio::signal::ctrl_c() => signal.map_err(err),
        result = &mut api => { api_finished = true; result.map_err(err).and_then(|value| value) },
    };
    let _ = shutdown.send(true);
    if let Some(task) = stratum_task {
        task.abort();
    }
    server.close().await;
    for worker in workers {
        let _ = worker.await;
    }
    if !api_finished {
        let _ = api.await;
    }
    // Cancelled async tasks may still own a blocking verifier/miner. Wait for
    // those permits before letting this command release its database handle.
    let drained = state
        .work
        .clone()
        .acquire_many_owned(stark_api::MAX_CRYPTO_WORKERS as u32)
        .await
        .map_err(err)?;
    drop(drained);
    result
}

fn same_chain(peers: &[NodeDescriptor], profile: Profile) -> Result<(), String> {
    match peers.iter().find(|peer| peer.chain_id != profile.chain_id) {
        Some(peer) => Err(format!(
            "peer descriptor is for {}, this database is {}",
            peer.chain_id, profile.chain_id
        )),
        None => Ok(()),
    }
}

async fn sync(options: Options) -> Result<(), String> {
    let db = PathBuf::from(options.required("--db")?);
    let maximum: usize = options
        .get("--max-blocks")
        .unwrap_or("8192")
        .parse()
        .map_err(err)?;
    if maximum == 0 || maximum > 1_000_000 {
        return Err("--max-blocks must be 1..1000000".into());
    }
    let state = new_state(db, options.network()?).await?;
    let mut client = connect_any(&options, state.profile).await?;
    let result = tokio::select! {
        signal = tokio::signal::ctrl_c() => signal.map_err(err),
        result = async {
            // A mirror follows the authority too: checkpoint first, then blocks.
            exchange_checkpoint(&mut client, &state, false, &mut None).await?;
            let mut total = 0;
            loop {
                let imported = client.pull_limited(&state, PEER_BATCH.min(maximum - total)).await?;
                total += imported;
                let local = head(&state).await?;
                println!("{}", serde_json::json!({"imported_blocks":total,"height":local.height,"hash":local.hash}));
                if imported == 0 { return Ok(()); }
                if total >= maximum {
                    let remote = client.head().await?;
                    if remote.hash == local.hash { return Ok(()); }
                    return Err("synchronization batch budget reached; rerun sync to continue".into());
                }
            }
        } => result,
    };
    client.close().await;
    let drained = state
        .work
        .clone()
        .acquire_many_owned(stark_api::MAX_CRYPTO_WORKERS as u32)
        .await
        .map_err(err)?;
    drop(drained);
    result
}

/// Bring a wallet mirror up to `peer`, the wallet's own node, over a direct
/// connection: checkpoint first, then at most `maximum` blocks. Returns the
/// mirror's head and whether it reached the node's tip. Prints no progress.
pub(crate) async fn follow(
    state: &NodeState,
    peer: &NodeDescriptor,
    maximum: usize,
) -> Result<(Head, bool), String> {
    if peer.chain_id != state.profile.chain_id {
        return Err("the node descriptor names another chain".into());
    }
    let mut client = Client::connect(peer, presets::Minimal).await?;
    let result = async {
        exchange_checkpoint(&mut client, state, false, &mut None).await?;
        let mut total = 0;
        while total < maximum {
            let imported = client
                .pull_limited(state, PEER_BATCH.min(maximum - total))
                .await?;
            if imported == 0 {
                break;
            }
            total += imported;
        }
        let local = head(state).await?;
        let reached = client.head().await?.hash == local.hash;
        Ok::<_, String>((local, reached))
    }
    .await;
    client.close().await;
    // Verifications still running hold permits: the caller drops the mirror
    // only after they end.
    drop(
        state
            .work
            .clone()
            .acquire_many_owned(stark_api::MAX_CRYPTO_WORKERS as u32)
            .await
            .map_err(err)?,
    );
    result
}

async fn broadcast(options: Options) -> Result<(), String> {
    let profile = match (options.network()?, options.peers()) {
        (Some(profile), _) => profile,
        (None, [one]) => Profile::for_chain(&descriptor(Path::new(one))?.chain_id)
            .ok_or("peer descriptor names an unknown chain")?,
        (None, _) => Profile::REGTEST,
    };
    let path = PathBuf::from(options.required("--tx")?);
    let transaction = tokio::task::spawn_blocking(move || {
        Transaction::decode(&read_bounded(&path, MAX_TRANSACTION_BYTES)?).map_err(err)
    })
    .await
    .map_err(err)??;
    let mut client = connect_any(&options, profile).await?;
    let result = tokio::select! {
        signal = tokio::signal::ctrl_c() => signal.map_err(err),
        result = client.submit_tx(&transaction) => result.map(|id| println!("{}", serde_json::json!({"accepted":true,"txid":hex::encode(id)}))),
    };
    client.close().await;
    result
}

/// Create or open the node identity and print what a manifest pins: the
/// endpoint id and the ML-DSA key id. Nothing secret is printed.
async fn identity(options: Options) -> Result<(), String> {
    let path = PathBuf::from(options.required("--identity")?);
    let profile = match options.network()? {
        Some(profile) => profile,
        None if path.exists() => identity_chain(&path)?,
        None => Profile::REGTEST,
    };
    let password = Zeroizing::new(
        std::env::var("ZOKA_PQ_PASSWORD")
            .map_err(|_| "set ZOKA_PQ_PASSWORD for the encrypted node identity")?,
    );
    let identity = tokio::task::spawn_blocking(move || identity_file(&path, profile, &password))
        .await
        .map_err(err)??;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "chain_id": profile.chain_id,
            "seed_entry": {
                "label": "seed1",
                "operator": "zoka-core",
                "endpoint_id": identity.endpoint_id().to_string(),
                "ml_dsa_key_id": identity.key_id(),
                "status": "active"
            }
        }))
        .map_err(err)?
    );
    Ok(())
}

/// Sign a checkpoint by hand, to move the network onto the right branch
/// (docs/CHECKPOINTS-AUTORITATIVOS.md, "Corregir a mano"). Only with the
/// compiled authority's keys; the result is verified before it is printed.
/// Deliver it with `POST /checkpoint` to a node, which relays it.
async fn checkpoint_sign(options: Options) -> Result<(), String> {
    let profile = options.network()?.unwrap_or(Profile::REGTEST);
    let dir = PathBuf::from(options.required("--authority")?);
    let height: u64 = options.required("--height")?.parse().map_err(err)?;
    let hash = options.required("--hash")?.trim().to_ascii_lowercase();
    let keys = checkpoint_authority::authority_keys_for_chain(profile.chain_id)
        .ok_or_else(|| format!("{} has no compiled checkpoint authority", profile.chain_id))?;
    let checkpoint = tokio::task::spawn_blocking(move || {
        let signer = load_authority(&dir, &keys)?;
        let checkpoint = signer
            .sign_all(&keys, profile.chain_id, height, &hash)
            .into_iter()
            .next()
            .ok_or("nothing signed")?;
        checkpoint
            .verify(&keys)
            .map_err(|e| format!("refusing to emit an invalid checkpoint: {e}"))?;
        Ok::<_, String>(checkpoint)
    })
    .await
    .map_err(err)??;
    let json = serde_json::to_vec(&checkpoint).map_err(err)?;
    if let Some(path) = options.get("--out") {
        write_new(Path::new(path), &json)?;
    }
    println!("{}", String::from_utf8(json).map_err(err)?);
    Ok(())
}

pub async fn run(args: &[String]) -> Result<(), String> {
    let command = args.first().map(String::as_str).unwrap_or("help");
    let rest = args.get(1..).unwrap_or(&[]);
    match command {
        "serve" => {
            serve(Options::parse(
                rest,
                &[
                    "--db",
                    "--identity",
                    "--api",
                    "--descriptor-out",
                    "--peer",
                    "--mine-to",
                    "--interval",
                    "--relay",
                    "--network",
                    "--stratum",
                    "--seeds",
                    "--announce",
                    "--authority",
                    "--public-api",
                ],
            )?)
            .await
        }
        "checkpoint-sign" => {
            checkpoint_sign(Options::parse(
                rest,
                &["--authority", "--network", "--height", "--hash", "--out"],
            )?)
            .await
        }
        "sync" => {
            sync(Options::parse(
                rest,
                &[
                    "--db",
                    "--peer",
                    "--relay",
                    "--max-blocks",
                    "--network",
                    "--seeds",
                ],
            )?)
            .await
        }
        "broadcast" => {
            broadcast(Options::parse(
                rest,
                &["--peer", "--tx", "--relay", "--network", "--seeds"],
            )?)
            .await
        }
        "identity" => identity(Options::parse(rest, &["--identity", "--network"])?).await,
        "help" | "--help" | "-h" => {
            println!("STARK v2 node. --network regtest (default for a new --db: CPU test PoW)\n\
or testnet (zoka-quantic-testnet-v2: RandomX, ASERT). An existing --db keeps its chain.\n\
  serve --db DIR --identity FILE [--network NAME] [--api 127.0.0.1:9140] [--descriptor-out FILE]\n\
        [--peer DESCRIPTOR ...] [--seeds builtin|none|MANIFEST] [--relay N0|none]\n\
        [--announce yes|no] [--authority DIR] [--mine-to ADDRESS] [--interval 120]\n\
        [--stratum 127.0.0.1:3333]   (testnet: XMRig logs in with its v2 address, full or a\n\
        short one the node knows; --mine-to pays logins that are not addresses. Regtest:\n\
        built-in miner.)\n\
        [--public-api 127.0.0.1:9181]   (an explorer's read-only API on its own loopback\n\
        port, for a tunnel to publish: GET only, any origin, its own rate; also\n\
        ZOKA_PQ_PUBLIC_API)\n\
  sync --db MIRROR_DIR [--peer DESCRIPTOR] [--network NAME] [--max-blocks 8192] [--relay N0|none]\n\
  broadcast --tx FILE [--peer DESCRIPTOR | --network NAME] [--relay N0|none]\n\
  identity --identity FILE [--network NAME]   (prints the endpoint id and ML-DSA key id a\n\
        manifest pins; creates the identity if the file does not exist)\n\
  checkpoint-sign --authority DIR --network NAME --height H --hash HASH [--out FILE]\n\
        (signs by hand to correct the branch; POST the output to a node's /checkpoint)\n\
--authority DIR makes the node the chain's checkpoint authority: DIR holds the team's\n\
checkpoint-authority.key and checkpoint-authority-mldsa.key, which must be the compiled keys.\n\
Short addresses: POST /address publishes a wallet's full address, which the node pushes to its\n\
peers; GET /address/SHORT answers the full one, asking the node's peers when it does not know it.\n\
Set ZOKA_PQ_PASSWORD to create/open the encrypted node identity. Files are never overwritten.\n\
Use a separate mirror database for wallet commands while the node database is open.\n\
Seeds come from the manifest compiled in for the chain (none on regtest); without --peer,\n\
sync and broadcast use them. The node learns more peers from its peers, dials each one and\n\
shares only those that authenticated. Testnet uses n0 relays and address lookup by default\n\
(no open port needed); regtest is direct. Ctrl+C stops the node.");
            Ok(())
        }
        _ => Err(format!("unknown STARK node command {command}; use help")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stark::protocol::wallet::Account;

    const P: Profile = Profile::REGTEST;
    const CHAIN: &str = P.chain_id;

    #[test]
    fn identity_is_encrypted_and_never_replaces_existing_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("node.identity");
        let created = identity_file(&path, P, "temporary test password").unwrap();
        let before = read_bounded(&path, MAX_IDENTITY_BYTES).unwrap();
        let stored: IdentityFile = serde_json::from_slice(&before).unwrap();
        assert_eq!(stored.schema, IDENTITY_SCHEMA);
        assert!(hex::decode(&stored.encrypted_seed_hex)
            .unwrap()
            .starts_with(b"ZOKAENC1"));
        let opened = identity_file(&path, P, "temporary test password").unwrap();
        assert_eq!(created.public_key(), opened.public_key());
        assert!(identity_file(&path, P, "wrong password").is_err());
        let err = identity_file(&path, Profile::TESTNET, "temporary test password")
            .err()
            .unwrap();
        assert!(err.contains("belongs to zoka-quantic-regtest-v2"), "{err}");
        assert_eq!(read_bounded(&path, MAX_IDENTITY_BYTES).unwrap(), before);

        let wallet = dir.path().join("wallet.json");
        let wallet_bytes = br#"{"schema":"zoka-quantic-stark-wallet-v2","chain_id":"zoka-quantic-regtest-v2","encrypted_seed_hex":"00"}"#;
        write_new(&wallet, wallet_bytes).unwrap();
        assert!(identity_file(&wallet, P, "test").is_err());
        assert_eq!(
            read_bounded(&wallet, MAX_IDENTITY_BYTES).unwrap(),
            wallet_bytes
        );

        let legacy = dir.path().join("legacy.identity");
        write_new(
            &legacy,
            &serde_json::to_vec(&IdentityFile {
                schema: IDENTITY_SCHEMA.into(),
                chain_id: CHAIN.into(),
                encrypted_seed_hex: "00".repeat(32),
            })
            .unwrap(),
        )
        .unwrap();
        assert!(identity_file(&legacy, P, "test")
            .err()
            .unwrap()
            .contains("authenticated encryption"));
        let truncated = dir.path().join("truncated.identity");
        let mut fake_ciphertext = b"ZOKAENC1".to_vec();
        fake_ciphertext.resize(32, 0);
        write_new(
            &truncated,
            &serde_json::to_vec(&IdentityFile {
                schema: IDENTITY_SCHEMA.into(),
                chain_id: CHAIN.into(),
                encrypted_seed_hex: hex::encode(fake_ciphertext),
            })
            .unwrap(),
        )
        .unwrap();
        assert!(identity_file(&truncated, P, "any password")
            .err()
            .unwrap()
            .contains("authenticated encryption"));
    }

    #[test]
    fn options_reject_ambiguity_and_excess_peers() {
        let args = |values: &[&str]| values.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(Options::parse(&args(&["--db", "a", "--db", "b"]), &["--db"]).is_err());
        assert!(Options::parse(&args(&["--db", "--peer"]), &["--db"]).is_err());
        assert!(Options::parse(&args(&["--unknown=1"]), &["--db"]).is_err());
        assert!(Options::parse(&args(&["--relay=N0"]), &["--relay"])
            .unwrap()
            .relay(P)
            .unwrap());
        let none = Options::parse(&args(&["--relay", "none"]), &["--relay"]).unwrap();
        assert!(!none.relay(Profile::TESTNET).unwrap());
        assert!(Options::default().relay(Profile::TESTNET).unwrap());
        assert!(!Options::default().relay(P).unwrap());
        assert!(Options::default().seeds(P).unwrap().is_empty());
        let none = Options::parse(&args(&["--seeds", "none"]), &["--seeds"]).unwrap();
        assert!(none.seeds(Profile::TESTNET).unwrap().is_empty());
        let peers: Vec<_> = (0..9)
            .flat_map(|_| ["--peer".to_string(), "peer.json".to_string()])
            .collect();
        assert!(Options::parse(&peers, &["--peer"]).is_err());
    }

    #[tokio::test]
    async fn peer_cycles_finish_a_fork_before_relaying_its_pending_payment() {
        use stark::protocol::{
            ledger::{ATOMS, PREMINE},
            model::Input,
            transaction::{transfer, Payment},
        };
        let dir = tempfile::tempdir().unwrap();
        let local = new_state(dir.path().join("local"), None).await.unwrap();
        let remote = new_state(dir.path().join("remote"), None).await.unwrap();
        let alice = Account::from_seed(&[91; 32], 3, 0).unwrap();
        let bob = Account::from_seed(&[92; 32], 3, 0).unwrap().address();
        let (genesis, _) = mine_once(&local, alice.address()).await.unwrap();
        let shared = genesis.clone();
        with_state(&remote, move |state| import_mined_locked(state, &shared))
            .await
            .unwrap();
        let (remote_branch, _) = mine_once(&remote, bob.clone()).await.unwrap();
        let (local_second, _) = mine_once(&local, alice.address()).await.unwrap();
        let (local_third, _) = mine_once(&local, alice.address()).await.unwrap();
        assert_ne!(remote_branch.header.hash, local_second.header.hash);

        // This payment's anchor is absent from the peer's current canonical
        // ledger. It becomes usable there only after adopting our complete fork.
        let payment_id = with_state(&local, move |state| {
            let ledger = stark::full_ledger(&state.storage, state.profile, &tip(state)?)?;
            let reward = stark::decode(&genesis)?.remove(0);
            let (note, _) = alice
                .receive(reward.statement.commitments[0], &reward.envelopes[0])
                .map_err(err)?;
            let payment = transfer(
                &alice,
                ledger.tree.root(),
                vec![Input {
                    key: alice.spend.clone(),
                    note,
                    path: ledger.tree.path(0).map_err(err)?,
                }],
                vec![
                    Payment::new(bob, ATOMS),
                    Payment::new(alice.address(), PREMINE - ATOMS - 200),
                ],
                200,
                20,
            )
            .map_err(err)?;
            let id = payment.txid();
            state
                .mempool
                .lock()
                .map_err(|_| "mempool poisoned")?
                .insert(&ledger, payment)?;
            Ok(id)
        })
        .await
        .unwrap();

        let server = stark_net::serve(
            remote.clone(),
            ServerIdentity::from_seed(&[93; 32]).unwrap(),
            presets::Minimal,
        )
        .await
        .unwrap();
        let mut peer = server.descriptor.clone();
        for mut socket in server.endpoint.bound_sockets() {
            socket.set_ip(if socket.is_ipv4() {
                std::net::Ipv4Addr::LOCALHOST.into()
            } else {
                std::net::Ipv6Addr::LOCALHOST.into()
            });
            peer.endpoint_addr = peer.endpoint_addr.with_ip_addr(socket);
        }
        let mut client = Client::connect(&peer, presets::Minimal).await.unwrap();
        let mut cursor = None;
        let mut sent = BTreeSet::new();

        // Only fork block 2 fits in this cycle. It remains a side branch and
        // the transaction must not be sent or cause a connection/cursor reset.
        peer_cycle_limited(&mut client, &local, &mut cursor, &mut sent, 1)
            .await
            .unwrap();
        assert_eq!(head(&remote).await.unwrap().hash, remote_branch.header.hash);
        assert!(remote
            .storage
            .block_exists(&local_second.header.hash)
            .unwrap());
        assert_eq!(cursor.as_ref().unwrap().last_height, 2);
        assert_eq!(cursor.as_ref().unwrap().last_hash, local_second.header.hash);
        assert!(remote.mempool.lock().unwrap().is_empty());
        assert!(sent.is_empty());

        // The same connection/cursor resumes at block 3. Once the peer switches
        // to this complete fork, its anchored payment enters the remote mempool.
        peer_cycle_limited(&mut client, &local, &mut cursor, &mut sent, 1)
            .await
            .unwrap();
        assert_eq!(head(&remote).await.unwrap().hash, local_third.header.hash);
        assert_eq!(cursor.as_ref().unwrap().last_height, 3);
        assert!(sent.contains(&payment_id));
        assert_eq!(
            remote.mempool.lock().unwrap().select()[0].txid(),
            payment_id
        );
        client.close().await;
        server.close().await;
    }

    #[tokio::test]
    async fn mining_worker_uses_shared_state_and_accepts_valid_side_branches() {
        let dir = tempfile::tempdir().unwrap();
        let state = new_state(dir.path().join("chain"), None).await.unwrap();
        let alice = Account::from_seed(&[81; 32], 3, 0).unwrap().address();
        let bob = Account::from_seed(&[82; 32], 3, 0).unwrap().address();
        let (genesis, canonical) = mine_once(&state, alice.clone()).await.unwrap();
        assert!(canonical);
        let (main, canonical) = mine_once(&state, alice).await.unwrap();
        assert!(canonical);
        assert_eq!(main.header.block_height, 2);
        let storage = state.storage.clone();
        let side = tokio::task::spawn_blocking(move || {
            stark::mine(&storage, P, &genesis, &bob, vec![], super::super::now_secs()).unwrap()
        })
        .await
        .unwrap();
        let hash = side.header.hash.clone();
        let canonical = with_state(&state, move |state| import_mined_locked(state, &side))
            .await
            .unwrap();
        assert!(!canonical);
        assert!(state.storage.block_exists(&hash).unwrap());
        assert_eq!(head(&state).await.unwrap().hash, main.header.hash);
        assert!(state.mempool.lock().unwrap().is_empty());
        assert_eq!(
            state.work.available_permits(),
            stark_api::MAX_CRYPTO_WORKERS
        );
    }

    /// A node on loopback with its pinned loops, peer manager and, given a
    /// signer, the checkpoint authority running.
    async fn start_node(
        state: NodeState,
        seed: u8,
        pins: Vec<NodeDescriptor>,
        stop: &watch::Receiver<bool>,
        signer: Option<Arc<CheckpointSigner>>,
    ) -> (
        Network,
        stark_net::Server,
        NodeDescriptor,
        Vec<tokio::task::JoinHandle<()>>,
    ) {
        let server = stark_net::serve(
            state.clone(),
            ServerIdentity::from_seed(&[seed; 32]).unwrap(),
            presets::Minimal,
        )
        .await
        .unwrap();
        let mut own = server.descriptor.clone();
        own.endpoint_addr = EndpointAddr::new(server.endpoint.id());
        for mut socket in server.endpoint.bound_sockets() {
            socket.set_ip(if socket.is_ipv4() {
                std::net::Ipv4Addr::LOCALHOST.into()
            } else {
                std::net::Ipv6Addr::LOCALHOST.into()
            });
            own.endpoint_addr = own.endpoint_addr.with_ip_addr(socket);
        }
        state
            .peers
            .book()
            .configure(server.endpoint.id(), false, &pins);
        let net = Network {
            state,
            endpoint: server.endpoint.clone(),
            own: Some(own.clone()),
        };
        let mut tasks: Vec<_> = pins
            .into_iter()
            .map(|peer| tokio::spawn(pinned_loop(net.clone(), peer, stop.clone())))
            .collect();
        tasks.push(tokio::spawn(manager_loop(net.clone(), stop.clone())));
        if let Some(signer) = signer {
            tasks.push(tokio::spawn(authority_loop(
                net.state.clone(),
                signer,
                stop.clone(),
            )));
        }
        (net, server, own, tasks)
    }

    async fn node_state(dir: &Path, name: &str, keys: Option<&AuthorityKeys>) -> NodeState {
        let mut state = new_state(dir.join(name), None).await.unwrap();
        state.authority = keys.cloned();
        state
    }

    /// Throwaway team keys, written the way `zoka checkpoint-keygen` does.
    fn test_authority(dir: &Path) -> AuthorityKeys {
        let ed25519 =
            checkpoint_authority::generate_signing_key(&dir.join(AUTHORITY_KEY_FILE)).unwrap();
        let mldsa =
            crate::hybrid_signature::generate_mldsa_seed(&dir.join(AUTHORITY_MLDSA_KEY_FILE))
                .unwrap();
        AuthorityKeys {
            ed25519: checkpoint_authority::parse_public_key(&ed25519).unwrap(),
            mldsa: Some(checkpoint_authority::parse_mldsa_public_key(&mldsa).unwrap()),
        }
    }

    async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
        while !check() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for {what}"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_node_that_knows_one_peer_finds_the_rest_and_follows_their_chain() {
        let dir = tempfile::tempdir().unwrap();
        let (shutdown, stop) = watch::channel(false);
        let alice = Account::from_seed(&[94; 32], 3, 0).unwrap().address();
        let state = node_state(dir.path(), "a", None).await;
        let (a, a_server, a_own, mut tasks) = start_node(state, 101, vec![], &stop, None).await;
        mine_once(&a.state, alice.clone()).await.unwrap();
        let state = node_state(dir.path(), "b", None).await;
        let (b, b_server, b_own, more) =
            start_node(state, 102, vec![a_own.clone()], &stop, None).await;
        tasks.extend(more);
        eventually("B's session with A", || b.state.peers.book().outbound_len() == 1).await;

        // C is given only B. B shares A (pinned, in session); C dials A, and
        // A learns B and C from their announces, after dialing each back.
        let state = node_state(dir.path(), "c", None).await;
        let (c, c_server, _, more) = start_node(state, 103, vec![b_own], &stop, None).await;
        tasks.extend(more);
        eventually("C to find and dial A", || {
            let book = c.state.peers.book();
            book.known_len() == 1 && book.outbound_len() == 2
        })
        .await;
        // C shares A (learned and dialed) and B (pinned, in session).
        let shared = c.state.peers.book().share(16, super::super::now_secs());
        assert_eq!(shared.len(), 2);
        assert!(shared.contains(&a_own));
        eventually("A to know B and C", || a.state.peers.book().known_len() == 2).await;

        let (second, _) = mine_once(&a.state, alice).await.unwrap();
        eventually("C to follow A", || {
            c.state.storage.get_last_block_hash().unwrap() == second.header.hash
        })
        .await;

        let _ = shutdown.send(true);
        for task in tasks {
            task.await.unwrap();
        }
        // The manager saved its book on the way out: it survives a restart.
        let saved = PeerBook::load(&c.state.storage, P);
        assert_eq!(saved.known_len(), 1);
        for server in [a_server, b_server, c_server] {
            server.close().await;
        }
    }

    #[test]
    fn the_authority_signs_only_with_the_compiled_keys() {
        let dir = tempfile::tempdir().unwrap();
        let team = dir.path().join("team");
        let keys = test_authority(&team);
        let signer = load_authority(&team, &keys).unwrap();
        let checkpoint = signer.sign_all(&keys, CHAIN, 3, &"ab".repeat(32)).remove(0);
        assert_eq!(
            checkpoint.version,
            checkpoint_authority::SIGNED_CHECKPOINT_HYBRID_VERSION
        );
        checkpoint.verify(&keys).unwrap();
        let other = test_authority(&dir.path().join("other"));
        let refused = load_authority(&team, &other).err().unwrap();
        assert!(refused.contains("Ed25519"), "{refused}");
        let mixed = AuthorityKeys {
            ed25519: keys.ed25519,
            mldsa: other.mldsa.clone(),
        };
        let refused = load_authority(&team, &mixed).err().unwrap();
        assert!(refused.contains("ML-DSA"), "{refused}");
        assert!(load_authority(&dir.path().join("missing"), &keys).is_err());
    }

    #[tokio::test]
    async fn a_checkpoint_on_another_branch_pauses_mining_until_the_node_moves_to_it() {
        let dir = tempfile::tempdir().unwrap();
        let keys = test_authority(&dir.path().join("team"));
        let signer = load_authority(&dir.path().join("team"), &keys).unwrap();
        let state = node_state(dir.path(), "chain", Some(&keys)).await;
        let alice = Account::from_seed(&[95; 32], 3, 0).unwrap().address();
        let bob = Account::from_seed(&[96; 32], 3, 0).unwrap().address();
        mine_once(&state, alice.clone()).await.unwrap();
        mine_once(&state, alice.clone()).await.unwrap();
        let mine_on = |parent: Block, to: Address| {
            let storage = state.storage.clone();
            tokio::task::spawn_blocking(move || {
                stark::mine(&storage, P, &parent, &to, vec![], super::super::now_secs())
            })
        };
        // A lighter branch from the genesis parent, then a checkpoint on it.
        let side = mine_on(P.genesis_parent(), bob.clone()).await.unwrap().unwrap();
        let shared = side.clone();
        assert!(!with_state(&state, move |state| import_mined_locked(state, &shared))
            .await
            .unwrap());
        let checkpoint = signer.sign_all(&keys, CHAIN, 1, &side.header.hash).remove(0);
        let accepted = checkpoint.clone();
        with_state(&state, move |state| state.accept_checkpoint(&accepted))
            .await
            .unwrap();
        let paused = mine_once(&state, alice.clone()).await.unwrap_err();
        assert!(paused.contains("mining paused"), "{paused}");

        // The checkpointed branch wins with equal work as soon as it grows.
        let next = mine_on(side, bob).await.unwrap().unwrap();
        let shared = next.clone();
        assert!(with_state(&state, move |state| import_mined_locked(state, &shared))
            .await
            .unwrap());
        assert!(with_state(&state, move |state| state.follows(&checkpoint))
            .await
            .unwrap());
        let (resumed, canonical) = mine_once(&state, alice).await.unwrap();
        assert!(canonical);
        assert_eq!(resumed.header.prev_hash, next.header.hash);
    }

    /// The team's node signs once a peer confirms its tip; a follower gets the
    /// checkpoint; a node that mined a heavier chain alone obeys it and moves
    /// to the authority's lighter chain.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_isolated_heavier_chain_yields_to_the_authority_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let team = dir.path().join("team");
        let keys = test_authority(&team);
        let signer = Arc::new(load_authority(&team, &keys).unwrap());
        let (shutdown, stop) = watch::channel(false);
        let alice = Account::from_seed(&[97; 32], 3, 0).unwrap().address();
        let carol = Account::from_seed(&[98; 32], 3, 0).unwrap().address();

        let state = node_state(dir.path(), "a", Some(&keys)).await;
        for _ in 0..8 {
            mine_once(&state, alice.clone()).await.unwrap();
        }
        let first = state.storage.get_canonical_block_hash_at_height(1).unwrap();
        let (a, a_server, a_own, mut tasks) =
            start_node(state, 111, vec![], &stop, Some(signer)).await;
        // Alone it signs nothing: it would bind the network to its own branch.
        tokio::time::sleep(AUTHORITY_TICK + Duration::from_secs(2)).await;
        assert!(a.state.latest_checkpoint().unwrap().is_none());

        let state = node_state(dir.path(), "b", Some(&keys)).await;
        let (b, b_server, _, more) =
            start_node(state, 112, vec![a_own.clone()], &stop, None).await;
        tasks.extend(more);
        // Tip 8, so the block 6 below it: height 2.
        eventually("B to receive A's checkpoint", || {
            b.state
                .latest_checkpoint()
                .unwrap()
                .is_some_and(|cp| cp.height == 2)
        })
        .await;

        let state = node_state(dir.path(), "c", Some(&keys)).await;
        for _ in 0..12 {
            mine_once(&state, carol.clone()).await.unwrap();
        }
        let (c, c_server, _, more) = start_node(state, 113, vec![a_own], &stop, None).await;
        tasks.extend(more);
        eventually("C to reorganize onto A's chain", || {
            c.state.storage.get_last_block_hash().unwrap()
                == a.state.storage.get_last_block_hash().unwrap()
        })
        .await;
        assert_eq!(c.state.storage.get_chain_height().unwrap(), 8);
        assert_eq!(a.state.storage.get_canonical_block_hash_at_height(1).unwrap(), first);
        assert_eq!(c.state.latest_checkpoint().unwrap().unwrap().height, 2);

        let _ = shutdown.send(true);
        for task in tasks {
            task.await.unwrap();
        }
        for server in [a_server, b_server, c_server] {
            server.close().await;
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_short_address_published_on_one_node_resolves_on_another() {
        use axum::{
            body::{to_bytes, Body},
            http::{Request, StatusCode},
        };
        use tower::ServiceExt;

        let dir = tempfile::tempdir().unwrap();
        let (shutdown, stop) = watch::channel(false);
        // A is the seed. B and C only dial it, as nodes behind NAT do.
        let state = node_state(dir.path(), "a", None).await;
        let (a, a_server, a_own, mut tasks) = start_node(state, 111, vec![], &stop, None).await;
        let state = node_state(dir.path(), "b", None).await;
        let (b, b_server, _, more) =
            start_node(state, 112, vec![a_own.clone()], &stop, None).await;
        tasks.extend(more);
        let state = node_state(dir.path(), "c", None).await;
        let (c, c_server, _, more) = start_node(state, 113, vec![a_own], &stop, None).await;
        tasks.extend(more);

        // Bob publishes on B (what `POST /address` does); B pushes it to A.
        let bob = Account::from_seed(&[97; 32], 3, 0).unwrap().address();
        let short = bob.short_address().unwrap();
        let record = AddressRecord::new(&bob).unwrap();
        b.state.directory.store(&b.state.storage, 3, &record).unwrap();
        stark_directory::mark_local(&b.state.storage, &short).unwrap();
        eventually("A to receive Bob's address", || {
            stark_directory::lookup(&a.state.storage, &short)
                .unwrap()
                .is_some()
        })
        .await;

        // C never heard of it: its API asks A through C's session.
        eventually("C's session with A", || c.state.peers.book().outbound_len() == 1).await;
        let request = Request::builder()
            .uri(format!("/address/{}", short.encode()))
            .body(Body::empty())
            .unwrap();
        let response = stark_api::router(c.state.clone()).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1 << 20).await.unwrap())
                .unwrap();
        assert_eq!(body["address"], bob.encode().unwrap());
        assert_eq!(
            stark_directory::lookup(&c.state.storage, &short).unwrap(),
            Some((bob, record))
        );

        let _ = shutdown.send(true);
        for task in tasks {
            task.await.unwrap();
        }
        for server in [a_server, b_server, c_server] {
            server.close().await;
        }
    }
}
