use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const PEER_STORE_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PeerStoreEntry {
    pub peer_id: Option<String>,
    pub addrs: Vec<String>,
    #[serde(default)]
    pub protocol_version: Option<String>,
    #[serde(default)]
    pub protocols: Vec<String>,
    pub last_connected_at: u64,
    pub last_failed_at: u64,
    pub failures: u32,
    pub score: i32,
    pub source: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PeerStoreFile {
    pub version: u32,
    pub peers: Vec<PeerStoreEntry>,
}

impl Default for PeerStoreFile {
    fn default() -> Self {
        Self {
            version: PEER_STORE_VERSION,
            peers: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct PeerStore {
    path: PathBuf,
    file: PeerStoreFile,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn normalize_addr(addr: &str) -> Option<String> {
    let addr = addr.trim();
    if addr.is_empty() {
        return None;
    }
    Some(addr.to_string())
}

impl PeerStore {
    pub fn empty(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            file: PeerStoreFile::default(),
        }
    }

    pub fn load_or_default(path: impl Into<PathBuf>) -> io::Result<Self> {
        let path = path.into();
        if !path.exists() {
            return Ok(Self {
                path,
                file: PeerStoreFile::default(),
            });
        }
        let raw = fs::read_to_string(&path)?;
        let mut file = serde_json::from_str::<PeerStoreFile>(&raw)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        file.peers.retain(|peer| !peer.addrs.is_empty());
        Ok(Self { path, file })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn len(&self) -> usize {
        self.file.peers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.file.peers.is_empty()
    }

    pub fn upsert_addr(&mut self, addr: &str, source: &str) -> bool {
        let Some(addr) = normalize_addr(addr) else {
            return false;
        };
        if self
            .file
            .peers
            .iter()
            .any(|peer| peer.addrs.iter().any(|known| known == &addr))
        {
            return false;
        }
        self.file.peers.push(PeerStoreEntry {
            peer_id: None,
            addrs: vec![addr],
            protocol_version: None,
            protocols: Vec::new(),
            last_connected_at: 0,
            last_failed_at: 0,
            failures: 0,
            score: 0,
            source: source.to_string(),
        });
        true
    }

    pub fn mark_connected(&mut self, addr: &str, peer_id: Option<&str>, source: &str) {
        let Some(addr) = normalize_addr(addr) else {
            return;
        };
        let now = now_secs();
        if let Some(entry) = self
            .file
            .peers
            .iter_mut()
            .find(|peer| peer.addrs.iter().any(|known| known == &addr))
        {
            if let Some(peer_id) = peer_id {
                entry.peer_id = Some(peer_id.to_string());
            }
            entry.last_connected_at = now;
            entry.failures = 0;
            entry.score = entry.score.saturating_add(10).min(100);
            if !source.is_empty() {
                entry.source = source.to_string();
            }
            return;
        }

        self.file.peers.push(PeerStoreEntry {
            peer_id: peer_id.map(str::to_string),
            addrs: vec![addr],
            protocol_version: None,
            protocols: Vec::new(),
            last_connected_at: now,
            last_failed_at: 0,
            failures: 0,
            score: 10,
            source: source.to_string(),
        });
    }

    pub fn mark_identified(
        &mut self,
        addr: &str,
        peer_id: Option<&str>,
        protocol_version: Option<&str>,
        protocols: &[String],
        source: &str,
    ) {
        self.mark_connected(addr, peer_id, source);
        let Some(addr) = normalize_addr(addr) else {
            return;
        };
        let Some(entry) = self
            .file
            .peers
            .iter_mut()
            .find(|peer| peer.addrs.iter().any(|known| known == &addr))
        else {
            return;
        };
        if let Some(version) = protocol_version.filter(|value| !value.trim().is_empty()) {
            entry.protocol_version = Some(version.to_string());
        }
        if !protocols.is_empty() {
            let mut protocols = protocols
                .iter()
                .map(|protocol| protocol.trim().to_string())
                .filter(|protocol| !protocol.is_empty())
                .collect::<Vec<_>>();
            protocols.sort();
            protocols.dedup();
            entry.protocols = protocols;
        }
    }

    pub fn mark_failed(&mut self, addr: &str) {
        let Some(addr) = normalize_addr(addr) else {
            return;
        };
        let now = now_secs();
        if let Some(entry) = self
            .file
            .peers
            .iter_mut()
            .find(|peer| peer.addrs.iter().any(|known| known == &addr))
        {
            entry.last_failed_at = now;
            entry.failures = entry.failures.saturating_add(1);
            entry.score = entry.score.saturating_sub(5).max(-100);
        }
    }

    pub fn dial_addrs(&self, max: usize) -> Vec<String> {
        let mut ranked = self.file.peers.clone();
        ranked.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| b.last_connected_at.cmp(&a.last_connected_at))
                .then_with(|| a.failures.cmp(&b.failures))
        });
        ranked
            .into_iter()
            .flat_map(|peer| peer.addrs.into_iter())
            .filter(|addr| !addr.trim().is_empty())
            // Entries saved by an older build (before discovered addresses were
            // filtered at insert time) can still have datacenter-internal/
            // loopback/link-local addresses on disk. Re-check on read so a
            // stale peer_store.json self-heals instead of redialing an
            // unreachable seed's 10.x interface forever.
            .filter(|addr| crate::network::p2p::discovered_addr_is_publicly_dialable(addr))
            .take(max)
            .collect()
    }

    /// Drops entries that failed repeatedly a long time ago and never
    /// connected successfully since, and caps total size so a long-running
    /// node's peer store can't grow without bound. A peer that failed many
    /// times but connected recently is deliberately kept - past failures
    /// (e.g. a network blip) shouldn't outweigh recent proven usefulness.
    pub fn prune(&mut self) {
        const MAX_ENTRIES: usize = 500;
        const STALE_FAILURE_THRESHOLD: u32 = 10;
        const SUCCESS_GRACE_SECS: u64 = 30 * 24 * 3600;

        let now = now_secs();
        self.file.peers.retain(|entry| {
            // Staleness is measured from the last *success*, not the last
            // failure. Keying it off the last failure made dead addresses
            // permanently unprunable: the node keeps retrying them, every
            // retry refreshes `last_failed_at`, so "failed long ago" was
            // never true for exactly the addresses we most wanted gone. That
            // is why decommissioned bootstrap IPs kept being dialed for
            // weeks after the servers were destroyed. What earns an address
            // its place is having been useful, not having been tried.
            let failing_badly = entry.failures >= STALE_FAILURE_THRESHOLD;
            let never_useful = entry.last_connected_at == 0;
            let long_since_useful = entry.last_connected_at > 0
                && now.saturating_sub(entry.last_connected_at) >= SUCCESS_GRACE_SECS;
            !(failing_badly && (never_useful || long_since_useful))
        });

        if self.file.peers.len() > MAX_ENTRIES {
            self.file.peers.sort_by(|a, b| {
                b.score
                    .cmp(&a.score)
                    .then_with(|| b.last_connected_at.cmp(&a.last_connected_at))
            });
            self.file.peers.truncate(MAX_ENTRIES);
        }
    }

    pub fn save(&mut self) -> io::Result<()> {
        self.prune();
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut by_addr = BTreeMap::<String, PeerStoreEntry>::new();
        for peer in &self.file.peers {
            for addr in &peer.addrs {
                if let Some(addr) = normalize_addr(addr) {
                    let mut entry = peer.clone();
                    entry.addrs = vec![addr.clone()];
                    by_addr.insert(addr, entry);
                }
            }
        }
        let file = PeerStoreFile {
            version: PEER_STORE_VERSION,
            peers: by_addr.into_values().collect(),
        };
        let raw = serde_json::to_string_pretty(&file)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        fs::write(&self.path, raw)
    }
}

/// El peerstore de Zenith, el de siempre (`peers.json`).
pub fn default_peer_store_path() -> PathBuf {
    peer_store_path_named(&peer_store_file_name(None))
}

/// El peerstore de una red de Zoka Quantic (`quantic-testnet`, …): uno propio
/// por red. Compartían `peers.json` con Zenith, y un nodo de la testnet de
/// Quantic marcaba en cada arranque las onions de Zenith del puerto 28080
/// (23/10/2026): las redes no se mezclan ni en a quién se llama.
pub fn quantic_peer_store_path(network_name: &str) -> PathBuf {
    peer_store_path_named(&peer_store_file_name(Some(network_name)))
}

/// `peers.json` para Zenith, `peers-<red>.json` para una red de Quantic.
pub fn peer_store_file_name(quantic_network: Option<&str>) -> String {
    match quantic_network {
        Some(network) => format!("peers-{network}.json"),
        None => "peers.json".to_string(),
    }
}

/// `ZOKA_PEER_STORE` manda (un archivo elegido a mano, para cualquier red);
/// si no, el archivo va en `ZOKA_HOME` o en `~/.zoka`.
fn peer_store_path_named(file_name: &str) -> PathBuf {
    if let Ok(path) = std::env::var("ZOKA_PEER_STORE") {
        let path = path.trim();
        if !path.is_empty() {
            return PathBuf::from(path);
        }
    }
    if let Ok(home) = std::env::var("ZOKA_HOME") {
        let home = home.trim();
        if !home.is_empty() {
            return PathBuf::from(home).join(file_name);
        }
    }
    home::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".zoka")
        .join(file_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_quantic_network_has_its_own_peer_store_apart_from_zenith() {
        assert_eq!(peer_store_file_name(None), "peers.json");
        assert_eq!(peer_store_file_name(Some("quantic-testnet")), "peers-quantic-testnet.json");
        assert_ne!(
            peer_store_file_name(Some("quantic-testnet")),
            peer_store_file_name(Some("quantic-mainnet"))
        );
    }

    #[test]
    fn dial_addrs_filters_out_stale_private_entries() {
        // A file written by an older build (before discovered addresses were
        // filtered at insert time) can still have a datacenter-internal
        // address on disk. dial_addrs must not hand that back out to be
        // redialed forever.
        let path = std::env::temp_dir().join(format!(
            "zoka-peer-store-test-stale-{}.json",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);
        let mut store = PeerStore::load_or_default(&path).unwrap();
        store.upsert_addr("/ip4/10.10.0.5/tcp/28080", "identify");
        store.upsert_addr("/ip4/203.0.113.9/tcp/28080", "seed");

        let addrs = store.dial_addrs(8);
        assert_eq!(addrs, vec!["/ip4/203.0.113.9/tcp/28080".to_string()]);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn prune_drops_addresses_that_keep_failing_and_never_connected() {
        // Regression: decommissioned bootstrap servers kept being dialed for
        // weeks after being destroyed. Staleness used to be measured from the
        // last *failure*, but the node retries these addresses constantly and
        // every retry refreshed that timestamp, so "failed long ago" was never
        // true and they could never age out.
        let path = std::env::temp_dir().join(format!(
            "zoka-peer-store-test-deadip-{}.json",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);
        let mut store = PeerStore::load_or_default(&path).unwrap();

        let dead = "/ip4/159.223.119.216/tcp/28080";
        let alive = "/ip4/203.0.113.7/tcp/28080";
        store.upsert_addr(dead, "seed");
        store.upsert_addr(alive, "seed");
        store.mark_identified(alive, Some("peer-alive"), None, &[], "identify");

        // Dead host: fails on every dial attempt, right up to this moment.
        for _ in 0..12 {
            store.mark_failed(dead);
        }

        store.prune();
        let addrs = store.dial_addrs(8);
        assert!(
            !addrs.iter().any(|a| a == dead),
            "a host that has failed 12 times and never once connected must be dropped, got {addrs:?}"
        );
        assert!(
            addrs.iter().any(|a| a == alive),
            "a peer that did connect must be kept, got {addrs:?}"
        );
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn prune_keeps_a_peer_that_failed_a_lot_but_connected_recently() {
        // The flip side: a peer having a bad night is not the same as a peer
        // that no longer exists.
        let path = std::env::temp_dir().join(format!(
            "zoka-peer-store-test-flaky-{}.json",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);
        let mut store = PeerStore::load_or_default(&path).unwrap();

        let flaky = "/ip4/203.0.113.8/tcp/28080";
        store.upsert_addr(flaky, "seed");
        for _ in 0..15 {
            store.mark_failed(flaky);
        }
        store.mark_identified(flaky, Some("peer-flaky"), None, &[], "identify");

        store.prune();
        assert!(
            store.dial_addrs(8).iter().any(|a| a == flaky),
            "recent proven usefulness must outweigh past failures"
        );
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn peer_store_ranks_connected_peers_first() {
        let path =
            std::env::temp_dir().join(format!("zoka-peer-store-test-{}.json", std::process::id()));
        let _ = fs::remove_file(&path);
        let mut store = PeerStore::load_or_default(&path).unwrap();
        store.upsert_addr("/ip4/203.0.113.1/tcp/28080", "seed");
        store.upsert_addr("/ip4/203.0.113.2/tcp/28080", "identify");
        store.mark_failed("/ip4/203.0.113.1/tcp/28080");
        store.mark_identified(
            "/ip4/203.0.113.2/tcp/28080",
            Some("peer-2"),
            Some("/zoka/1.0.2-mainnet-zenith"),
            &["/zoka/messages/v1".to_string(), "/zoka/pbft/v1".to_string()],
            "identify",
        );

        let addrs = store.dial_addrs(2);
        assert_eq!(
            addrs.first().map(String::as_str),
            Some("/ip4/203.0.113.2/tcp/28080")
        );
        store.save().unwrap();

        let loaded = PeerStore::load_or_default(&path).unwrap();
        assert_eq!(loaded.len(), 2);
        let identified = loaded
            .file
            .peers
            .iter()
            .find(|peer| peer.peer_id.as_deref() == Some("peer-2"))
            .expect("identified peer");
        assert_eq!(
            identified.protocol_version.as_deref(),
            Some("/zoka/1.0.2-mainnet-zenith")
        );
        assert!(identified
            .protocols
            .iter()
            .any(|protocol| protocol == "/zoka/messages/v1"));
        let _ = fs::remove_file(&path);
    }
}
