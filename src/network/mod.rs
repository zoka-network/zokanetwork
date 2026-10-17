//! Red P2P Privada para ZOKA Blockchain
//!
//! Proporciona:
//! - ComunicaciÃ³n P2P encriptada con Noise protocol
//! - Descubrimiento de pares usando DHT (Kademlia)
//! - Protocolo de mensajes privado
//! - GestiÃ³n de conexiones e identidad

pub mod block_request_response;
pub mod handler;
pub mod header_request_response;
pub mod private_tx_bodies_request_response;
pub mod private_tx_digests_request_response;
pub mod wallet_digest_request_response;
pub mod iroh_snapshot_transfer;
pub mod message;
pub mod p2p;
pub mod peer_store;
pub mod private_tx_request_response;
pub mod quantic_light_request_response;
pub mod snapshot_request_response;

#[allow(unused_imports)]
pub use handler::MessageHandler;
#[allow(unused_imports)]
pub use message::ZokaMessage;
#[allow(unused_imports)]
pub use p2p::NetworkPrivacyConfig;
#[allow(unused_imports)]
pub use p2p::Node;

/// ConfiguraciÃ³n del nodo P2P
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct NodeConfig {
    /// Puerto de escucha
    pub listen_port: u16,
    /// Direcciones de bootstrap (peers conocidos)
    pub bootstrap_peers: Vec<String>,
    /// MÃ¡ximo nÃºmero de conexiones
    pub max_connections: u32,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            listen_port: 9000,
            bootstrap_peers: vec![],
            max_connections: 100,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_config_default() {
        let config = NodeConfig::default();
        assert_eq!(config.listen_port, 9000);
        assert_eq!(config.max_connections, 100);
    }
}
