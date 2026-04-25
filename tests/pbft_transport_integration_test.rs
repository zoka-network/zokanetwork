#[cfg(test)]
mod pbft_transport_tests {
    use std::time::Duration;

    use tokio::time::timeout;

    use zoka::consensus::{PbftMessage, PbftMessageType};
    use zoka::network::message::{PbftNetworkMessage, ZokaMessage};
    use zoka::network::Node;

    async fn wait_connected_pair(node_a: &mut Node, node_b: &mut Node) -> Result<(), String> {
        let peer_a = node_a.peer_id();
        let peer_b = node_b.peer_id();

        let (res_a, res_b) = tokio::join!(
            timeout(
                Duration::from_secs(10),
                node_a.wait_for_peer_connection(peer_b)
            ),
            timeout(
                Duration::from_secs(10),
                node_b.wait_for_peer_connection(peer_a)
            ),
        );

        let res_a = res_a.map_err(|_| "node A connection timeout".to_string())?;
        let res_b = res_b.map_err(|_| "node B connection timeout".to_string())?;
        res_a.map_err(|e| format!("node A connection error: {e}"))?;
        res_b.map_err(|e| format!("node B connection error: {e}"))?;
        Ok(())
    }

    async fn run_propagation_attempt() -> Result<(), String> {
        let mut node_a = Node::new(0)
            .await
            .map_err(|e| format!("node A init: {e}"))?;
        let mut node_b = Node::new(0)
            .await
            .map_err(|e| format!("node B init: {e}"))?;

        node_a
            .listen("/ip4/127.0.0.1/tcp/0")
            .await
            .map_err(|e| format!("node A listen: {e}"))?;
        node_b
            .listen("/ip4/127.0.0.1/tcp/0")
            .await
            .map_err(|e| format!("node B listen: {e}"))?;

        let addr_a = timeout(Duration::from_secs(10), node_a.wait_for_listen_addr())
            .await
            .map_err(|_| "node A listen addr timeout".to_string())?
            .map_err(|e| format!("node A listen addr error: {e}"))?;
        let addr_b = timeout(Duration::from_secs(10), node_b.wait_for_listen_addr())
            .await
            .map_err(|_| "node B listen addr timeout".to_string())?
            .map_err(|e| format!("node B listen addr error: {e}"))?;

        node_a
            .connect_to_peer(&addr_b.to_string())
            .await
            .map_err(|e| format!("node A dial node B: {e}"))?;
        let _ = node_b.connect_to_peer(&addr_a.to_string()).await;

        wait_connected_pair(&mut node_a, &mut node_b).await?;
        node_a
            .wait_for_min_connected_peers(1, Duration::from_secs(10))
            .await
            .map_err(|e| format!("node A min-peer warmup: {e}"))?;
        node_b
            .wait_for_min_connected_peers(1, Duration::from_secs(10))
            .await
            .map_err(|e| format!("node B min-peer warmup: {e}"))?;

        node_a
            .pump_for(Duration::from_secs(2))
            .await
            .map_err(|e| format!("warmup pump node A: {e}"))?;
        node_b
            .pump_for(Duration::from_secs(2))
            .await
            .map_err(|e| format!("warmup pump node B: {e}"))?;

        let pbft = ZokaMessage::Pbft(PbftNetworkMessage::Consensus(PbftMessage::with_timestamp(
            PbftMessageType::Commit,
            2,
            42,
            "blockhash-42".to_string(),
            "n2".to_string(),
            10_000,
        )));

        node_a
            .publish_message_with_retry(&pbft, 220, Duration::from_millis(120))
            .await
            .map_err(|e| format!("failed to publish PBFT message after retries: {e}"))?;

        let (recv_result, _) = tokio::join!(
            timeout(Duration::from_secs(10), node_b.next_message()),
            node_a.pump_for(Duration::from_secs(2)),
        );
        let received = recv_result
            .map_err(|_| "receive timeout".to_string())?
            .map_err(|e| format!("receive error: {e}"))?;

        match received {
            ZokaMessage::Pbft(PbftNetworkMessage::Consensus(msg)) => {
                if msg.msg_type != PbftMessageType::Commit
                    || msg.view != 2
                    || msg.sequence != 42
                    || msg.block_hash != "blockhash-42"
                    || msg.sender != "n2"
                    || msg.timestamp_ms != 10_000
                {
                    return Err("PBFT payload mismatch".to_string());
                }
            }
            other => {
                return Err(format!("unexpected message variant: {other:?}"));
            }
        }

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_pbft_message_propagates_over_live_gossipsub() {
        let mut last_error = String::new();
        for attempt in 1..=3 {
            match run_propagation_attempt().await {
                Ok(()) => return,
                Err(err) => {
                    last_error = format!("attempt {attempt} failed: {err}");
                    tokio::time::sleep(Duration::from_millis(300)).await;
                }
            }
        }

        panic!("{last_error}");
    }
}
