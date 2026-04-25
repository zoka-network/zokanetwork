//! Peer Discovery using DHT (Kademlia)

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Información de un peer en la red
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PeerInfo {
    /// Identificador único del peer
    pub peer_id: String,
    /// Direcciones de red conocidas
    pub addresses: Vec<String>,
    /// Timestamp de último visto
    pub last_seen: u64,
    /// Reputación del peer (0-100)
    pub reputation: u8,
}

impl PeerInfo {
    /// Crear nueva información de peer
    pub fn new(peer_id: String) -> Self {
        Self {
            peer_id,
            addresses: Vec::new(),
            last_seen: 0,
            reputation: 50, // Reputación inicial neutral
        }
    }

    /// Agregar dirección al peer
    pub fn add_address(&mut self, addr: String) {
        if !self.addresses.contains(&addr) {
            self.addresses.push(addr);
        }
    }

    /// Actualizar reputación (aumentar si es confiable)
    pub fn increase_reputation(&mut self) {
        if self.reputation < 100 {
            self.reputation += 10;
        }
    }

    /// Disminuir reputación (disminuir si falla)
    pub fn decrease_reputation(&mut self) {
        if self.reputation > 0 {
            self.reputation -= 10;
        }
    }
}

/// Descubridor de peers
pub struct PeerDiscovery {
    known_peers: std::collections::HashMap<String, PeerInfo>,
    whitelist: HashSet<String>, // Peers confiables
}

impl PeerDiscovery {
    /// Crear nuevo descubridor
    pub fn new() -> Self {
        Self {
            known_peers: Default::default(),
            whitelist: HashSet::new(),
        }
    }

    /// Agregar peer a lista conocida
    pub fn add_peer(&mut self, peer_info: PeerInfo) {
        self.known_peers
            .insert(peer_info.peer_id.clone(), peer_info);
    }

    /// Obtener peer por ID
    pub fn get_peer(&self, peer_id: &str) -> Option<&PeerInfo> {
        self.known_peers.get(peer_id)
    }

    /// Obtener peers activos (reputación > 30)
    pub fn get_active_peers(&self) -> Vec<&PeerInfo> {
        self.known_peers
            .values()
            .filter(|p| p.reputation > 30)
            .collect()
    }

    /// Agregar peer a whitelist (confiable)
    pub fn whitelist_peer(&mut self, peer_id: String) {
        self.whitelist.insert(peer_id);
    }

    /// Verificar si peer está en whitelist
    pub fn is_whitelisted(&self, peer_id: &str) -> bool {
        self.whitelist.contains(peer_id)
    }

    /// Obtener peers en whitelist
    pub fn get_whitelisted_peers(&self) -> Vec<&PeerInfo> {
        self.whitelist
            .iter()
            .filter_map(|id| self.known_peers.get(id))
            .collect()
    }

    /// Número de peers conocidos
    pub fn peer_count(&self) -> usize {
        self.known_peers.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_peer_info_creation() {
        let peer = PeerInfo::new("QmTest1234".to_string());
        assert_eq!(peer.peer_id, "QmTest1234");
        assert_eq!(peer.reputation, 50);
    }

    #[test]
    fn test_add_address() {
        let mut peer = PeerInfo::new("QmTest".to_string());
        peer.add_address("/ip4/127.0.0.1/tcp/9000".to_string());
        assert_eq!(peer.addresses.len(), 1);

        // No agregar duplicados
        peer.add_address("/ip4/127.0.0.1/tcp/9000".to_string());
        assert_eq!(peer.addresses.len(), 1);
    }

    #[test]
    fn test_reputation() {
        let mut peer = PeerInfo::new("QmTest".to_string());
        assert_eq!(peer.reputation, 50);

        peer.increase_reputation();
        assert_eq!(peer.reputation, 60);

        peer.decrease_reputation();
        peer.decrease_reputation();
        assert_eq!(peer.reputation, 40);
    }

    #[test]
    fn test_peer_discovery() {
        let mut discovery = PeerDiscovery::new();

        let mut peer1 = PeerInfo::new("QmPeer1".to_string());
        peer1.add_address("/ip4/192.168.1.1/tcp/9000".to_string());

        discovery.add_peer(peer1);
        assert_eq!(discovery.peer_count(), 1);

        let peer = discovery.get_peer("QmPeer1");
        assert!(peer.is_some());
    }

    #[test]
    fn test_whitelist() {
        let mut discovery = PeerDiscovery::new();
        let peer = PeerInfo::new("QmPeer1".to_string());
        discovery.add_peer(peer);

        discovery.whitelist_peer("QmPeer1".to_string());
        assert!(discovery.is_whitelisted("QmPeer1"));
        assert!(!discovery.is_whitelisted("QmPeer2"));
    }

    #[test]
    fn test_active_peers() {
        let mut discovery = PeerDiscovery::new();

        let mut peer1 = PeerInfo::new("QmPeer1".to_string());
        peer1.reputation = 80;
        discovery.add_peer(peer1);

        let mut peer2 = PeerInfo::new("QmPeer2".to_string());
        peer2.reputation = 20;
        discovery.add_peer(peer2);

        let active = discovery.get_active_peers();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].peer_id, "QmPeer1");
    }
}
