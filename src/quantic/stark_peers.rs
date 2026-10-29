//! Peer table and seed manifest of the STARK v2 network.
//!
//! The book keeps endpoints this node has dialed and authenticated itself
//! (`known`) and a bounded queue of announced or shared endpoints still to be
//! dialed (`pending`). Only known entries that answered recently are shared
//! with other peers. Pinned peers (`--peer` files and manifest seeds) are
//! dialed by their own loops and never enter the book; while one of them has
//! a session it is shared like a fresh known entry.
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    net::SocketAddr,
    str::FromStr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex, MutexGuard,
    },
};

use iroh::{EndpointAddr, EndpointId, RelayUrl};
use rand::{seq::SliceRandom, Rng};
use serde::{Deserialize, Serialize};

use super::{
    stark::Profile,
    stark_net::{NodeDescriptor, ALPN, DESCRIPTOR_VERSION},
};
use crate::storage::RocksDbStorage;

const BOOK_KEY: &str = "quantic:stark:peers";
const BOOK_SCHEMA: &str = "zoka-quantic-stark-peers-v1";
const MAX_BOOK_BYTES: usize = 1024 * 1024;
pub const MAX_KNOWN: usize = 256;
pub const MAX_PENDING: usize = 64;
/// Known peers that answered within this window are shared.
const SHARE_WINDOW_SECS: u64 = 24 * 3600;
/// A known peer is forgotten after this many failures in a row, once it has
/// not answered for a week.
const FORGET_FAILURES: u32 = 8;
const FORGET_SECS: u64 = 7 * 24 * 3600;
const RETRY_BASE_SECS: u64 = 60;
const RETRY_MAX_SECS: u64 = 6 * 3600;

const MANIFEST_SCHEMA: &str = "zoka-quantic-stark-network-v1";
pub const MAX_MANIFEST_BYTES: usize = 256 * 1024;
const MAX_SEEDS: usize = 16;
const TESTNET_MANIFEST: &str = include_str!("../../docs/quantic-testnet-v2-network.v1.json");

fn err(value: impl std::fmt::Display) -> String {
    value.to_string()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    descriptor: NodeDescriptor,
    last_ok: u64,
    failures: u32,
    #[serde(skip)]
    retry_at: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BookFile {
    schema: String,
    chain_id: String,
    peers: Vec<Entry>,
}

pub struct PeerBook {
    chain_id: &'static str,
    /// Address lookup resolves endpoint ids: learned transport addresses are
    /// dropped, so an announce cannot aim this node's dials at a third party.
    relay: bool,
    own: Option<EndpointId>,
    /// As shared: without transport addresses in relay mode.
    pinned: BTreeMap<EndpointId, NodeDescriptor>,
    known: BTreeMap<EndpointId, Entry>,
    pending: VecDeque<NodeDescriptor>,
    /// Endpoints with an authenticated outbound session right now.
    connected: BTreeSet<EndpointId>,
    /// The tip height each outbound session last reported.
    observed: BTreeMap<EndpointId, u64>,
    dirty: bool,
}

impl PeerBook {
    pub fn new(profile: Profile) -> Self {
        Self {
            chain_id: profile.chain_id,
            relay: false,
            own: None,
            pinned: BTreeMap::new(),
            known: BTreeMap::new(),
            pending: VecDeque::new(),
            connected: BTreeSet::new(),
            observed: BTreeMap::new(),
            dirty: false,
        }
    }

    /// An unreadable book is replaced on the next save, not reported: a peer
    /// table is a cache, and node commands must not write to stderr for it.
    pub fn load(storage: &RocksDbStorage, profile: Profile) -> Self {
        let mut book = Self::new(profile);
        let file = storage
            .get_raw(BOOK_KEY)
            .ok()
            .flatten()
            .filter(|bytes| bytes.len() <= MAX_BOOK_BYTES)
            .and_then(|bytes| serde_json::from_slice::<BookFile>(&bytes).ok())
            .filter(|file| file.schema == BOOK_SCHEMA && file.chain_id == profile.chain_id);
        match file {
            Some(file) => {
                for entry in file.peers.into_iter().take(MAX_KNOWN) {
                    if let Some(descriptor) = book.sanitize(entry.descriptor) {
                        book.known.insert(
                            descriptor.endpoint_addr.id,
                            Entry {
                                descriptor,
                                ..entry
                            },
                        );
                    }
                }
            }
            None => book.dirty = storage.get_raw(BOOK_KEY).ok().flatten().is_some(),
        }
        book
    }

    /// Serialized only when something changed since the last save.
    pub fn snapshot(&mut self) -> Option<Vec<u8>> {
        if !std::mem::take(&mut self.dirty) {
            return None;
        }
        serde_json::to_vec(&BookFile {
            schema: BOOK_SCHEMA.into(),
            chain_id: self.chain_id.into(),
            peers: self.known.values().cloned().collect(),
        })
        .ok()
    }

    pub fn save(snapshot: Option<Vec<u8>>, storage: &RocksDbStorage) -> Result<(), String> {
        match snapshot {
            Some(bytes) => storage.put_raw(BOOK_KEY, bytes).map_err(err),
            None => Ok(()),
        }
    }

    /// Called once the node's endpoint exists. Entries that are now pinned or
    /// that name this node leave the book, and in relay mode learned entries
    /// lose their transport addresses.
    pub fn configure(&mut self, own: EndpointId, relay: bool, pinned: &[NodeDescriptor]) {
        self.own = Some(own);
        self.relay = relay;
        self.pinned = pinned
            .iter()
            .map(|peer| {
                let mut shared = peer.clone();
                if relay {
                    shared.endpoint_addr = EndpointAddr::new(peer.endpoint_addr.id);
                }
                (peer.endpoint_addr.id, shared)
            })
            .collect();
        let known = std::mem::take(&mut self.known);
        for (_, entry) in known {
            if let Some(descriptor) = self.sanitize(entry.descriptor.clone()) {
                self.known.insert(
                    descriptor.endpoint_addr.id,
                    Entry {
                        descriptor,
                        ..entry
                    },
                );
            } else {
                self.dirty = true;
            }
        }
        let pending = std::mem::take(&mut self.pending);
        self.pending = pending
            .into_iter()
            .filter_map(|d| self.sanitize(d))
            .collect();
    }

    fn sanitize(&self, mut descriptor: NodeDescriptor) -> Option<NodeDescriptor> {
        let id = descriptor.endpoint_addr.id;
        if descriptor.validate().is_err()
            || descriptor.chain_id != self.chain_id
            || self.own == Some(id)
            || self.pinned.contains_key(&id)
        {
            return None;
        }
        if self.relay {
            descriptor.endpoint_addr = EndpointAddr::new(id);
        }
        Some(descriptor)
    }

    /// An announced or shared endpoint, to dial before it is trusted to share.
    pub fn learn(&mut self, descriptor: NodeDescriptor) {
        let Some(descriptor) = self.sanitize(descriptor) else {
            return;
        };
        let id = descriptor.endpoint_addr.id;
        if self.known.contains_key(&id) || self.pending.iter().any(|d| d.endpoint_addr.id == id) {
            return;
        }
        if self.pending.len() >= MAX_PENDING {
            // A random victim: whoever floods the queue cannot also choose
            // which honest endpoints it pushes out.
            let victim = rand::thread_rng().gen_range(0..self.pending.len());
            self.pending.remove(victim);
        }
        self.pending.push_back(descriptor);
    }

    pub fn next_pending(&mut self) -> Option<NodeDescriptor> {
        self.pending.pop_front()
    }

    /// The endpoint authenticated with the expected ML-DSA key.
    pub fn verified(&mut self, descriptor: NodeDescriptor, now: u64) {
        let Some(descriptor) = self.sanitize(descriptor) else {
            return;
        };
        let id = descriptor.endpoint_addr.id;
        if !self.known.contains_key(&id) && self.known.len() >= MAX_KNOWN {
            let worst = self
                .known
                .iter()
                .filter(|(id, _)| !self.connected.contains(*id))
                .max_by_key(|(_, e)| (e.failures, std::cmp::Reverse(e.last_ok)))
                .map(|(id, _)| *id);
            match worst {
                Some(worst) => {
                    self.known.remove(&worst);
                }
                None => return,
            }
        }
        self.known.insert(
            id,
            Entry {
                descriptor,
                last_ok: now,
                failures: 0,
                retry_at: 0,
            },
        );
        self.dirty = true;
    }

    /// A known endpoint could not be reached or authenticated.
    pub fn failed(&mut self, id: EndpointId, now: u64) {
        let Some(entry) = self.known.get_mut(&id) else {
            return;
        };
        entry.failures = entry.failures.saturating_add(1);
        let delay = RETRY_BASE_SECS
            .saturating_mul(1 << entry.failures.min(16))
            .min(RETRY_MAX_SECS);
        entry.retry_at = now.saturating_add(delay);
        if entry.failures >= FORGET_FAILURES && now.saturating_sub(entry.last_ok) >= FORGET_SECS {
            self.known.remove(&id);
        }
        self.dirty = true;
    }

    /// A session that had authenticated ended; wait a little before redialing.
    pub fn ended(&mut self, id: EndpointId, now: u64) {
        if let Some(entry) = self.known.get_mut(&id) {
            entry.retry_at = now.saturating_add(RETRY_BASE_SECS);
        }
    }

    /// A random known endpoint that is due and not in a session.
    pub fn candidate(&self, now: u64, busy: &BTreeSet<EndpointId>) -> Option<NodeDescriptor> {
        let due: Vec<_> = self
            .known
            .iter()
            .filter(|(id, e)| {
                e.retry_at <= now && !busy.contains(*id) && !self.connected.contains(*id)
            })
            .map(|(_, e)| &e.descriptor)
            .collect();
        due.choose(&mut rand::thread_rng()).map(|d| (*d).clone())
    }

    /// What `PEERS` answers: a random sample of endpoints that authenticated
    /// within a day and have not failed since, and of pinned peers in session.
    pub fn share(&self, limit: usize, now: u64) -> Vec<NodeDescriptor> {
        let mut fresh: Vec<_> = self
            .known
            .values()
            .filter(|e| e.failures == 0 && now.saturating_sub(e.last_ok) <= SHARE_WINDOW_SECS)
            .map(|e| e.descriptor.clone())
            .chain(
                self.pinned
                    .iter()
                    .filter(|(id, _)| self.connected.contains(*id))
                    .map(|(_, peer)| peer.clone()),
            )
            .collect();
        fresh.shuffle(&mut rand::thread_rng());
        fresh.truncate(limit);
        fresh
    }

    pub fn connected(&mut self, id: EndpointId) {
        self.connected.insert(id);
    }
    pub fn disconnected(&mut self, id: EndpointId) {
        self.connected.remove(&id);
        self.observed.remove(&id);
    }
    pub fn observe(&mut self, id: EndpointId, height: u64) {
        if self.connected.contains(&id) {
            self.observed.insert(id, height);
        }
    }
    pub fn known_len(&self) -> usize {
        self.known.len()
    }
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }
    pub fn outbound_len(&self) -> usize {
        self.connected.len()
    }
}

/// The book plus the inbound session count, shared by the server, the
/// outbound loops and the API.
pub struct Peers {
    book: Mutex<PeerBook>,
    inbound: AtomicUsize,
}

impl Peers {
    pub fn new(book: PeerBook) -> Arc<Self> {
        Arc::new(Self {
            book: Mutex::new(book),
            inbound: AtomicUsize::new(0),
        })
    }
    /// Book methods never panic while holding it, so a poisoned lock still
    /// holds a consistent table.
    pub fn book(&self) -> MutexGuard<'_, PeerBook> {
        self.book.lock().unwrap_or_else(|p| p.into_inner())
    }
    /// What a checkpoint authority needs before it signs: some peer is in
    /// session and no outbound peer reported a higher tip. A node alone would
    /// otherwise sign the branch it mines by itself and bind the network to it.
    pub fn confirm_tip(&self, height: u64) -> bool {
        let book = self.book();
        let connected = book.outbound_len() > 0 || self.inbound.load(Ordering::Relaxed) > 0;
        connected && book.observed.values().all(|seen| *seen <= height)
    }
    /// Counts one inbound session for as long as the guard lives.
    pub fn inbound(self: &Arc<Self>) -> Inbound {
        self.inbound.fetch_add(1, Ordering::Relaxed);
        Inbound(self.clone())
    }
    pub fn status(&self) -> serde_json::Value {
        let book = self.book();
        serde_json::json!({
            "outbound": book.outbound_len(),
            "inbound": self.inbound.load(Ordering::Relaxed),
            "known": book.known_len(),
            "pending": book.pending_len(),
            "relay": book.relay,
            // The highest tip an outbound peer reported: how far this node
            // still has to sync. Zero without outbound sessions.
            "best_height": book.observed.values().copied().max().unwrap_or(0),
        })
    }
}

pub struct Inbound(Arc<Peers>);
impl Drop for Inbound {
    fn drop(&mut self) {
        self.0.inbound.fetch_sub(1, Ordering::Relaxed);
    }
}

/// The published network document: which chain, and the seeds a new node
/// dials first. Embedded in the binary for public chains.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: String,
    pub status: String,
    pub chain_id: String,
    pub alpn: String,
    #[serde(default)]
    pub transport: Option<String>,
    pub seed_nodes: Vec<SeedNode>,
    #[serde(default)]
    pub notes: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeedNode {
    pub label: String,
    pub operator: String,
    /// iroh endpoint id (hex). With relays on, address lookup finds the rest.
    pub endpoint_id: String,
    /// [`super::stark_net::key_id`] of the seed's ML-DSA-65 key.
    pub ml_dsa_key_id: String,
    #[serde(default)]
    pub relay_url: Option<String>,
    /// For nodes without relays (local networks and tests).
    #[serde(default)]
    pub direct_addresses: Vec<SocketAddr>,
    pub status: String,
    #[serde(default)]
    pub note: Option<String>,
}

impl Manifest {
    pub fn parse(bytes: &[u8], profile: Profile) -> Result<Self, String> {
        let manifest: Self = serde_json::from_slice(bytes).map_err(err)?;
        if manifest.schema != MANIFEST_SCHEMA {
            return Err("not a STARK network manifest".into());
        }
        if manifest.chain_id != profile.chain_id {
            return Err(format!(
                "this manifest is for {}, the database is {}",
                manifest.chain_id, profile.chain_id
            ));
        }
        if manifest.alpn.as_bytes() != ALPN {
            return Err("manifest protocol is not this node's".into());
        }
        if !matches!(manifest.status.as_str(), "pre-genesis" | "active") {
            return Err("manifest status must be pre-genesis or active".into());
        }
        if manifest.seed_nodes.len() > MAX_SEEDS {
            return Err(format!("a manifest lists at most {MAX_SEEDS} seeds"));
        }
        manifest.seeds()?;
        Ok(manifest)
    }

    /// The manifest compiled into this binary, if the chain is public.
    pub fn builtin(profile: Profile) -> Result<Option<Self>, String> {
        if profile == Profile::TESTNET {
            Self::parse(TESTNET_MANIFEST.as_bytes(), profile).map(Some)
        } else {
            Ok(None)
        }
    }

    /// Descriptors of the active seeds.
    pub fn seeds(&self) -> Result<Vec<NodeDescriptor>, String> {
        let mut seen = BTreeSet::new();
        let mut seeds = Vec::new();
        for seed in &self.seed_nodes {
            if !matches!(seed.status.as_str(), "active" | "retired") {
                return Err(format!("seed {}: status must be active or retired", seed.label));
            }
            let id = EndpointId::from_str(&seed.endpoint_id)
                .map_err(|e| format!("seed {}: endpoint id: {e}", seed.label))?;
            if !seen.insert(id) {
                return Err(format!("seed {}: repeated endpoint id", seed.label));
            }
            let mut address = EndpointAddr::new(id);
            if let Some(url) = &seed.relay_url {
                address = address.with_relay_url(
                    RelayUrl::from_str(url).map_err(|e| format!("seed {}: {e}", seed.label))?,
                );
            }
            for socket in &seed.direct_addresses {
                address = address.with_ip_addr(*socket);
            }
            let descriptor = NodeDescriptor {
                version: DESCRIPTOR_VERSION,
                chain_id: self.chain_id.clone(),
                endpoint_addr: address,
                ml_dsa_key_id: seed.ml_dsa_key_id.clone(),
            };
            descriptor
                .validate()
                .map_err(|e| format!("seed {}: {e}", seed.label))?;
            if seed.status == "active" {
                seeds.push(descriptor);
            }
        }
        Ok(seeds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quantic::stark_net::ServerIdentity;

    const P: Profile = Profile::TESTNET;

    fn peer(seed: u8) -> NodeDescriptor {
        let identity = ServerIdentity::from_seed(&[seed; 32]).unwrap();
        NodeDescriptor {
            version: DESCRIPTOR_VERSION,
            chain_id: P.chain_id.into(),
            endpoint_addr: EndpointAddr::new(identity.endpoint_id())
                .with_ip_addr("203.0.113.7:4433".parse().unwrap()),
            ml_dsa_key_id: identity.key_id(),
        }
    }
    fn id(seed: u8) -> EndpointId {
        peer(seed).endpoint_addr.id
    }

    #[test]
    fn learned_endpoints_wait_for_a_dial_and_never_name_this_node_or_a_pin() {
        let mut book = PeerBook::new(P);
        book.configure(id(1), true, &[peer(2)]);
        book.learn(peer(1));
        book.learn(peer(2));
        let mut other_chain = peer(3);
        other_chain.chain_id = Profile::REGTEST.chain_id.into();
        book.learn(other_chain);
        assert_eq!(book.pending_len(), 0);

        book.learn(peer(3));
        book.learn(peer(3));
        assert_eq!(book.pending_len(), 1);
        assert!(book.share(16, 100).is_empty(), "unverified is never shared");
        book.connected(id(2));
        let shared = book.share(16, 100);
        assert_eq!(shared.len(), 1, "a pinned peer in session is shared");
        assert!(shared[0].endpoint_addr.addrs.is_empty());
        book.disconnected(id(2));
        let pending = book.next_pending().unwrap();
        // Relay mode: only the id is kept, address lookup finds the rest.
        assert!(pending.endpoint_addr.addrs.is_empty());
        book.verified(pending, 100);
        assert_eq!(book.share(16, 100).len(), 1);
        assert_eq!(book.share(16, 100 + SHARE_WINDOW_SECS + 1).len(), 0);
        book.learn(peer(3));
        assert_eq!(book.pending_len(), 0, "already known");

        let mut direct = PeerBook::new(P);
        direct.configure(id(1), false, &[]);
        direct.learn(peer(4));
        assert_eq!(direct.next_pending().unwrap().endpoint_addr.addrs.len(), 1);
    }

    #[test]
    fn the_book_is_bounded_backs_off_and_forgets_dead_peers() {
        let mut book = PeerBook::new(P);
        book.configure(id(0), true, &[]);
        for seed in 1..=(MAX_PENDING + 10) as u8 {
            book.learn(peer(seed));
        }
        assert_eq!(book.pending_len(), MAX_PENDING);

        let mut book = PeerBook::new(P);
        book.configure(id(0), true, &[]);
        book.verified(peer(1), 1_000);
        book.failed(id(1), 1_000);
        assert!(book.candidate(1_000, &BTreeSet::new()).is_none());
        assert!(book.candidate(1_000 + 2 * RETRY_BASE_SECS, &BTreeSet::new()).is_some());
        assert!(book.share(16, 1_000).is_empty(), "a failing peer is not shared");
        for _ in 1..FORGET_FAILURES {
            book.failed(id(1), 2_000);
        }
        assert_eq!(book.known_len(), 1, "failures alone, within a week, keep it");
        book.failed(id(1), 1_000 + FORGET_SECS);
        assert_eq!(book.known_len(), 0);

        let mut book = PeerBook::new(P);
        book.configure(id(0), true, &[]);
        book.verified(peer(1), 10);
        book.connected(id(1));
        for seed in 2..=(MAX_KNOWN + 1) as u16 {
            let mut root = [7u8; 32];
            root[..2].copy_from_slice(&seed.to_be_bytes());
            let identity = ServerIdentity::from_seed(&root).unwrap();
            book.verified(
                NodeDescriptor {
                    version: DESCRIPTOR_VERSION,
                    chain_id: P.chain_id.into(),
                    endpoint_addr: EndpointAddr::new(identity.endpoint_id()),
                    ml_dsa_key_id: identity.key_id(),
                },
                20 + seed as u64,
            );
        }
        assert_eq!(book.known_len(), MAX_KNOWN);
        assert!(
            book.known.contains_key(&id(1)),
            "a peer in session is never evicted"
        );
        assert!(book.candidate(1_000, &BTreeSet::new()).is_some());
    }

    #[test]
    fn an_authority_needs_a_peer_and_none_ahead_to_confirm_its_tip() {
        let peers = Peers::new(PeerBook::new(P));
        assert!(!peers.confirm_tip(10), "alone");
        let inbound = peers.inbound();
        assert!(peers.confirm_tip(10));
        drop(inbound);
        peers.book().connected(id(1));
        peers.book().observe(id(1), 12);
        assert!(!peers.confirm_tip(10), "a peer is ahead");
        assert!(peers.confirm_tip(12));
        peers.book().disconnected(id(1));
        peers.book().observe(id(1), 99);
        assert!(!peers.confirm_tip(12), "nobody in session");
    }

    #[test]
    fn the_book_survives_a_restart_and_ignores_another_chain() {
        let dir = tempfile::tempdir().unwrap();
        let storage = RocksDbStorage::new(dir.path()).unwrap();
        let mut book = PeerBook::load(&storage, P);
        book.configure(id(0), false, &[]);
        assert!(book.snapshot().is_none(), "nothing to save");
        book.verified(peer(5), 50);
        PeerBook::save(book.snapshot(), &storage).unwrap();
        let mut reopened = PeerBook::load(&storage, P);
        reopened.configure(id(0), false, &[]);
        assert_eq!(reopened.share(16, 60), vec![peer(5)]);
        assert_eq!(PeerBook::load(&storage, Profile::REGTEST).known_len(), 0);
        storage.put_raw(BOOK_KEY, b"{not json".to_vec()).unwrap();
        let mut broken = PeerBook::load(&storage, P);
        assert_eq!(broken.known_len(), 0);
        assert!(broken.snapshot().is_some(), "an unreadable book is rewritten");
    }

    #[test]
    fn manifests_pin_seeds_by_id_and_key_and_the_builtin_one_parses() {
        let builtin = Manifest::builtin(P).unwrap().unwrap();
        assert_eq!(builtin.chain_id, P.chain_id);
        // Launched: the team's PC is seed1, pinned by id and key alone (n0
        // address lookup finds the rest).
        let seeds = builtin.seeds().unwrap();
        assert_eq!(seeds.len(), 1);
        assert_eq!(
            seeds[0].endpoint_addr.id.to_string(),
            "e3bf05e22b0f491d74e07028ffa02db833bc9d06f441acb820cbf723c7eb70c4"
        );
        assert!(seeds[0].endpoint_addr.addrs.is_empty());
        assert!(Manifest::builtin(Profile::REGTEST).unwrap().is_none());

        let seed = peer(9);
        let json = |status: &str, key: &str| {
            serde_json::json!({
                "schema": MANIFEST_SCHEMA, "status": "active", "chain_id": P.chain_id,
                "alpn": std::str::from_utf8(ALPN).unwrap(),
                "seed_nodes": [{"label": "seed1", "operator": "test",
                    "endpoint_id": seed.endpoint_addr.id.to_string(),
                    "ml_dsa_key_id": key, "status": status,
                    "direct_addresses": ["127.0.0.1:4433"]}]
            })
            .to_string()
        };
        let manifest = Manifest::parse(json("active", &seed.ml_dsa_key_id).as_bytes(), P).unwrap();
        let seeds = manifest.seeds().unwrap();
        assert_eq!(seeds.len(), 1);
        assert_eq!(seeds[0].endpoint_addr.id, seed.endpoint_addr.id);
        assert_eq!(seeds[0].ml_dsa_key_id, seed.ml_dsa_key_id);
        let retired = Manifest::parse(json("retired", &seed.ml_dsa_key_id).as_bytes(), P).unwrap();
        assert!(retired.seeds().unwrap().is_empty());
        assert!(Manifest::parse(json("active", "00").as_bytes(), P).is_err());
        assert!(Manifest::parse(json("maybe", &seed.ml_dsa_key_id).as_bytes(), P).is_err());
        assert!(Manifest::parse(json("active", &seed.ml_dsa_key_id).as_bytes(), Profile::REGTEST).is_err());
    }
}
