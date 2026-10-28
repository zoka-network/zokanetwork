//! Explicit-path node, mirror synchronization and broadcast tools for the STARK
//! v2 chains. The database marker names the chain; `--network` picks it for a
//! new database.
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use iroh::endpoint::presets;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use zeroize::Zeroizing;

use super::{
    stark::{self, Profile},
    stark_api::{self, NodeState},
    stark_cli::{open_db, read_bounded, write_new},
    stark_net::{self, Client, Head, NodeDescriptor, ServerIdentity},
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
    fn relay(&self) -> Result<bool, String> {
        match self.get("--relay") {
            None | Some("none") => Ok(false),
            Some("N0") => Ok(true),
            _ => Err("--relay must be none or N0".into()),
        }
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
        let ledger = stark::state_after(&state.storage, state.profile, &current)?;
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

async fn peer_loop(
    state: NodeState,
    peer: NodeDescriptor,
    relay: bool,
    mut stop: watch::Receiver<bool>,
) {
    loop {
        let connected = tokio::select! { _ = stopped(&mut stop) => return, result = connect(&peer, relay) => result };
        match connected {
            Ok(mut client) => {
                let mut cursor = None;
                let mut sent = BTreeSet::new();
                loop {
                    let result = tokio::select! {
                        _ = stopped(&mut stop) => { client.close().await; return; },
                        result = peer_cycle(&mut client, &state, &mut cursor, &mut sent) => result,
                    };
                    if let Err(reason) = result {
                        eprintln!("STARK peer: {reason}; retrying");
                        break;
                    }
                    tokio::select! { _ = stopped(&mut stop) => { client.close().await; return; }, _ = tokio::time::sleep(Duration::from_secs(5)) => {} }
                }
                client.close().await;
            }
            Err(reason) => eprintln!("STARK peer connection: {reason}; retrying"),
        }
        tokio::select! { _ = stopped(&mut stop) => return, _ = tokio::time::sleep(Duration::from_secs(5)) => {} }
    }
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
    let relay = options.relay()?;
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
    // The descriptor contains public transport and authentication keys only.
    println!(
        "{}",
        serde_json::json!({"chain_id":profile.chain_id,"api":format!("http://{actual_bind}"),
        "pow_algorithm":profile.pow_algorithm(),"xmrig_compatible":profile.xmrig_compatible(),
        "stratum":options.get("--stratum"),"descriptor":server.descriptor})
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
    for peer in peers {
        workers.push(tokio::spawn(peer_loop(
            state.clone(),
            peer,
            relay,
            stop.clone(),
        )));
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

fn one_peer(options: &Options) -> Result<NodeDescriptor, String> {
    if options.peers().len() != 1 {
        return Err("this command requires exactly one --peer descriptor file".into());
    }
    descriptor(Path::new(&options.peers()[0]))
}

async fn sync(options: Options) -> Result<(), String> {
    let db = PathBuf::from(options.required("--db")?);
    let peer = one_peer(&options)?;
    let relay = options.relay()?;
    let maximum: usize = options
        .get("--max-blocks")
        .unwrap_or("8192")
        .parse()
        .map_err(err)?;
    if maximum == 0 || maximum > 1_000_000 {
        return Err("--max-blocks must be 1..1000000".into());
    }
    let state = new_state(db, options.network()?).await?;
    same_chain(std::slice::from_ref(&peer), state.profile)?;
    let mut client = connect(&peer, relay).await?;
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
    let peer = one_peer(&options)?;
    let relay = options.relay()?;
    let path = PathBuf::from(options.required("--tx")?);
    let transaction = tokio::task::spawn_blocking(move || {
        Transaction::decode(&read_bounded(&path, MAX_TRANSACTION_BYTES)?).map_err(err)
    })
    .await
    .map_err(err)??;
    let mut client = connect(&peer, relay).await?;
    let result = tokio::select! {
        signal = tokio::signal::ctrl_c() => signal.map_err(err),
        result = client.submit_tx(&transaction) => result.map(|id| println!("{}", serde_json::json!({"accepted":true,"txid":hex::encode(id)}))),
    };
    client.close().await;
    result
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
                ],
            )?)
            .await
        }
        "sync" => {
            sync(Options::parse(
                rest,
                &["--db", "--peer", "--relay", "--max-blocks", "--network"],
            )?)
            .await
        }
        "broadcast" => broadcast(Options::parse(rest, &["--peer", "--tx", "--relay"])?).await,
        "help" | "--help" | "-h" => {
            println!("STARK v2 node. --network regtest (default for a new --db: CPU test PoW)\n\
or testnet (zoka-quantic-testnet-v2: RandomX, ASERT). An existing --db keeps its chain.\n\
  serve --db DIR --identity FILE [--network NAME] [--api 127.0.0.1:9140] [--descriptor-out FILE]\n\
        [--peer DESCRIPTOR ...] [--mine-to ADDRESS] [--interval 120] [--relay=N0]\n\
        [--stratum 127.0.0.1:3333]   (testnet: XMRig logs in with its v2 address;\n\
        --mine-to pays logins that are not addresses. Regtest: built-in miner.)\n\
  sync --db MIRROR_DIR --peer DESCRIPTOR [--network NAME] [--max-blocks 8192] [--relay=N0]\n\
  broadcast --peer DESCRIPTOR --tx FILE [--relay=N0]\n\
Set ZOKA_PQ_PASSWORD to create/open the encrypted node identity. Files are never overwritten.\n\
Use a separate mirror database for wallet commands while the node database is open.\n\
Exchange public peer descriptors through an authenticated channel before connecting.\n\
Default transport is direct (no relay); N0 explicitly enables public relays. Ctrl+C stops the node.");
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
            .relay()
            .unwrap());
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
            let ledger = stark::state_after(&state.storage, state.profile, &tip(state)?)?;
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
}
