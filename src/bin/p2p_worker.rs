use std::error::Error;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use tokio::time::sleep;

use zoka::consensus::{PbftMessage, PbftMessageType};
use zoka::network::message::{PbftNetworkMessage, ZokaMessage};
use zoka::network::Node;

#[derive(Debug, Clone)]
struct WorkerConfig {
    dial: Option<String>,
    send_pbft: u64,
    expect_pbft: u64,
    expect_sender_prefix: Option<String>,
    send_interval_ms: u64,
    run_ms: u64,
    sender_id: String,
    sequence_start: u64,
    addr_file: Option<PathBuf>,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            dial: None,
            send_pbft: 0,
            expect_pbft: 0,
            expect_sender_prefix: None,
            send_interval_ms: 150,
            run_ms: 15_000,
            sender_id: "worker".to_string(),
            sequence_start: 1,
            addr_file: None,
        }
    }
}

impl WorkerConfig {
    fn from_args() -> Result<Self, String> {
        let mut cfg = Self::default();
        let mut args = std::env::args().skip(1);

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--dial" => {
                    cfg.dial = Some(args.next().ok_or("--dial requires a value")?);
                }
                "--send-pbft" => {
                    let value = args.next().ok_or("--send-pbft requires a value")?;
                    cfg.send_pbft = value
                        .parse::<u64>()
                        .map_err(|_| "--send-pbft must be an integer".to_string())?;
                }
                "--expect-pbft" => {
                    let value = args.next().ok_or("--expect-pbft requires a value")?;
                    cfg.expect_pbft = value
                        .parse::<u64>()
                        .map_err(|_| "--expect-pbft must be an integer".to_string())?;
                }
                "--expect-sender-prefix" => {
                    cfg.expect_sender_prefix = Some(
                        args.next()
                            .ok_or("--expect-sender-prefix requires a value")?,
                    );
                }
                "--send-interval-ms" => {
                    let value = args.next().ok_or("--send-interval-ms requires a value")?;
                    cfg.send_interval_ms = value
                        .parse::<u64>()
                        .map_err(|_| "--send-interval-ms must be an integer".to_string())?;
                }
                "--run-ms" => {
                    let value = args.next().ok_or("--run-ms requires a value")?;
                    cfg.run_ms = value
                        .parse::<u64>()
                        .map_err(|_| "--run-ms must be an integer".to_string())?;
                }
                "--sender-id" => {
                    cfg.sender_id = args.next().ok_or("--sender-id requires a value")?;
                }
                "--sequence-start" => {
                    let value = args.next().ok_or("--sequence-start requires a value")?;
                    cfg.sequence_start = value
                        .parse::<u64>()
                        .map_err(|_| "--sequence-start must be an integer".to_string())?;
                }
                "--addr-file" => {
                    cfg.addr_file = Some(PathBuf::from(
                        args.next().ok_or("--addr-file requires a value")?,
                    ));
                }
                "--help" | "-h" => {
                    return Err(Self::help_text());
                }
                unknown => {
                    return Err(format!("Unknown arg: {}\n\n{}", unknown, Self::help_text()));
                }
            }
        }

        Ok(cfg)
    }

    fn help_text() -> String {
        [
            "Usage: p2p_worker [options]",
            "",
            "Options:",
            "  --dial <multiaddr>         Dial this peer after listening",
            "  --send-pbft <count>        Number of PBFT messages to publish",
            "  --expect-pbft <count>      Number of PBFT messages to receive before success",
            "  --expect-sender-prefix <p> Count only PBFT messages whose sender starts with prefix",
            "  --send-interval-ms <ms>    Delay between published PBFT messages (default: 150)",
            "  --run-ms <milliseconds>    Max runtime window (default: 15000)",
            "  --sender-id <id>           Sender id embedded in PBFT messages",
            "  --sequence-start <n>       Initial PBFT sequence number (default: 1)",
            "  --addr-file <path>         File path to write the local listen address",
        ]
        .join("\n")
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let cfg = match WorkerConfig::from_args() {
        Ok(cfg) => cfg,
        Err(help_or_err) => {
            eprintln!("{}", help_or_err);
            return Err("invalid worker args".into());
        }
    };

    let mut node = Node::new(0).await?;
    node.listen("/ip4/127.0.0.1/tcp/0").await?;
    let listen_addr = node.wait_for_listen_addr().await?;
    let dialable_addr = format!("{}/p2p/{}", listen_addr, node.peer_id());
    println!("LISTEN_ADDR={}", dialable_addr);

    if let Some(path) = cfg.addr_file.as_ref() {
        std::fs::write(path, dialable_addr.as_bytes())?;
    }

    if let Some(peer_addr) = cfg.dial.as_ref() {
        node.connect_to_peer(peer_addr).await?;
        let _ = tokio::time::timeout(Duration::from_secs(12), node.wait_for_any_connection()).await;
        let _ = tokio::time::timeout(
            Duration::from_secs(12),
            node.wait_for_min_connected_peers(1, Duration::from_secs(11)),
        )
        .await;

        // Give gossipsub time to exchange subscriptions and build a stable PBFT mesh.
        let mesh_deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < mesh_deadline {
            if node.pbft_subscribed_peer_count() > 0 {
                break;
            }
            node.pump_for(Duration::from_millis(120)).await?;
            sleep(Duration::from_millis(60)).await;
        }
        node.pump_for(Duration::from_millis(300)).await?;
    }

    let start = Instant::now();
    let runtime = Duration::from_millis(cfg.run_ms);
    let deadline = start + runtime;

    let mut sent = 0u64;
    for i in 0..cfg.send_pbft {
        let msg = PbftNetworkMessage::Consensus(PbftMessage::with_timestamp(
            PbftMessageType::Commit,
            1,
            cfg.sequence_start + i,
            format!("pbft-block-{}", cfg.sequence_start + i),
            cfg.sender_id.clone(),
            (i + 1) * 100,
        ));

        let mut published = false;
        let mut last_publish_error = String::new();
        for attempt in 0..120 {
            match node.publish_pbft(msg.clone()) {
                Ok(()) => {
                    published = true;
                    break;
                }
                Err(err) => {
                    last_publish_error = err.to_string();
                    if attempt % 20 == 0 {
                        eprintln!(
                            "publish retry attempt={} peers={} err={}",
                            attempt,
                            node.connected_peer_count(),
                            last_publish_error
                        );
                    }
                }
            }
            if Instant::now() >= deadline {
                break;
            }
            if node.connected_peer_count() == 0 {
                if let Some(peer_addr) = cfg.dial.as_ref() {
                    if attempt % 6 == 0 {
                        let _ = node.connect_to_peer(peer_addr).await;
                    }
                }
            }
            node.pump_for(Duration::from_millis(100)).await?;
            sleep(Duration::from_millis(50)).await;
        }

        if !published {
            return Err(format!(
                "failed to publish PBFT message {} (peers={}, last_error={})",
                i + 1,
                node.connected_peer_count(),
                last_publish_error
            )
            .into());
        }

        sent += 1;
        println!("SENT_COUNT={}", sent);
        node.pump_for(Duration::from_millis(150)).await?;
        if cfg.send_interval_ms > 0 {
            sleep(Duration::from_millis(cfg.send_interval_ms)).await;
        }
    }

    let mut received = 0u64;
    let success_linger = if cfg.expect_pbft <= 1 {
        Duration::from_secs(2)
    } else {
        Duration::from_secs(20)
    };
    let mut reached_expect_at: Option<Instant> = None;

    while Instant::now() < deadline {
        if received >= cfg.expect_pbft {
            let reached_at = reached_expect_at.get_or_insert_with(Instant::now);
            if reached_at.elapsed() >= success_linger {
                break;
            }
        }

        let now = Instant::now();
        let remaining = deadline.saturating_duration_since(now);
        if remaining.is_zero() {
            break;
        }

        let wait_budget = remaining.min(Duration::from_millis(600));
        match tokio::time::timeout(wait_budget, node.next_message()).await {
            Ok(Ok(ZokaMessage::Pbft(pbft_msg))) => {
                if let Some(prefix) = cfg.expect_sender_prefix.as_deref() {
                    let sender = match &pbft_msg {
                        PbftNetworkMessage::Consensus(inner) => inner.sender.as_str(),
                        PbftNetworkMessage::ViewChange(inner) => inner.sender.as_str(),
                        PbftNetworkMessage::NewView(inner) => inner.sender.as_str(),
                    };
                    if !sender.starts_with(prefix) {
                        continue;
                    }
                }
                received += 1;
                println!("RECEIVED_COUNT={}", received);
            }
            Ok(Ok(_)) => {}
            Ok(Err(e)) => {
                eprintln!("transport receive error: {}", e);
            }
            Err(_) => {
                node.pump_for(Duration::from_millis(60)).await?;
            }
        }
    }

    if received < cfg.expect_pbft {
        return Err(format!(
            "expected {} PBFT messages, got {}",
            cfg.expect_pbft, received
        )
        .into());
    }

    Ok(())
}
