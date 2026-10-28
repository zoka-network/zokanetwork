//! Explicit-path node, mirror synchronization and broadcast tools for the STARK
//! v2 chains. The database marker names the chain; `--network` picks it for a
//! new database.
//!
//! A serving node keeps a session with every pinned peer (`--peer` files and
//! the manifest's seeds) and with up to eight peers learned through the
//! exchange in `stark_net`. All of them are dialed from the node's own server
//! endpoint.
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
    stark_net::{self, Client, Head, NodeDescriptor, ServerIdentity},
    stark_peers::{Manifest, PeerBook, MAX_MANIFEST_BYTES},
    stark_stratum::StarkBackend,
};
use crate::{
    state::chain::{apply_block, ChainError},
    types::Block,
};
use stark::protocol::{
    transaction::{Transaction, MAX_TRANSACTION_BYTES},
    wallet::Address,
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

async fn mine_once(state: &NodeState, to: Address) -> Result<(Block, bool), String> {
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
    with_state(state, |state| {
        let block = tip(state)?;
        Ok(Head {
            chain_id: state.profile.chain_id.into(),
            height: block.header.block_height,
            hash: block.header.hash,
        })
    })
    .await
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
    let mut cursor = None;
    let mut sent = BTreeSet::new();
    let mut exchange = tokio::time::Instant::now();
    loop {
        tokio::select! {
            _ = stopped(stop) => return Ok(()),
            result = peer_cycle(client, &net.state, &mut cursor, &mut sent) => result?,
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
        tokio::select! { _ = stopped(stop) => return Ok(()), _ = tokio::time::sleep(Duration::from_secs(5)) => {} }
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
    let interval: u64 = options
        .get("--interval")
        .unwrap_or("120")
        .parse()
        .map_err(err)?;
    if !(1..=86_400).contains(&interval) {
        return Err("--interval must be 1..86400 seconds".into());
    }
    let network = options.network()?;
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
    let identity =
        tokio::task::spawn_blocking(move || identity_file(&identity_path, profile, &password))
            .await
            .map_err(err)??;
    let listener = tokio::net::TcpListener::bind(bind).await.map_err(err)?;
    let actual_bind = listener.local_addr().map_err(err)?;
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
        "announce":announce,"descriptor":server.descriptor})
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
    for peer in pinned {
        workers.push(tokio::spawn(pinned_loop(net.clone(), peer, stop.clone())));
    }
    workers.push(tokio::spawn(manager_loop(net, stop.clone())));
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
                ],
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
        [--announce yes|no] [--mine-to ADDRESS] [--interval 120]\n\
        [--stratum 127.0.0.1:3333]   (testnet: XMRig logs in with its v2 address;\n\
        --mine-to pays logins that are not addresses. Regtest: built-in miner.)\n\
  sync --db MIRROR_DIR [--peer DESCRIPTOR] [--network NAME] [--max-blocks 8192] [--relay N0|none]\n\
  broadcast --tx FILE [--peer DESCRIPTOR | --network NAME] [--relay N0|none]\n\
  identity --identity FILE [--network NAME]   (prints the endpoint id and ML-DSA key id a\n\
        manifest pins; creates the identity if the file does not exist)\n\
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

    /// A node on loopback with its pinned loops and peer manager running.
    async fn start_node(
        dir: &Path,
        seed: u8,
        pins: Vec<NodeDescriptor>,
        stop: &watch::Receiver<bool>,
    ) -> (
        Network,
        stark_net::Server,
        NodeDescriptor,
        Vec<tokio::task::JoinHandle<()>>,
    ) {
        let state = new_state(dir.join(format!("node-{seed}")), None)
            .await
            .unwrap();
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
        (net, server, own, tasks)
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
        let (a, a_server, a_own, mut tasks) = start_node(dir.path(), 101, vec![], &stop).await;
        mine_once(&a.state, alice.clone()).await.unwrap();
        let (b, b_server, b_own, more) =
            start_node(dir.path(), 102, vec![a_own.clone()], &stop).await;
        tasks.extend(more);
        eventually("B's session with A", || b.state.peers.book().outbound_len() == 1).await;

        // C is given only B. B shares A (pinned, in session); C dials A, and
        // A learns B and C from their announces, after dialing each back.
        let (c, c_server, _, more) = start_node(dir.path(), 103, vec![b_own], &stop).await;
        tasks.extend(more);
        eventually("C to find and dial A", || {
            let book = c.state.peers.book();
            book.known_len() == 1 && book.outbound_len() == 2
        })
        .await;
        assert_eq!(c.state.peers.book().share(16, super::super::now_secs())[0], a_own);
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
}
