//! What the phone wallet (ZKAPriv) asks of the network, without a node of its
//! own: pages of the chain, sending a payment and the short-address
//! directory. Straight to the seeds (iroh, with n0's relays), or only through
//! Tor: the stream transport over a SOCKS port, the embedded Arti's on the
//! phone, to the seeds' onion addresses (`docs/MODO-TOR.md`). Over Tor there
//! is no other way out: if no onion seed answers, nothing is sent.
//!
//! The proof is built on the phone (`zoka_pq_pool::light`); the seed only
//! gets the finished transaction, as from any peer. What the phone reads is
//! checked against every block header, and the chain of headers against the
//! latest checkpoint, signed by the team with Ed25519 and ML-DSA-65: no seed
//! or web cache can show the wallet another chain.
//!
//! The seeds are only the first door: the phone keeps the peers the nodes it
//! talks to share ([`PhonePeers`]) and dials those too, so any node can serve
//! it and the seeds being down does not cut it off.
use std::{net::SocketAddr, sync::Arc};

use iroh::{endpoint::presets, EndpointAddr, EndpointId, TransportAddr};
use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};

use super::{
    stark::{
        protocol::{
            scan::{ScanPage2, MAX_SCAN_BLOCKS},
            transaction::Transaction,
            wallet::{Address, ShortAddress},
        },
        Profile,
    },
    stark_directory::AddressRecord,
    stark_net::{self, Client, NodeDescriptor},
    stark_peers::Manifest,
};
use crate::checkpoint_authority::SignedCheckpoint;
use crate::network::stream_transport::{SocksDialer, StreamOnly, StreamTransport};

/// How the phone reaches the network.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Direct,
    /// Only through Tor, by this SOCKS port.
    Tor(SocketAddr),
}

/// The seeds of the network in this binary (none for a local chain).
pub fn builtin_seeds(profile: Profile) -> Result<Vec<NodeDescriptor>, String> {
    match Manifest::builtin(profile)? {
        Some(manifest) => manifest.seeds(),
        None => Ok(Vec::new()),
    }
}

/// One peer on `route`, authenticated by its pinned ML-DSA key.
async fn dial(peer: &NodeDescriptor, route: Route) -> Result<Client, String> {
    match route {
        Route::Direct => Client::connect(peer, presets::N0).await,
        Route::Tor(proxy) => {
            let dialer = StreamTransport::new(None, Arc::new(SocksDialer { proxy }))
                .map_err(|e| e.to_string())?;
            Client::connect(peer, StreamOnly(dialer)).await
        }
    }
}

/// The first of `seeds` that answers on `route`, authenticated by its pinned
/// ML-DSA key like any peer.
pub async fn connect(seeds: &[NodeDescriptor], route: Route) -> Result<Client, String> {
    let mut none = PhonePeers::new(Profile::REGTEST);
    connect_with(&mut none, seeds, route).await
}

/// Like [`connect`], with the peers the phone learned: up to
/// [`FIRST_LEARNED`] that answered lately, then the seeds, then up to
/// [`LAST_LEARNED`] others. Each answer or failure is noted in `peers`.
pub async fn connect_with(
    peers: &mut PhonePeers,
    seeds: &[NodeDescriptor],
    route: Route,
) -> Result<Client, String> {
    let candidates = peers.candidates(seeds, route, super::now_secs());
    let mut last = match route {
        Route::Direct => "no seed to dial".to_string(),
        Route::Tor(_) => "no seed with an onion address".to_string(),
    };
    for peer in &candidates {
        let id = peer.endpoint_addr.id;
        match dial(peer, route).await {
            Ok(client) => {
                peers.answered(id, super::now_secs());
                return Ok(client);
            }
            Err(reason) => {
                peers.failed(id, super::now_secs());
                last = format!("peer {}: {reason}", id.fmt_short());
            }
        }
    }
    Err(last)
}

/// Ask the node on `client` for the peers it knows and keep them. A node
/// that does not answer costs nothing: the seeds are still there.
pub async fn learn_from(client: &mut Client, peers: &mut PhonePeers, seeds: &[NodeDescriptor]) {
    if let Ok(found) = client.peers().await {
        peers.learn(found, seeds);
    }
}

/// Learned peers a phone keeps at most.
pub const MAX_PHONE_PEERS: usize = 64;
/// Learned peers that answered lately, tried before the seeds.
pub const FIRST_LEARNED: usize = 2;
/// Other learned peers, tried once the seeds did not answer.
pub const LAST_LEARNED: usize = 4;
/// A peer that answered within this window and has not failed since is
/// tried before the seeds.
const FRESH_SECS: u64 = 7 * 24 * 3600;
/// A peer is forgotten after this many failures in a row, unless it
/// answered within [`FRESH_SECS`].
const FORGET_FAILURES: u32 = 8;
const PHONE_PEERS_SCHEMA: &str = "zoka-quantic-phone-peers-v1";
/// Bigger than [`MAX_PHONE_PEERS`] descriptors could be: not ours.
pub const MAX_PHONE_PEERS_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PhonePeer {
    descriptor: NodeDescriptor,
    /// Unix seconds of the last authenticated session; 0 if never.
    last_ok: u64,
    failures: u32,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PhonePeersFile {
    schema: String,
    chain_id: String,
    peers: Vec<PhonePeer>,
}

/// The peers a phone learned from the nodes it talked to (`PEERS`), kept
/// between calls. A learned peer is authenticated by the ML-DSA key its
/// descriptor names, as a seed is, and what it serves is checked like any
/// other page (headers, checkpoint): a bad one can stall the wallet for a
/// call, not fool it.
///
/// Only the endpoint id and onion addresses are kept, as a node in relay mode
/// does (`stark_peers`): address lookup finds the rest, and a shared
/// descriptor cannot aim the phone's dials at an IP or a clear-net host of
/// someone else's choosing.
pub struct PhonePeers {
    chain_id: &'static str,
    peers: Vec<PhonePeer>,
}

impl PhonePeers {
    pub fn new(profile: Profile) -> Self {
        Self {
            chain_id: profile.chain_id,
            peers: Vec::new(),
        }
    }

    /// A file of another chain, schema or shape is a cache: start empty.
    pub fn decode(bytes: &[u8], profile: Profile) -> Self {
        let mut book = Self::new(profile);
        let file = (bytes.len() <= MAX_PHONE_PEERS_BYTES)
            .then(|| serde_json::from_slice::<PhonePeersFile>(bytes).ok())
            .flatten()
            .filter(|f| f.schema == PHONE_PEERS_SCHEMA && f.chain_id == profile.chain_id);
        for peer in file.map(|f| f.peers).unwrap_or_default() {
            if book.peers.len() >= MAX_PHONE_PEERS {
                break;
            }
            let Some(descriptor) = book.keepable(peer.descriptor) else {
                continue;
            };
            if !book.has(descriptor.endpoint_addr.id) {
                book.peers.push(PhonePeer { descriptor, ..peer });
            }
        }
        book
    }

    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(&PhonePeersFile {
            schema: PHONE_PEERS_SCHEMA.into(),
            chain_id: self.chain_id.into(),
            peers: self.peers.clone(),
        })
        .unwrap_or_default()
    }

    pub fn len(&self) -> usize {
        self.peers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }

    /// How many answered within [`FRESH_SECS`] and have not failed since.
    pub fn fresh(&self, now: u64) -> usize {
        self.peers.iter().filter(|p| Self::is_fresh(p, now)).count()
    }

    fn has(&self, id: EndpointId) -> bool {
        self.peers
            .iter()
            .any(|p| p.descriptor.endpoint_addr.id == id)
    }

    fn is_fresh(peer: &PhonePeer, now: u64) -> bool {
        peer.last_ok > 0 && peer.failures == 0 && now.saturating_sub(peer.last_ok) <= FRESH_SECS
    }

    /// The descriptor as kept: valid, of this chain, with its onion
    /// addresses only.
    fn keepable(&self, mut descriptor: NodeDescriptor) -> Option<NodeDescriptor> {
        if descriptor.validate().is_err() || descriptor.chain_id != self.chain_id {
            return None;
        }
        let onions: Vec<_> = descriptor
            .endpoint_addr
            .addrs
            .iter()
            .filter(|a| is_onion(a))
            .cloned()
            .collect();
        descriptor.endpoint_addr =
            EndpointAddr::new(descriptor.endpoint_addr.id).with_addrs(onions);
        Some(descriptor)
    }

    /// Peers a node shared. New ones wait, untried, behind the seeds until
    /// one answers. When the book is full a random untried one makes room:
    /// whoever floods it cannot push out the peers that answered.
    pub fn learn(&mut self, found: Vec<NodeDescriptor>, seeds: &[NodeDescriptor]) {
        for descriptor in found {
            let Some(descriptor) = self.keepable(descriptor) else {
                continue;
            };
            let id = descriptor.endpoint_addr.id;
            if self.has(id) || seeds.iter().any(|s| s.endpoint_addr.id == id) {
                continue;
            }
            if self.peers.len() >= MAX_PHONE_PEERS {
                let untried: Vec<usize> = (0..self.peers.len())
                    .filter(|i| self.peers[*i].last_ok == 0)
                    .collect();
                match untried.choose(&mut rand::thread_rng()) {
                    Some(victim) => {
                        self.peers.swap_remove(*victim);
                    }
                    None => continue,
                }
            }
            self.peers.push(PhonePeer {
                descriptor,
                last_ok: 0,
                failures: 0,
            });
        }
    }

    /// The order [`connect_with`] dials, each in the form `route` can use.
    fn candidates(&self, seeds: &[NodeDescriptor], route: Route, now: u64) -> Vec<NodeDescriptor> {
        let mut rng = rand::thread_rng();
        let mut fresh: Vec<&PhonePeer> = self
            .peers
            .iter()
            .filter(|p| Self::is_fresh(p, now))
            .collect();
        fresh.shuffle(&mut rng);
        let first: Vec<NodeDescriptor> = fresh
            .iter()
            .take(FIRST_LEARNED)
            .map(|p| p.descriptor.clone())
            .collect();
        let mut rest: Vec<NodeDescriptor> = self
            .peers
            .iter()
            .map(|p| p.descriptor.clone())
            .filter(|d| {
                !first
                    .iter()
                    .any(|f| f.endpoint_addr.id == d.endpoint_addr.id)
            })
            .collect();
        rest.shuffle(&mut rng);
        rest.truncate(LAST_LEARNED);
        let mut seeds = seeds.to_vec();
        seeds.shuffle(&mut rng);
        match route {
            // Without Tor a learned peer is its id: address lookup and the
            // relays find it. Seeds go as the manifest has them.
            Route::Direct => {
                let bare = |mut d: NodeDescriptor| {
                    d.endpoint_addr = EndpointAddr::new(d.endpoint_addr.id);
                    d
                };
                let first = first.into_iter().map(bare);
                let rest = rest.into_iter().map(bare);
                first.chain(seeds).chain(rest).collect()
            }
            // Only onions: the learned ones have nothing else already, the
            // seeds lose their other addresses here.
            Route::Tor(_) => {
                stark_net::over_tor(first.into_iter().chain(seeds).chain(rest).collect())
            }
        }
    }

    fn answered(&mut self, id: EndpointId, now: u64) {
        if let Some(peer) = self
            .peers
            .iter_mut()
            .find(|p| p.descriptor.endpoint_addr.id == id)
        {
            peer.last_ok = now;
            peer.failures = 0;
        }
    }

    fn failed(&mut self, id: EndpointId, now: u64) {
        let Some(index) = self
            .peers
            .iter()
            .position(|p| p.descriptor.endpoint_addr.id == id)
        else {
            return;
        };
        let peer = &mut self.peers[index];
        peer.failures = peer.failures.saturating_add(1);
        let answered_lately = peer.last_ok > 0 && now.saturating_sub(peer.last_ok) < FRESH_SECS;
        if peer.failures >= FORGET_FAILURES && !answered_lately {
            self.peers.swap_remove(index);
        }
    }
}

/// A stream address whose host is an onion service: through Tor it reaches
/// only that service, never a clear-net host.
fn is_onion(addr: &TransportAddr) -> bool {
    let TransportAddr::Custom(custom) = addr else {
        return false;
    };
    crate::network::stream_transport::dial_target(custom)
        .and_then(|target| {
            target
                .rsplit_once(':')
                .map(|(host, _)| host.to_ascii_lowercase())
        })
        .is_some_and(|host| host.ends_with(".onion"))
}

/// One page of the chain from `from`, at most [`MAX_SCAN_BLOCKS`] blocks.
pub async fn page(client: &mut Client, from: u32) -> Result<ScanPage2, String> {
    client.scan_page(from, MAX_SCAN_BLOCKS).await
}

/// A checkpoint of `profile`'s chain whose signature holds: its height and
/// block hash, to confirm what a light wallet read
/// (`LightWallet::confirm_checkpoint`). On testnet v2 the signature has to be
/// the hybrid one (Ed25519 and ML-DSA-65).
pub fn verify_checkpoint(
    checkpoint: &SignedCheckpoint,
    profile: Profile,
) -> Result<(u32, String), String> {
    if checkpoint.chain_id != profile.chain_id {
        return Err("a checkpoint of another chain".into());
    }
    checkpoint
        .verify_for_declared_chain()
        .map_err(|e| e.to_string())?;
    let height = u32::try_from(checkpoint.height).map_err(|e| e.to_string())?;
    Ok((height, checkpoint.block_hash.clone()))
}

/// The latest checkpoint the seed has, verified ([`verify_checkpoint`]).
pub async fn checkpoint(
    client: &mut Client,
    profile: Profile,
) -> Result<Option<(u32, String)>, String> {
    match client.checkpoint().await? {
        Some(checkpoint) => verify_checkpoint(&checkpoint, profile).map(Some),
        None => Ok(None),
    }
}

/// Hand a payment to the network. The answer is its id once the seed's
/// mempool took it; a refusal says why (an expired anchor, say).
pub async fn submit(client: &mut Client, tx: &Transaction) -> Result<[u8; 32], String> {
    client.submit_tx(tx).await
}

/// The full address a short one names, if the directory knows it. The
/// answer is checked against the short address: the directory cannot lie.
pub async fn resolve(
    client: &mut Client,
    short: &ShortAddress,
    network: u8,
) -> Result<Option<Address>, String> {
    let Some(record) = client.get_address(&short.digest).await? else {
        return Ok(None);
    };
    let full = Address::decode(&record.address, network).map_err(|e| e.to_string())?;
    if !short.matches(&full) {
        return Err("the directory answered another address".into());
    }
    Ok(Some(full))
}

/// Offer the wallet's own address to the directory, so that its short form
/// works for others. True if the seed took it (or already had it).
pub async fn publish(client: &mut Client, address: &Address) -> Result<bool, String> {
    let record = AddressRecord::new(address).map_err(|e| e.to_string())?;
    client.submit_address(&record).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::stream_transport::{dial_addr, TcpDialer};
    use crate::quantic::{
        stark::protocol::{light::LightWallet, wallet::Account},
        stark_net::{serve, ServerIdentity},
    };
    const ONION: &str = "u2soz3q63237afuccfxf3jzytpljpbxpvyzt3jumrr45ukzowdakayqd.onion:9444";

    /// A testnet descriptor with an IP, an onion and a clear-net stream address.
    fn descriptor(seed: u8) -> NodeDescriptor {
        let identity = ServerIdentity::from_seed(&[seed; 32]).unwrap();
        NodeDescriptor {
            version: stark_net::DESCRIPTOR_VERSION,
            chain_id: Profile::TESTNET.chain_id.into(),
            endpoint_addr: EndpointAddr::new(identity.endpoint_id()).with_addrs([
                TransportAddr::Ip("203.0.113.7:4433".parse().unwrap()),
                TransportAddr::Custom(dial_addr(ONION).unwrap()),
                TransportAddr::Custom(dial_addr("victim.example:443").unwrap()),
            ]),
            ml_dsa_key_id: identity.key_id(),
        }
    }

    fn id(seed: u8) -> EndpointId {
        descriptor(seed).endpoint_addr.id
    }

    #[test]
    fn a_phone_keeps_learned_ids_and_onions_but_never_a_seed() {
        let seed = descriptor(1);
        let mut other_chain = descriptor(2);
        other_chain.chain_id = Profile::REGTEST.chain_id.into();
        let mut book = PhonePeers::new(Profile::TESTNET);
        book.learn(
            vec![
                seed.clone(),
                other_chain,
                descriptor(3),
                descriptor(3),
                descriptor(4),
            ],
            &[seed],
        );
        assert_eq!(book.len(), 2);
        let kept = &book.peers[0].descriptor.endpoint_addr;
        assert_eq!(kept.id, id(3));
        assert_eq!(kept.addrs.len(), 1, "no IP and no clear-net host");
        assert!(kept.addrs.iter().all(is_onion));
        assert_eq!(
            book.fresh(super::super::now_secs()),
            0,
            "untried until one answers"
        );

        // The file: the same book back; anything else is an empty cache.
        let again = PhonePeers::decode(&book.encode(), Profile::TESTNET);
        assert_eq!(again.len(), 2);
        assert!(PhonePeers::decode(&book.encode(), Profile::REGTEST).is_empty());
        assert!(PhonePeers::decode(b"{not json", Profile::TESTNET).is_empty());
    }

    #[test]
    fn a_phone_tries_peers_that_answered_then_the_seeds_then_the_rest() {
        let now = 10 * FRESH_SECS;
        let seeds = vec![descriptor(1), descriptor(2)];
        let mut book = PhonePeers::new(Profile::TESTNET);
        book.learn((10..16).map(descriptor).collect(), &seeds);
        book.answered(id(10), now - 60);
        book.answered(id(11), now - 60);

        let order = book.candidates(&seeds, Route::Direct, now);
        let ids: Vec<EndpointId> = order.iter().map(|d| d.endpoint_addr.id).collect();
        assert_eq!(ids.len(), FIRST_LEARNED + seeds.len() + LAST_LEARNED);
        let mut first = ids[..2].to_vec();
        first.sort();
        let mut fresh = vec![id(10), id(11)];
        fresh.sort();
        assert_eq!(first, fresh, "the ones that answered go first");
        let mut middle = ids[2..4].to_vec();
        middle.sort();
        let mut seed_ids = vec![id(1), id(2)];
        seed_ids.sort();
        assert_eq!(middle, seed_ids, "then the seeds");
        assert!(ids[4..].iter().all(|i| (12..16).any(|s| id(s) == *i)));
        for learned in order
            .iter()
            .filter(|d| !seed_ids.contains(&d.endpoint_addr.id))
        {
            assert!(
                learned.endpoint_addr.addrs.is_empty(),
                "without Tor a learned peer is its id"
            );
        }

        // Over Tor, stream addresses only: a learned peer keeps just its
        // onion; a seed keeps what the manifest gives it.
        let tor = book.candidates(&seeds, Route::Tor("127.0.0.1:9150".parse().unwrap()), now);
        assert_eq!(tor.len(), FIRST_LEARNED + seeds.len() + LAST_LEARNED);
        for peer in &tor {
            assert!(!peer.endpoint_addr.addrs.is_empty());
            if !seed_ids.contains(&peer.endpoint_addr.id) {
                assert!(peer.endpoint_addr.addrs.iter().all(is_onion));
            }
        }

        // A failure takes a peer out of the first ones; untried peers are
        // forgotten after FORGET_FAILURES, one that answered lately is not.
        book.failed(id(10), now);
        assert_eq!(book.fresh(now), 1);
        for _ in 0..FORGET_FAILURES {
            book.failed(id(12), now);
            book.failed(id(11), now);
        }
        assert!(!book.has(id(12)));
        assert!(book.has(id(11)));
    }

    #[test]
    fn a_flood_of_shared_peers_cannot_push_out_the_ones_that_answered() {
        let mut book = PhonePeers::new(Profile::TESTNET);
        book.learn(
            (0..MAX_PHONE_PEERS as u8)
                .map(|s| descriptor(s.wrapping_add(100)))
                .collect(),
            &[],
        );
        assert_eq!(book.len(), MAX_PHONE_PEERS);
        for s in 0..MAX_PHONE_PEERS as u8 {
            book.answered(id(s.wrapping_add(100)), 1);
        }
        book.learn((0..16).map(descriptor).collect(), &[]);
        assert_eq!(book.len(), MAX_PHONE_PEERS);
        assert!(
            book.peers.iter().all(|p| p.last_ok == 1),
            "full of peers that answered"
        );
        // With untried ones in the book, a newcomer takes one of their places.
        book.peers[0].last_ok = 0;
        book.learn(vec![descriptor(200)], &[]);
        assert_eq!(book.len(), MAX_PHONE_PEERS);
        assert!(book.has(id(200)));
    }

    #[test]
    fn a_checkpoint_counts_only_with_the_team_hybrid_signature() {
        // A real one of the v2 testnet, checked under its own chain id: the
        // v3 testnet has none yet, and the chain id is inside the signature.
        let v2 = Profile {
            chain_id: crate::economics::QUANTIC_PQ_TESTNET_V2_CHAIN_ID,
            ..Profile::TESTNET
        };
        let real: SignedCheckpoint =
            serde_json::from_str(include_str!("testdata/checkpoint-v2-3651.json")).unwrap();
        let (height, hash) = verify_checkpoint(&real, v2).unwrap();
        assert_eq!(height, 3651);
        assert_eq!(hash, real.block_hash);
        // Another block, a missing ML-DSA half or another chain: refused.
        let mut other = real.clone();
        other.block_hash = "0".repeat(64);
        assert!(verify_checkpoint(&other, v2).is_err());
        let mut classical_only = real.clone();
        classical_only.mldsa_signature = None;
        assert!(verify_checkpoint(&classical_only, v2).is_err());
        assert!(verify_checkpoint(&real, Profile::REGTEST).is_err());
        assert!(verify_checkpoint(&real, Profile::TESTNET).is_err());
        // Relabeled for the v3 testnet, the v2 signature does not hold.
        let mut relabeled = real.clone();
        relabeled.chain_id = Profile::TESTNET.chain_id.into();
        assert!(verify_checkpoint(&relabeled, Profile::TESTNET).is_err());
    }

    #[test]
    fn the_testnet_seeds_include_one_reachable_over_tor() {
        assert!(builtin_seeds(Profile::REGTEST).unwrap().is_empty());
        let seeds = builtin_seeds(Profile::TESTNET).unwrap();
        let manifest = Manifest::builtin(Profile::TESTNET).unwrap().unwrap();
        if manifest.status == "pre-genesis" {
            // The v3 testnet gets its seeds at launch; then the explorer's
            // seed carries the onion address again, and this checks it.
            assert!(seeds.is_empty());
            return;
        }
        assert!(!seeds.is_empty());
        let onion = stark_net::over_tor(seeds);
        assert!(!onion.is_empty(), "Tor mode needs an onion seed");
        assert!(onion.iter().all(|seed| seed
            .endpoint_addr
            .addrs
            .iter()
            .all(|a| matches!(a, TransportAddr::Custom(_)))));
        assert!(builtin_seeds(Profile::REGTEST).unwrap().is_empty());
    }

    /// The phone's whole path over the stream transport (Tor mode without
    /// Tor): pages from a seed, a light wallet that reads them, a payment
    /// proved on the "phone", sent and mined, and the directory.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_phone_reads_pays_and_publishes_over_the_stream_transport() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(crate::storage::RocksDbStorage::new(dir.path()).unwrap());
        storage
            .put_raw(
                crate::quantic::stark_api::DB_MARKER,
                Profile::REGTEST.chain_id.as_bytes().to_vec(),
            )
            .unwrap();
        let state = crate::quantic::stark_api::NodeState::new(storage).unwrap();
        let alice = Arc::new(Account::from_seed(&[81; 32], 3, 0).unwrap());
        let bob = Account::from_seed(&[82; 32], 3, 0).unwrap();
        for _ in 0..3 {
            super::super::stark_node::mine_once(&state, alice.address())
                .await
                .unwrap();
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = listener.local_addr().unwrap().to_string();
        let transport = StreamTransport::new(Some(&target), Arc::new(TcpDialer)).unwrap();
        let server = serve(
            state.clone(),
            ServerIdentity::from_seed(&[83; 32]).unwrap(),
            StreamOnly(transport.clone()),
        )
        .await
        .unwrap();
        let _listening = transport.listen(listener);
        let mut seed = server.descriptor.clone();
        seed.endpoint_addr = EndpointAddr::new(server.endpoint.id())
            .with_addrs([TransportAddr::Custom(dial_addr(&target).unwrap())]);
        let dialer = StreamTransport::new(None, Arc::new(TcpDialer)).unwrap();
        let mut client = Client::connect(&seed, StreamOnly(dialer)).await.unwrap();

        let mut light = LightWallet::new(3, 1).unwrap();
        light
            .add_page(&alice, &page(&mut client, light.next()).await.unwrap())
            .unwrap();
        assert_eq!((light.height(), light.tip()), (3, 3));
        // Past the tip: an empty page, not an error.
        assert!(page(&mut client, 9).await.unwrap().blocks.is_empty());

        let plan = light
            .plan(&alice, bob.address(), 3_000_000, b"over the stream")
            .unwrap();
        let prover = alice.clone();
        let tx = tokio::task::spawn_blocking(move || plan.prove(&prover).unwrap())
            .await
            .unwrap();
        light.reserve(&tx).unwrap();
        assert_eq!(submit(&mut client, &tx).await.unwrap(), tx.txid());
        super::super::stark_node::mine_once(&state, alice.address())
            .await
            .unwrap();
        light
            .add_page(&alice, &page(&mut client, light.next()).await.unwrap())
            .unwrap();
        assert!(!light.balance().pending, "mined");
        assert_eq!(light.history().last().unwrap().paid(), 3_000_000);

        // The directory: unknown, published, then resolved and checked.
        let short = bob.address().short_address().unwrap();
        assert!(resolve(&mut client, &short, 3).await.unwrap().is_none());
        assert!(publish(&mut client, &bob.address()).await.unwrap());
        let found = resolve(&mut client, &short, 3).await.unwrap().unwrap();
        assert_eq!(found.encode().unwrap(), bob.address().encode().unwrap());

        // The node shares a peer it authenticated; the phone keeps it.
        let mut other = descriptor(90);
        other.chain_id = Profile::REGTEST.chain_id.into();
        state
            .peers
            .book()
            .verified(other.clone(), super::super::now_secs());
        let mut regtest_peers = PhonePeers::new(Profile::REGTEST);
        learn_from(&mut client, &mut regtest_peers, &[seed.clone()]).await;
        assert_eq!(regtest_peers.len(), 1);
        assert_eq!(
            regtest_peers.peers[0].descriptor.endpoint_addr.id,
            other.endpoint_addr.id
        );
        server.close().await;
    }
}
