use std::fs;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::time::timeout;
use zoka::network::message::{BlockResponseMessage, PingMessage, SyncStateMessage, ZokaMessage};
use zoka::network::Node;
use zoka::state::{AccountInclusionProof, StateManager};
use zoka::storage::RocksDbStorage;
use zoka::types::Block;

fn print_usage() {
    println!("zoka CLI");
    println!();
    println!("Commands:");
    println!("  node-init [--db <path>] [--genesis-address <addr>] [--genesis-balance <amount>]");
    println!("  state-root [--db <path>]");
    println!("  account-proof <address> [--db <path>]");
    println!("  proof-verify <expected_root> <proof_json_file>");
    println!("  net-listen [--listen <multiaddr>] [--seconds <n>]");
    println!(
        "  net-send-ping --dial <multiaddr> [--listen <multiaddr>] [--nonce <n>] [--retries <n>]"
    );
    println!("  net-send-sync --dial <multiaddr> --height <h> --state-root <root> [--listen <multiaddr>] [--retries <n>]");
    println!("  net-send-block-request --dial <multiaddr> --height <h> [--hash <block_hash>] [--listen <multiaddr>] [--retries <n>] [--wait-ms <n>]");
    println!("  net-serve-blocks [--db <path>] [--listen <multiaddr>] [--dial <multiaddr>] [--seconds <n>] [--publish-retries <n>]");
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn option_value(args: &[String], flag: &str) -> Option<String> {
    args.windows(2)
        .find(|pair| pair[0] == flag)
        .map(|pair| pair[1].clone())
}

fn parse_u64_arg(value: Option<String>, default: u64, flag_name: &str) -> Result<u64, String> {
    match value {
        Some(raw) => raw
            .parse::<u64>()
            .map_err(|_| format!("invalid numeric value for {flag_name}: {raw}")),
        None => Ok(default),
    }
}

async fn open_state_manager(db_path: &str) -> Result<Arc<StateManager>, String> {
    let storage = open_storage(db_path)?;
    let manager = Arc::new(StateManager::new(storage));
    manager
        .load_from_storage()
        .await
        .map_err(|e| format!("failed to load state: {e}"))?;
    Ok(manager)
}

fn open_storage(db_path: &str) -> Result<Arc<RocksDbStorage>, String> {
    Ok(Arc::new(
        RocksDbStorage::new(db_path).map_err(|e| format!("failed to open storage: {e}"))?,
    ))
}

async fn cmd_node_init(args: &[String]) -> Result<(), String> {
    let db_path = option_value(args, "--db").unwrap_or_else(|| ".zoka/blockchain.db".to_string());
    let genesis_address =
        option_value(args, "--genesis-address").unwrap_or_else(|| "alice".to_string());
    let genesis_balance = parse_u64_arg(
        option_value(args, "--genesis-balance"),
        10_000,
        "--genesis-balance",
    )?;

    let manager = open_state_manager(&db_path).await?;
    if manager.get_account(&genesis_address).await.is_none() {
        manager
            .create_account(genesis_address.clone(), genesis_balance)
            .await
            .map_err(|e| format!("failed to create genesis account: {e}"))?;
        println!(
            "Genesis account created: address={}, balance={}",
            genesis_address, genesis_balance
        );
    } else {
        println!("Genesis account already exists: {}", genesis_address);
    }

    let state = manager.get_state().await;
    println!("DB: {}", db_path);
    println!(
        "State snapshot: height={}, hash={}, state_root={}, accounts={}",
        state.current_height,
        state.current_hash,
        manager.get_state_root().await,
        state.accounts.len()
    );
    Ok(())
}

async fn cmd_state_root(args: &[String]) -> Result<(), String> {
    let db_path = option_value(args, "--db").unwrap_or_else(|| ".zoka/blockchain.db".to_string());
    let manager = open_state_manager(&db_path).await?;
    println!("{}", manager.get_state_root().await);
    Ok(())
}

async fn cmd_account_proof(args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        return Err("usage: account-proof <address> [--db <path>]".to_string());
    }
    let address = args[0].clone();
    let db_path = option_value(args, "--db").unwrap_or_else(|| ".zoka/blockchain.db".to_string());
    let manager = open_state_manager(&db_path).await?;
    let proof = manager
        .get_account_inclusion_proof(&address)
        .await
        .ok_or_else(|| format!("account not found: {address}"))?;
    let json = serde_json::to_string_pretty(&proof).map_err(|e| format!("json error: {e}"))?;
    println!("{json}");
    Ok(())
}

async fn cmd_proof_verify(args: &[String]) -> Result<(), String> {
    if args.len() < 2 {
        return Err("usage: proof-verify <expected_root> <proof_json_file>".to_string());
    }
    let expected_root = args[0].clone();
    let proof_path = args[1].clone();
    let raw = fs::read_to_string(&proof_path)
        .map_err(|e| format!("failed to read proof file {proof_path}: {e}"))?;
    let proof: AccountInclusionProof =
        serde_json::from_str(&raw).map_err(|e| format!("invalid proof json: {e}"))?;
    let valid = StateManager::verify_account_inclusion_proof(&expected_root, &proof);
    println!("{}", serde_json::json!({ "valid": valid }));
    Ok(())
}

async fn cmd_net_listen(args: &[String]) -> Result<(), String> {
    let listen_addr =
        option_value(args, "--listen").unwrap_or_else(|| "/ip4/127.0.0.1/tcp/0".to_string());
    let seconds = parse_u64_arg(option_value(args, "--seconds"), 20, "--seconds")?;

    let mut node = Node::new(0)
        .await
        .map_err(|e| format!("node init error: {e}"))?;
    node.listen(&listen_addr)
        .await
        .map_err(|e| format!("listen error: {e}"))?;
    let announced = timeout(Duration::from_secs(10), node.wait_for_listen_addr())
        .await
        .map_err(|_| "listen address timeout".to_string())?
        .map_err(|e| format!("listen address error: {e}"))?;
    println!("Listening on {}", announced);

    let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);
    while tokio::time::Instant::now() < deadline {
        match timeout(Duration::from_millis(800), node.next_message()).await {
            Ok(Ok(message)) => {
                let as_json =
                    serde_json::to_string(&message).unwrap_or_else(|_| "<invalid>".to_string());
                println!("recv {}", as_json);
            }
            Ok(Err(err)) => {
                eprintln!("receive error: {err}");
            }
            Err(_) => {
                node.pump_for(Duration::from_millis(100))
                    .await
                    .map_err(|e| format!("pump error: {e}"))?;
            }
        }
    }

    Ok(())
}

async fn connect_and_warmup(node: &mut Node, dial_addr: &str) -> Result<(), String> {
    node.connect_to_peer(dial_addr)
        .await
        .map_err(|e| format!("dial error: {e}"))?;
    timeout(Duration::from_secs(10), node.wait_for_any_connection())
        .await
        .map_err(|_| "connection timeout".to_string())?
        .map_err(|e| format!("connection error: {e}"))?;
    node.wait_for_min_connected_peers(1, Duration::from_secs(10))
        .await
        .map_err(|e| format!("peer warmup error: {e}"))?;
    Ok(())
}

async fn cmd_net_send_ping(args: &[String]) -> Result<(), String> {
    let dial = option_value(args, "--dial")
        .ok_or_else(|| "usage: net-send-ping --dial <multiaddr> [--listen <multiaddr>] [--nonce <n>] [--retries <n>]".to_string())?;
    let listen_addr =
        option_value(args, "--listen").unwrap_or_else(|| "/ip4/127.0.0.1/tcp/0".to_string());
    let nonce = parse_u64_arg(option_value(args, "--nonce"), now_secs(), "--nonce")?;
    let retries = parse_u64_arg(option_value(args, "--retries"), 25, "--retries")? as u32;

    let mut node = Node::new(0)
        .await
        .map_err(|e| format!("node init error: {e}"))?;
    node.listen(&listen_addr)
        .await
        .map_err(|e| format!("listen error: {e}"))?;
    let announced = timeout(Duration::from_secs(10), node.wait_for_listen_addr())
        .await
        .map_err(|_| "listen address timeout".to_string())?
        .map_err(|e| format!("listen address error: {e}"))?;
    println!("Local listener: {}", announced);

    connect_and_warmup(&mut node, &dial).await?;
    let ping = ZokaMessage::Ping(PingMessage {
        nonce,
        timestamp: now_secs(),
    });
    node.publish_message_with_retry(&ping, retries, Duration::from_millis(150))
        .await
        .map_err(|e| format!("publish ping failed: {e}"))?;
    println!("Ping sent: nonce={}", nonce);
    Ok(())
}

async fn cmd_net_send_sync(args: &[String]) -> Result<(), String> {
    let dial = option_value(args, "--dial")
        .ok_or_else(|| "usage: net-send-sync --dial <multiaddr> --height <h> --state-root <root> [--listen <multiaddr>] [--retries <n>]".to_string())?;
    let listen_addr =
        option_value(args, "--listen").unwrap_or_else(|| "/ip4/127.0.0.1/tcp/0".to_string());
    let height = parse_u64_arg(
        Some(option_value(args, "--height").ok_or_else(|| "--height is required".to_string())?),
        0,
        "--height",
    )?;
    let state_root =
        option_value(args, "--state-root").ok_or_else(|| "--state-root is required".to_string())?;
    let retries = parse_u64_arg(option_value(args, "--retries"), 25, "--retries")? as u32;

    let mut node = Node::new(0)
        .await
        .map_err(|e| format!("node init error: {e}"))?;
    node.listen(&listen_addr)
        .await
        .map_err(|e| format!("listen error: {e}"))?;
    let announced = timeout(Duration::from_secs(10), node.wait_for_listen_addr())
        .await
        .map_err(|_| "listen address timeout".to_string())?
        .map_err(|e| format!("listen address error: {e}"))?;
    println!("Local listener: {}", announced);

    connect_and_warmup(&mut node, &dial).await?;
    let sync = ZokaMessage::SyncState(SyncStateMessage {
        last_height: height,
        state_root: state_root.clone(),
        timestamp: now_secs(),
    });
    node.publish_message_with_retry(&sync, retries, Duration::from_millis(150))
        .await
        .map_err(|e| format!("publish sync failed: {e}"))?;
    println!(
        "SyncState sent: height={}, state_root={}",
        height, state_root
    );
    Ok(())
}

async fn cmd_net_send_block_request(args: &[String]) -> Result<(), String> {
    let dial = option_value(args, "--dial").ok_or_else(|| {
        "usage: net-send-block-request --dial <multiaddr> --height <h> [--hash <block_hash>] [--listen <multiaddr>] [--retries <n>] [--wait-ms <n>]".to_string()
    })?;
    let listen_addr =
        option_value(args, "--listen").unwrap_or_else(|| "/ip4/127.0.0.1/tcp/0".to_string());
    let height = parse_u64_arg(
        Some(option_value(args, "--height").ok_or_else(|| "--height is required".to_string())?),
        0,
        "--height",
    )?;
    let expected_hash = option_value(args, "--hash");
    let retries = parse_u64_arg(option_value(args, "--retries"), 25, "--retries")? as u32;
    let wait_ms = parse_u64_arg(option_value(args, "--wait-ms"), 15_000, "--wait-ms")?;

    let mut node = Node::new(0)
        .await
        .map_err(|e| format!("node init error: {e}"))?;
    node.listen(&listen_addr)
        .await
        .map_err(|e| format!("listen error: {e}"))?;
    let announced = timeout(Duration::from_secs(10), node.wait_for_listen_addr())
        .await
        .map_err(|_| "listen address timeout".to_string())?
        .map_err(|e| format!("listen address error: {e}"))?;
    println!("Local listener: {}", announced);

    connect_and_warmup(&mut node, &dial).await?;
    let response = node
        .request_block_with_retry(
            height,
            expected_hash.clone(),
            retries,
            Duration::from_millis(wait_ms),
        )
        .await
        .map_err(|e| format!("block request failed: {e}"))?;

    let decoded: Option<Block> = serde_json::from_slice(&response.encrypted_block).ok();
    if let Some(block) = decoded {
        println!(
            "{}",
            serde_json::json!({
                "received": true,
                "decoded_block": true,
                "requested_height": height,
                "block_height": block.header.block_height,
                "block_hash": block.header.hash,
                "tx_count": block.transactions.len(),
                "state_root": block.header.state_root,
            })
        );
    } else {
        println!(
            "{}",
            serde_json::json!({
                "received": true,
                "decoded_block": false,
                "requested_height": height,
                "block_height": response.block_height,
                "block_hash": response.block_hash,
                "payload_len": response.encrypted_block.len(),
                "payload_hex": hex::encode(&response.encrypted_block),
            })
        );
    }
    Ok(())
}

async fn cmd_net_serve_blocks(args: &[String]) -> Result<(), String> {
    let db_path = option_value(args, "--db").unwrap_or_else(|| ".zoka/blockchain.db".to_string());
    let listen_addr =
        option_value(args, "--listen").unwrap_or_else(|| "/ip4/127.0.0.1/tcp/0".to_string());
    let dial_addr = option_value(args, "--dial");
    let seconds = parse_u64_arg(option_value(args, "--seconds"), 30, "--seconds")?;
    let publish_retries = parse_u64_arg(
        option_value(args, "--publish-retries"),
        20,
        "--publish-retries",
    )? as u32;

    let storage = open_storage(&db_path)?;
    let mut node = Node::new(0)
        .await
        .map_err(|e| format!("node init error: {e}"))?;
    node.listen(&listen_addr)
        .await
        .map_err(|e| format!("listen error: {e}"))?;
    let announced = timeout(Duration::from_secs(10), node.wait_for_listen_addr())
        .await
        .map_err(|_| "listen address timeout".to_string())?
        .map_err(|e| format!("listen address error: {e}"))?;
    println!("Serving block responses on {} (db={})", announced, db_path);

    if let Some(dial) = dial_addr {
        connect_and_warmup(&mut node, &dial).await?;
    }

    let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);
    while tokio::time::Instant::now() < deadline {
        match timeout(Duration::from_millis(800), node.next_message()).await {
            Ok(Ok(ZokaMessage::BlockRequest { block_height })) => {
                let found = storage
                    .get_block_by_height(block_height)
                    .map_err(|e| format!("storage height lookup error: {e}"))?;

                if let Some(block) = found {
                    let payload =
                        serde_json::to_vec(&block).map_err(|e| format!("json error: {e}"))?;
                    let response = ZokaMessage::BlockResponse(BlockResponseMessage {
                        block_height: block.header.block_height,
                        encrypted_block: payload,
                        block_hash: block.header.hash.clone(),
                        timestamp: now_secs(),
                    });
                    node.publish_message_with_retry(
                        &response,
                        publish_retries,
                        Duration::from_millis(120),
                    )
                    .await
                    .map_err(|e| format!("publish block response failed: {e}"))?;
                    println!(
                        "served block: height={}, hash={}",
                        block.header.block_height, block.header.hash
                    );
                } else {
                    println!("requested block not found: height={}", block_height);
                }
            }
            Ok(Ok(_)) => {}
            Ok(Err(err)) => eprintln!("receive error: {err}"),
            Err(_) => {
                node.pump_for(Duration::from_millis(100))
                    .await
                    .map_err(|e| format!("pump error: {e}"))?;
            }
        }
    }

    Ok(())
}

async fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        print_usage();
        return Ok(());
    }

    let command = &args[0];
    let tail = &args[1..];
    match command.as_str() {
        "help" | "--help" | "-h" => {
            print_usage();
            Ok(())
        }
        "node-init" => cmd_node_init(tail).await,
        "state-root" => cmd_state_root(tail).await,
        "account-proof" => cmd_account_proof(tail).await,
        "proof-verify" => cmd_proof_verify(tail).await,
        "net-listen" => cmd_net_listen(tail).await,
        "net-send-ping" => cmd_net_send_ping(tail).await,
        "net-send-sync" => cmd_net_send_sync(tail).await,
        "net-send-block-request" => cmd_net_send_block_request(tail).await,
        "net-serve-blocks" => cmd_net_serve_blocks(tail).await,
        other => Err(format!("unknown command: {other}")),
    }
}

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("{err}");
        print_usage();
        std::process::exit(1);
    }
}
