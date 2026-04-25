// Integration tests for P2P network module
#[cfg(test)]
mod network_tests {
    use zoka::consensus::{PbftMessage, PbftMessageType, PbftNewView, PbftViewChange};
    use zoka::network::{
        discovery::PeerDiscovery, discovery::PeerInfo, handler::DefaultMessageHandler,
        message::PbftNetworkMessage, message::ZokaMessage, NodeConfig,
    };

    #[test]
    fn test_node_config() {
        let config = NodeConfig::default();
        assert_eq!(config.listen_port, 9000);
        assert_eq!(config.max_connections, 100);
        assert!(config.bootstrap_peers.is_empty());
    }

    #[test]
    fn test_peer_discovery() {
        let mut discovery = PeerDiscovery::new();

        let peer1 = PeerInfo::new("QmPeer1".to_string());
        discovery.add_peer(peer1);

        assert_eq!(discovery.peer_count(), 1);
        assert!(discovery.get_peer("QmPeer1").is_some());
        assert!(discovery.get_peer("QmNonexistent").is_none());
    }

    #[test]
    fn test_peer_reputation() {
        let mut discovery = PeerDiscovery::new();

        let mut peer = PeerInfo::new("QmTest".to_string());
        peer.reputation = 50;
        discovery.add_peer(peer);

        // Aumentar reputación
        discovery.get_peer("QmTest").unwrap(); // Solo lectura en este test

        // En implementación real, actualizaríamos aquí
        let active = discovery.get_active_peers();
        assert_eq!(active.len(), 1);
    }

    #[test]
    fn test_message_serialization() {
        use zoka::network::message::PingMessage;

        let ping = PingMessage {
            nonce: 42,
            timestamp: 1234567890,
        };

        let msg = ZokaMessage::Ping(ping);
        let bytes = msg.to_bytes().unwrap();
        let restored = ZokaMessage::from_bytes(&bytes).unwrap();

        assert_eq!(msg.timestamp(), restored.timestamp());
    }

    #[tokio::test]
    async fn test_message_handler() {
        use zoka::network::message::PingMessage;
        use zoka::network::MessageHandler;

        let handler = DefaultMessageHandler::new();

        let ping = PingMessage {
            nonce: 100,
            timestamp: 9999,
        };

        let msg = ZokaMessage::Ping(ping);
        let result = handler.handle_message(msg).await;

        assert!(result.is_ok());
        assert_eq!(handler.message_count().await, 1);
    }

    #[tokio::test]
    async fn test_multiple_message_handling() {
        use zoka::network::message::PingMessage;
        use zoka::network::MessageHandler;

        let handler = DefaultMessageHandler::new();

        for i in 0..10 {
            let ping = PingMessage {
                nonce: i,
                timestamp: 1000 + i as u64,
            };

            let msg = ZokaMessage::Ping(ping);
            handler.handle_message(msg).await.ok();
        }

        assert_eq!(handler.message_count().await, 10);
    }

    #[test]
    fn test_whitelist_peers() {
        let mut discovery = PeerDiscovery::new();

        let peer1 = PeerInfo::new("QmTrusted".to_string());
        discovery.add_peer(peer1);

        discovery.whitelist_peer("QmTrusted".to_string());

        assert!(discovery.is_whitelisted("QmTrusted"));
        assert!(!discovery.is_whitelisted("QmUntrusted"));

        let whitelisted = discovery.get_whitelisted_peers();
        assert_eq!(whitelisted.len(), 1);
    }

    #[test]
    fn test_pbft_network_serialization_roundtrip() {
        let consensus_msg = PbftMessage::with_timestamp(
            PbftMessageType::Commit,
            3,
            12,
            "0xblock".to_string(),
            "n2".to_string(),
            4242,
        );
        let msg = ZokaMessage::Pbft(PbftNetworkMessage::Consensus(consensus_msg));
        let bytes = msg.to_bytes().unwrap();
        let restored = ZokaMessage::from_bytes(&bytes).unwrap();
        assert_eq!(restored.timestamp(), 4242);

        let vc_msg = ZokaMessage::Pbft(PbftNetworkMessage::ViewChange(PbftViewChange::new(
            4,
            "n3".to_string(),
            5000,
        )));
        let vc_bytes = vc_msg.to_bytes().unwrap();
        let vc_restored = ZokaMessage::from_bytes(&vc_bytes).unwrap();
        assert_eq!(vc_restored.timestamp(), 5000);

        let nv_msg = ZokaMessage::Pbft(PbftNetworkMessage::NewView(PbftNewView::new(
            4,
            "n0".to_string(),
            vec!["n0".to_string(), "n1".to_string(), "n2".to_string()],
            5100,
        )));
        let nv_bytes = nv_msg.to_bytes().unwrap();
        let nv_restored = ZokaMessage::from_bytes(&nv_bytes).unwrap();
        assert_eq!(nv_restored.timestamp(), 5100);
    }
}
