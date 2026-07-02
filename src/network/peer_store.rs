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
            .take(max)
            .collect()
    }

    pub fn save(&self) -> io::Result<()> {
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

pub fn default_peer_store_path() -> PathBuf {
    if let Ok(path) = std::env::var("ZOKA_PEER_STORE") {
        let path = path.trim();
        if !path.is_empty() {
            return PathBuf::from(path);
        }
    }
    if let Ok(home) = std::env::var("ZOKA_HOME") {
        let home = home.trim();
        if !home.is_empty() {
            return PathBuf::from(home).join("peers.json");
        }
    }
    home::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".zoka")
        .join("peers.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_store_ranks_connected_peers_first() {
        let path =
            std::env::temp_dir().join(format!("zoka-peer-store-test-{}.json", std::process::id()));
        let _ = fs::remove_file(&path);
        let mut store = PeerStore::load_or_default(&path).unwrap();
        store.upsert_addr("/ip4/10.0.0.1/tcp/28080", "seed");
        store.upsert_addr("/ip4/10.0.0.2/tcp/28080", "identify");
        store.mark_failed("/ip4/10.0.0.1/tcp/28080");
        store.mark_identified(
            "/ip4/10.0.0.2/tcp/28080",
            Some("peer-2"),
            Some("/zoka/1.0.2-mainnet-zenith"),
            &["/zoka/messages/v1".to_string(), "/zoka/pbft/v1".to_string()],
            "identify",
        );

        let addrs = store.dial_addrs(2);
        assert_eq!(
            addrs.first().map(String::as_str),
            Some("/ip4/10.0.0.2/tcp/28080")
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
