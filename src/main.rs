use std::collections::VecDeque;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use curve25519_dalek::constants::RISTRETTO_BASEPOINT_POINT;
use curve25519_dalek::scalar::Scalar;
use rand::seq::SliceRandom;
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::time::timeout;
use zoka::economics;
use zoka::identity::HDWallet;
use zoka::network::p2p::{PrivacyTransportMode, PrivacyTransportStatus, PROTOCOL_FREEZE_ID};
use zoka::network::Node;
use zoka::private_wallet::derive_spending_key_from_recovery_seed;
use zoka::private_wallet::PrivateTransaction;
use zoka::private_wallet::PrivateWallet;
use zoka::private_wallet::PrivateWalletManager;
use zoka::state::StateManager;
use zoka::storage::RocksDbStorage;
use zoka::wallet::storage::{PrivateWalletMetadata, WalletStorage};
use zoka::zk::groth16_backend::PRIVATE_TX_MAX_INPUTS;

/// Las líneas de `fullnode` de la ayuda de Quantic. ZSilent lee `zoka --help`
/// y busca ahí `--tor-proxy`, `--tor-only` y `--external-addr` antes de lanzar
/// el nodo: si falta uno, no lo arranca. Pasó con la 3.0.0, que no nombraba
/// `--external-addr`, y ZSilent se negaba a arrancar con la onion.
#[cfg(feature = "quantic")]
const QUANTIC_FULLNODE_USAGE: [&str; 3] = [
    "  zoka fullnode --network quantic-testnet [--quantic-mine-to <address>] [--threads n] [--no-mine]",
    "               [--seed <multiaddr>] [--listen <multiaddr>] [--tor-proxy host:port] [--tor-only]",
    "               [--external-addr <multiaddr>] [--network-config <file>] [--serve-iroh-snapshot]",
];

/// La ayuda de una build de Quantic: solo lo que sirve en sus dos redes.
#[cfg(feature = "quantic")]
fn print_quantic_usage() {
    println!();
    println!("{}", dashboard_title("ZOKA Quantic", ANSI_CYAN));
    println!("{}", dashboard_divider());
    println!(
        "  {}",
        paint(
            "Two networks: quantic-testnet (default, test coins) and quantic-mainnet. Zenith was retired on 2026-10-21.",
            ANSI_DIM
        )
    );
    println!("{}", dashboard_divider());
    println!("{}", dashboard_section("Wallet (zoka quantic-wallet <action> --network quantic-testnet)", ANSI_GREEN));
    println!("  new --name <wallet> --password-stdin            a new wallet (24-word phrase, shown once)");
    println!("  generate-phrase | restore --name <w> --phrase-env <VAR> --password-stdin");
    println!("  list [--password-stdin] | address --name <w>");
    println!("  balance | history [--limit n] --name <w> --password-stdin");
    println!("  estimate-fee --amount <ZKA> | send --to <address> --amount <ZKA> [--memo <text>] --name <w> --password-stdin");
    println!("  show-phrase | change-password --new-password-env <VAR> | backup --out <file> | import-backup --file <file>");
    println!("  (pick the wallet with --name or --address; add --json for machine output; --rpc <url> for another node)");
    println!("{}", dashboard_section("Node", ANSI_GREEN));
    for line in QUANTIC_FULLNODE_USAGE {
        println!("{line}");
    }
    println!("  zoka mine-status --network quantic-testnet [--json]");
    println!("  zoka node-status | height | block-info | chain-stats   [--network quantic-testnet]");
    println!("{}", dashboard_section("Network team", ANSI_GREEN));
    println!("  zoka checkpoint-keygen --network quantic-testnet     the authority key (both halves, stays on this PC)");
    println!("  zoka quantic-genesis --network quantic-testnet --premine-to <address> --out <file>");
    println!("  zoka checkpoint-status | checkpoint-sign | snapshot-info | snapshot-export | iroh-id");
    println!("  zoka version");
    println!();
}

// Zenith: no se llama en una build de Quantic (Zenith se retiró el 21/10/2026).
#[cfg_attr(feature = "quantic", allow(dead_code))]
fn print_usage() {
    println!();
    println!("{}", dashboard_title("ZOKA private console", ANSI_CYAN));
    println!("{}", dashboard_divider());
    println!(
        "  {}",
        paint(
            "Privacy first. Wallets, transfers, mining and fullnodes use private flows by default.",
            ANSI_DIM
        )
    );
    println!(
        "  {}",
        paint(
            "Run `zoka` without arguments for the guided console.",
            ANSI_DIM
        )
    );
    println!(
        "  {}",
        paint(
            "Network: Zoka Quantic testnet (the Zenith chain, zoka-mainnet-zenith-v1). Testnet ZKA are test coins; `--network mainnet` is still accepted as its former name.",
            ANSI_DIM
        )
    );
    println!("{}", dashboard_divider());
    println!("{}", dashboard_section("Everyday commands", ANSI_GREEN));
    println!(
        "  zoka create <label> --network testnet --password <pwd>|--password-env <var>|--password-stdin [--seed-words 12|24]"
    );
    println!(
        "  zoka restore \"<recovery phrase>\" --network testnet --password <pwd>|--password-env <var>|--password-stdin [--name <label>]"
    );
    println!("  zoka wallets [--network testnet] [--password <pwd>]");
    println!("  zoka balance <wallet-address> [--network testnet] [--password <pwd>]");
    println!("  zoka receive <wallet-address> [--network testnet] [--password <pwd>]");
    println!(
        "  zoka send <from-zka1> <to-zka1> <amount> [--network testnet] [--password <pwd>] --yes"
    );
    println!("  zoka pending [--network testnet] [--db <path>]");
    println!("  zoka backup <wallet-address> <path> [--password <pwd>]");
    println!(
        "  zoka restore-backup <path> [--name <label>] --password <pwd>|--password-env <var>|--password-stdin"
    );
    println!("  zoka wallet-generate-seed [--seed-words 12|24] [--json]");
    println!(
        "  zoka mine <wallet-zka1> [--network testnet] [--password <pwd>] [--db <path>] [--rpc <http-url>] [--network-config <file>] [--minutes <n>] [--blocks <n>] [--power low|normal|max] [--allow-unsynced-mining]"
    );
    println!(
        "  zoka node [--network testnet] [--password <pwd>|--password-env <var>|--password-stdin] [--seed <multiaddr> ...] [--power low|normal|max] [--tor-proxy host:port] [--tor-only] [--external-addr <multiaddr> ...]"
    );
    println!(
        "  zoka node --sync-only [--network testnet]  (descarga y valida bloques sin abrir ninguna wallet: no mina, no publica ofertas y no escanea tx privadas)"
    );
    println!("  zoka node-status [--db <path>] [--password <pwd>]");
    println!("  zoka status");
    println!("  Wallet files stay local under .zoka/<network>/private_wallets");
    println!("{}", dashboard_section("Full command names", ANSI_MAGENTA));
    println!(
        "  wallet-generate-seed | wallet-create | wallet-restore-from-seed | wallet-restore-from-backup | wallet-list"
    );
    println!(
        "  wallet-balance | wallet-receive | wallet-history | wallet-backup | wallet-wipe (delete local wallet)"
    );
    println!("  wallet-export-secrets | wallet-change-passphrase");
    println!(
        "  tx-send | tx-pending | private-tx-publish | private-tx-audit | estimate-fee | mine | fullnode | node-status | sync-status"
    );
    println!("{}", dashboard_section("Team mining", ANSI_CYAN));
    println!(
        "  team-status [--json]                       (team, members and whether you are hashing)"
    );
    println!("  team-create [--json]                       (creates a team; prints the invite code to share)");
    println!("  team-join --code <invite-code> [--json]");
    println!("  team-leave");
    println!("  team-start [--threads <n>] | team-stop");
    println!(
        "  All of these talk to the node on this machine (127.0.0.1:3000 by default; --rpc to change it).
  Whole blocks rotate by contributed work and are paid from the chain to each member's own address."
    );
    println!("{}", dashboard_section("Chain and operators", ANSI_YELLOW));
    println!("  height [--network testnet] [--db <path>] [--json]");
    println!(
        "  block-info --height <n> [--network testnet] [--db <path>] [--json]  (miner address and reward for a specific block height)"
    );
    println!("  chain-stats [--network testnet] [--db <path>] [--json]");
    println!(
        "  chain-economics-migrate [--network testnet] [--db <path>] [--dry-run|--yes] [--repair-supply] [--json]"
    );
    println!(
        "  private-pending-diagnostics [--network testnet] [--db <path>] [--height <n>] [--limit <n>] [--json]"
    );
    println!(
        "  rpc-list [--network testnet] [--network-config <file>] [--json]  (lists configured read-only public RPC endpoints)"
    );
    println!(
        "  rpc-status [--rpc <http-url>] [--network-config <file>] [--json]  (read-only RPC health + height)"
    );
    println!(
        "  rpc-height [--rpc <http-url>] [--network-config <file>] [--json]  (read-only RPC chain height)"
    );
    println!(
        "  rpc-sync [--network testnet] [--network-config <file>] [--db <path>] [--limit <n>] [--json]  (bootstrap local chain from configured RPC)"
    );
    println!(
        "  rpc-mempool [--rpc <http-url>] [--network-config <file>] [--json]  (read-only RPC mempool size)"
    );
    println!(
        "  genesis-verify <path-to-genesis.json> [--json]  (offline validation; prints chain id, pow algorithm, validator count, sha-256 digest)"
    );
    println!(
        "  mainnet-network-lint [path-to-mainnet-network.v1.json] [--json] [--allow-placeholders]"
    );
    println!(
        "  release-keygen --out <path> [--label <label>] [--force] [--json]  (writes an ed25519 signing key file; prints the verifying key)"
    );
    println!("{}", dashboard_section("Snapshots", ANSI_BLUE));
    println!("  snapshot-info <snapshot-path> [--json] [--require-signature] [--signer-key <hex>]");
    println!(
        "  snapshot-export <out-path> [--db <path>] [--sign-key-hex <hex>]  (writes zoka-snapshot-v1 JSON; signs with ed25519 if a key is provided)"
    );
    println!(
        "  snapshot-import <in-path> [--db <path>] [--require-signature] [--signer-key <hex>]  (verifies + replays every block via state::chain::apply_block)"
    );
    println!(
        "  snapshot-fetch-iroh --iroh-id <endpoint-id> [--db <path>] [--require-signature] [--signer-key <hex>]  (pulls a snapshot from a seed over iroh QUIC/hole-punching and imports it)"
    );
    println!(
        "  chain-prune --keep-last <N> [--db <path>] [--dry-run] [--json]  (drops block bodies older than head-N; floors --keep-last at PRUNE_MIN_RETAINED_BLOCKS so difficulty retarget keeps working)"
    );
}

/// Read one line from stdin. Returns `Ok(None)` on EOF (stdin closed or
/// non-interactive), `Ok(Some(trimmed))` otherwise. Distinguishing EOF from an
/// empty line is what lets `prompt_required` fail fast in a headless run
/// instead of spinning forever on a `read_line` that keeps returning 0 bytes.
fn read_prompt_line(prompt: &str) -> Result<Option<String>, String> {
    print!("{}", paint(prompt, ANSI_CYAN));
    io::stdout()
        .flush()
        .map_err(|e| format!("stdout flush failed: {e}"))?;
    let mut line = String::new();
    let read = io::stdin()
        .read_line(&mut line)
        .map_err(|e| format!("stdin read failed: {e}"))?;
    if read == 0 {
        return Ok(None);
    }
    Ok(Some(line.trim().to_string()))
}

fn prompt_line(prompt: &str) -> Result<String, String> {
    Ok(read_prompt_line(prompt)?.unwrap_or_default())
}

fn prompt_optional(prompt: &str) -> Result<Option<String>, String> {
    let value = prompt_line(prompt)?;
    if value.is_empty() {
        Ok(None)
    } else {
        Ok(Some(value))
    }
}

fn prompt_u64_with_default(prompt: &str, default: u64) -> Result<u64, String> {
    let value = prompt_line(prompt)?;
    if value.is_empty() {
        return Ok(default);
    }
    value
        .parse::<u64>()
        .map_err(|_| format!("invalid number: {}", value))
}

fn prompt_required(prompt: &str) -> Result<String, String> {
    loop {
        match read_prompt_line(prompt)? {
            Some(value) if !value.is_empty() => return Ok(value),
            Some(_) => {
                println!("Value is required.");
            }
            None => {
                return Err("no input available on stdin (non-interactive session); \
                     provide the value via a flag or the config file instead"
                    .to_string());
            }
        }
    }
}

fn prompt_yes(prompt: &str) -> Result<bool, String> {
    let value = prompt_line(prompt)?;
    Ok(matches!(value.to_ascii_lowercase().as_str(), "y" | "yes"))
}

fn prompt_yes_default(prompt: &str, default: bool) -> Result<bool, String> {
    let value = prompt_line(prompt)?;
    if value.is_empty() {
        return Ok(default);
    }
    match value.to_ascii_lowercase().as_str() {
        "y" | "yes" => Ok(true),
        "n" | "no" => Ok(false),
        _ => {
            println!("Please answer y or n.");
            prompt_yes_default(prompt, default)
        }
    }
}

fn prompt_mining_cpu_threads() -> Result<usize, String> {
    let available = default_mining_threads();
    println!("{}", dashboard_divider());
    println!("{}", dashboard_section("CPU Power", ANSI_GREEN));
    println!("{}", dashboard_line("Available CPU threads", available));
    println!(
        "  {}",
        paint("1) Low - 1 thread, keeps the PC responsive", ANSI_WHITE)
    );
    println!(
        "  {}",
        paint("2) Normal - about half of available threads", ANSI_WHITE)
    );
    println!(
        "  {}",
        paint("3) Max - all available CPU threads", ANSI_WHITE)
    );
    println!("  {}", paint("4) Custom thread count", ANSI_WHITE));
    loop {
        match prompt_line("CPU power [2]: ")?.as_str() {
            "" | "2" => return mining_threads_for_power_preset("normal"),
            "1" => return mining_threads_for_power_preset("low"),
            "3" => return mining_threads_for_power_preset("max"),
            "4" => {
                let raw = prompt_u64_with_default(
                    &format!("Thread count (1-{available}, default {available}): "),
                    available as u64,
                )?;
                if raw == 0 {
                    println!("Thread count must be at least 1.");
                    continue;
                }
                return Ok(raw.min(256) as usize);
            }
            _ => println!("Invalid option"),
        }
    }
}

fn push_opt_arg(args: &mut Vec<String>, flag: &str, value: Option<String>) {
    if let Some(v) = value {
        args.push(flag.to_string());
        args.push(v);
    }
}

#[derive(Debug, Clone, Serialize)]
struct WalletSecretsExport {
    wallet_name: String,
    wallet_type: String,
    address: String,
    user_address: String,
    mnemonic: Option<String>,
    scan_private_key_hex: String,
    scan_public_key_hex: String,
    spend_private_key_hex: String,
    spend_public_key_hex: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct MainnetNetworkConfig {
    schema: String,
    #[serde(default)]
    status: String,
    chain_id: String,
    protocol_freeze_id: String,
    #[serde(default)]
    transport_policy: MainnetTransportPolicy,
    #[serde(default)]
    seed_nodes: Vec<MainnetSeedNode>,
    #[serde(default)]
    public_rpc: Vec<MainnetRpcEndpoint>,
    #[serde(default)]
    desktop: MainnetDesktopConfig,
    #[serde(default, rename = "mining")]
    _mining: Option<serde_json::Value>,
    #[serde(default, rename = "joining")]
    _joining: Option<serde_json::Value>,
    #[serde(default, rename = "trusted_setup")]
    _trusted_setup: Option<serde_json::Value>,
    /// Release-coordination metadata. Consensus enforces the compiled
    /// checkpoint; this document section records the independent publication
    /// gate and operator recovery policy without changing runtime consensus.
    #[serde(default, rename = "recovery")]
    _recovery: Option<serde_json::Value>,
    #[serde(default)]
    snapshot_signers: Vec<String>,
    #[serde(default)]
    notes: Vec<String>,
    /// Minimum desktop client version the network expects; enforced by
    /// ZSilent, not by this binary. Accepted (not just ignored) so the
    /// manifest's deny_unknown_fields doesn't reject the whole file.
    #[serde(default, rename = "minimum_client_version")]
    _minimum_client_version: String,
}

#[derive(Debug, Clone, Serialize)]
struct MainnetNetworkLintReport {
    path: String,
    ok: bool,
    schema: String,
    status: String,
    chain_id: String,
    protocol_freeze_id: String,
    seed_count: usize,
    active_seed_count: usize,
    active_seed_candidates_without_clearnet: usize,
    active_seed_candidates_with_clearnet: usize,
    public_rpc_count: usize,
    active_rpc_count: usize,
    snapshot_signer_count: usize,
    errors: Vec<String>,
    warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ReleaseKeygenPublicReport {
    schema: &'static str,
    label: String,
    public_key_hex: String,
    secret_key_file: String,
    created_unix_secs: u64,
}

#[derive(Debug, Serialize)]
struct ReleaseKeypairFile {
    schema: &'static str,
    label: String,
    public_key_hex: String,
    secret_key_hex: String,
    created_unix_secs: u64,
    warning: &'static str,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct MainnetTransportPolicy {
    #[serde(default)]
    anonymous_transport_preferred: bool,
    #[serde(default)]
    clearnet_requires_explicit_operator_choice: bool,
    #[serde(default)]
    supported: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct MainnetSeedNode {
    label: String,
    operator: String,
    region: String,
    #[serde(default)]
    multiaddr: Option<String>,
    #[serde(default)]
    onion: Option<String>,
    #[serde(default)]
    i2p: Option<String>,
    /// iroh EndpointId (base32 public key) of this seed's snapshot-serving
    /// endpoint, if it offers iroh snapshot bootstrap. A fresh node can pull a
    /// full-chain snapshot from this id over iroh (QUIC + NAT hole-punching,
    /// no open port) instead of crawling block-by-block IBD. Stable across
    /// restarts (see network::iroh_snapshot_transfer::load_or_generate_secret_key).
    #[serde(default)]
    iroh: Option<String>,
    role: String,
    status: String,
    #[serde(default, rename = "note")]
    _note: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MainnetRpcEndpoint {
    label: String,
    operator: String,
    region: String,
    url: String,
    #[serde(default)]
    onion: Option<String>,
    #[serde(default)]
    i2p: Option<String>,
    status: String,
    logging_policy: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct MainnetDesktopConfig {
    #[serde(default)]
    local_rpc_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SeedCandidate {
    label: String,
    address: String,
    transport: String,
}

impl MainnetNetworkConfig {
    fn validate_for_network(&self, network: NetworkProfile) -> Result<(), String> {
        if self.schema != network.network_config_schema() {
            return Err(format!(
                "network config schema '{}' is not supported for {}",
                self.schema,
                network.display_name()
            ));
        }
        if self.chain_id != network.chain_id() {
            return Err(format!(
                "network config chain_id '{}' does not match binary chain_id '{}'",
                self.chain_id,
                network.chain_id()
            ));
        }
        if self.protocol_freeze_id != network.protocol_id() {
            return Err(format!(
                "network config protocol '{}' does not match binary protocol '{}'",
                self.protocol_freeze_id,
                network.protocol_id()
            ));
        }
        if self
            .transport_policy
            .supported
            .iter()
            .any(|v| v.trim().is_empty())
        {
            return Err("network config contains an empty supported transport".to_string());
        }
        for seed in &self.seed_nodes {
            if seed.label.trim().is_empty()
                || seed.operator.trim().is_empty()
                || seed.region.trim().is_empty()
                || seed.role.trim().is_empty()
                || seed.status.trim().is_empty()
            {
                return Err("network config contains an incomplete seed node entry".to_string());
            }
            if seed.role != "seed" {
                return Err(format!(
                    "network config seed '{}' has unsupported role '{}'",
                    seed.label, seed.role
                ));
            }
        }
        for rpc in &self.public_rpc {
            if rpc.label.trim().is_empty()
                || rpc.operator.trim().is_empty()
                || rpc.region.trim().is_empty()
                || rpc.url.trim().is_empty()
                || rpc.status.trim().is_empty()
                || rpc.logging_policy.trim().is_empty()
            {
                return Err("network config contains an incomplete RPC entry".to_string());
            }
            if non_empty_opt(rpc.onion.as_deref()).is_none()
                && non_empty_opt(rpc.i2p.as_deref()).is_none()
                && self.transport_policy.anonymous_transport_preferred
            {
                return Err(format!(
                    "network config RPC '{}' is missing anonymous onion/i2p endpoint",
                    rpc.label
                ));
            }
        }
        Ok(())
    }

    fn active_seed_candidates(&self, allow_clearnet: bool) -> Vec<SeedCandidate> {
        let requires_clearnet_opt_in = self
            .transport_policy
            .clearnet_requires_explicit_operator_choice;
        let mut out = Vec::new();

        for seed in self
            .seed_nodes
            .iter()
            .filter(|seed| seed.status.eq_ignore_ascii_case("active"))
        {
            if let Some(onion) = non_empty_opt(seed.onion.as_deref()) {
                out.push(SeedCandidate {
                    label: seed.label.clone(),
                    address: onion.to_string(),
                    transport: "tor".to_string(),
                });
            }
            if let Some(i2p) = non_empty_opt(seed.i2p.as_deref()) {
                out.push(SeedCandidate {
                    label: seed.label.clone(),
                    address: i2p.to_string(),
                    transport: "i2p".to_string(),
                });
            }
            if let Some(multiaddr) = non_empty_opt(seed.multiaddr.as_deref()) {
                if allow_clearnet || !requires_clearnet_opt_in {
                    out.push(SeedCandidate {
                        label: seed.label.clone(),
                        address: multiaddr.to_string(),
                        transport: "tcp".to_string(),
                    });
                }
            }
        }

        out
    }

    fn active_rpc_count(&self) -> usize {
        self.public_rpc
            .iter()
            .filter(|rpc| rpc.status.eq_ignore_ascii_case("active"))
            .count()
    }

    fn active_seed_node_count(&self) -> usize {
        self.seed_nodes
            .iter()
            .filter(|seed| seed.status.eq_ignore_ascii_case("active"))
            .count()
    }

    /// iroh EndpointIds of active seeds that advertise snapshot bootstrap, in
    /// manifest order. A fresh node tries these to pull a full-chain snapshot
    /// over iroh before falling back to block-by-block IBD. (Wired into the
    /// fullnode auto-bootstrap next; already used by tooling that reads the
    /// manifest.)
    #[allow(dead_code)]
    fn iroh_snapshot_seed_ids(&self) -> Vec<String> {
        self.seed_nodes
            .iter()
            .filter(|seed| seed.status.eq_ignore_ascii_case("active"))
            .filter_map(|seed| {
                seed.iroh
                    .as_ref()
                    .map(|id| id.trim())
                    .filter(|id| !id.is_empty())
                    .map(|id| id.to_string())
            })
            .collect()
    }
}

fn looks_like_placeholder(value: &str) -> bool {
    let lowered = value.to_ascii_lowercase();
    lowered.contains("111111")
        || lowered.contains("222222")
        || lowered.contains("333333")
        || lowered.contains("aaaaaaaa")
        || lowered.contains("placeholder")
        || lowered.contains("example")
        || lowered.contains("seedna111")
        || lowered.contains("seedeu111")
        || lowered.contains("seedsa111")
        || lowered.contains("rpceu111")
}

fn lint_mainnet_network_config(
    path: &Path,
    allow_placeholders: bool,
) -> Result<MainnetNetworkLintReport, String> {
    let config = load_mainnet_network_config(path)?;
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    let release_ready_status = config.status.eq_ignore_ascii_case("active")
        || config.status.eq_ignore_ascii_case("launch-ready");
    if !release_ready_status {
        warnings.push(format!(
            "network status is '{}'; this config is usable for runtime bootstrap but is not release-ready",
            if config.status.is_empty() {
                "unknown"
            } else {
                config.status.as_str()
            }
        ));
    }

    let active_seed_count = config
        .seed_nodes
        .iter()
        .filter(|seed| seed.status.eq_ignore_ascii_case("active"))
        .count();
    if active_seed_count < 3 {
        let msg = format!(
            "only {active_seed_count} active seed node(s); release policy expects at least 3"
        );
        if release_ready_status {
            errors.push(msg);
        } else {
            warnings.push(msg);
        }
    }
    let active_regions = config
        .seed_nodes
        .iter()
        .filter(|seed| seed.status.eq_ignore_ascii_case("active"))
        .map(|seed| seed.region.as_str())
        .collect::<std::collections::HashSet<_>>();
    if active_seed_count > 0 && active_regions.len() < 3 {
        let msg = format!(
            "active seed nodes cover {} region(s); release policy expects multiple independent regions",
            active_regions.len()
        );
        if release_ready_status {
            errors.push(msg);
        } else {
            warnings.push(msg);
        }
    }

    if config.snapshot_signers.is_empty() {
        let msg = "no snapshot signer public keys configured".to_string();
        if release_ready_status {
            errors.push(msg);
        } else {
            warnings.push(msg);
        }
    }
    for signer in &config.snapshot_signers {
        if hex::decode(signer)
            .map(|bytes| bytes.len() != 32)
            .unwrap_or(true)
        {
            errors.push(format!(
                "snapshot signer '{}' is not a 32-byte hex public key",
                signer
            ));
        }
        if looks_like_placeholder(signer) {
            errors.push(format!(
                "snapshot signer '{}' looks like a placeholder",
                signer
            ));
        }
    }

    for seed in &config.seed_nodes {
        for (field, value) in [
            ("multiaddr", seed.multiaddr.as_deref()),
            ("onion", seed.onion.as_deref()),
            ("i2p", seed.i2p.as_deref()),
        ] {
            if let Some(value) = non_empty_opt(value) {
                if looks_like_placeholder(value) {
                    let msg = format!("seed '{}' {field} looks like a placeholder", seed.label);
                    if allow_placeholders {
                        warnings.push(msg);
                    } else {
                        errors.push(msg);
                    }
                }
            }
        }
        if seed.status.eq_ignore_ascii_case("active")
            && non_empty_opt(seed.onion.as_deref()).is_none()
            && non_empty_opt(seed.i2p.as_deref()).is_none()
            && config.transport_policy.anonymous_transport_preferred
        {
            errors.push(format!(
                "active seed '{}' has no onion or i2p endpoint while anonymous transport is preferred",
                seed.label
            ));
        }
    }

    for rpc in &config.public_rpc {
        if rpc.status.eq_ignore_ascii_case("active")
            && config.transport_policy.anonymous_transport_preferred
            && non_empty_opt(rpc.onion.as_deref()).is_none()
            && non_empty_opt(rpc.i2p.as_deref()).is_none()
        {
            errors.push(format!(
                "active RPC '{}' has no onion or i2p endpoint",
                rpc.label
            ));
        }
        for (field, value) in [
            ("url", Some(rpc.url.as_str())),
            ("onion", rpc.onion.as_deref()),
            ("i2p", rpc.i2p.as_deref()),
        ] {
            if let Some(value) = non_empty_opt(value) {
                if looks_like_placeholder(value) {
                    let msg = format!("RPC '{}' {field} looks like a placeholder", rpc.label);
                    if allow_placeholders {
                        warnings.push(msg);
                    } else {
                        errors.push(msg);
                    }
                }
            }
        }
    }

    if let Some(local_rpc) = non_empty_opt(Some(config.desktop.local_rpc_url.as_str())) {
        match parse_rpc_http_url(local_rpc) {
            Ok(parsed) if parsed.host == "localhost" || parsed.host.starts_with("127.") => {}
            Ok(_) | Err(_) => {
                warnings.push(
                    "desktop.local_rpc_url is not a localhost HTTP endpoint; desktop wallets should talk to a local node by default"
                        .to_string(),
                );
            }
        }
    }

    Ok(MainnetNetworkLintReport {
        path: path.display().to_string(),
        ok: errors.is_empty(),
        schema: config.schema.clone(),
        status: config.status.clone(),
        chain_id: config.chain_id.clone(),
        protocol_freeze_id: config.protocol_freeze_id.clone(),
        seed_count: config.seed_nodes.len(),
        active_seed_count,
        active_seed_candidates_without_clearnet: config.active_seed_candidates(false).len(),
        active_seed_candidates_with_clearnet: config.active_seed_candidates(true).len(),
        public_rpc_count: config.public_rpc.len(),
        active_rpc_count: config.active_rpc_count(),
        snapshot_signer_count: config.snapshot_signers.len(),
        errors,
        warnings,
    })
}

fn non_empty_opt(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|v| !v.is_empty())
}

fn env_non_empty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn miner_launcher_metadata() -> (Option<String>, Option<String>) {
    let launcher = env_non_empty("ZOKA_MINER_LAUNCHER").or_else(|| {
        std::env::current_exe()
            .ok()
            .and_then(|path| path.to_str().map(|value| value.to_ascii_lowercase()))
            .filter(|path| path.contains("zsilent"))
            .map(|_| "zsilent".to_string())
    });
    let launcher_version = env_non_empty("ZOKA_MINER_LAUNCHER_VERSION")
        .or_else(|| env_non_empty("ZSILENT_APP_VERSION"));
    (launcher, launcher_version)
}

fn default_mainnet_network_config_path() -> PathBuf {
    PathBuf::from("docs").join("mainnet-network.v1.json")
}

fn network_config_path_from_env(network: NetworkProfile) -> Option<PathBuf> {
    // ZSilent exporta ZOKA_MAINNET_NETWORK_CONFIG para Zenith. La variable de
    // la testnet retirada no se lee: apuntaria a un manifiesto de otra cadena.
    let network_specific = match network {
        NetworkProfile::Zenith => "ZOKA_MAINNET_NETWORK_CONFIG",
        NetworkProfile::QuanticMainnet => "ZOKA_QUANTIC_MAINNET_NETWORK_CONFIG",
        NetworkProfile::QuanticTestnet => "ZOKA_QUANTIC_TESTNET_NETWORK_CONFIG",
        NetworkProfile::QuanticRegtest => "ZOKA_QUANTIC_REGTEST_NETWORK_CONFIG",
    };
    env_non_empty(network_specific)
        .or_else(|| env_non_empty("ZOKA_NETWORK_CONFIG"))
        .map(PathBuf::from)
}

fn network_config_path_from_args(args: &[String], network: NetworkProfile) -> PathBuf {
    option_value(args, "--network-config")
        .map(PathBuf::from)
        .or_else(|| network_config_path_from_env(network))
        .unwrap_or_else(|| PathBuf::from(network.default_network_config_path()))
}

fn configured_local_rpc_url_from_path(
    network: NetworkProfile,
    config_path: Option<&Path>,
) -> Option<String> {
    env_non_empty("ZOKA_LOCAL_RPC_URL")
        .or_else(|| env_non_empty("ZOKA_NODE_ENDPOINT"))
        .or_else(|| env_non_empty("ZOKA_CLI_NODE_ENDPOINT"))
        .or_else(|| {
            let path = config_path
                .map(PathBuf::from)
                .or_else(|| network_config_path_from_env(network))
                .unwrap_or_else(|| PathBuf::from(network.default_network_config_path()));
            // load_network_config_for now falls back to the embedded manifest
            // when the file is missing on disk (typical for users who run the
            // CLI standalone, outside of a checkout that has docs/).
            load_network_config_for(&path, network)
                .ok()
                .and_then(|config| {
                    non_empty_opt(Some(config.desktop.local_rpc_url.as_str())).map(str::to_string)
                })
        })
        .map(|url| url.trim_end_matches('/').to_string())
}

fn configured_local_rpc_url(network: NetworkProfile) -> Option<String> {
    configured_local_rpc_url_from_path(network, None)
}

fn required_local_rpc_url(network: NetworkProfile) -> Result<String, String> {
    configured_local_rpc_url(network).ok_or_else(|| {
        format!(
            "local RPC is not configured for {}. Set ZOKA_LOCAL_RPC_URL, ZOKA_NODE_ENDPOINT, or desktop.local_rpc_url in the network manifest.",
            network.display_name()
        )
    })
}

fn api_bind_addr_from_local_rpc_url(url: &str) -> Result<String, String> {
    let parsed = parse_rpc_http_url(url)?;
    Ok(format!("{}:{}", parsed.host, parsed.port))
}

fn configured_api_bind_addr(args: &[String], network: NetworkProfile) -> Result<String, String> {
    if let Some(bind) = env_non_empty("ZOKA_API_BIND") {
        return Ok(bind);
    }
    let path = network_config_path_from_args(args, network);
    let rpc = configured_local_rpc_url_from_path(network, Some(&path)).ok_or_else(|| {
        format!(
            "API bind address is not configured for {}. Set ZOKA_API_BIND or desktop.local_rpc_url in '{}'.",
            network.display_name(),
            path.display()
        )
    })?;
    api_bind_addr_from_local_rpc_url(&rpc)
}

#[cfg(test)]
fn parse_mainnet_network_config(raw: &str) -> Result<MainnetNetworkConfig, String> {
    parse_network_config_for(raw, NetworkProfile::Zenith)
}

fn parse_network_config_for(
    raw: &str,
    network: NetworkProfile,
) -> Result<MainnetNetworkConfig, String> {
    let config: MainnetNetworkConfig =
        serde_json::from_str(raw).map_err(|e| format!("invalid network config json: {e}"))?;
    config.validate_for_network(network)?;
    Ok(config)
}

fn load_mainnet_network_config(path: &Path) -> Result<MainnetNetworkConfig, String> {
    load_network_config_for(path, NetworkProfile::Zenith)
}

/// Network manifests are baked into the binary so a fresh `zoka.exe` works in
/// any working directory, even when shipped without the `docs/` folder. The
/// loader still prefers a real file on disk if it exists (for operators
/// pointing at a custom config) and falls back to the embedded copy otherwise.
const EMBEDDED_MAINNET_NETWORK_CONFIG: &str = include_str!("../docs/mainnet-network.v1.json");
const EMBEDDED_QUANTIC_MAINNET_NETWORK_CONFIG: &str =
    include_str!("../docs/quantic-mainnet-network.v1.json");
const EMBEDDED_QUANTIC_TESTNET_NETWORK_CONFIG: &str =
    include_str!("../docs/quantic-testnet-network.v1.json");
const EMBEDDED_QUANTIC_REGTEST_NETWORK_CONFIG: &str =
    include_str!("../docs/quantic-regtest-network.v1.json");

fn embedded_network_config_for(network: NetworkProfile) -> &'static str {
    match network {
        NetworkProfile::Zenith => EMBEDDED_MAINNET_NETWORK_CONFIG,
        NetworkProfile::QuanticMainnet => EMBEDDED_QUANTIC_MAINNET_NETWORK_CONFIG,
        NetworkProfile::QuanticTestnet => EMBEDDED_QUANTIC_TESTNET_NETWORK_CONFIG,
        NetworkProfile::QuanticRegtest => EMBEDDED_QUANTIC_REGTEST_NETWORK_CONFIG,
    }
}

fn load_network_config_for(
    path: &Path,
    network: NetworkProfile,
) -> Result<MainnetNetworkConfig, String> {
    if path.exists() {
        let raw = fs::read_to_string(path)
            .map_err(|e| format!("failed to read network config '{}': {e}", path.display()))?;
        return parse_network_config_for(&raw, network);
    }
    parse_network_config_for(embedded_network_config_for(network), network)
}

fn write_secrets_export(path: &str, payload: &WalletSecretsExport) -> Result<(), String> {
    let target = PathBuf::from(path);
    if let Some(parent) = target.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create export directory: {e}"))?;
        }
    }
    let encoded = serde_json::to_string_pretty(payload)
        .map_err(|e| format!("failed to encode secrets json: {e}"))?;
    fs::write(&target, encoded)
        .map_err(|e| format!("failed to write secrets export '{}': {e}", target.display()))
}

fn private_wallet_secrets_export(wallet: &PrivateWallet) -> WalletSecretsExport {
    let mnemonic = HDWallet::new(wallet.scan_key.clone())
        .and_then(|w| w.seed_to_mnemonic())
        .ok()
        .map(|words| words.join(" "));
    let scan_private_key_hex = hex::encode(&wallet.scan_key);
    let spend_private_key_hex = hex::encode(&wallet.spending_key);
    let scan_public_key_hex = public_key_hex_from_secret(&wallet.scan_key);
    let spend_public_key_hex = if wallet.anonymity_public_key.is_empty() {
        public_key_hex_from_secret(&wallet.spending_key)
    } else {
        wallet.anonymity_public_key.clone()
    };
    WalletSecretsExport {
        wallet_name: wallet.name.clone(),
        wallet_type: "private".to_string(),
        address: wallet.commitment_key.clone(),
        user_address: user_private_address_display(&wallet.commitment_key),
        mnemonic,
        scan_private_key_hex,
        scan_public_key_hex,
        spend_private_key_hex,
        spend_public_key_hex,
    }
}

fn count_wallet_files_in_dir(dir: &Path, suffix: &str) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.ends_with(suffix))
        .count()
}

fn print_interactive_menu_screen(
    title: &str,
    accent: &str,
    subtitle: Option<&str>,
    items: &[&str],
) {
    println!();
    println!("{}", dashboard_title(title, accent));
    println!("{}", dashboard_divider());
    if let Some(subtitle) = subtitle {
        println!("  {}", paint(subtitle, ANSI_DIM));
        println!("{}", dashboard_divider());
    }
    for item in items {
        println!("  {}", paint(item, ANSI_WHITE));
    }
}

fn wallet_storage_for_network(network: NetworkProfile) -> Result<WalletStorage, String> {
    WalletStorage::from_base_dir(network.local_dir())
        .map_err(|e| format!("wallet storage init failed: {e}"))
}

fn wallet_storage_for_chain_id(chain_id: &str) -> Result<WalletStorage, String> {
    let network = NetworkProfile::from_chain_id(chain_id)?;
    wallet_storage_for_network(network)
}

fn wallet_storage_for_args(args: &[String]) -> Result<(NetworkProfile, WalletStorage), String> {
    let network = NetworkProfile::from_args(args)?;
    let storage = wallet_storage_for_network(network)?;
    Ok((network, storage))
}

fn wallet_storage_for_default_network() -> Result<(NetworkProfile, WalletStorage), String> {
    wallet_storage_for_args(&[])
}

fn wallet_storage_path_text_for(storage: &WalletStorage) -> String {
    display_path(storage.private_wallets_dir())
}

fn wallet_storage_path_text() -> String {
    wallet_storage_for_default_network()
        .map(|(_, storage)| wallet_storage_path_text_for(&storage))
        .unwrap_or_else(|_| ".zoka/mainnet/private_wallets".to_string())
}

fn local_private_wallet_file_names(storage: &WalletStorage) -> Vec<String> {
    let Ok(entries) = fs::read_dir(storage.private_wallets_dir()) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter_map(|filename| {
            filename
                .strip_suffix(".private.json")
                .map(|name| name.to_string())
        })
        .collect();
    names.sort();
    names
}

fn local_private_wallet_address_choices(storage: &WalletStorage) -> Vec<(String, String)> {
    storage
        .load_all_private_wallet_metadata()
        .unwrap_or_default()
        .into_iter()
        .map(|wallet| {
            (
                wallet.name,
                user_private_address_display(&wallet.commitment_key),
            )
        })
        .collect()
}

fn prompt_wallet_selector_with_single_default() -> Result<String, String> {
    let (_, storage) = wallet_storage_for_default_network()?;
    let wallets = local_private_wallet_address_choices(&storage);
    if wallets.len() == 1 {
        let (name, default) = wallets
            .first()
            .cloned()
            .ok_or_else(|| "wallet list changed while reading local storage".to_string())?;
        let prompt = format!("Wallet address [{default}] ({name}): ");
        return Ok(prompt_optional(&prompt)?.unwrap_or(default));
    }
    if !wallets.is_empty() {
        println!(
            "{}",
            dashboard_section("Local wallet addresses", ANSI_MAGENTA)
        );
        for (name, address) in wallets.iter().take(8) {
            println!("  {}  {}", paint(name, ANSI_DIM), address);
        }
        if wallets.len() > 8 {
            println!("  ... {} more", wallets.len().saturating_sub(8));
        }
    }
    prompt_required("Wallet address (zka1...): ")
}

fn private_wallet_file_path_text(storage: &WalletStorage, wallet_name: &str) -> String {
    let path = storage
        .private_wallets_dir()
        .join(format!("{wallet_name}.private.json"));
    display_path(&path)
}

fn display_path(path: &Path) -> String {
    let text = path.display().to_string();
    #[cfg(windows)]
    {
        text.replace('/', "\\")
    }
    #[cfg(not(windows))]
    {
        text
    }
}

fn write_fullnode_ready_file(
    path: &str,
    status: &str,
    payload: serde_json::Value,
) -> Result<(), String> {
    let ready_path = Path::new(path);
    if let Some(parent) = ready_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("ready file directory create failed: {e}"))?;
        }
    }

    let mut output = serde_json::Map::new();
    output.insert(
        "schema".to_string(),
        serde_json::Value::String("zoka-fullnode-ready-v1".to_string()),
    );
    output.insert(
        "status".to_string(),
        serde_json::Value::String(status.to_string()),
    );
    if let serde_json::Value::Object(extra) = payload {
        for (key, value) in extra {
            output.insert(key, value);
        }
    }

    let json = serde_json::to_string_pretty(&serde_json::Value::Object(output))
        .map_err(|e| format!("ready file serialize failed: {e}"))?;
    fs::write(ready_path, json).map_err(|e| format!("ready file write failed: {e}"))
}

fn seed_address_for_peer(addr: &str, peer_id: &str) -> String {
    let trimmed = addr.trim().trim_end_matches('/');
    if trimmed.contains("/p2p/") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/p2p/{peer_id}")
    }
}

fn seed_addresses_for_peer(addrs: &[String], peer_id: &str) -> Vec<String> {
    let mut out = Vec::new();
    for addr in addrs {
        let seed = seed_address_for_peer(addr, peer_id);
        if !out.contains(&seed) {
            out.push(seed);
        }
    }
    out
}

fn print_wallet_ready_notice(storage: &WalletStorage, wallet_name: &str) {
    println!(
        "Saved encrypted file: {}",
        private_wallet_file_path_text(storage, wallet_name)
    );
    println!("Ready for local genesis, mining rewards, receiving and private sends.");
    println!(
        "You can create more wallets; each one keeps its own password, keys and recovery phrase."
    );
}

fn print_main_dashboard_screen() {
    let wallet_files = wallet_storage_for_default_network()
        .map(|(_, storage)| {
            count_wallet_files_in_dir(storage.private_wallets_dir(), ".private.json")
        })
        .unwrap_or(0);
    let mining_state = load_user_mining_state_for_display().unwrap_or_default();
    let mining_status = if mining_state.is_mining {
        badge("MINING", ANSI_GREEN)
    } else {
        badge("READY", ANSI_YELLOW)
    };

    println!();
    println!("{}", dashboard_title("ZOKA Console", ANSI_CYAN));
    println!("{}", dashboard_divider());
    println!("{}", dashboard_section("Overview", ANSI_BLUE));
    println!("{}", dashboard_line("Wallets", wallet_files));
    println!("{}", dashboard_line("Mining", mining_status));
    println!(
        "{}",
        dashboard_line("Local height", mining_state.chain_height)
    );
    println!("{}", dashboard_line("Storage", wallet_storage_path_text()));
    println!("{}", dashboard_divider());
    println!("{}", dashboard_section("Main Menu", ANSI_GREEN));
    for item in [
        "1) Wallets",
        "2) Transfers",
        "3) Mining",
        "4) Node",
        "5) Status",
        "h) Help",
        "0) Exit",
    ] {
        println!("  {}", paint(item, ANSI_WHITE));
    }
}

fn interactive_wallet_menu() -> Result<bool, String> {
    loop {
        print_interactive_menu_screen(
            "Wallets",
            ANSI_MAGENTA,
            Some("Local encrypted wallets. You can keep more than one wallet on this device."),
            &[
                "1) Create wallet",
                "2) Restore from recovery phrase",
                "3) Restore from keys / view-only",
                "4) List wallets",
                "5) Balance",
                "6) History",
                "7) Backup wallet",
                "8) Export recovery / secrets",
                "9) Change passphrase",
                "10) Delete local wallet",
                "0) Back",
                "q) Exit",
            ],
        );

        match prompt_line("> ")?.as_str() {
            "1" => {
                let name = prompt_optional("Wallet label (optional): ")?;
                let password = prompt_required("Password: ")?;
                let export_path = prompt_optional("Export secrets to file (optional path): ")?;
                let mut args = Vec::new();
                push_opt_arg(&mut args, "--name", name);
                args.push("--password".to_string());
                args.push(password);
                push_opt_arg(&mut args, "--export-secrets", export_path);
                if let Err(err) = cmd_private_wallet_create(&args) {
                    eprintln!("{}", err);
                }
            }
            "2" => {
                let name = prompt_optional("Wallet label (optional): ")?;
                let seed = prompt_required("Recovery phrase: ")?;
                let restore_height =
                    prompt_u64_with_default("Restore height (default 0): ", 0)?.to_string();
                let password = prompt_required("Password: ")?;
                let mut args = vec![
                    "--seed".to_string(),
                    seed,
                    "--restore-height".to_string(),
                    restore_height,
                    "--password".to_string(),
                    password,
                ];
                push_opt_arg(&mut args, "--name", name);
                if let Err(err) = cmd_wallet_restore_from_seed(&args) {
                    eprintln!("{}", err);
                }
            }
            "3" => {
                let name = prompt_optional("Wallet label (optional): ")?;
                let scan_key_hex = prompt_required("Scan key hex (32 bytes): ")?;
                let spend_mode =
                    prompt_optional("Spend key hex (leave empty for view-only restore): ")?;
                let spend_public_hex = if spend_mode.is_none() {
                    Some(prompt_required(
                        "Spend public key hex (required for view-only): ",
                    )?)
                } else {
                    None
                };
                let restore_height =
                    prompt_u64_with_default("Restore height (default 0): ", 0)?.to_string();
                let password = prompt_required("Password: ")?;
                let mut args = vec![
                    "--scan-key-hex".to_string(),
                    scan_key_hex,
                    "--restore-height".to_string(),
                    restore_height,
                    "--password".to_string(),
                    password,
                ];
                push_opt_arg(&mut args, "--name", name);
                if let Some(spend_key_hex) = spend_mode {
                    args.push("--spend-key-hex".to_string());
                    args.push(spend_key_hex);
                } else if let Some(spend_pub_hex) = spend_public_hex {
                    args.push("--spend-public-key-hex".to_string());
                    args.push(spend_pub_hex);
                    args.push("--view-only".to_string());
                }
                if let Err(err) = cmd_wallet_restore_from_keys(&args) {
                    eprintln!("{}", err);
                }
            }
            "4" => {
                let password = prompt_optional("Password (optional): ")?;
                let mut args = Vec::new();
                push_opt_arg(&mut args, "--password", password);
                if let Err(err) = cmd_private_wallet_list(&args) {
                    eprintln!("{}", err);
                }
            }
            "5" => {
                let name = prompt_required("Wallet address (zka1...): ")?;
                let password = prompt_optional("Password (optional): ")?;
                let mut args = vec!["--address".to_string(), name];
                push_opt_arg(&mut args, "--password", password);
                if let Err(err) = cmd_private_balance(&args) {
                    eprintln!("{}", err);
                }
            }
            "6" => {
                let name = prompt_required("Wallet address (zka1...): ")?;
                let limit =
                    prompt_u64_with_default("History limit (default 20): ", 20)?.to_string();
                let password = prompt_optional("Password (optional): ")?;
                let mut args = vec!["--address".to_string(), name, "--limit".to_string(), limit];
                push_opt_arg(&mut args, "--password", password);
                if let Err(err) = cmd_private_history(&args) {
                    eprintln!("{}", err);
                }
            }
            "7" => {
                let name = prompt_required("Wallet address (zka1...): ")?;
                let out = prompt_required("Backup file path: ")?;
                let password = prompt_optional("Password (optional): ")?;
                let mut args = vec!["--address".to_string(), name, "--out".to_string(), out];
                push_opt_arg(&mut args, "--password", password);
                if let Err(err) = cmd_wallet_backup_unified(&args) {
                    eprintln!("{}", err);
                }
            }
            "8" => {
                let name = prompt_required("Wallet address (zka1...): ")?;
                let password = prompt_optional("Password (optional): ")?;
                let out = prompt_optional("Export file path (optional): ")?;
                let as_json = prompt_yes("Print full secrets here? [y/N]: ")?;
                let mut args = vec!["--address".to_string(), name];
                push_opt_arg(&mut args, "--password", password);
                push_opt_arg(&mut args, "--out", out);
                if as_json {
                    args.push("--json".to_string());
                }
                if let Err(err) = cmd_wallet_export_secrets(&args) {
                    eprintln!("{}", err);
                }
            }
            "9" => {
                let name = prompt_required("Wallet address (zka1...): ")?;
                let old_password = prompt_optional("Current password (optional): ")?;
                let new_password = prompt_required("New password: ")?;
                let mut args = vec![
                    "--address".to_string(),
                    name,
                    "--new-password".to_string(),
                    new_password,
                ];
                push_opt_arg(&mut args, "--old-password", old_password);
                if let Err(err) = cmd_wallet_change_passphrase_unified(&args) {
                    eprintln!("{}", err);
                }
            }
            "10" => {
                let name = prompt_required("Wallet address (zka1...): ")?;
                let password = prompt_optional("Password (optional): ")?;
                println!("This deletes only the encrypted wallet file on this computer.");
                println!(
                    "You can restore it later only if you saved the recovery phrase or a backup."
                );
                if !prompt_yes("Delete this local wallet file? [y/N]: ")? {
                    println!("Cancelled.");
                    continue;
                }
                let mut args = vec!["--address".to_string(), name, "--force".to_string()];
                push_opt_arg(&mut args, "--password", password);
                if let Err(err) = cmd_wallet_wipe_unified(&args) {
                    eprintln!("{}", err);
                }
            }
            "0" => return Ok(false),
            "q" | "Q" => return Ok(true),
            _ => println!("Invalid option"),
        }
    }
}

fn interactive_transfer_menu() -> Result<bool, String> {
    loop {
        print_interactive_menu_screen(
            "Transfers",
            ANSI_BLUE,
            Some("Send and receive privately. Fees are estimated automatically."),
            &[
                "1) Send ZKA",
                "2) Receive address",
                "3) Balance",
                "4) History",
                "5) Pending transfers",
                "6) Estimate fee",
                "0) Back",
                "q) Exit",
            ],
        );

        match prompt_line("> ")?.as_str() {
            "1" => {
                let from = prompt_required("From wallet address (zka1...): ")?;
                let to = prompt_required("To wallet address (zka1...): ")?;
                let amount = prompt_required("Amount in ZKA: ")?;
                let password = prompt_optional("Password (optional): ")?;
                let db_path = DEFAULT_CHAIN_DB_PATH.to_string();
                let from_commitment = match require_private_network_address(&from, "--from") {
                    Ok(value) => value,
                    Err(err) => {
                        eprintln!("{}", err);
                        continue;
                    }
                };
                let to_commitment = match require_private_recipient_address(&to, "--to") {
                    Ok(value) => value,
                    Err(err) => {
                        eprintln!("{}", err);
                        continue;
                    }
                };
                let storage = match wallet_storage_for_network(NetworkProfile::Zenith) {
                    Ok(value) => value,
                    Err(err) => {
                        eprintln!("{}", err);
                        continue;
                    }
                };
                let manager = match load_private_wallet_manager_for_network(
                    &storage,
                    password.as_deref(),
                    NetworkProfile::Zenith,
                ) {
                    Ok(value) => value,
                    Err(err) => {
                        eprintln!("{}", err);
                        continue;
                    }
                };
                if let Err(err) =
                    resolve_network_wallet_address_required(&manager, &storage, &from, "--from")
                {
                    eprintln!("{}", err);
                    continue;
                }
                let from_display = user_private_address_display(&from_commitment);
                let to_display = user_private_address_display(&to_commitment);
                let preview_args = vec![
                    "--amount".to_string(),
                    amount.clone(),
                    "--db".to_string(),
                    db_path.clone(),
                ];
                let amount_atoms = match parse_zka_amount_arg(Some(amount.clone()), 0, "--amount") {
                    Ok(value) => value,
                    Err(err) => {
                        eprintln!("{}", err);
                        continue;
                    }
                };
                let fee_atoms = match resolve_fee(&preview_args) {
                    Ok(value) => value,
                    Err(err) => {
                        eprintln!("{}", err);
                        continue;
                    }
                };
                println!("{}", dashboard_divider());
                println!("{}", dashboard_section("Confirm Private Send", ANSI_YELLOW));
                println!("{}", dashboard_line("From", &from_display));
                println!("{}", dashboard_line("To", &to_display));
                println!("{}", dashboard_line("Amount", format_zka(amount_atoms)));
                println!("{}", dashboard_line("Estimated fee", format_zka(fee_atoms)));
                println!("{}", dashboard_line("Node queue", &db_path));
                if !prompt_yes("Confirm send? [y/N]: ")? {
                    println!("Cancelled.");
                    continue;
                }
                let mut args = vec![
                    "--from".to_string(),
                    from_display,
                    "--to".to_string(),
                    to_display,
                    "--amount".to_string(),
                    amount,
                    "--db".to_string(),
                    db_path,
                    "--yes".to_string(),
                ];
                push_opt_arg(&mut args, "--password", password);
                if let Err(err) = cmd_tx_send_unified(&args) {
                    eprintln!("{}", err);
                }
            }
            "2" => {
                let wallet = prompt_required("Wallet address (zka1...): ")?;
                let password = prompt_optional("Password (optional): ")?;
                let mut args = vec!["--address".to_string(), wallet];
                push_opt_arg(&mut args, "--password", password);
                if let Err(err) = cmd_receive_address(&args) {
                    eprintln!("{}", err);
                }
            }
            "3" => {
                let wallet = prompt_required("Wallet address (zka1...): ")?;
                let password = prompt_optional("Password (optional): ")?;
                let mut args = vec!["--address".to_string(), wallet];
                push_opt_arg(&mut args, "--password", password);
                if let Err(err) = cmd_private_balance(&args) {
                    eprintln!("{}", err);
                }
            }
            "4" => {
                let wallet = prompt_required("Wallet address (zka1...): ")?;
                let limit =
                    prompt_u64_with_default("History limit (default 20): ", 20)?.to_string();
                let password = prompt_optional("Password (optional): ")?;
                let mut args = vec![
                    "--address".to_string(),
                    wallet,
                    "--limit".to_string(),
                    limit,
                ];
                push_opt_arg(&mut args, "--password", password);
                if let Err(err) = cmd_private_history(&args) {
                    eprintln!("{}", err);
                }
            }
            "5" => {
                let db_path = prompt_optional("Node DB path (empty = .zoka/blockchain.db): ")?
                    .unwrap_or_else(|| DEFAULT_CHAIN_DB_PATH.to_string());
                let args = vec!["--db".to_string(), db_path];
                if let Err(err) = cmd_pending_transfers(&args) {
                    eprintln!("{}", err);
                }
            }
            "6" => {
                let amount = prompt_required("Amount in ZKA: ")?;
                let args = vec!["--amount".to_string(), amount];
                if let Err(err) = cmd_estimate_fee(&args) {
                    eprintln!("{}", err);
                }
            }
            "0" => return Ok(false),
            "q" | "Q" => return Ok(true),
            _ => println!("Invalid option"),
        }
    }
}

async fn interactive_mining_menu() -> Result<bool, String> {
    loop {
        print_interactive_menu_screen(
            "Mining",
            ANSI_GREEN,
            Some("CPU mining into one of your local wallets."),
            &[
                "1) Start mining",
                "2) Mining status",
                "3) Mining console",
                "0) Back",
                "q) Exit",
            ],
        );

        match prompt_line("> ")?.as_str() {
            "1" => {
                // Runs the same connected-fullnode path as Node > Start mining
                // node: `cmd_mine_easy` mined into the local chain DB only,
                // with no P2P networking at all, so nothing it "mined" was
                // ever broadcast to real peers or credited on the real
                // chain -- it looked like it worked and then silently went
                // nowhere. Real mainnet mining needs the connected node.
                let wallet = prompt_wallet_selector_with_single_default()?;
                let password = prompt_optional("Password (optional): ")?;
                let threads = prompt_mining_cpu_threads()?;
                let seconds = prompt_optional("Run seconds (empty = keep running): ")?;
                let allow_isolated =
                    prompt_yes_default("Allow standalone start if seeds fail? [Y/n]: ", true)?;
                let mut args = vec!["--address".to_string(), wallet];
                args.push("--threads".to_string());
                args.push(threads.to_string());
                push_opt_arg(&mut args, "--password", password);
                push_opt_arg(&mut args, "--seconds", seconds);
                if allow_isolated {
                    args.push("--allow-isolated-start".to_string());
                }
                if let Err(err) = cmd_fullnode_auto_boxed(&args).await {
                    eprintln!("{}", err);
                }
            }
            "2" => {
                if let Err(err) = cmd_mine_status(&[]) {
                    eprintln!("{}", err);
                }
            }
            "3" => {
                if let Err(err) = cmd_mine_console(&[]) {
                    eprintln!("{}", err);
                }
            }
            "0" => return Ok(false),
            "q" | "Q" => return Ok(true),
            _ => println!("Invalid option"),
        }
    }
}

async fn interactive_node_menu() -> Result<bool, String> {
    loop {
        print_interactive_menu_screen(
            "Node",
            ANSI_CYAN,
            Some("Run a fullnode, mine if you choose, and inspect the local node state."),
            &[
                "1) Start mining node",
                "2) Start verifier node (no mining)",
                "3) Local node status",
                "4) Show node wallet recovery info",
                "0) Back",
                "q) Exit",
            ],
        );

        match prompt_line("> ")?.as_str() {
            "1" => {
                println!("Leave fields empty to use defaults.");
                let wallet = prompt_optional("Wallet address (zka1..., optional): ")?;
                let password = prompt_optional("Wallet password (optional): ")?;
                let seed = prompt_optional("Seed peer multiaddr (optional): ")?;
                let use_network_config =
                    prompt_yes_default("Use configured seed network? [Y/n]: ", true)?;
                let allow_isolated =
                    prompt_yes_default("Allow standalone start if seeds fail? [y/N]: ", false)?;
                let threads = prompt_mining_cpu_threads()?;
                let seconds = prompt_optional("Run seconds (empty = keep running): ")?;
                let show_keys = prompt_yes("Show recovery phrase / keys on startup? [y/N]: ")?;
                let mut args = Vec::new();
                push_opt_arg(&mut args, "--seconds", seconds);
                push_opt_arg(&mut args, "--address", wallet);
                push_opt_arg(&mut args, "--password", password);
                push_opt_arg(&mut args, "--seed", seed);
                args.push("--threads".to_string());
                args.push(threads.to_string());
                if !use_network_config {
                    args.push("--no-network-config".to_string());
                }
                if allow_isolated {
                    args.push("--allow-isolated-start".to_string());
                }
                if show_keys {
                    args.push("--show-keys".to_string());
                }
                if let Err(err) = cmd_fullnode_auto_boxed(&args).await {
                    eprintln!("{}", err);
                }
            }
            "2" => {
                println!("Verifier mode keeps the node online and syncing without local PoW.");
                let wallet = prompt_optional("Wallet address (zka1..., optional): ")?;
                let password = prompt_optional("Wallet password (optional): ")?;
                let seed = prompt_optional("Seed peer multiaddr (optional): ")?;
                let use_network_config =
                    prompt_yes_default("Use configured seed network? [Y/n]: ", true)?;
                let allow_isolated =
                    prompt_yes_default("Allow standalone start if seeds fail? [y/N]: ", false)?;
                let seconds = prompt_optional("Run seconds (empty = keep running): ")?;
                let mut args = Vec::new();
                push_opt_arg(&mut args, "--seconds", seconds);
                push_opt_arg(&mut args, "--address", wallet);
                push_opt_arg(&mut args, "--password", password);
                push_opt_arg(&mut args, "--seed", seed);
                args.push("--no-mine".to_string());
                if !use_network_config {
                    args.push("--no-network-config".to_string());
                }
                if allow_isolated {
                    args.push("--allow-isolated-start".to_string());
                }
                if let Err(err) = cmd_fullnode_auto_boxed(&args).await {
                    eprintln!("{}", err);
                }
            }
            "3" => {
                let db = prompt_optional("Node DB path (empty = .zoka/blockchain.db): ")?;
                let wallet = prompt_optional("Wallet address (zka1..., optional): ")?;
                let password = prompt_optional("Wallet password (optional): ")?;
                let mut args = Vec::new();
                push_opt_arg(&mut args, "--db", db);
                push_opt_arg(&mut args, "--address", wallet);
                push_opt_arg(&mut args, "--password", password);
                if let Err(err) = cmd_node_status(&args) {
                    eprintln!("{}", err);
                }
            }
            "4" => {
                let wallet = prompt_optional("Wallet address (zka1..., optional): ")?;
                let password = prompt_optional("Wallet password (optional): ")?;
                let create_if_missing = prompt_yes("Create if missing? [y/N]: ")?;
                let mut args = Vec::new();
                push_opt_arg(&mut args, "--address", wallet);
                push_opt_arg(&mut args, "--password", password);
                if create_if_missing {
                    args.push("--create-if-missing".to_string());
                }
                if let Err(err) = cmd_node_wallet_info(&args) {
                    eprintln!("{}", err);
                }
            }
            "0" => return Ok(false),
            "q" | "Q" => return Ok(true),
            _ => println!("Invalid option"),
        }
    }
}

fn interactive_status_menu() -> Result<bool, String> {
    loop {
        print_interactive_menu_screen(
            "Status",
            ANSI_YELLOW,
            Some("One dashboard for wallet, transfers, mining and the local node."),
            &[
                "1) Overview dashboard",
                "2) Local node status",
                "3) Mining dashboard",
                "4) Wallet sync status",
                "5) Wallet list",
                "6) Wallet history",
                "0) Back",
                "q) Exit",
            ],
        );

        match prompt_line("> ")?.as_str() {
            "1" => {
                let db = prompt_optional("DB path (optional): ")?;
                let wallet = prompt_optional("Wallet address (zka1..., optional): ")?;
                let password = prompt_optional("Password (optional): ")?;
                let mut args = Vec::new();
                push_opt_arg(&mut args, "--db", db);
                push_opt_arg(&mut args, "--address", wallet);
                push_opt_arg(&mut args, "--password", password);
                if let Err(err) = cmd_status_overview(&args) {
                    eprintln!("{}", err);
                }
            }
            "2" => {
                let db = prompt_optional("Node DB path (empty = .zoka/blockchain.db): ")?;
                let wallet = prompt_optional("Wallet address (zka1..., optional): ")?;
                let password = prompt_optional("Password (optional): ")?;
                let mut args = Vec::new();
                push_opt_arg(&mut args, "--db", db);
                push_opt_arg(&mut args, "--address", wallet);
                push_opt_arg(&mut args, "--password", password);
                if let Err(err) = cmd_node_status(&args) {
                    eprintln!("{}", err);
                }
            }
            "3" => {
                let wallet = prompt_optional("Wallet address (zka1..., optional): ")?;
                let db = prompt_optional("DB path (optional): ")?;
                let password = prompt_optional("Password (optional): ")?;
                let mut args = Vec::new();
                push_opt_arg(&mut args, "--address", wallet);
                push_opt_arg(&mut args, "--db", db);
                push_opt_arg(&mut args, "--password", password);
                if let Err(err) = cmd_mine_console(&args) {
                    eprintln!("{}", err);
                }
            }
            "4" => {
                let wallet = prompt_required("Wallet address (zka1...): ")?;
                let db = prompt_optional("DB path (optional): ")?;
                let password = prompt_optional("Password (optional): ")?;
                let mut args = vec!["--address".to_string(), wallet];
                push_opt_arg(&mut args, "--db", db);
                push_opt_arg(&mut args, "--password", password);
                if let Err(err) = cmd_sync_status_unified(&args) {
                    eprintln!("{}", err);
                }
            }
            "5" => {
                let password = prompt_optional("Password (optional): ")?;
                let mut args = Vec::new();
                push_opt_arg(&mut args, "--password", password);
                if let Err(err) = cmd_wallet_list_unified(&args) {
                    eprintln!("{}", err);
                }
            }
            "6" => {
                let wallet = prompt_required("Wallet address (zka1...): ")?;
                let limit = prompt_u64_with_default("Limit (default 20): ", 20)?.to_string();
                let password = prompt_optional("Password (optional): ")?;
                let mut args = vec![
                    "--address".to_string(),
                    wallet,
                    "--limit".to_string(),
                    limit,
                ];
                push_opt_arg(&mut args, "--password", password);
                if let Err(err) = cmd_private_history(&args) {
                    eprintln!("{}", err);
                }
            }
            "0" => return Ok(false),
            "q" | "Q" => return Ok(true),
            _ => println!("Invalid option"),
        }
    }
}

// Zenith: no se llama en una build de Quantic (Zenith se retiró el 21/10/2026).
#[cfg_attr(feature = "quantic", allow(dead_code))]
async fn run_interactive_menu() -> Result<(), String> {
    loop {
        print_main_dashboard_screen();

        match prompt_line("> ")?.as_str() {
            "1" => {
                if interactive_wallet_menu()? {
                    return Ok(());
                }
            }
            "2" => {
                if interactive_transfer_menu()? {
                    return Ok(());
                }
            }
            "3" => {
                if interactive_mining_menu().await? {
                    return Ok(());
                }
            }
            "4" => {
                if interactive_node_menu().await? {
                    return Ok(());
                }
            }
            "5" => {
                if interactive_status_menu()? {
                    return Ok(());
                }
            }
            "h" | "H" => print_usage(),
            "0" => return Ok(()),
            _ => println!("Invalid option"),
        }
    }
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

fn option_values(args: &[String], flag: &str) -> Vec<String> {
    args.windows(2)
        .filter(|pair| pair[0] == flag)
        .map(|pair| pair[1].clone())
        .collect()
}

fn normalize_positional_alias_args(args: &[String], positional_flags: &[&str]) -> Vec<String> {
    let mut normalized = Vec::new();
    let mut next_positional = 0usize;
    let mut i = 0usize;

    while i < args.len() {
        let arg = &args[i];
        if arg.starts_with('-') {
            normalized.push(arg.clone());
            if i + 1 < args.len() && !args[i + 1].starts_with('-') {
                normalized.push(args[i + 1].clone());
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }

        while next_positional < positional_flags.len()
            && normalized
                .iter()
                .any(|existing| existing == positional_flags[next_positional])
        {
            next_positional += 1;
        }

        if let Some(flag) = positional_flags.get(next_positional) {
            normalized.push((*flag).to_string());
            normalized.push(arg.clone());
            next_positional += 1;
        } else {
            normalized.push(arg.clone());
        }
        i += 1;
    }

    normalized
}

fn parse_u64_arg(value: Option<String>, default: u64, flag_name: &str) -> Result<u64, String> {
    match value {
        Some(raw) => raw
            .parse::<u64>()
            .map_err(|_| format!("invalid numeric value for {flag_name}: {raw}")),
        None => Ok(default),
    }
}

fn parse_zka_amount_arg(
    value: Option<String>,
    default_atoms: u64,
    flag_name: &str,
) -> Result<u64, String> {
    match value {
        Some(raw) => economics::parse_zka_amount(&raw)
            .map_err(|reason| format!("invalid ZKA amount for {flag_name}: {reason}")),
        None => Ok(default_atoms),
    }
}

fn format_zka_amount(atoms: u64) -> String {
    economics::format_zka_amount(atoms)
}

fn format_zka(atoms: u64) -> String {
    format!("{} ZKA", format_zka_amount(atoms))
}

fn zka_to_atoms_checked(zka: u64, label: &str) -> Result<u64, String> {
    economics::zka_to_atoms(zka)
        .ok_or_else(|| format!("{label} overflows atomic ZKA representation"))
}

fn parse_u32_arg(value: Option<String>, default: u32, flag_name: &str) -> Result<u32, String> {
    match value {
        Some(raw) => raw
            .parse::<u32>()
            .map_err(|_| format!("invalid numeric value for {flag_name}: {raw}")),
        None => Ok(default),
    }
}

fn default_mining_threads() -> usize {
    std::thread::available_parallelism()
        .map(|threads| threads.get())
        .unwrap_or(1)
        .max(1)
}

fn mining_threads_for_power_preset(preset: &str) -> Result<usize, String> {
    let available = default_mining_threads();
    match preset.to_ascii_lowercase().as_str() {
        "low" => Ok(1),
        "normal" | "balanced" => Ok((available / 2).max(1)),
        "max" | "maximum" | "all" => Ok(available),
        other => Err(format!(
            "invalid --power '{}'; expected low|normal|max",
            other
        )),
    }
}

fn mining_power_label_for_threads(threads: usize) -> String {
    let available = default_mining_threads();
    if threads >= available {
        format!("max ({threads}/{available} CPU thread(s))")
    } else if threads == (available / 2).max(1) && threads > 1 {
        format!("normal ({threads}/{available} CPU thread(s))")
    } else if threads == 1 {
        format!("low (1/{available} CPU thread(s))")
    } else {
        format!("custom ({threads}/{available} CPU thread(s))")
    }
}

fn seed_words_to_entropy_bytes(words: u64) -> Result<usize, String> {
    match words {
        12 => Ok(16),
        24 => Ok(32),
        other => Err(format!(
            "invalid --seed-words '{}'; expected 12 or 24",
            other
        )),
    }
}

fn parse_thread_count_arg(args: &[String]) -> Result<usize, String> {
    if let Some(raw) = option_value(args, "--threads") {
        let parsed = parse_u64_arg(Some(raw), default_mining_threads() as u64, "--threads")?;
        if parsed == 0 {
            return Err("--threads must be >= 1".to_string());
        }
        return Ok(parsed.min(256) as usize);
    }
    if let Some(power) = option_value(args, "--power") {
        return mining_threads_for_power_preset(&power);
    }
    let raw = default_mining_threads() as u64;
    if raw == 0 {
        return Err("--threads must be >= 1".to_string());
    }
    Ok(raw.min(256) as usize)
}

fn required_option_value(args: &[String], flag: &str) -> Result<String, String> {
    option_value(args, flag)
        .ok_or_else(|| format!("missing required argument: {flag} (run with no args to see usage)"))
}

fn secret_from_env_var(var_name: &str, flag: &str) -> Result<String, String> {
    if var_name.trim().is_empty() {
        return Err(format!("{flag} cannot be empty"));
    }
    std::env::var(var_name)
        .map_err(|_| format!("{flag} references unset environment variable: {var_name}"))
}

fn stdin_secret(flag: &str) -> Result<String, String> {
    let mut secret = String::new();
    io::stdin()
        .read_to_string(&mut secret)
        .map_err(|e| format!("failed to read {flag}: {e}"))?;
    Ok(secret.trim_end_matches(&['\r', '\n'][..]).to_string())
}

fn password_value(args: &[String]) -> Result<Option<String>, String> {
    if let Some(password) = option_value(args, "--password") {
        return Ok(Some(password));
    }
    if let Some(password) = option_value(args, "--wallet-password") {
        return Ok(Some(password));
    }
    if let Some(var_name) =
        option_value(args, "--password-env").or_else(|| option_value(args, "--wallet-password-env"))
    {
        return secret_from_env_var(&var_name, "--password-env").map(Some);
    }
    if has_flag(args, "--password-stdin") || has_flag(args, "--wallet-password-stdin") {
        return stdin_secret("--password-stdin").map(Some);
    }
    Ok(None)
}

fn required_password_value(args: &[String]) -> Result<String, String> {
    let password = password_value(args)?.ok_or_else(|| {
        "missing required argument: --password, --password-env, or --password-stdin".to_string()
    })?;
    if password.is_empty() {
        return Err(
            "--password cannot be empty: choose a non-empty passphrase to protect this wallet"
                .to_string(),
        );
    }
    Ok(password)
}

fn required_seed_phrase_value(args: &[String]) -> Result<String, String> {
    if let Some(seed) = option_value(args, "--seed").or_else(|| option_value(args, "--mnemonic")) {
        return Ok(seed);
    }
    if let Some(var_name) =
        option_value(args, "--seed-env").or_else(|| option_value(args, "--mnemonic-env"))
    {
        return secret_from_env_var(&var_name, "--seed-env");
    }
    if has_flag(args, "--seed-stdin") || has_flag(args, "--mnemonic-stdin") {
        return stdin_secret("--seed-stdin");
    }
    Err("missing required argument: --seed, --seed-env, or --seed-stdin".to_string())
}

fn wallet_selector_value(args: &[String]) -> Option<String> {
    option_value(args, "--address")
}

fn required_wallet_selector(args: &[String]) -> Result<String, String> {
    wallet_selector_value(args)
        .ok_or_else(|| "missing wallet selector: use --address <wallet-address>".to_string())
}

fn require_private_network_address(raw: &str, flag: &str) -> Result<String, String> {
    if !(raw.starts_with("zka1") || raw.starts_with("zpriv:")) {
        return Err(format!(
            "{flag}: expected a full zka1 wallet address; local wallet names are not accepted"
        ));
    }
    validate_user_private_address(raw).map_err(|reason| format!("{flag}: {reason}"))
}

/// Destinatario de un envío: una dirección `zka1` como siempre, o una v2
/// `zkq1` (Zoka Quantic) si la red ya fijó su activación y la build la
/// soporta. Antes de la activación la red rechazaría la transacción: mejor
/// decirlo acá que dejar notas reservadas por un envío que no va a entrar.
fn require_private_recipient_address(raw: &str, flag: &str) -> Result<String, String> {
    if !zoka::private_wallet::is_hybrid_payment_address(raw) {
        return require_private_network_address(raw, flag);
    }
    if economics::pq_hybrid_outputs_activation_height().is_none() {
        return Err(format!(
            "{flag}: zkq1 hybrid addresses are not active on this network yet; use the recipient's zka1 address"
        ));
    }
    zoka::private_wallet::canonical_send_recipient(raw).map_err(|e| format!("{flag}: {e}"))
}

fn find_private_wallet_name_by_commitment(
    manager: &PrivateWalletManager,
    commitment: &str,
) -> Option<String> {
    // Una zkq1 no es el commitment de ninguna wallet: se compara con la
    // dirección v2 que cada wallet deriva de sus claves.
    let hybrid = zoka::private_wallet::is_hybrid_payment_address(commitment);
    manager.get_private_wallet_names().into_iter().find(|name| {
        manager
            .get_private_wallet(name)
            .map(|wallet| {
                wallet.commitment_key == commitment
                    || (hybrid
                        && wallet.hybrid_payment_address().ok().as_deref() == Some(commitment))
            })
            .unwrap_or(false)
    })
}

fn find_private_wallet_metadata_name_by_commitment(
    storage: &WalletStorage,
    commitment: &str,
) -> Option<String> {
    storage
        .load_all_private_wallet_metadata()
        .ok()?
        .into_iter()
        .find(|wallet| wallet.commitment_key == commitment)
        .map(|wallet| wallet.name)
}

fn resolve_network_wallet_address_required(
    manager: &PrivateWalletManager,
    storage: &WalletStorage,
    wallet_address: &str,
    flag: &str,
) -> Result<(String, String), String> {
    let commitment = require_private_network_address(wallet_address, flag)?;
    if let Some(wallet_name) = find_private_wallet_name_by_commitment(manager, &commitment) {
        return Ok((wallet_name, commitment));
    }
    let display_address = user_private_address_display(&commitment);
    if let Some(wallet_name) = find_private_wallet_metadata_name_by_commitment(storage, &commitment)
    {
        return Err(format!(
            "{flag}: wallet address '{}' belongs to local wallet '{}' but it could not be unlocked with the provided password",
            display_address, wallet_name
        ));
    }
    Err(format!(
        "{flag}: wallet address '{}' is valid but no unlocked local wallet matches it; run 'wallet-list' to see available wallet addresses",
        display_address
    ))
}

fn resolve_private_wallet_name(
    manager: &PrivateWalletManager,
    wallet_selector: &str,
) -> Option<String> {
    if manager.get_private_wallet(wallet_selector).is_some() {
        return Some(wallet_selector.to_string());
    }
    let normalized = zpriv_from_user_private_address(wallet_selector)?;
    manager.get_private_wallet_names().into_iter().find(|name| {
        manager
            .get_private_wallet(name)
            .map(|wallet| wallet.commitment_key == normalized)
            .unwrap_or(false)
    })
}

fn resolve_private_wallet_name_required(
    manager: &PrivateWalletManager,
    wallet_selector: &str,
) -> Result<String, String> {
    let commitment = require_private_network_address(wallet_selector, "--address")?;
    find_private_wallet_name_by_commitment(manager, &commitment).ok_or_else(|| {
        let display_address = user_private_address_display(&commitment);
        format!(
            "--address: wallet address '{}' is valid but no unlocked local wallet matches it; run 'wallet-list' to see available wallet addresses",
            display_address
        )
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct UserMiningState {
    total_blocks: u64,
    total_rewards: u64,
    is_mining: bool,
    chain_height: u64,
    current_difficulty_bits: u32,
    estimated_hashrate_hps: u64,
    avg_solve_time_secs: f64,
    last_solve_time_secs: f64,
    virtual_time_elapsed_secs: f64,
    total_supply_emitted: u64,
    mode: String,
    algorithm: String,
    last_block_hash: String,
    pending_rewards: Vec<PendingMiningReward>,
    last_updated: u64,
    mining_process_id: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PendingMiningReward {
    wallet_address: String,
    amount: u64,
    source_height: u64,
    unlock_height: u64,
    block_hash: String,
    #[serde(default)]
    private_coinbase: Option<zoka::types::PrivateCoinbaseOutput>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct FeeMarketSnapshot {
    pending_private_txs: u64,
    avg_recent_block_txs: f64,
    updated_at: u64,
}

#[derive(Debug, Clone)]
struct WalletRuntimeSnapshot {
    wallet_name: String,
    address: String,
    balance: u64,
    locked_balance: u64,
    total_balance: u64,
    tx_count: usize,
    view_only: bool,
    restore_height: u64,
}

#[derive(Debug, Clone)]
struct MiningEngineConfig {
    chain_id: String,
    wallet_selector: String,
    password: Option<String>,
    blocks: u64,
    mode: String,
    hashrate_hps: u64,
    threads: usize,
    base_difficulty: u32,
    unsafe_rehearsal_difficulty_bits: Option<u32>,
    target_block_time_secs: u64,
    /// RocksDB path for canonical chain persistence. When provided every
    /// mined block is persisted via `state::chain::apply_block` (single
    /// ingress path shared with peer-received blocks). Defaults to the
    /// fullnode's standard DB when None.
    chain_db_path: Option<String>,
    /// Public RPC peers used to pull pending private transactions immediately
    /// before mining/assembling a block. This keeps standalone miners aligned
    /// with the network queue instead of producing empty private bundles.
    rpc_sync_endpoints: Vec<String>,
    /// Abort in-flight PoW work (checked on the same 800ms tick as the
    /// stale-parent guard) when POST /local/mining/stop flips the local
    /// mining control off. Only set by the fullnode loop when mining is
    /// governed by that control (no --mine flag); standalone miners and
    /// --mine nodes ignore the control and must not churn on it.
    honor_local_mining_control: bool,
    /// A network-difficulty solution already found by a mining-team member.
    /// When present, the normal solver is skipped but the candidate is still
    /// recomputed, self-validated, and persisted through the canonical block
    /// path. The coordinator wallet remains the local storage/fee wallet;
    /// coinbase is paid to the recipient committed by the team template.
    team_solution: Option<zoka::team_mining::NetworkSolution>,
}

#[derive(Debug, Clone)]
struct MiningBatchSummary {
    blocks_mined: u64,
    reward_credited: u64,
    last_difficulty_bits: u32,
    /// Blocks produced in this batch, in mining order. Callers that own a
    /// libp2p node use these to publish a `ZokaMessage::BlockResponse` so
    /// peers can apply the same block via `state::chain::apply_block`.
    /// Standalone `mine` callers without a node simply ignore this.
    produced_blocks: Vec<zoka::types::Block>,
    /// Set only when the batch assembled a mining-team solution. This lets
    /// the network loop advance durable turn accounting after canonical
    /// persistence, without treating a remote member's payout as local.
    team_recipient: Option<String>,
}

#[derive(Debug, Clone)]
struct MiningProgressSnapshot {
    height: u64,
    difficulty_bits: u32,
    nonce: u64,
    hashes_attempted: u64,
    elapsed_secs: f64,
    estimated_hashrate_hps: f64,
}

struct TeamShareTask {
    template_id: String,
    cancel: Arc<AtomicBool>,
    handle: tokio::task::JoinHandle<Result<TeamShareSolveSummary, String>>,
}

struct TeamShareSolveSummary {
    share: zoka::team_mining::SubmittedShare,
    hashes_attempted: u64,
    elapsed_secs: f64,
}

impl Default for UserMiningState {
    fn default() -> Self {
        Self {
            total_blocks: 0,
            total_rewards: 0,
            is_mining: false,
            chain_height: 0,
            current_difficulty_bits: POW_INITIAL_DIFFICULTY_BITS,
            estimated_hashrate_hps: 0,
            avg_solve_time_secs: 0.0,
            last_solve_time_secs: 0.0,
            virtual_time_elapsed_secs: 0.0,
            total_supply_emitted: 0,
            mode: MAINNET_MINING_MODE.to_string(),
            algorithm: active_pow_algorithm().to_string(),
            last_block_hash: genesis_pow_hash(),
            pending_rewards: vec![],
            last_updated: now_secs(),
            mining_process_id: None,
        }
    }
}

const EMISSION_MAX_SUPPLY: u64 = economics::MAX_SUPPLY_ATOMS;
const EMISSION_HALVING_INTERVAL: u64 = economics::HALVING_INTERVAL_BLOCKS;
const FEE_MARKET_STEP_ATOMS: u64 = economics::ATOMS_PER_ZKA / 10_000;
const MAINNET_CHAIN_ID: &str = economics::MAINNET_CHAIN_ID;
const TESTNET_CHAIN_ID: &str = economics::TESTNET_CHAIN_ID;
const MAINNET_MINING_MODE: &str = "pow-mainnet";
/// Lo que contesta el nodo si alguien todavía pide Zenith.
#[cfg(feature = "quantic")]
const ZENITH_RETIRED: &str = "Zenith was retired on 2026-10-21. Zoka has two networks: quantic-testnet and quantic-mainnet. Zenith runs only on releases before 3.0.";
/// Quantic mina con su propio camino (`zoka::quantic::miner`); el modo solo se
/// muestra.
const QUANTIC_MINING_MODE: &str = "pow-quantic";
const TESTNET_MINING_MODE: &str = "pow-testnet";
const POW_TARGET_BLOCK_TIME_SECS: u64 = economics::TARGET_BLOCK_TIME_SECS;
const POW_INITIAL_DIFFICULTY_BITS: u32 = economics::POW_INITIAL_DIFFICULTY_BITS;
const POW_MIN_DIFFICULTY_BITS: u32 = economics::POW_MIN_DIFFICULTY_BITS;
const POW_MAX_DIFFICULTY_BITS: u32 = economics::POW_MAX_DIFFICULTY_BITS;
const COINBASE_MATURITY_BLOCKS: u64 = economics::COINBASE_MATURITY_BLOCKS;
const LOCAL_PRIVATE_TXS_PER_BLOCK_LIMIT: usize = economics::PRIVATE_TX_HASHES_PER_BLOCK_LIMIT;
const MINING_STALE_AFTER_SECS: u64 = 30;
const DEFAULT_CHAIN_DB_PATH: &str = ".zoka/mainnet/blockchain.db";

/// Perfil de red local. Hay uno solo: Zenith (`zoka-mainnet-zenith-v1`), que
/// desde octubre de 2026 es la testnet de Zoka Quantic.
///
/// Todo lo que identifica a Zenith en disco y en el protocolo queda igual: el
/// chain id (de el sale el hash del genesis), `.zoka/mainnet`, el manifiesto y
/// el genesis `mainnet`, el modo de mineria y la etiqueta interna "mainnet"
/// que las bases guardan en `chain:genesis:network`. Mover los datos haria que
/// un cliente viejo y uno nuevo vieran wallets distintas, y Zoka Quantic va a
/// tener su propio perfil y sus propias rutas, que ningun cliente viejo conoce.
///
/// `zoka-testnet-v1` nunca funciono como red publica y quedo retirada: lo que
/// haya en `.zoka/testnet` no se abre ni se toca.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NetworkProfile {
    Zenith,
    /// Zoka Quantic, la red principal: Ironwood y X-Wing desde el bloque 1.
    /// Arranca recién cuando su génesis está fijo en el binario.
    QuanticMainnet,
    /// Zoka Quantic: su testnet nueva, con Ironwood y X-Wing desde el bloque
    /// 1 (etapa 5). Solo en builds con la feature `quantic`.
    QuanticTestnet,
    /// Redes locales de Quantic: prueba de trabajo liviana, maduración de 2
    /// bloques y un premine a una clave de prueba pública.
    QuanticRegtest,
}

impl NetworkProfile {
    fn from_args(args: &[String]) -> Result<Self, String> {
        let raw = option_value(args, "--network")
            .or_else(|| std::env::var("ZOKA_NETWORK").ok())
            .unwrap_or_else(|| Self::DEFAULT_NAME.to_string());
        Self::from_name(&raw)
    }

    /// Desde el 21/10/2026 Zoka tiene dos redes, las de Zoka Quantic, y Zenith
    /// se retiró (decisión del usuario: que las redes no se mezclen). Una build
    /// sin `quantic` no puede seguir ninguna de las dos y queda como la de
    /// Zenith, para las pruebas de su código.
    #[cfg(feature = "quantic")]
    const DEFAULT_NAME: &'static str = "quantic-testnet";
    #[cfg(not(feature = "quantic"))]
    const DEFAULT_NAME: &'static str = "testnet";

    fn from_name(raw: &str) -> Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            // La testnet y la mainnet son las de Quantic: no hay otra red.
            #[cfg(feature = "quantic")]
            "testnet" | "quantic-testnet" => Ok(Self::QuanticTestnet),
            #[cfg(feature = "quantic")]
            "mainnet" | "quantic-mainnet" | "quantic" => Ok(Self::QuanticMainnet),
            #[cfg(feature = "quantic")]
            "zenith" => Err(ZENITH_RETIRED.to_string()),
            // "mainnet" es el nombre anterior de Zenith: lo pasaban ZSilent 2.x,
            // los scripts de minería y este mismo binario al relanzarse.
            #[cfg(not(feature = "quantic"))]
            "testnet" | "zenith" | "mainnet" => Ok(Self::Zenith),
            #[cfg(not(feature = "quantic"))]
            "quantic-mainnet" | "quantic" => Self::quantic(Self::QuanticMainnet),
            #[cfg(not(feature = "quantic"))]
            "quantic-testnet" => Self::quantic(Self::QuanticTestnet),
            // Una red local de pruebas: nunca pública.
            "quantic-regtest" => Self::quantic(Self::QuanticRegtest),
            other => Err(format!(
                "invalid --network '{other}'; expected quantic-testnet (testnet) or quantic-mainnet (mainnet), or quantic-regtest for a local test network"
            )),
        }
    }

    /// Los perfiles de Quantic existen en todas las builds para poder
    /// nombrarlos, pero solo arrancan en una con la feature `quantic`.
    fn quantic(profile: Self) -> Result<Self, String> {
        if cfg!(feature = "quantic") {
            Ok(profile)
        } else {
            Err(format!(
                "{} needs a build with the quantic feature (cargo build --features quantic)",
                profile.label()
            ))
        }
    }

    fn is_quantic(self) -> bool {
        matches!(self, Self::QuanticMainnet | Self::QuanticTestnet | Self::QuanticRegtest)
    }

    /// El protocolo que anuncia el nodo; los manifiestos tienen que coincidir.
    fn protocol_id(self) -> &'static str {
        match self {
            Self::Zenith => PROTOCOL_FREEZE_ID,
            Self::QuanticMainnet => zoka::network::p2p::QUANTIC_MAINNET_PROTOCOL_ID,
            Self::QuanticTestnet => zoka::network::p2p::QUANTIC_TESTNET_PROTOCOL_ID,
            Self::QuanticRegtest => zoka::network::p2p::QUANTIC_REGTEST_PROTOCOL_ID,
        }
    }

    /// El puerto P2P por omisión: uno distinto por red, para que la PC pueda
    /// correr Zenith y Quantic a la vez sin `--listen`.
    fn default_p2p_port(self) -> u16 {
        match self {
            Self::Zenith => 28080,
            Self::QuanticMainnet => 28380,
            Self::QuanticTestnet => 28180,
            Self::QuanticRegtest => 28280,
        }
    }

    /// Lo que reemplaza a `zoka` en los nombres de los tópicos.
    fn topic_prefix(self) -> Option<&'static str> {
        match self {
            Self::Zenith => None,
            Self::QuanticMainnet => Some(zoka::network::p2p::QUANTIC_MAINNET_TOPIC_PREFIX),
            Self::QuanticTestnet => Some(zoka::network::p2p::QUANTIC_TESTNET_TOPIC_PREFIX),
            Self::QuanticRegtest => Some(zoka::network::p2p::QUANTIC_REGTEST_TOPIC_PREFIX),
        }
    }

    fn from_chain_id(chain_id: &str) -> Result<Self, String> {
        match chain_id {
            MAINNET_CHAIN_ID => Ok(Self::Zenith),
            economics::QUANTIC_MAINNET_CHAIN_ID => Self::quantic(Self::QuanticMainnet),
            economics::QUANTIC_TESTNET_CHAIN_ID => Self::quantic(Self::QuanticTestnet),
            economics::QUANTIC_REGTEST_CHAIN_ID => Self::quantic(Self::QuanticRegtest),
            TESTNET_CHAIN_ID => Err(format!(
                "chain id '{TESTNET_CHAIN_ID}' was retired: it never ran as a public network. Zenith ({MAINNET_CHAIN_ID}) is the testnet now"
            )),
            other => Err(format!(
                "unknown chain id '{other}' for local wallet storage profile"
            )),
        }
    }

    fn chain_id(self) -> &'static str {
        match self {
            Self::Zenith => MAINNET_CHAIN_ID,
            Self::QuanticMainnet => economics::QUANTIC_MAINNET_CHAIN_ID,
            Self::QuanticTestnet => economics::QUANTIC_TESTNET_CHAIN_ID,
            Self::QuanticRegtest => economics::QUANTIC_REGTEST_CHAIN_ID,
        }
    }

    fn default_db_path(self) -> &'static str {
        match self {
            Self::Zenith => DEFAULT_CHAIN_DB_PATH,
            Self::QuanticMainnet => ".zoka/quantic-mainnet/blockchain.db",
            Self::QuanticTestnet => ".zoka/quantic-testnet/blockchain.db",
            Self::QuanticRegtest => ".zoka/quantic-regtest/blockchain.db",
        }
    }

    fn local_dir(self) -> &'static str {
        match self {
            Self::Zenith => ".zoka/mainnet",
            Self::QuanticMainnet => ".zoka/quantic-mainnet",
            Self::QuanticTestnet => ".zoka/quantic-testnet",
            Self::QuanticRegtest => ".zoka/quantic-regtest",
        }
    }

    fn default_genesis_path(self) -> &'static str {
        match self {
            Self::Zenith => "docs/genesis.mainnet.v1.json",
            Self::QuanticMainnet => "docs/genesis.quantic-mainnet.v1.json",
            Self::QuanticTestnet => "docs/genesis.quantic-testnet.v1.json",
            Self::QuanticRegtest => "docs/genesis.quantic-regtest.v1.json",
        }
    }

    fn default_network_config_path(self) -> &'static str {
        match self {
            Self::Zenith => "docs/mainnet-network.v1.json",
            Self::QuanticMainnet => "docs/quantic-mainnet-network.v1.json",
            Self::QuanticTestnet => "docs/quantic-testnet-network.v1.json",
            Self::QuanticRegtest => "docs/quantic-regtest-network.v1.json",
        }
    }

    fn network_config_schema(self) -> &'static str {
        match self {
            Self::Zenith => "zoka-mainnet-network-v1",
            Self::QuanticMainnet => "zoka-quantic-mainnet-network-v1",
            Self::QuanticTestnet => "zoka-quantic-testnet-network-v1",
            Self::QuanticRegtest => "zoka-quantic-regtest-network-v1",
        }
    }

    fn mining_mode(self) -> &'static str {
        match self {
            Self::Zenith => MAINNET_MINING_MODE,
            Self::QuanticMainnet => QUANTIC_MINING_MODE,
            Self::QuanticTestnet => QUANTIC_MINING_MODE,
            Self::QuanticRegtest => QUANTIC_MINING_MODE,
        }
    }

    /// Identificador interno y estable: va en el JSON (`network`), en la API,
    /// en la marca de genesis de la base y en `--network` al relanzarse.
    /// Para mostrar, `display_name`.
    fn label(self) -> &'static str {
        match self {
            Self::Zenith => "mainnet",
            Self::QuanticMainnet => "quantic-mainnet",
            Self::QuanticTestnet => "quantic-testnet",
            Self::QuanticRegtest => "quantic-regtest",
        }
    }

    fn display_name(self) -> &'static str {
        match self {
            Self::Zenith => economics::ZENITH_DISPLAY_NAME,
            Self::QuanticMainnet => "Zoka Quantic Mainnet",
            Self::QuanticTestnet => "Zoka Quantic Testnet",
            Self::QuanticRegtest => "Zoka Quantic Regtest (local)",
        }
    }

    fn role(self) -> &'static str {
        match self {
            Self::Zenith => economics::ZENITH_NETWORK_ROLE,
            Self::QuanticMainnet => "mainnet",
            Self::QuanticTestnet => "testnet",
            Self::QuanticRegtest => "regtest",
        }
    }
}

fn chain_db_path_for_args(args: &[String]) -> Result<(NetworkProfile, String), String> {
    let network = NetworkProfile::from_args(args)?;
    let db_path =
        option_value(args, "--db").unwrap_or_else(|| network.default_db_path().to_string());
    Ok((network, db_path))
}

/// Algorithm id chosen for the chain ZOKA currently runs against. With
/// `MAINNET_CHAIN_ID = zoka-mainnet-v1` this resolves to RandomX in
/// release builds compiled with `--features randomx`, and to the CPU
/// fallback otherwise; the `engine_for` call is the only place where
/// that choice is enforced at runtime.
fn active_pow_algorithm() -> &'static str {
    economics::pow_algorithm_for_chain(MAINNET_CHAIN_ID)
}

fn active_pow_algorithm_for_chain(chain_id: &str) -> &'static str {
    economics::pow_algorithm_for_chain(chain_id)
}

fn next_block_reward(height: u64, total_supply_emitted: u64, reward_override: Option<u64>) -> u64 {
    if let Some(reward) = reward_override {
        let remaining = EMISSION_MAX_SUPPLY.saturating_sub(total_supply_emitted);
        return reward.min(remaining);
    }
    economics::block_reward_for_height(height, total_supply_emitted)
}

fn genesis_pow_hash() -> String {
    genesis_pow_hash_for_chain(MAINNET_CHAIN_ID)
}

fn genesis_pow_hash_for_chain(chain_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"zoka-mainnet-genesis-pow");
    hasher.update([0x1f]);
    hasher.update(chain_id.as_bytes());
    hex::encode(hasher.finalize())
}

fn pow_engine_for_height_for_chain(
    chain_id: &str,
    height: u64,
) -> Result<Box<dyn zoka::mining::pow::PowEngine>, String> {
    zoka::mining::pow::engine_for(chain_id, height).map_err(|e| match e {
        zoka::mining::pow::PowError::BackendNotAvailable(id) => format!(
            "PoW backend '{id}' is required by chain '{}' but is not compiled into this binary; rebuild with: cargo build --release --features randomx (see docs/protocol-overview.md)",
            chain_id
        ),
        other => format!("pow engine init failed: {other}"),
    })
}

fn pow_engine_pool_for_height_for_chain(
    chain_id: &str,
    height: u64,
    threads: usize,
) -> Result<Vec<Box<dyn zoka::mining::pow::PowEngine>>, String> {
    zoka::mining::pow::engine_pool_for(chain_id, height, threads.max(1)).map_err(|e| {
        match e {
            zoka::mining::pow::PowError::BackendNotAvailable(id) => format!(
                "PoW backend '{id}' is required by chain '{}' but is not compiled into this binary; rebuild with: cargo build --release --features randomx (see docs/protocol-overview.md)",
                chain_id
            ),
            other => format!("pow engine pool init failed: {other}"),
        }
    })
}

fn build_pow_input_for_chain(
    chain_id: &str,
    prev_hash: &str,
    wallet_address: &str,
    height: u64,
    timestamp: u64,
    nonce: u64,
) -> Vec<u8> {
    zoka::mining::pow::build_pow_input(
        chain_id,
        prev_hash,
        wallet_address,
        height,
        timestamp,
        nonce,
    )
}

fn solve_pow_nonce(
    chain_id: &str,
    height: u64,
    difficulty_bits: u32,
    prev_hash: &str,
    wallet_address: &str,
    timestamp: u64,
    threads: usize,
    fallback_hashrate_hps: u64,
    honor_local_mining_control: bool,
    mut progress: impl FnMut(&MiningProgressSnapshot) -> Result<(), String>,
) -> Result<(Vec<u8>, u64, u64, f64), String> {
    let worker_count = threads.max(1);
    let engines = pow_engine_pool_for_height_for_chain(chain_id, height, worker_count)?;
    let started = Instant::now();
    let found = Arc::new(AtomicBool::new(false));
    let hashes_attempted = Arc::new(AtomicU64::new(0));
    let (sender, receiver) = mpsc::channel::<Result<(Vec<u8>, u64), String>>();
    let prev_hash = prev_hash.to_string();
    let wallet_address = wallet_address.to_string();

    thread::scope(|scope| {
        for (lane, engine) in engines.into_iter().enumerate() {
            let sender = sender.clone();
            let found = Arc::clone(&found);
            let hashes_attempted = Arc::clone(&hashes_attempted);
            let prev_hash = prev_hash.clone();
            let wallet_address = wallet_address.clone();
            scope.spawn(move || {
                let stride = worker_count as u64;
                let mut nonce = lane as u64;
                let mut local_hashes = 0u64;
                while !found.load(Ordering::Relaxed) {
                    let input = build_pow_input_for_chain(
                        chain_id,
                        &prev_hash,
                        &wallet_address,
                        height,
                        timestamp,
                        nonce,
                    );
                    let hash = match engine.calculate_hash(&input) {
                        Ok(hash) => hash,
                        Err(err) => {
                            if local_hashes > 0 {
                                hashes_attempted.fetch_add(local_hashes, Ordering::Relaxed);
                            }
                            found.store(true, Ordering::Relaxed);
                            let _ = sender.send(Err(format!("pow hashing failed: {err}")));
                            return;
                        }
                    };
                    local_hashes = local_hashes.saturating_add(1);
                    if local_hashes >= 128 {
                        hashes_attempted.fetch_add(local_hashes, Ordering::Relaxed);
                        local_hashes = 0;
                    }
                    if zoka::mining::pow::hash_meets_difficulty(&hash, difficulty_bits) {
                        if local_hashes > 0 {
                            hashes_attempted.fetch_add(local_hashes, Ordering::Relaxed);
                        }
                        found.store(true, Ordering::Relaxed);
                        let _ = sender.send(Ok((hash, nonce)));
                        return;
                    }
                    nonce = nonce.wrapping_add(stride);
                }
                if local_hashes > 0 {
                    hashes_attempted.fetch_add(local_hashes, Ordering::Relaxed);
                }
            });
        }
        drop(sender);

        loop {
            match receiver.recv_timeout(Duration::from_millis(800)) {
                Ok(result) => {
                    found.store(true, Ordering::Relaxed);
                    return result.map(|(hash, nonce)| {
                        (
                            hash,
                            nonce,
                            hashes_attempted.load(Ordering::Relaxed),
                            started.elapsed().as_secs_f64(),
                        )
                    });
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // Someone else's block already claimed this height while
                    // we were hashing - stop the workers now instead of
                    // grinding minutes on dead work. `found` must be set
                    // BEFORE returning: thread::scope blocks until every
                    // spawned worker exits, and workers only watch `found`.
                    if zoka::api::global_network_runtime().local_chain_height() >= height {
                        found.store(true, Ordering::Relaxed);
                        return Err(format!(
                            "stale parent: chain advanced past height {height} during PoW solve"
                        ));
                    }
                    if zoka::api::global_network_runtime().checkpoint_quarantined() {
                        found.store(true, Ordering::Relaxed);
                        return Err(format!(
                            "mining stopped: checkpoint quarantine activated during PoW solve at height {height}"
                        ));
                    }
                    // The user switched mining off (POST /local/mining/stop)
                    // while this solve was in flight - obey within one tick
                    // so the node stops reporting local_mining and control
                    // UIs (ZSilent's toggle) don't bounce back to ON.
                    if honor_local_mining_control && local_mining_stop_requested() {
                        found.store(true, Ordering::Relaxed);
                        return Err(format!(
                            "mining stopped: local mining control disabled during PoW solve at height {height}"
                        ));
                    }
                    let elapsed_secs = started.elapsed().as_secs_f64();
                    let attempted = hashes_attempted.load(Ordering::Relaxed);
                    let estimated_hashrate_hps = if elapsed_secs > 0.0 {
                        attempted as f64 / elapsed_secs
                    } else {
                        fallback_hashrate_hps as f64
                    };
                    progress(&MiningProgressSnapshot {
                        height,
                        difficulty_bits,
                        nonce: attempted,
                        hashes_attempted: attempted,
                        elapsed_secs,
                        estimated_hashrate_hps,
                    })?;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    found.store(true, Ordering::Relaxed);
                    return Err("pow workers stopped before finding a solution".to_string());
                }
            }
        }
    })
}

fn solve_team_share(
    template: zoka::team_mining::MiningTemplate,
    miner_address: String,
    difficulty_bits: u32,
    threads: usize,
    cancel: Arc<AtomicBool>,
) -> Result<TeamShareSolveSummary, String> {
    if !template.validate_id() || difficulty_bits == 0 {
        return Err("invalid mining-team work assignment".to_string());
    }
    let worker_count = threads.max(1);
    let engines =
        pow_engine_pool_for_height_for_chain(&template.chain_id, template.height, worker_count)?;
    let started = Instant::now();
    let found = Arc::new(AtomicBool::new(false));
    let hashes_attempted = Arc::new(AtomicU64::new(0));
    let (sender, receiver) = mpsc::channel::<Result<(Vec<u8>, u64), String>>();
    let nonce_seed = rand::random::<u64>();

    thread::scope(|scope| {
        for (lane, engine) in engines.into_iter().enumerate() {
            let sender = sender.clone();
            let found = Arc::clone(&found);
            let cancel = Arc::clone(&cancel);
            let hashes_attempted = Arc::clone(&hashes_attempted);
            let template = template.clone();
            let stride = worker_count as u64;
            scope.spawn(move || {
                let mut nonce = nonce_seed.wrapping_add(lane as u64);
                let mut local_hashes = 0u64;
                while !found.load(Ordering::Relaxed) && !cancel.load(Ordering::Relaxed) {
                    let input = build_pow_input_for_chain(
                        &template.chain_id,
                        &template.prev_hash,
                        &template.recipient_address,
                        template.height,
                        template.timestamp,
                        nonce,
                    );
                    let hash = match engine.calculate_hash(&input) {
                        Ok(hash) => hash,
                        Err(error) => {
                            if local_hashes > 0 {
                                hashes_attempted.fetch_add(local_hashes, Ordering::Relaxed);
                            }
                            found.store(true, Ordering::Relaxed);
                            let _ = sender.send(Err(format!("team share hashing failed: {error}")));
                            return;
                        }
                    };
                    local_hashes = local_hashes.saturating_add(1);
                    if local_hashes >= 128 {
                        hashes_attempted.fetch_add(local_hashes, Ordering::Relaxed);
                        local_hashes = 0;
                    }
                    if zoka::mining::pow::hash_meets_difficulty(&hash, difficulty_bits) {
                        if local_hashes > 0 {
                            hashes_attempted.fetch_add(local_hashes, Ordering::Relaxed);
                        }
                        found.store(true, Ordering::Relaxed);
                        let _ = sender.send(Ok((hash, nonce)));
                        return;
                    }
                    nonce = nonce.wrapping_add(stride);
                }
                if local_hashes > 0 {
                    hashes_attempted.fetch_add(local_hashes, Ordering::Relaxed);
                }
            });
        }
        drop(sender);

        loop {
            match receiver.recv_timeout(Duration::from_millis(500)) {
                Ok(result) => {
                    found.store(true, Ordering::Relaxed);
                    return result.map(|(hash, nonce)| {
                        let elapsed_secs = started.elapsed().as_secs_f64();
                        TeamShareSolveSummary {
                            share: zoka::team_mining::SubmittedShare {
                                team_id: template.team_id.clone(),
                                template_id: template.id.clone(),
                                miner_address,
                                nonce,
                                pow_hash_hex: hex::encode(hash),
                                difficulty_bits,
                                timestamp: now_secs(),
                            },
                            hashes_attempted: hashes_attempted.load(Ordering::Relaxed),
                            elapsed_secs,
                        }
                    });
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if cancel.load(Ordering::Relaxed) {
                        found.store(true, Ordering::Relaxed);
                        return Err("team share work cancelled".to_string());
                    }
                    if zoka::api::global_network_runtime().local_chain_height() >= template.height {
                        found.store(true, Ordering::Relaxed);
                        return Err(format!(
                            "stale team template: chain advanced past height {}",
                            template.height
                        ));
                    }
                    if zoka::api::global_network_runtime().checkpoint_quarantined() {
                        found.store(true, Ordering::Relaxed);
                        return Err("team share work stopped by checkpoint quarantine".to_string());
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    found.store(true, Ordering::Relaxed);
                    return Err("team share workers stopped before finding a share".to_string());
                }
            }
        }
    })
}

fn verify_team_solution_for_candidate(
    solution: &zoka::team_mining::NetworkSolution,
    chain_id: &str,
    height: u64,
    prev_hash: &str,
    expected_difficulty_bits: u32,
) -> Result<Vec<u8>, String> {
    let template = &solution.template;
    if !template.validate_id()
        || template.chain_id != chain_id
        || template.height != height
        || template.prev_hash != prev_hash
        || template.network_difficulty_bits != expected_difficulty_bits
        || solution.share.team_id != template.team_id
        || solution.share.template_id != template.id
        || solution.share.miner_address.trim().is_empty()
    {
        return Err(
            "stale or inconsistent mining-team solution for canonical chain head".to_string(),
        );
    }
    let claimed_hash = hex::decode(&solution.share.pow_hash_hex)
        .map_err(|_| "mining-team solution contains invalid PoW hex".to_string())?;
    let input = build_pow_input_for_chain(
        chain_id,
        prev_hash,
        &template.recipient_address,
        height,
        template.timestamp,
        solution.share.nonce,
    );
    let engine = pow_engine_for_height_for_chain(chain_id, height)?;
    let recomputed = engine
        .calculate_hash(&input)
        .map_err(|error| format!("team solution PoW recomputation failed: {error}"))?;
    if recomputed != claimed_hash
        || !zoka::mining::pow::hash_meets_difficulty(&recomputed, expected_difficulty_bits)
    {
        return Err("mining-team solution does not satisfy its canonical template".to_string());
    }
    Ok(recomputed)
}

fn benchmark_hashrate_hps_for_chain(chain_id: &str, duration_ms: u64) -> u64 {
    let bench_for = Duration::from_millis(duration_ms.clamp(250, 2_000));
    let Ok(vm) = pow_engine_for_height_for_chain(chain_id, 0) else {
        return 0;
    };
    let started = Instant::now();
    let mut nonce = 0u64;
    let mut hashes = 0u64;
    while started.elapsed() < bench_for {
        let payload = build_pow_input_for_chain(
            chain_id,
            &genesis_pow_hash_for_chain(chain_id),
            "benchmark",
            0,
            now_secs(),
            nonce,
        );
        let _ = vm.calculate_hash(&payload);
        nonce = nonce.wrapping_add(1);
        hashes = hashes.saturating_add(1);
    }
    let elapsed = started.elapsed().as_secs_f64();
    if elapsed <= 0.0 {
        0
    } else {
        (hashes as f64 / elapsed) as u64
    }
}

fn retarget_difficulty(current: u32, solve_secs: f64, target_block_time_secs: u64) -> u32 {
    let target = target_block_time_secs.max(1) as f64;
    let current = current.clamp(POW_MIN_DIFFICULTY_BITS, POW_MAX_DIFFICULTY_BITS);
    if solve_secs < target * 0.5 {
        current.saturating_add(1).min(POW_MAX_DIFFICULTY_BITS)
    } else if solve_secs > target * 2.0 {
        current.saturating_sub(1).max(POW_MIN_DIFFICULTY_BITS)
    } else {
        current
    }
}

/// Techo del diagnóstico de minería, antes de rotarlo.
///
/// `mining_progress_log` escribe cada 3 segundos mientras se mina: unas 29 000
/// líneas por día, y el archivo sólo crecía. En la PC del usuario llegó a 44 MB.
/// Con la rotación el gasto queda acotado a dos veces este número, y lo que se
/// conserva son las líneas recientes, que son las que sirven para diagnosticar.
const MINING_LOG_MAX_BYTES: u64 = 4 * 1024 * 1024;

fn mining_log(message: &str) {
    eprintln!("{}", paint(&format!("mining: {message}"), ANSI_DIM));
    // En la carpeta de la red de este proceso. Iba siempre a la de Zenith,
    // también desde un nodo de Quantic.
    let network = zoka::api::global_startup_status()
        .chain_id()
        .and_then(|chain_id| NetworkProfile::from_chain_id(&chain_id).ok())
        .unwrap_or(NetworkProfile::Zenith);
    let path = PathBuf::from(network.local_dir()).join("mining_diagnostic.log");
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    rotate_mining_log_if_oversized(&path);
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        use std::io::Write as _;
        let _ = writeln!(f, "{} mining: {}", now_secs(), message);
    }
}

/// Rota el diagnóstico a una única copia `.1` cuando pasa el techo.
///
/// Una sola copia a propósito: esto sirve para mirar qué pasó recién, no para
/// llevar historia. Si el rename falla —otro proceso lo tiene abierto— se deja
/// como está y se reintenta en la línea siguiente; perder una rotación es mucho
/// más barato que perder la línea que se estaba por escribir.
fn rotate_mining_log_if_oversized(path: &Path) {
    let oversized = fs::metadata(path)
        .map(|meta| meta.len() >= MINING_LOG_MAX_BYTES)
        .unwrap_or(false);
    if !oversized {
        return;
    }
    let _ = fs::rename(path, path.with_extension("log.1"));
}

fn mining_progress_log(progress: &MiningProgressSnapshot) {
    mining_log(&format!(
        "progress height={} difficulty_bits={} hashes={} nonce={} elapsed={:.1}s hashrate={}",
        progress.height,
        progress.difficulty_bits,
        progress.hashes_attempted,
        progress.nonce,
        progress.elapsed_secs,
        format_hashrate_hps(progress.estimated_hashrate_hps)
    ));
}

/// La línea de estado de la consola en Zoka Quantic: el minero es el de
/// `quantic::runtime`. La wallet de Zenith no existe ahí, y el nodo solo tiene
/// la dirección de `--quantic-mine-to`, no sus claves: no hay saldo que
/// mostrar. Con los números de Zenith decía `mining=off` y `0 block(s)`
/// mientras minaba, con un error de dirección `zka1` en rojo.
#[cfg(feature = "quantic")]
fn print_quantic_console_status(height: u64, peers: usize, last_event: &str) {
    let status = zoka::quantic::runtime::mining_status();
    // Una dirección de Quantic tiene unos 1.700 caracteres.
    let address = status.address.as_deref().unwrap_or("—");
    let shown = match (address.get(..14), address.get(address.len().saturating_sub(8)..)) {
        (Some(head), Some(tail)) if address.len() > 24 => format!("{head}…{tail}"),
        _ => address.to_string(),
    };
    print_mining_console_status(
        &shown,
        height,
        peers,
        status.configured && status.enabled,
        status.hashrate_hps,
        None,
        None,
        status.blocks_mined,
        status.rewards_atoms,
        last_event,
    );
}

/// Sin la feature `quantic` el nodo no sigue cadenas de Quantic.
#[cfg(not(feature = "quantic"))]
fn print_quantic_console_status(height: u64, peers: usize, last_event: &str) {
    print_mining_console_status("—", height, peers, false, 0, None, None, 0, 0, last_event);
}

/// One compact, colored, scrolling status line for the mining console. No
/// screen clear: it appends to the terminal like a log so it coexists with
/// the streaming `mining: progress …` lines instead of wiping them.
fn print_mining_console_status(
    wallet_address: &str,
    height: u64,
    peers: usize,
    mining: bool,
    hashrate_hps: u64,
    wallet_balance_atoms: Option<u64>,
    wallet_balance_error: Option<&str>,
    session_blocks: u64,
    session_rewards: u64,
    last_event: &str,
) {
    let peers_color = if peers >= 2 {
        ANSI_GREEN
    } else if peers == 1 {
        ANSI_YELLOW
    } else {
        ANSI_RED
    };
    let mining_label = if mining {
        paint("on", ANSI_GREEN)
    } else {
        paint("off", ANSI_YELLOW)
    };
    let balance_label = match (wallet_balance_atoms, wallet_balance_error) {
        (Some(atoms), _) => paint(&format_zka_amount(atoms), ANSI_CYAN),
        (None, Some(err)) => paint(
            &format!("— ({})", truncate_dashboard_text(err, 60)),
            ANSI_RED,
        ),
        (None, None) => paint("—", ANSI_DIM),
    };
    println!(
        "{}  wallet={}  height={}  peers={}  mining={mining_label}  hashrate={}  balance={balance_label}  session={} block(s) / {}  {} {}",
        paint("status", ANSI_DIM),
        paint(wallet_address, ANSI_CYAN),
        paint(&height.to_string(), ANSI_BOLD),
        paint(&peers.to_string(), peers_color),
        paint(&format_hashrate_hps(hashrate_hps as f64), ANSI_MAGENTA),
        paint(&session_blocks.to_string(), ANSI_GREEN),
        paint(&format_zka_amount(session_rewards), ANSI_GREEN),
        paint("|", ANSI_DIM),
        paint(&truncate_dashboard_text(last_event, 80), ANSI_DIM),
    );
}

/// A one-time, prominent, hard-to-miss line printed the moment this session
/// wins a block — separate from the routine status line so the actual
/// "you got paid" event doesn't blend into the scrollback.
fn print_block_found_banner(height: u64, reward_atoms: u64) {
    println!(
        "{ANSI_BOLD}{ANSI_GREEN}*** Block found! height={height}  reward={} ***{ANSI_RESET}",
        format_zka_amount(reward_atoms)
    );
}

fn user_mining_state_path_for_network(network: NetworkProfile) -> Result<PathBuf, String> {
    let dir = PathBuf::from(network.local_dir());
    fs::create_dir_all(&dir).map_err(|e| format!("failed to create {}: {e}", dir.display()))?;
    Ok(dir.join("user_mining_state.json"))
}

fn load_user_mining_state_for_network(network: NetworkProfile) -> Result<UserMiningState, String> {
    let path = user_mining_state_path_for_network(network)?;
    if !path.exists() {
        return Ok(UserMiningState::default());
    }
    let raw = fs::read_to_string(&path)
        .map_err(|e| format!("failed to read mining state {}: {e}", path.display()))?;
    // The mining-state file is a local runtime cache (mining stats/progress),
    // not consensus data. A partial or interrupted write (a Ctrl+C mid-save, or
    // the machine losing power) leaves it empty, truncated or zero-filled, and
    // that must not stop the node from starting — reset to default.
    //
    // Deliberately silent. This binary's output is consumed by ZSilent, which
    // runs it with stderr merged into stdout and parses the result as JSON, so
    // a diagnostic line here does not warn anybody: it turns a condition the
    // node already recovers from into a wallet that refuses to unlock, with an
    // error naming this file. That is exactly what happened on 2026-07-29.
    let (mut state, recovered) = match serde_json::from_str::<UserMiningState>(&raw) {
        Ok(state) => (state, false),
        Err(_) => (UserMiningState::default(), true),
    };
    if state.total_supply_emitted == 0 && state.total_rewards > 0 {
        state.total_supply_emitted = state.total_rewards.min(EMISSION_MAX_SUPPLY);
    }
    if state.mode.is_empty() {
        state.mode = network.mining_mode().to_string();
    }
    if state.algorithm.is_empty() {
        state.algorithm = if state.mode == "pow-sim" {
            "sha256-sim".to_string()
        } else {
            active_pow_algorithm_for_chain(network.chain_id()).to_string()
        };
    }
    if state.last_block_hash.is_empty() {
        state.last_block_hash = genesis_pow_hash_for_chain(network.chain_id());
    }
    if recovered {
        // Reparar también en disco, no sólo en memoria: un archivo ilegible que
        // se deja donde está vuelve a fallar en cada comando siguiente.
        let _ = save_user_mining_state_for_network(network, &state);
    }
    Ok(state)
}

#[cfg(windows)]
fn process_id_alive(pid: u32) -> bool {
    let filter = format!("PID eq {pid}");
    let Ok(output) = std::process::Command::new("tasklist")
        .args(["/FI", &filter, "/FO", "CSV", "/NH"])
        .output()
    else {
        return true;
    };
    if !output.status.success() {
        return true;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.contains(&format!(",\"{pid}\",")) || stdout.contains(&format!("\"{pid}\""))
}

#[cfg(not(windows))]
fn process_id_alive(_pid: u32) -> bool {
    true
}

fn mining_process_is_live(state: &UserMiningState) -> bool {
    if !state.is_mining {
        return false;
    }
    match state.mining_process_id {
        Some(pid) => process_id_alive(pid),
        None => now_secs().saturating_sub(state.last_updated) <= MINING_STALE_AFTER_SECS,
    }
}

fn load_user_mining_state_for_display() -> Result<UserMiningState, String> {
    load_user_mining_state_for_display_for_network(NetworkProfile::Zenith)
}

fn load_user_mining_state_for_display_for_network(
    network: NetworkProfile,
) -> Result<UserMiningState, String> {
    let mut state = load_user_mining_state_for_network(network)?;
    if state.is_mining && !mining_process_is_live(&state) {
        state.is_mining = false;
        state.estimated_hashrate_hps = 0;
        state.mining_process_id = None;
        state.last_updated = now_secs();
        save_user_mining_state_for_network(network, &state)?;
    }
    Ok(state)
}

fn save_user_mining_state_for_network(
    network: NetworkProfile,
    state: &UserMiningState,
) -> Result<(), String> {
    let path = user_mining_state_path_for_network(network)?;
    // Compacto, no `to_string_pretty`. Este archivo no lo lee nadie a mano, y lo
    // que pesa son los range proofs de las recompensas sin madurar: bytes
    // sueltos que en JSON indentado ocupan una línea cada uno. Con pretty el
    // archivo llegó a 737 KB para ~60 entradas. Achicarlo además angosta la
    // ventana en la que un corte lo puede dejar a medio escribir.
    let raw = serde_json::to_string(state)
        .map_err(|e| format!("failed to encode mining state json: {e}"))?;
    write_file_atomically(&path, raw.as_bytes())
}

/// Escribe un archivo pequeño de forma atómica y durable.
///
/// `fs::write` trunca el destino y después escribe encima. Si la máquina se
/// corta en el medio, NTFS deja el archivo con su largo final pero sin datos:
/// una tira de ceros que ya no parsea, y que no se recupera sola. Pasó de
/// verdad — un reinicio dejó `user_mining_state.json` con 737 KB de NUL.
///
/// El `sync_all()` antes del rename no es decorativo: sin él el rename puede
/// llegar al disco antes que el contenido, que es justo el caso que rompe.
fn write_file_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    {
        let mut file = fs::File::create(&tmp)
            .map_err(|e| format!("failed to create {}: {e}", tmp.display()))?;
        file.write_all(bytes)
            .map_err(|e| format!("failed to write {}: {e}", tmp.display()))?;
        file.sync_all()
            .map_err(|e| format!("failed to flush {}: {e}", tmp.display()))?;
    }
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("failed to replace {}: {e}", path.display())
    })
}

fn legacy_pow_sim_state_detected(state: &UserMiningState) -> bool {
    state.mode == "pow-sim" || state.algorithm == "sha256-sim"
}

fn legacy_pow_sim_reset_message() -> String {
    "legacy pow-sim state detected; reset the active .zoka/<network>/user_mining_state.json, private_wallets and local chain DB before starting PoW".to_string()
}

fn fee_snapshot_db_path(db_path: Option<&str>) -> &str {
    db_path.unwrap_or(DEFAULT_CHAIN_DB_PATH)
}

fn network_pending_private_txs(db_path: Option<&str>) -> u64 {
    let storage = match RocksDbStorage::new(fee_snapshot_db_path(db_path)) {
        Ok(s) => s,
        Err(_) => return 0,
    };
    storage.pending_private_transaction_count().unwrap_or(0)
}

fn local_pending_private_tx_hashes(db_path: &str, limit: usize) -> Result<Vec<String>, String> {
    local_pending_private_tx_hashes_at(db_path, limit, None)
}

fn local_pending_private_tx_hashes_at(
    db_path: &str,
    limit: usize,
    target_block_height: Option<u64>,
) -> Result<Vec<String>, String> {
    Ok(
        local_pending_private_tx_selection_diagnostics_at(db_path, limit, target_block_height)?
            .selected,
    )
}

fn local_pending_private_tx_selection_diagnostics_at(
    db_path: &str,
    limit: usize,
    target_block_height: Option<u64>,
) -> Result<PendingPrivateTxSelectionDiagnostics, String> {
    let storage = match open_storage(db_path) {
        Ok(storage) => storage,
        // Sidecar files are only candidates after they have been imported,
        // decoded, and validated. Mining file names directly could commit a
        // tx hash without its spent-nullifier set while RocksDB is locked.
        Err(e) if is_storage_lock_error(&e) => {
            return Ok(PendingPrivateTxSelectionDiagnostics {
                target_block_height: target_block_height.unwrap_or(0),
                limit,
                total_candidates: 0,
                selected: Vec::new(),
                candidates: Vec::new(),
                storage_locked: true,
            })
        }
        Err(e) => return Err(e),
    };
    let hashes = storage
        .iterate_pending_private_transaction_hashes()
        .map_err(|e| format!("failed to read pending private txs: {e}"))?;
    let target_block_height = target_block_height
        .unwrap_or_else(|| storage.get_chain_height().unwrap_or(0).saturating_add(1));
    Ok(diagnose_pending_private_tx_selection(
        &storage,
        hashes,
        limit,
        target_block_height,
    ))
}

fn private_tx_spend_tags(tx: &zoka::private_wallet::PrivateTransaction) -> Vec<String> {
    let mut tags = Vec::new();
    let primary = tx.nullifier.trim();
    if !primary.is_empty() {
        tags.push(primary.to_string());
    }
    for tag in &tx.input_nullifiers {
        let tag = tag.trim();
        if !tag.is_empty() {
            tags.push(tag.to_string());
        }
    }
    tags.sort();
    tags.dedup();
    tags
}

fn validate_private_tx_for_pending_pool(tx: &PrivateTransaction) -> Result<(), String> {
    let mut validator = PrivateWalletManager::new();
    validator
        .validate_inbound_private_transaction(tx)
        .map_err(|e| format!("private transaction validation failed: {e}"))
}

fn valid_private_tx_hash_format(hash: &str) -> bool {
    !hash.is_empty()
        && hash.len() <= 128
        && hash
            .chars()
            .all(|c| c.is_ascii_hexdigit() || matches!(c, '-' | '_'))
}

fn archive_committed_private_tx_body_json(
    storage: &RocksDbStorage,
    requested_hash: &str,
    value: &serde_json::Value,
) -> Result<(), String> {
    let requested_hash = requested_hash.trim();
    if !valid_private_tx_hash_format(requested_hash) {
        return Err("private transaction hash format is invalid".to_string());
    }
    let tx: PrivateTransaction = serde_json::from_value(value.clone())
        .map_err(|e| format!("failed to parse private tx body: {e}"))?;
    if tx.tx_hash.trim() != requested_hash {
        return Err(format!(
            "private transaction body hash mismatch: requested {requested_hash}, got {}",
            tx.tx_hash
        ));
    }
    validate_private_tx_for_pending_pool(&tx)?;
    let body =
        serde_json::to_vec(&tx).map_err(|e| format!("failed to serialize private tx: {e}"))?;
    storage
        .save_private_transaction_bytes(requested_hash, &body)
        .map_err(|e| format!("failed to archive private tx: {e}"))
}

fn validate_private_tx_for_block_candidate(
    tx: &PrivateTransaction,
    target_block_height: u64,
) -> Result<(), String> {
    validate_private_tx_for_block_candidate_at(
        tx,
        target_block_height,
        economics::private_tx_v2_i1_activation_height(),
        economics::private_tx_canonical_bundle_activation_height(),
    )
}

fn validate_private_tx_for_block_candidate_at(
    tx: &PrivateTransaction,
    target_block_height: u64,
    v2_i1_activation_height: Option<u64>,
    canonical_bundle_activation_height: Option<u64>,
) -> Result<(), String> {
    let mut validator = PrivateWalletManager::new();
    validator
        .validate_inbound_private_transaction_at(
            tx,
            target_block_height,
            v2_i1_activation_height,
            canonical_bundle_activation_height,
        )
        .map_err(|e| format!("private transaction validation failed: {e}"))
}

#[derive(Debug, Clone, Serialize)]
struct PendingPrivateTxCandidateDiagnostic {
    hash: String,
    selected: bool,
    reason: String,
    tag_count: usize,
}

#[derive(Debug, Clone, Serialize)]
struct PendingPrivateTxSelectionDiagnostics {
    target_block_height: u64,
    limit: usize,
    total_candidates: usize,
    selected: Vec<String>,
    candidates: Vec<PendingPrivateTxCandidateDiagnostic>,
    storage_locked: bool,
}

fn diagnose_pending_private_tx_selection(
    storage: &RocksDbStorage,
    hashes: Vec<String>,
    limit: usize,
    target_block_height: u64,
) -> PendingPrivateTxSelectionDiagnostics {
    let total_candidates = hashes.len();
    let mut selected = Vec::new();
    let mut seen_spend_tags = HashSet::new();
    let mut candidates = Vec::new();

    for hash in hashes {
        if selected.len() >= limit {
            candidates.push(PendingPrivateTxCandidateDiagnostic {
                hash,
                selected: false,
                reason: "limit reached".to_string(),
                tag_count: 0,
            });
            continue;
        }

        let Some(body) = storage.get_private_transaction_bytes(&hash).ok().flatten() else {
            candidates.push(PendingPrivateTxCandidateDiagnostic {
                hash,
                selected: false,
                reason: "missing private tx body".to_string(),
                tag_count: 0,
            });
            continue;
        };

        let tx = match serde_json::from_slice::<zoka::private_wallet::PrivateTransaction>(&body) {
            Ok(tx) => tx,
            Err(err) => {
                candidates.push(PendingPrivateTxCandidateDiagnostic {
                    hash,
                    selected: false,
                    reason: format!("invalid private tx body: {err}"),
                    tag_count: 0,
                });
                continue;
            }
        };

        if tx.tx_hash.trim() != hash.trim() {
            candidates.push(PendingPrivateTxCandidateDiagnostic {
                hash,
                selected: false,
                reason: format!("body hash mismatch: {}", tx.tx_hash),
                tag_count: 0,
            });
            continue;
        }

        if let Err(err) = validate_private_tx_for_block_candidate(&tx, target_block_height) {
            candidates.push(PendingPrivateTxCandidateDiagnostic {
                hash,
                selected: false,
                reason: err,
                tag_count: 0,
            });
            continue;
        }

        let tags = private_tx_spend_tags(&tx);
        if tags.is_empty() {
            candidates.push(PendingPrivateTxCandidateDiagnostic {
                hash,
                selected: false,
                reason: "no spend/nullifier tags".to_string(),
                tag_count: 0,
            });
            continue;
        }

        let tag_count = tags.len();
        if tags.iter().any(|tag| seen_spend_tags.contains(tag)) {
            candidates.push(PendingPrivateTxCandidateDiagnostic {
                hash,
                selected: false,
                reason: "conflicts with earlier selected pending private tx".to_string(),
                tag_count,
            });
            continue;
        }

        if tags
            .iter()
            .any(|tag| storage.is_nullifier_spent(tag).unwrap_or(false))
        {
            candidates.push(PendingPrivateTxCandidateDiagnostic {
                hash,
                selected: false,
                reason: "nullifier already spent on-chain".to_string(),
                tag_count,
            });
            continue;
        }

        seen_spend_tags.extend(tags);
        selected.push(hash.clone());
        candidates.push(PendingPrivateTxCandidateDiagnostic {
            hash,
            selected: true,
            reason: "selected".to_string(),
            tag_count,
        });
    }

    PendingPrivateTxSelectionDiagnostics {
        target_block_height,
        limit,
        total_candidates,
        selected,
        candidates,
        storage_locked: false,
    }
}

fn summarize_pending_private_tx_selection(
    diagnostics: &PendingPrivateTxSelectionDiagnostics,
) -> String {
    if diagnostics.storage_locked {
        return "storage locked while reading pending private txs".to_string();
    }
    if diagnostics.total_candidates == 0 {
        return "no pending private tx candidates".to_string();
    }
    let mut reason_counts = HashMap::<String, usize>::new();
    for candidate in &diagnostics.candidates {
        if candidate.selected {
            continue;
        }
        *reason_counts.entry(candidate.reason.clone()).or_default() += 1;
    }
    let mut reasons: Vec<String> = reason_counts
        .into_iter()
        .map(|(reason, count)| format!("{count}x {reason}"))
        .collect();
    reasons.sort();
    format!(
        "height={} candidates={} selected={} skipped=[{}]",
        diagnostics.target_block_height,
        diagnostics.total_candidates,
        diagnostics.selected.len(),
        reasons.join("; ")
    )
}

#[cfg(test)]
fn select_non_conflicting_pending_private_tx_hashes(
    storage: &RocksDbStorage,
    hashes: Vec<String>,
    limit: usize,
    target_block_height: u64,
) -> Vec<String> {
    diagnose_pending_private_tx_selection(storage, hashes, limit, target_block_height).selected
}

#[cfg(test)]
#[allow(dead_code, unreachable_code)]
fn select_non_conflicting_pending_private_tx_hashes_legacy(
    storage: &RocksDbStorage,
    hashes: Vec<String>,
    limit: usize,
    target_block_height: u64,
) -> Vec<String> {
    return diagnose_pending_private_tx_selection(
        storage,
        hashes.clone(),
        limit,
        target_block_height,
    )
    .selected;

    let mut selected = Vec::new();
    let mut seen_spend_tags = HashSet::new();

    for hash in hashes {
        if selected.len() >= limit {
            break;
        }
        let Some(body) = storage.get_private_transaction_bytes(&hash).ok().flatten() else {
            continue;
        };
        let Ok(tx) = serde_json::from_slice::<zoka::private_wallet::PrivateTransaction>(&body)
        else {
            continue;
        };
        if validate_private_tx_for_block_candidate(&tx, target_block_height).is_err() {
            continue;
        }
        let tags = private_tx_spend_tags(&tx);
        if tags.is_empty() || tags.iter().any(|tag| seen_spend_tags.contains(tag)) {
            continue;
        }
        // Skip any tx whose nullifier is already committed on-chain — it is
        // a double-spend and `apply_block_inner` would reject the block.
        if tags
            .iter()
            .any(|tag| storage.is_nullifier_spent(tag).unwrap_or(false))
        {
            continue;
        }
        seen_spend_tags.extend(tags);
        selected.push(hash);
    }

    selected
}

/// Return the sidecar queue directory for a given chain DB path.
/// When the DB is locked by the miner another process writes pending private
/// transactions here so the miner can import them between blocks.
fn private_tx_sidecar_dir(db_path: &str) -> std::path::PathBuf {
    let base = std::path::Path::new(db_path)
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    base.join("pending_private_queue")
}

#[derive(Debug, Clone, Default)]
struct SidecarPrivateTxImportSummary {
    imported: usize,
    removed_confirmed: usize,
    skipped_invalid: usize,
    skipped_io: usize,
}

impl SidecarPrivateTxImportSummary {
    fn total_activity(&self) -> usize {
        self.imported.saturating_add(self.removed_confirmed)
    }
}

fn list_sidecar_private_tx_hashes(db_path: &str) -> Vec<String> {
    let sidecar = private_tx_sidecar_dir(db_path);
    let entries = match std::fs::read_dir(&sidecar) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };
    let mut hashes: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            (path.extension().and_then(|s| s.to_str()) == Some("privtx"))
                .then(|| {
                    path.file_stem()
                        .and_then(|s| s.to_str())
                        .map(|s| s.to_string())
                })
                .flatten()
        })
        .collect();
    hashes.sort();
    hashes.dedup();
    hashes
}

fn private_tx_hash_is_confirmed(storage: &RocksDbStorage, hash: &str) -> bool {
    let hash = hash.trim();
    if hash.is_empty() {
        return false;
    }
    let tip = storage.get_chain_height().unwrap_or(0);
    for height in 1..=tip {
        let Ok(Some(block)) = storage.get_block_by_height(height) else {
            continue;
        };
        if block
            .private_tx_hashes
            .iter()
            .any(|candidate| candidate == hash)
        {
            return true;
        }
    }
    false
}

fn private_tx_has_spent_onchain_tag(storage: &RocksDbStorage, tx: &PrivateTransaction) -> bool {
    private_tx_spend_tags(tx)
        .iter()
        .any(|tag| storage.is_nullifier_spent(tag).unwrap_or(false))
}

fn persist_local_private_tx_for_node(
    db_path: &str,
    tx_hash: &str,
    tx: &zoka::private_wallet::PrivateTransaction,
) -> Result<(), String> {
    validate_private_tx_for_pending_pool(tx)?;
    let body = serde_json::to_vec(tx)
        .map_err(|e| format!("failed to serialize private transaction: {e}"))?;

    // Try to write directly to the chain DB.  If the miner holds the LOCK
    // (different process) fall back to the sidecar queue directory so the
    // send always succeeds.  The miner imports sidecar files at the
    // beginning of every block.
    match open_storage(db_path) {
        Ok(storage) => {
            storage
                .save_private_transaction_bytes(tx_hash, &body)
                .map_err(|e| format!("failed to persist private tx body: {e}"))?;
            storage
                .save_pending_private_transaction_hash(tx_hash)
                .map_err(|e| format!("failed to mark private tx pending: {e}"))?;
        }
        Err(_) => {
            // DB is locked — write to sidecar directory instead.
            let sidecar = private_tx_sidecar_dir(db_path);
            std::fs::create_dir_all(&sidecar)
                .map_err(|e| format!("failed to create sidecar queue dir: {e}"))?;
            let file = sidecar.join(format!("{tx_hash}.privtx"));
            std::fs::write(&file, &body)
                .map_err(|e| format!("failed to write pending tx to sidecar: {e}"))?;
        }
    }
    Ok(())
}

/// Import any private transactions sitting in the sidecar queue into the
/// chain DB and return the count of newly imported entries.
fn import_sidecar_private_txs(db_path: &str, storage: &RocksDbStorage) -> usize {
    import_sidecar_private_txs_with_summary(db_path, storage).imported
}

fn import_sidecar_private_txs_with_summary(
    db_path: &str,
    storage: &RocksDbStorage,
) -> SidecarPrivateTxImportSummary {
    let sidecar = private_tx_sidecar_dir(db_path);
    if !sidecar.exists() {
        return SidecarPrivateTxImportSummary::default();
    }
    let entries = match std::fs::read_dir(&sidecar) {
        Ok(e) => e,
        Err(_) => return SidecarPrivateTxImportSummary::default(),
    };
    let mut summary = SidecarPrivateTxImportSummary::default();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("privtx") {
            continue;
        }
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(_) => {
                summary.skipped_io = summary.skipped_io.saturating_add(1);
                continue;
            }
        };
        let tx_hash = match path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
        {
            Some(h) => h,
            None => {
                summary.skipped_invalid = summary.skipped_invalid.saturating_add(1);
                continue;
            }
        };
        let tx = match serde_json::from_slice::<PrivateTransaction>(&bytes) {
            Ok(tx) if tx.tx_hash == tx_hash => tx,
            _ => {
                summary.skipped_invalid = summary.skipped_invalid.saturating_add(1);
                continue;
            }
        };
        if validate_private_tx_for_pending_pool(&tx).is_err() {
            summary.skipped_invalid = summary.skipped_invalid.saturating_add(1);
            continue;
        }
        if private_tx_hash_is_confirmed(storage, &tx_hash)
            || private_tx_has_spent_onchain_tag(storage, &tx)
        {
            // Dos motivos muy distintos caen aca: que la transaccion ya entro
            // en la cadena, o que otra se llevo antes la nota que gasta. El
            // primero es el final feliz; el segundo es un gasto doble que
            // ningun minero podra incluir jamas.
            //
            // Se guarda el cuerpo antes de borrar la cola. Sin eso, la unica
            // copia desaparecia y `/private/tx/<hash>` respondia 404: quien
            // envio se quedaba mirando "pendiente" sin forma de averiguar que
            // su transaccion estaba muerta. Guardarlo cuesta unos kilobytes y
            // permite que el nodo explique que paso.
            if !private_tx_hash_is_confirmed(storage, &tx_hash) {
                let _ = storage.save_private_transaction_bytes(&tx_hash, &bytes);
                tracing::warn!(
                    "descartando la transaccion privada {tx_hash} de la cola: la nota que \
                     gasta ya fue gastada por otra transaccion. No la va a incluir ningun \
                     minero -- la wallet que la creo esta ofreciendo una nota que ya no \
                     tiene, y le conviene reescanear."
                );
            }
            let _ = storage.delete_pending_private_transaction_hash(&tx_hash);
            if std::fs::remove_file(&path).is_ok() {
                summary.removed_confirmed = summary.removed_confirmed.saturating_add(1);
            }
            continue;
        }
        if storage
            .save_private_transaction_bytes(&tx_hash, &bytes)
            .is_ok()
            && storage
                .save_pending_private_transaction_hash(&tx_hash)
                .is_ok()
        {
            let _ = std::fs::remove_file(&path);
            summary.imported = summary.imported.saturating_add(1);
        }
    }
    summary
}

/// Cada cuánto se reintenta publicar una transacción privada sin minar.
///
/// 30 s es holgado frente al bloque objetivo de 120 s: da varias oportunidades
/// antes de que a nadie le importe ya, y no satura un enlace onion.
const PRIVATE_TX_REGOSSIP_INTERVAL_SECS: u64 = 30;

/// Cuántas se reenvían por ronda. Acota el gasto cuando la cola es larga.
const PRIVATE_TX_REGOSSIP_BATCH: usize = 8;

/// Cuerpos de transacciones privadas que siguen pendientes y toca reintentar.
///
/// Una transacción privada se publicaba **una sola vez**, al crearla. Si ese
/// único intento se perdía —Tor en un mal momento, un peer que todavía no se
/// suscribió al tópico— quedaba viva en el pool local y muerta para el resto de
/// la red: el emisor la veía "pending" para siempre, ningún minero la incluía, y
/// al vencer la reserva el dinero volvía a su cuenta sin explicación.
///
/// Pasó el 2026-07-30 con 70 ZKA entre dos nodos conectados sólo por onion. La
/// red de seguridad que existía —tirar de RPC públicos— no ayuda: en esta red
/// `public_rpc` está vacío.
///
/// Se leen del pool pendiente y no del sidecar: el sidecar se borra en cuanto
/// se importa, así que no sobrevive para un segundo intento.
fn pending_private_tx_bodies_to_regossip(
    storage: &RocksDbStorage,
    limit: usize,
) -> Vec<(String, Vec<u8>)> {
    let Ok(hashes) = storage.iterate_pending_private_transaction_hashes() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for hash in hashes {
        if out.len() >= limit {
            break;
        }
        if let Ok(Some(bytes)) = storage.get_private_transaction_bytes(&hash) {
            out.push((hash, bytes));
        }
    }
    out
}

fn prune_confirmed_pending_private_txs(storage: &RocksDbStorage, db_path: Option<&str>) -> usize {
    let hashes = match storage.iterate_pending_private_transaction_hashes() {
        Ok(hashes) => hashes,
        Err(_) => return 0,
    };
    let mut pruned = 0usize;
    for hash in hashes {
        let mut should_prune = private_tx_hash_is_confirmed(storage, &hash);
        if !should_prune {
            if let Ok(Some(body)) = storage.get_private_transaction_bytes(&hash) {
                if let Ok(tx) = serde_json::from_slice::<PrivateTransaction>(&body) {
                    should_prune = private_tx_has_spent_onchain_tag(storage, &tx);
                }
            }
        }
        if should_prune
            && storage
                .delete_pending_private_transaction_hash(&hash)
                .is_ok()
        {
            if let Some(path) = db_path {
                let _ = std::fs::remove_file(
                    private_tx_sidecar_dir(path).join(format!("{hash}.privtx")),
                );
            }
            pruned = pruned.saturating_add(1);
        }
    }
    pruned
}

/// Poll public RPC peers for their pending private transactions and stage any
/// we don't already have into the sidecar queue, so the local mining loop can
/// include them. Returns how many new bodies were staged. Best-effort: any RPC
/// or parse failure is skipped silently so this never disrupts the main loop.
///
/// This is the NAT-friendly pull path that complements libp2p gossip: a miner
/// that only opens outbound HTTP still receives private txs published to a
/// public node.
fn pull_pending_private_txs_from_rpc(db_path: &str, rpc_endpoints: &[String]) -> usize {
    if rpc_endpoints.is_empty() {
        return 0;
    }
    let sidecar = private_tx_sidecar_dir(db_path);

    // Hashes we already hold locally (committed bodies or queued sidecar files)
    // so we don't restage or refetch them every cycle.
    let have_committed = |hash: &str| -> bool {
        RocksDbStorage::new(db_path)
            .ok()
            .and_then(|s| s.get_private_transaction_bytes(hash).ok().flatten())
            .is_some()
    };
    let sidecar_has = |hash: &str| -> bool { sidecar.join(format!("{hash}.privtx")).exists() };

    let mut staged = 0usize;
    for rpc in rpc_endpoints {
        let value = match rpc_http_get_json(rpc, "/private/pending", 5) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let Some(list) = value.get("pending").and_then(|p| p.as_array()) else {
            continue;
        };
        for tx in list {
            let Some(hash) = tx.get("tx_hash").and_then(|h| h.as_str()) else {
                continue;
            };
            let hash = hash.trim();
            if !valid_private_tx_hash_format(hash) {
                continue;
            }
            if sidecar_has(hash) || have_committed(hash) {
                continue;
            }
            let Ok(bytes) = serde_json::to_vec(tx) else {
                continue;
            };
            if std::fs::create_dir_all(&sidecar).is_ok()
                && std::fs::write(sidecar.join(format!("{hash}.privtx")), &bytes).is_ok()
            {
                staged += 1;
            }
        }
    }
    staged
}

const PRIVATE_TX_BODY_BACKFILL_WINDOW_BLOCKS: u64 = 5_000;
const PRIVATE_TX_BODY_PULL_INTERVAL_SECS: u64 = 12;

fn collect_private_tx_hashes_in_height_range(
    storage: &RocksDbStorage,
    from_height: u64,
    to_height: u64,
) -> Vec<String> {
    if from_height == 0 || from_height > to_height {
        return Vec::new();
    }

    let mut hashes = Vec::new();
    let mut seen = HashSet::new();
    for height in from_height..=to_height {
        let Ok(Some(block)) = storage.get_block_by_height(height) else {
            continue;
        };
        for hash in block.private_tx_hashes {
            let hash = hash.trim();
            if valid_private_tx_hash_format(hash) && seen.insert(hash.to_string()) {
                hashes.push(hash.to_string());
            }
        }
    }
    hashes
}

fn missing_private_tx_body_hashes(storage: &RocksDbStorage, hashes: &[String]) -> Vec<String> {
    hashes
        .iter()
        .filter_map(|hash| {
            let hash = hash.trim();
            if !valid_private_tx_hash_format(hash) {
                return None;
            }
            if storage
                .get_private_transaction_bytes(hash)
                .ok()
                .flatten()
                .is_some()
            {
                return None;
            }
            Some(hash.to_string())
        })
        .collect()
}

fn should_advance_private_body_scan(
    has_rpc_endpoints: bool,
    initial_backfill: bool,
    missing_count: usize,
    fetched: usize,
) -> bool {
    if missing_count == 0 {
        return true;
    }
    if !has_rpc_endpoints {
        return false;
    }
    if initial_backfill && missing_count > 0 && fetched == 0 {
        return false;
    }
    true
}

/// Best-effort body backfill for already-committed private tx hashes. This is
/// deliberately outside consensus: failures only leave the local archive as-is.
fn pull_missing_committed_private_tx_bodies(
    storage: &RocksDbStorage,
    rpc_endpoints: &[String],
    hashes: &[String],
) -> usize {
    if rpc_endpoints.is_empty() || hashes.is_empty() {
        return 0;
    }

    let mut fetched = 0usize;
    for hash in hashes {
        let hash = hash.trim();
        if !valid_private_tx_hash_format(hash) {
            continue;
        }
        if storage
            .get_private_transaction_bytes(hash)
            .ok()
            .flatten()
            .is_some()
        {
            continue;
        }

        for rpc in rpc_endpoints {
            let value = match rpc_http_get_json(rpc, &format!("/private/tx/{hash}"), 5) {
                Ok(value) => value,
                Err(_) => continue,
            };
            if archive_committed_private_tx_body_json(storage, hash, &value).is_ok() {
                fetched += 1;
                break;
            }
        }
    }
    fetched
}

fn clear_committed_local_private_txs(
    storage: &RocksDbStorage,
    hashes: &[String],
    db_path: Option<&str>,
) -> Result<(), String> {
    if hashes.is_empty() {
        return Ok(());
    }
    for hash in hashes {
        storage
            .delete_pending_private_transaction_hash(hash)
            .map_err(|e| format!("failed to clear pending private tx '{hash}': {e}"))?;
        if let Some(path) = db_path {
            let _ =
                std::fs::remove_file(private_tx_sidecar_dir(path).join(format!("{hash}.privtx")));
        }
    }
    Ok(())
}

fn persist_fee_market_snapshot(db_path: Option<&str>, snapshot: &FeeMarketSnapshot) {
    if let Ok(storage) = RocksDbStorage::new(fee_snapshot_db_path(db_path)) {
        if let Ok(encoded) = serde_json::to_vec(snapshot) {
            let _ = storage.put_raw("fee:snapshot:v1", encoded);
        }
    }
}

fn recent_chain_activity_avg_txs(db_path: Option<&str>) -> f64 {
    let db = db_path.unwrap_or(DEFAULT_CHAIN_DB_PATH);
    let storage = match RocksDbStorage::new(db) {
        Ok(s) => s,
        Err(_) => return 0.0,
    };
    let height = match storage.get_chain_height() {
        Ok(h) if h > 0 => h,
        _ => return 0.0,
    };
    let window = 20u64;
    let start = height.saturating_sub(window.saturating_sub(1));
    let mut observed_blocks = 0u64;
    let mut tx_load = 0u64;
    for h in start..=height {
        if let Ok(Some(block)) = storage.get_block_by_height(h) {
            observed_blocks = observed_blocks.saturating_add(1);
            let public_user_txs = block.transactions.len().saturating_sub(1) as u64;
            tx_load = tx_load
                .saturating_add(public_user_txs)
                .saturating_add(block.private_tx_hashes.len() as u64);
        }
    }
    if observed_blocks == 0 {
        0.0
    } else {
        tx_load as f64 / observed_blocks as f64
    }
}

fn refresh_fee_market_snapshot(db_path: Option<&str>, password: Option<&str>) -> FeeMarketSnapshot {
    let _ = password;
    let snapshot = FeeMarketSnapshot {
        pending_private_txs: network_pending_private_txs(db_path),
        avg_recent_block_txs: recent_chain_activity_avg_txs(db_path),
        updated_at: now_secs(),
    };
    persist_fee_market_snapshot(db_path, &snapshot);
    snapshot
}

fn private_wallet_exists_on_disk(storage: &WalletStorage, wallet_name: &str) -> bool {
    storage.private_wallet_file_exists(wallet_name)
}

fn generate_available_private_wallet_name(storage: &WalletStorage, prefix: &str) -> String {
    let stamp = now_secs();
    for suffix in 0..10_000u32 {
        let candidate = if suffix == 0 {
            format!("{prefix}-{stamp}")
        } else {
            format!("{prefix}-{stamp}-{suffix}")
        };
        if !private_wallet_exists_on_disk(storage, &candidate) {
            return candidate;
        }
    }
    format!("{prefix}-{}", rand::random::<u32>())
}

fn signing_seed_from_bytes(bytes: &[u8]) -> [u8; 32] {
    let digest = Sha256::digest(bytes);
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&digest[..32]);
    seed
}

fn scalar_from_secret_bytes(bytes: &[u8]) -> Scalar {
    Scalar::from_bytes_mod_order(signing_seed_from_bytes(bytes))
}

fn public_key_hex_from_secret(secret: &[u8]) -> String {
    let point = scalar_from_secret_bytes(secret) * RISTRETTO_BASEPOINT_POINT;
    hex::encode(point.compress().to_bytes())
}

fn private_payment_address(scan_secret: &[u8], spend_public_hex: &str) -> String {
    let scan_public_hex = public_key_hex_from_secret(scan_secret);
    format!("zpriv:{}:{}", scan_public_hex, spend_public_hex)
}

fn private_address_checksum(payload: &[u8]) -> [u8; 4] {
    let mut hasher = Sha256::new();
    hasher.update(b"zoka-private-address-v1");
    hasher.update(payload);
    let digest = hasher.finalize();
    [digest[0], digest[1], digest[2], digest[3]]
}

fn user_private_address_from_zpriv(zpriv: &str) -> Option<String> {
    let parts = zpriv.split(':').collect::<Vec<_>>();
    if parts.len() != 3 || parts[0] != "zpriv" {
        return None;
    }
    let scan = hex::decode(parts[1]).ok()?;
    let spend = hex::decode(parts[2]).ok()?;
    if scan.len() != 32 || spend.len() != 32 {
        return None;
    }
    let mut payload = Vec::with_capacity(64 + 4);
    payload.extend_from_slice(&scan);
    payload.extend_from_slice(&spend);
    payload.extend_from_slice(&private_address_checksum(&payload));
    Some(format!("zka1{}", bs58::encode(payload).into_string()))
}

fn zpriv_from_user_private_address(addr: &str) -> Option<String> {
    validate_user_private_address(addr).ok()
}

/// Validates an external user-facing address (zka1...) or canonical zpriv: form
/// and returns the canonical `zpriv:scan:spend` string. Errors carry the specific
/// reason (prefix, base58, length, checksum) so end users see what to fix.
fn validate_user_private_address(addr: &str) -> Result<String, String> {
    let trimmed = addr.trim();
    if trimmed.is_empty() {
        return Err("address is empty: provide a zka1... address".to_string());
    }
    if trimmed.starts_with("zpriv:") {
        let parts: Vec<&str> = trimmed.split(':').collect();
        if parts.len() != 3 {
            return Err(
                "invalid zpriv address: expected 'zpriv:<scan-hex>:<spend-hex>' format".to_string(),
            );
        }
        let scan = hex::decode(parts[1])
            .map_err(|_| "invalid zpriv address: scan key is not valid hex".to_string())?;
        let spend = hex::decode(parts[2])
            .map_err(|_| "invalid zpriv address: spend key is not valid hex".to_string())?;
        if scan.len() != 32 {
            return Err(format!(
                "invalid zpriv address: scan key must be 32 bytes (got {})",
                scan.len()
            ));
        }
        if spend.len() != 32 {
            return Err(format!(
                "invalid zpriv address: spend key must be 32 bytes (got {})",
                spend.len()
            ));
        }
        return Ok(trimmed.to_string());
    }
    let encoded = trimmed.strip_prefix("zka1").ok_or_else(|| {
        "invalid address: must start with 'zka1' (run 'wallet-list' to see your wallet addresses)"
            .to_string()
    })?;
    let bytes = bs58::decode(encoded).into_vec().map_err(|_| {
        "invalid address: contains characters that are not valid Base58 (check for typos)"
            .to_string()
    })?;
    if bytes.len() != 68 {
        return Err(format!(
            "invalid address: decoded length must be 68 bytes (got {}); the address is truncated or has extra characters",
            bytes.len()
        ));
    }
    let (payload, checksum) = bytes.split_at(64);
    if checksum != private_address_checksum(payload).as_slice() {
        return Err(
            "invalid address: checksum mismatch (likely a typo or copy/paste error)".to_string(),
        );
    }
    let scan = &payload[..32];
    let spend = &payload[32..64];
    Ok(format!(
        "zpriv:{}:{}",
        hex::encode(scan),
        hex::encode(spend)
    ))
}

fn user_private_address_display(raw: &str) -> String {
    user_private_address_from_zpriv(raw).unwrap_or_else(|| raw.to_string())
}

fn load_private_wallet_manager_for_network(
    storage: &WalletStorage,
    password: Option<&str>,
    network: NetworkProfile,
) -> Result<PrivateWalletManager, String> {
    let manager = read_private_wallet_manager(storage, password)?;
    let Ok(mining_state) = load_user_mining_state_for_network(network) else {
        return Ok(manager);
    };
    let has_mature_reward = mining_state.pending_rewards.iter().any(|reward| {
        reward.unlock_height <= mining_state.chain_height
            && resolve_private_wallet_name(&manager, &reward.wallet_address).is_some()
    });
    if !has_mature_reward {
        return Ok(manager);
    }
    // Acreditar una recompensa madura guarda las wallets. Sobre la copia de
    // arriba, leída sin el candado, pisaría lo que otro proceso guardó entre
    // la lectura y la escritura: la reserva de un envío, por ejemplo. Por eso
    // se relee con el candado tomado.
    with_fresh_private_wallets(storage, password, |manager| {
        if let Ok(mut mining_state) = load_user_mining_state_for_network(network) {
            let _ = settle_mature_mining_rewards(
                storage,
                manager,
                password,
                network,
                &mut mining_state,
            );
        }
        Ok(())
    })
    .map(|((), manager)| manager)
}

/// Corre `change` sobre las wallets tal como están en disco ahora, con el
/// candado tomado desde la lectura hasta que `change` termina (y con él, lo que
/// guarde). Es la forma de escribir desde un proceso que tiene una copia en
/// memoria que pudo quedar vieja: esa copia no se guarda nunca.
fn with_fresh_private_wallets<T>(
    storage: &WalletStorage,
    password: Option<&str>,
    change: impl FnOnce(&mut PrivateWalletManager) -> Result<T, String>,
) -> Result<(T, PrivateWalletManager), String> {
    let _wallet_lock = storage
        .lock_private_wallets()
        .map_err(|e| format!("failed to lock wallet directory: {e}"))?;
    let mut manager = read_private_wallet_manager(storage, password)?;
    let output = change(&mut manager)?;
    Ok((output, manager))
}

/// Lee las wallets del disco sin escribir nada.
fn read_private_wallet_manager(
    storage: &WalletStorage,
    password: Option<&str>,
) -> Result<PrivateWalletManager, String> {
    let mut manager = PrivateWalletManager::new();
    let wallets = storage
        .load_all_private_wallets(password)
        .map_err(|e| format!("failed to load private wallets: {e}"))?;
    let mut wallets_by_commitment: Vec<PrivateWallet> = Vec::new();
    for wallet in wallets {
        if let Some(existing) = wallets_by_commitment
            .iter_mut()
            .find(|existing| existing.commitment_key == wallet.commitment_key)
        {
            let preferred_name = preferred_private_wallet_name(&existing.name, &wallet.name);
            if should_replace_duplicate_private_wallet(existing, &wallet) {
                *existing = wallet;
            }
            existing.name = preferred_name;
            continue;
        }
        wallets_by_commitment.push(wallet);
    }
    for wallet in wallets_by_commitment {
        let name = wallet.name.clone();
        let _ = manager.create_private_wallet(&name);
        if let Some(slot) = manager.get_private_wallet_mut(&name) {
            *slot = wallet;
            slot.normalize_key_material();
        }
    }
    // Rebuild NullifierSet from persisted spent_nullifiers so double-spend
    // protection is not lost across node restarts.
    manager.rebuild_nullifier_set();
    // Y el guardia de comisiones ya cobradas, por el mismo motivo: si arranca
    // vacio, volver a escanear un bloque viejo las acredita de nuevo.
    manager.rebuild_claimed_fee_tx_hashes();
    Ok(manager)
}

fn private_wallet_name_has_numeric_suffix(name: &str) -> bool {
    name.rsplit_once('-')
        .map(|(_, suffix)| !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()))
        .unwrap_or(false)
}

fn preferred_private_wallet_name(a: &str, b: &str) -> String {
    let a_has_suffix = private_wallet_name_has_numeric_suffix(a);
    let b_has_suffix = private_wallet_name_has_numeric_suffix(b);
    if a_has_suffix != b_has_suffix {
        return if a_has_suffix { b } else { a }.to_string();
    }
    if a.len() != b.len() {
        return if a.len() < b.len() { a } else { b }.to_string();
    }
    if a <= b { a } else { b }.to_string()
}

fn should_replace_duplicate_private_wallet(
    existing: &PrivateWallet,
    candidate: &PrivateWallet,
) -> bool {
    let existing_score = (
        existing.private_transaction_count(),
        existing.private_notes.len(),
        existing.private_balance,
    );
    let candidate_score = (
        candidate.private_transaction_count(),
        candidate.private_notes.len(),
        candidate.private_balance,
    );
    candidate_score > existing_score
}

/// Punto único por el que pasa toda escritura de wallet.
///
/// El lock de acá solo ordena las escrituras: no impide guardar una copia vieja.
/// Quien escribe tiene que haber leído con el lock ya tomado, o la escritura
/// pisa lo que otro proceso guardó desde esa lectura. Así se perdían reservas
/// de envíos: el cierre del ciclo de minería y `wallet-balance` guardaban la
/// copia que habían leído antes y la nota volvía a figurar disponible (saldo
/// inflado, y el envío siguiente la gastaba otra vez). Con una copia de larga
/// vida, escribir con [`with_fresh_private_wallets`].
///
/// Es reentrante, así que anida sin trabarse con quien ya lo tomó arriba.
fn persist_private_wallet(
    storage: &WalletStorage,
    manager: &PrivateWalletManager,
    wallet_name: &str,
    password: Option<&str>,
) -> Result<(), String> {
    let _lock = storage
        .lock_private_wallets()
        .map_err(|e| format!("failed to lock wallet directory: {e}"))?;
    let wallet = manager
        .get_private_wallet(wallet_name)
        .ok_or_else(|| format!("private wallet not found: {wallet_name}"))?;
    storage
        .save_private_wallet(wallet, password)
        .map_err(|e| format!("failed to save private wallet '{}': {e}", wallet_name))
}

fn persist_all_private_wallets(
    storage: &WalletStorage,
    manager: &PrivateWalletManager,
    password: Option<&str>,
) -> Result<(), String> {
    for name in manager.get_private_wallet_names() {
        persist_private_wallet(storage, manager, &name, password)?;
    }
    Ok(())
}

fn locked_rewards_for_address(state: &UserMiningState, wallet_address: &str) -> u64 {
    state
        .pending_rewards
        .iter()
        .filter(|reward| reward.wallet_address == wallet_address)
        .map(|reward| reward.amount)
        .sum()
}

fn settle_mature_mining_rewards(
    storage: &WalletStorage,
    manager: &mut PrivateWalletManager,
    password: Option<&str>,
    network: NetworkProfile,
    state: &mut UserMiningState,
) -> Result<u64, String> {
    if state.pending_rewards.is_empty() {
        return Ok(0);
    }

    let mut matured_total = 0u64;
    let mut remaining = Vec::with_capacity(state.pending_rewards.len());
    let mut touched_wallets = false;
    for reward in state.pending_rewards.drain(..) {
        if reward.unlock_height > state.chain_height {
            remaining.push(reward);
            continue;
        }
        let Some(wallet_name) = resolve_private_wallet_name(manager, &reward.wallet_address) else {
            remaining.push(reward);
            continue;
        };
        if let Some(private_coinbase) = &reward.private_coinbase {
            manager
                .claim_private_coinbase_output(
                    &wallet_name,
                    &reward.block_hash,
                    private_coinbase,
                    reward.amount,
                    state.chain_height,
                )
                .map_err(|e| format!("failed to claim matured private coinbase: {e}"))?;
        } else {
            manager
                .deposit_to_private_wallet(&wallet_name, reward.amount)
                .map_err(|e| format!("failed to credit legacy matured mining reward: {e}"))?;
        }
        touched_wallets = true;
        matured_total = matured_total.saturating_add(reward.amount);
    }
    state.pending_rewards = remaining;
    if touched_wallets {
        persist_all_private_wallets(storage, manager, password)?;
        save_user_mining_state_for_network(network, state)?;
    }
    Ok(matured_total)
}

fn pending_private_coinbase_exists(
    state: &UserMiningState,
    wallet_address: &str,
    block_hash: &str,
    output_id: &str,
) -> bool {
    state.pending_rewards.iter().any(|reward| {
        reward.wallet_address == wallet_address
            && reward.block_hash == block_hash
            && reward
                .private_coinbase
                .as_ref()
                .map(|coinbase| coinbase.output_id.as_str() == output_id)
                .unwrap_or(false)
    })
}

fn wallet_has_private_note(
    manager: &PrivateWalletManager,
    wallet_name: &str,
    output_id: &str,
) -> bool {
    manager
        .get_private_wallet(wallet_name)
        .map(|wallet| {
            wallet
                .private_notes
                .iter()
                .any(|note| note.note_id == output_id)
        })
        .unwrap_or(false)
}

fn register_private_coinbase_reward_from_block(
    manager: &PrivateWalletManager,
    wallet_name: &str,
    wallet_address: &str,
    wallet_commitment: &str,
    state: &mut UserMiningState,
    block: &zoka::types::Block,
) -> bool {
    let Some(private_coinbase) = block.private_coinbase.as_ref() else {
        return false;
    };
    if private_coinbase.recipient_commitment != wallet_commitment {
        return false;
    }
    if wallet_has_private_note(manager, wallet_name, &private_coinbase.output_id) {
        return false;
    }
    if pending_private_coinbase_exists(
        state,
        wallet_address,
        &block.header.hash,
        &private_coinbase.output_id,
    ) {
        return false;
    }

    let amount = block.transactions.first().map(|tx| tx.amount).unwrap_or(0);
    if amount == 0 {
        return false;
    }

    state.pending_rewards.push(PendingMiningReward {
        wallet_address: wallet_address.to_string(),
        amount,
        source_height: block.header.block_height,
        unlock_height: private_coinbase.unlock_height,
        block_hash: block.header.hash.clone(),
        private_coinbase: Some(private_coinbase.clone()),
    });
    true
}

fn settle_synced_private_coinbase_rewards(
    storage: &WalletStorage,
    manager: &mut PrivateWalletManager,
    password: Option<&str>,
    network: NetworkProfile,
    state: &mut UserMiningState,
    registered: usize,
) -> Result<usize, String> {
    let settled = settle_mature_mining_rewards(storage, manager, password, network, state)?;
    if registered > 0 && settled == 0 {
        save_user_mining_state_for_network(network, state)?;
    }
    Ok(registered.saturating_add(if settled > 0 { 1 } else { 0 }))
}

fn claim_private_fee_rewards_from_block(
    manager: &mut PrivateWalletManager,
    wallet_name: &str,
    block: &zoka::types::Block,
) -> Result<usize, String> {
    if block.private_fee_claims.is_empty() {
        return Ok(0);
    }
    let claimed_hashes = block
        .private_fee_claims
        .iter()
        .map(|claim| claim.tx_hash.as_str())
        .collect::<HashSet<_>>();
    for body in &block.private_tx_bodies {
        let Ok(tx) = serde_json::from_slice::<PrivateTransaction>(body) else {
            continue;
        };
        if claimed_hashes.contains(tx.tx_hash.as_str()) {
            let _ = manager.cache_confirmed_private_transaction_for_fee_claim(tx);
        }
    }
    let outcome = match manager
        .claim_private_fees_from_block_claims(wallet_name, &block.private_fee_claims)
    {
        Ok(outcome) => outcome,
        Err(error)
            if error
                .to_string()
                .contains("No valid private fee claims found") =>
        {
            return Ok(0);
        }
        Err(error) => {
            return Err(format!("failed to scan private mining fee claims: {error}"));
        }
    };
    Ok(outcome.claimed_tx_hashes.len())
}

fn sync_private_coinbase_outputs_from_blocks<I>(
    storage: &WalletStorage,
    manager: &mut PrivateWalletManager,
    wallet_name: &str,
    password: Option<&str>,
    network: NetworkProfile,
    chain_height: u64,
    blocks: I,
) -> Result<usize, String>
where
    I: IntoIterator<Item = zoka::types::Block>,
{
    let wallet = manager
        .get_private_wallet(wallet_name)
        .ok_or_else(|| format!("private wallet not found: {wallet_name}"))?;
    let wallet_commitment = wallet.commitment_key.clone();
    let wallet_address = user_private_address_display(&wallet_commitment);
    let restore_height = wallet.restore_height.max(1);

    let mut state = load_user_mining_state_for_network(network).unwrap_or_default();
    state.chain_height = state.chain_height.max(chain_height);

    let mut registered = 0usize;
    let mut claimed_fees = 0usize;
    for block in blocks {
        if block.header.block_height < restore_height {
            continue;
        }
        claimed_fees = claimed_fees.saturating_add(claim_private_fee_rewards_from_block(
            manager,
            wallet_name,
            &block,
        )?);
        if register_private_coinbase_reward_from_block(
            manager,
            wallet_name,
            &wallet_address,
            &wallet_commitment,
            &mut state,
            &block,
        ) {
            registered = registered.saturating_add(1);
        }
    }

    let settled = settle_synced_private_coinbase_rewards(
        storage, manager, password, network, &mut state, registered,
    )?;
    if claimed_fees > 0 {
        persist_private_wallet(storage, manager, wallet_name, password)?;
    }

    // Advance the wallet's scan checkpoint past every block this call covered
    // so the next unlock only scans new blocks instead of re-reading the
    // entire chain from height 1 every time. Coinbases are still
    // de-duplicated by output_id/block_hash above, so this is purely a
    // performance optimization, not a correctness dependency.
    advance_private_wallet_restore_height(storage, manager, wallet_name, password, chain_height)?;

    Ok(settled.saturating_add(claimed_fees))
}

/// Bumps `wallet.restore_height` to `scanned_through + 1` so future coinbase
/// syncs skip blocks already scanned for this wallet. No-op if the wallet is
/// already past `scanned_through` (e.g. a concurrent sync advanced it).
fn advance_private_wallet_restore_height(
    storage: &WalletStorage,
    manager: &mut PrivateWalletManager,
    wallet_name: &str,
    password: Option<&str>,
    scanned_through: u64,
) -> Result<(), String> {
    let Some(wallet) = manager.get_private_wallet_mut(wallet_name) else {
        return Ok(());
    };
    if scanned_through < wallet.restore_height {
        return Ok(());
    }
    wallet.restore_height = scanned_through.saturating_add(1);
    persist_private_wallet(storage, manager, wallet_name, password)
}

fn sync_private_coinbase_outputs_from_rpc(
    storage: &WalletStorage,
    manager: &mut PrivateWalletManager,
    wallet_name: &str,
    password: Option<&str>,
    network: NetworkProfile,
    rpc: &str,
) -> Result<usize, String> {
    let height_body = rpc_http_get_json(rpc, "/chain/height", 3)?;
    let chain_height = rpc_json_u64(&height_body, &["height", "chain_height"])
        .ok_or_else(|| "RPC height response did not include height".to_string())?;
    let mut state = load_user_mining_state_for_network(network).unwrap_or_default();
    state.chain_height = state.chain_height.max(chain_height);

    let wallet = manager
        .get_private_wallet(wallet_name)
        .ok_or_else(|| format!("private wallet not found: {wallet_name}"))?;
    let wallet_commitment = wallet.commitment_key.clone();
    let wallet_address = user_private_address_display(&wallet_commitment);
    let restore_height = wallet.restore_height.max(1);

    let mut registered = 0usize;
    let mut claimed_fees = 0usize;
    let mut from = restore_height;
    while from <= chain_height {
        let to = from.saturating_add(255).min(chain_height);
        let responses = fetch_rpc_block_range(rpc, from, to)?;
        for response in responses {
            let mut block: zoka::types::Block =
                serde_json::from_slice(&response.encrypted_block)
                    .map_err(|e| format!("RPC private coinbase block decode failed: {e}"))?;
            if block.private_tx_bodies.is_empty() {
                block.private_tx_bodies = response.private_tx_bodies;
            }
            claimed_fees = claimed_fees.saturating_add(claim_private_fee_rewards_from_block(
                manager,
                wallet_name,
                &block,
            )?);
            if register_private_coinbase_reward_from_block(
                manager,
                wallet_name,
                &wallet_address,
                &wallet_commitment,
                &mut state,
                &block,
            ) {
                registered = registered.saturating_add(1);
            }
        }
        from = to.saturating_add(1);
    }

    let settled = settle_synced_private_coinbase_rewards(
        storage, manager, password, network, &mut state, registered,
    )?;
    if claimed_fees > 0 {
        persist_private_wallet(storage, manager, wallet_name, password)?;
    }

    advance_private_wallet_restore_height(storage, manager, wallet_name, password, chain_height)?;

    Ok(settled.saturating_add(claimed_fees))
}

fn push_unique_rpc_url(urls: &mut Vec<String>, url: String) {
    let normalized = url.trim().trim_end_matches('/').to_string();
    if !normalized.is_empty() && !urls.iter().any(|existing| existing == &normalized) {
        urls.push(normalized);
    }
}

fn wallet_sync_rpc_urls(network: NetworkProfile) -> Vec<String> {
    let mut urls = Vec::new();
    // Wallet balance/history must match the mobile wallet view. Prefer the
    // active public bootstrap archive first (same source ZKAPriv scans), and
    // keep the local node as a fallback when the bootstrap is unavailable.
    if let Ok(active) = active_rpc_endpoints_from_config(&[], network) {
        for rpc in active {
            push_unique_rpc_url(&mut urls, rpc.url);
        }
    }
    if let Some(local) = configured_local_rpc_url(network) {
        push_unique_rpc_url(&mut urls, local);
    }
    urls
}

fn private_tx_archive_rpc_urls(network: NetworkProfile) -> Vec<String> {
    let mut urls = Vec::new();
    // Try the local node first when one is configured (e.g. a wallet CLI
    // shelled out to by a desktop app with a node already running
    // alongside it). It's usually reachable, it's the same node that will
    // gossip/mine the tx, and unlike the public bootstrap list it can't be
    // silently pointing at decommissioned infrastructure — a sender whose
    // only public RPC endpoints are all down would otherwise have no way
    // to hand this tx to anyone at all.
    if let Some(local) = configured_local_rpc_url(network) {
        push_unique_rpc_url(&mut urls, local);
    }
    if let Ok(active) = active_rpc_endpoints_from_config(&[], network) {
        for rpc in active {
            push_unique_rpc_url(&mut urls, rpc.url);
        }
    }
    urls
}

fn publish_private_tx_body_to_rpc(rpc: &str, tx: &PrivateTransaction) -> Result<(), String> {
    let body = serde_json::to_value(tx)
        .map_err(|e| format!("failed to encode private tx {}: {e}", tx.tx_hash))?;
    rpc_http_post_json(rpc, "/private/tx", &body, 10).map(|_| ())
}

fn publish_private_tx_body_to_active_rpcs(
    network: NetworkProfile,
    tx: &PrivateTransaction,
) -> (usize, Vec<String>) {
    let mut published = 0usize;
    let mut errors = Vec::new();
    for rpc in private_tx_archive_rpc_urls(network) {
        match publish_private_tx_body_to_rpc(&rpc, tx) {
            Ok(()) => published = published.saturating_add(1),
            Err(err) => errors.push(format!("{rpc}: {err}")),
        }
    }
    (published, errors)
}

/// Collect serialized private transaction bodies for every hash committed
/// in `block` that we have locally under `priv_tx:<hash>`. Returned bodies
/// are intended to be attached to a `BlockResponse` P2P message so peers
/// can persist them and serve `/private/tx/:hash` without depending on
/// the original sender ever having POSTed to a central archive.
fn collect_private_tx_bodies_from_storage(
    storage: &RocksDbStorage,
    block: &zoka::types::Block,
) -> Vec<Vec<u8>> {
    if !block.private_tx_bodies.is_empty() {
        return block.private_tx_bodies.clone();
    }
    if block.private_tx_hashes.is_empty() {
        return Vec::new();
    }
    block
        .private_tx_hashes
        .iter()
        .filter_map(|hash| storage.get_private_transaction_bytes(hash).ok().flatten())
        .collect()
}

/// Collect every body selected for a future canonical private bundle.
/// Unlike P2P attachments, this is all-or-nothing: a miner must never
/// produce a post-activation block whose committed body list is partial.
fn collect_canonical_private_tx_bodies_for_hashes(
    storage: &RocksDbStorage,
    hashes: &[String],
) -> Option<Vec<Vec<u8>>> {
    hashes
        .iter()
        .map(|hash| storage.get_private_transaction_bytes(hash).ok().flatten())
        .collect()
}

/// Derive the sorted, de-duplicated set of spent nullifiers (primary +
/// input nullifiers) for the private txs selected for a block, reading each
/// body from `priv_tx:<hash>` storage. This is committed in the block's
/// Merkle root so every node can enforce the no-double-spend rule without
/// needing the tx bodies. Hashes whose body is missing locally are skipped
/// (the miner only includes txs whose bodies it holds, so this should not
/// happen for the local mining path).
fn collect_private_nullifiers_for_hashes(
    storage: &RocksDbStorage,
    hashes: &[String],
) -> Vec<String> {
    let mut nullifiers = Vec::new();
    for hash in hashes {
        let Some(body) = storage.get_private_transaction_bytes(hash).ok().flatten() else {
            continue;
        };
        let Ok(tx) = serde_json::from_slice::<zoka::private_wallet::PrivateTransaction>(&body)
        else {
            continue;
        };
        nullifiers.extend(private_tx_spend_tags(&tx));
    }
    nullifiers.sort();
    nullifiers.dedup();
    nullifiers
}

/// Same as `collect_private_tx_bodies_from_storage` but opens the DB at
/// `db_path` itself. Returns `Vec::new()` (and silently swallows the
/// error) when the path is missing or storage cannot be opened — body
/// attachment is best-effort and must never abort the block broadcast.
fn collect_private_tx_bodies_for_block_at_path(
    db_path: Option<&str>,
    block: &zoka::types::Block,
) -> Vec<Vec<u8>> {
    let Some(path) = db_path else {
        return Vec::new();
    };
    let Ok(storage) = RocksDbStorage::new(path) else {
        return Vec::new();
    };
    collect_private_tx_bodies_from_storage(&storage, block)
}

fn sync_private_coinbase_outputs_from_available_rpc(
    storage: &WalletStorage,
    manager: &mut PrivateWalletManager,
    wallet_name: &str,
    password: Option<&str>,
    network: NetworkProfile,
) -> Result<usize, String> {
    let mut errors = Vec::new();
    for rpc in wallet_sync_rpc_urls(network) {
        match sync_private_coinbase_outputs_from_rpc(
            storage,
            manager,
            wallet_name,
            password,
            network,
            &rpc,
        ) {
            Ok(imported) => return Ok(imported),
            Err(err) => errors.push(format!("{rpc}: {err}")),
        }
    }
    if errors.is_empty() {
        Err(format!(
            "no wallet sync RPC endpoint configured for {}",
            network.display_name()
        ))
    } else {
        Err(format!(
            "wallet sync RPC failed for every configured endpoint: {}",
            errors.join("; ")
        ))
    }
}

fn sync_private_coinbase_outputs_from_chain_db(
    storage: &WalletStorage,
    manager: &mut PrivateWalletManager,
    wallet_name: &str,
    password: Option<&str>,
    network: NetworkProfile,
    db_path: Option<&str>,
) -> Result<usize, String> {
    let Some(db_path) = db_path else {
        return sync_private_coinbase_outputs_from_available_rpc(
            storage,
            manager,
            wallet_name,
            password,
            network,
        );
    };
    let chain_storage = match open_storage(db_path) {
        Ok(storage) => storage,
        Err(e) if is_storage_lock_error(&e) => {
            return sync_private_coinbase_outputs_from_available_rpc(
                storage,
                manager,
                wallet_name,
                password,
                network,
            );
        }
        Err(e) => return Err(e),
    };
    let chain_height = chain_storage
        .get_chain_height()
        .map_err(|e| format!("failed to read chain height: {e}"))?;
    if chain_height == 0 {
        return sync_private_coinbase_outputs_from_available_rpc(
            storage,
            manager,
            wallet_name,
            password,
            network,
        );
    }
    // Only read blocks the wallet hasn't scanned yet. `restore_height` is
    // advanced past `chain_height` after every successful sync (see
    // `advance_private_wallet_restore_height`), so a wallet that is already
    // caught up reads zero blocks here instead of replaying the whole chain
    // on every unlock.
    let scan_from = manager
        .get_private_wallet(wallet_name)
        .map(|wallet| wallet.restore_height.max(1))
        .unwrap_or(1);
    let mut blocks = Vec::new();
    for height in scan_from..=chain_height {
        if let Some(mut block) = chain_storage
            .get_block_by_height(height)
            .map_err(|e| format!("failed to read block at height {height}: {e}"))?
        {
            if block.private_tx_bodies.is_empty() {
                block.private_tx_bodies =
                    collect_private_tx_bodies_from_storage(&chain_storage, &block);
            }
            blocks.push(block);
        }
    }
    sync_private_coinbase_outputs_from_blocks(
        storage,
        manager,
        wallet_name,
        password,
        network,
        chain_height,
        blocks,
    )
}

fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|a| a == flag)
}

fn is_storage_lock_error(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    lower.contains("lock")
        || lower.contains("locked")
        || lower.contains("being used by another process")
        || lower.contains("utilizado por otro proceso")
        || lower.contains("acceso al archivo")
}

fn local_rpc_height(network: NetworkProfile) -> Option<u64> {
    let rpc = configured_local_rpc_url(network)?;
    rpc_http_get_json(&rpc, "/chain/height", 2)
        .ok()
        .and_then(|body| rpc_json_u64(&body, &["height", "chain_height"]))
}

fn current_known_height_for_network(db_path: Option<&str>, network: NetworkProfile) -> u64 {
    let mut height = 0u64;
    let db = db_path.unwrap_or_else(|| network.default_db_path());
    for attempt in 0..3 {
        match RocksDbStorage::new(db) {
            Ok(storage) => {
                match storage.get_chain_height() {
                    Ok(h) => height = height.max(h),
                    Err(err) => {
                        let err = err.to_string();
                        if attempt < 2 && is_storage_lock_error(&err) {
                            thread::sleep(Duration::from_millis(150));
                            continue;
                        }
                    }
                }
                break;
            }
            Err(err) => {
                let err = err.to_string();
                if attempt < 2 && is_storage_lock_error(&err) {
                    thread::sleep(Duration::from_millis(150));
                    continue;
                }
                if let Some(h) = local_rpc_height(network) {
                    height = height.max(h);
                }
                break;
            }
        }
    }
    if db_path.is_some() {
        return height;
    }
    if let Ok(state) = load_user_mining_state_for_network(network) {
        height = height.max(state.chain_height);
    }
    height
}

const ANSI_RESET: &str = "\x1B[0m";
const ANSI_BOLD: &str = "\x1B[1m";
const ANSI_DIM: &str = "\x1B[2m";
const ANSI_WHITE: &str = "\x1B[37m";
const ANSI_CYAN: &str = "\x1B[36m";
const ANSI_BLUE: &str = "\x1B[34m";
const ANSI_GREEN: &str = "\x1B[32m";
const ANSI_YELLOW: &str = "\x1B[33m";
const ANSI_RED: &str = "\x1B[31m";
const ANSI_MAGENTA: &str = "\x1B[35m";

#[cfg(windows)]
#[link(name = "Kernel32")]
extern "system" {
    fn GetStdHandle(nStdHandle: i32) -> *mut core::ffi::c_void;
    fn GetConsoleMode(hConsoleHandle: *mut core::ffi::c_void, lpMode: *mut u32) -> i32;
    fn SetConsoleMode(hConsoleHandle: *mut core::ffi::c_void, dwMode: u32) -> i32;
}

#[cfg(windows)]
fn enable_ansi_console_support() {
    unsafe {
        const STD_OUTPUT_HANDLE: i32 = -11;
        const STD_ERROR_HANDLE: i32 = -12;
        const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;

        for handle_kind in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            let handle = GetStdHandle(handle_kind);
            if handle.is_null() {
                continue;
            }
            let mut mode = 0u32;
            if GetConsoleMode(handle, &mut mode as *mut u32) == 0 {
                continue;
            }
            let _ = SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
        }
    }
}

#[cfg(not(windows))]
fn enable_ansi_console_support() {}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

fn format_runtime_duration(duration: Duration) -> String {
    let total = duration.as_secs();
    let hours = total / 3_600;
    let minutes = (total % 3_600) / 60;
    let seconds = total % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}

fn format_hashrate_hps(value: f64) -> String {
    if value <= 0.0 {
        "warming up".to_string()
    } else if value < 1.0 {
        format!("{value:.2} H/s")
    } else if value < 1_000.0 {
        format!("{value:.0} H/s")
    } else if value < 1_000_000.0 {
        format!("{:.2} KH/s", value / 1_000.0)
    } else {
        format!("{:.2} MH/s", value / 1_000_000.0)
    }
}

fn clear_runtime_screen() -> Result<(), String> {
    print!("\x1B[2J\x1B[H");
    io::stdout()
        .flush()
        .map_err(|e| format!("stdout flush failed: {e}"))
}

fn set_terminal_title(title: &str) -> Result<(), String> {
    print!("\x1B]0;{}\x07", title);
    io::stdout()
        .flush()
        .map_err(|e| format!("stdout flush failed: {e}"))
}

fn paint(text: &str, ansi_prefix: &str) -> String {
    format!("{ansi_prefix}{text}{ANSI_RESET}")
}

fn badge(text: &str, ansi_prefix: &str) -> String {
    format!("{ANSI_BOLD}{ansi_prefix}[ {text} ]{ANSI_RESET}")
}

fn dashboard_title(title: &str, ansi_prefix: &str) -> String {
    paint(title, &format!("{ANSI_BOLD}{ansi_prefix}"))
}

fn dashboard_section(title: &str, ansi_prefix: &str) -> String {
    format!(
        "{} {}",
        paint(">", &format!("{ANSI_BOLD}{ansi_prefix}")),
        paint(title, &format!("{ANSI_BOLD}{ansi_prefix}"))
    )
}

fn dashboard_label(label: &str) -> String {
    paint(label, ANSI_DIM)
}

fn dashboard_value(value: &str) -> String {
    paint(value, ANSI_WHITE)
}

fn dashboard_line(label: &str, value: impl std::fmt::Display) -> String {
    format!(
        "  {:<18} {}",
        dashboard_label(label),
        dashboard_value(&value.to_string())
    )
}

fn dashboard_divider() -> &'static str {
    "-------------------------------------------------------------------------------"
}

fn phase_badge(phase: &str) -> String {
    match phase {
        "warmup" => badge("WARMUP", ANSI_YELLOW),
        "running" => badge("RUNNING", ANSI_GREEN),
        "stopped" => badge("STOPPED", ANSI_RED),
        other => badge(&other.to_ascii_uppercase(), ANSI_BLUE),
    }
}

fn bool_badge(value: bool, when_true: &str, when_false: &str) -> String {
    if value {
        badge(when_true, ANSI_GREEN)
    } else {
        badge(when_false, ANSI_YELLOW)
    }
}

fn first_local_wallet_selector_from_storage(
    storage: &WalletStorage,
    password: Option<&str>,
    network: NetworkProfile,
) -> Result<Option<String>, String> {
    let manager = load_private_wallet_manager_for_network(storage, password, network)?;
    let mut names = manager.get_private_wallet_names();
    names.sort();
    Ok(names.into_iter().next())
}

fn load_wallet_runtime_snapshot_from_storage(
    storage: &WalletStorage,
    wallet_selector: &str,
    password: Option<&str>,
    network: NetworkProfile,
) -> Result<WalletRuntimeSnapshot, String> {
    let manager = load_private_wallet_manager_for_network(storage, password, network)?;
    let mining_state = load_user_mining_state_for_network(network).unwrap_or_default();
    let wallet_name = resolve_private_wallet_name_required(&manager, wallet_selector)?;
    let wallet = manager
        .get_private_wallet(&wallet_name)
        .ok_or_else(|| format!("wallet not found: {wallet_name}"))?;
    let address = user_private_address_display(&wallet.commitment_key);
    let locked_balance = locked_rewards_for_address(&mining_state, &address);
    let spendable_balance = wallet.private_balance;
    Ok(WalletRuntimeSnapshot {
        wallet_name,
        address,
        balance: spendable_balance,
        locked_balance,
        total_balance: spendable_balance.saturating_add(locked_balance),
        tx_count: wallet.private_transactions.len(),
        view_only: wallet.view_only,
        restore_height: wallet.restore_height,
    })
}

struct CachedDashboardWalletSnapshot {
    key: (NetworkProfile, String),
    loaded_at: Instant,
    snapshot: WalletRuntimeSnapshot,
}

fn dashboard_wallet_snapshot_cache(
) -> &'static std::sync::Mutex<Option<CachedDashboardWalletSnapshot>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<Option<CachedDashboardWalletSnapshot>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(None))
}

const DASHBOARD_WALLET_SNAPSHOT_TTL: Duration = Duration::from_secs(30);

/// Dashboard-only throttled wrapper around
/// [`load_wallet_runtime_snapshot_from_storage`]. The fullnode runtime screen
/// redraws roughly once a second, and each raw load decrypts every private
/// wallet with Argon2 (a deliberately expensive KDF), so on a node that holds
/// a wallet the redraw loop kept a CPU core busy re-deriving the same
/// balances. The snapshot is display-only (address/balances/counts, no
/// secrets), so a short TTL is safe — balances move on ~block cadence anyway.
/// One-shot CLI callers keep using the direct loader (no caching wanted
/// there).
fn load_wallet_runtime_snapshot_for_dashboard(
    storage: &WalletStorage,
    wallet_selector: &str,
    password: Option<&str>,
    network: NetworkProfile,
) -> Result<WalletRuntimeSnapshot, String> {
    let key = (network, wallet_selector.to_string());
    if let Ok(guard) = dashboard_wallet_snapshot_cache().lock() {
        if let Some(cached) = guard.as_ref() {
            if cached.key == key && cached.loaded_at.elapsed() < DASHBOARD_WALLET_SNAPSHOT_TTL {
                return Ok(cached.snapshot.clone());
            }
        }
    }
    let snapshot =
        load_wallet_runtime_snapshot_from_storage(storage, wallet_selector, password, network)?;
    if let Ok(mut guard) = dashboard_wallet_snapshot_cache().lock() {
        *guard = Some(CachedDashboardWalletSnapshot {
            key,
            loaded_at: Instant::now(),
            snapshot: snapshot.clone(),
        });
    }
    Ok(snapshot)
}

fn transport_mode_label(mode: &PrivacyTransportMode) -> &'static str {
    match mode {
        PrivacyTransportMode::Direct => "direct",
        PrivacyTransportMode::Tor => "tor",
        PrivacyTransportMode::I2p => "i2p",
        PrivacyTransportMode::Hybrid => "hybrid",
    }
}

fn transport_status_summary(status: &PrivacyTransportStatus) -> String {
    format!(
        "{} | Tor reachable={} | I2P reachable={}",
        transport_mode_label(&status.mode),
        yes_no(status.tor_reachable),
        yes_no(status.i2p_reachable)
    )
}

fn truncate_dashboard_text(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let truncated: String = value.chars().take(max_chars.saturating_sub(3)).collect();
    format!("{truncated}...")
}

fn render_mining_dashboard(
    wallet: Option<&WalletRuntimeSnapshot>,
    state: &UserMiningState,
    fee_snapshot: &FeeMarketSnapshot,
    db_height: u64,
    session_elapsed: Duration,
    session_blocks: u64,
    session_rewards: u64,
    target_blocks: Option<u64>,
    deadline: Option<Instant>,
    cpu_threads: Option<usize>,
    status_line: &str,
) -> String {
    let mut output = String::new();
    let next_reward = next_block_reward(state.chain_height, state.total_supply_emitted, None);
    let remaining_supply = EMISSION_MAX_SUPPLY.saturating_sub(state.total_supply_emitted);
    let halving_in = if next_reward == 0 {
        0
    } else {
        EMISSION_HALVING_INTERVAL - (state.chain_height % EMISSION_HALVING_INTERVAL)
    };
    let session_target = match (target_blocks, deadline) {
        (Some(blocks), Some(limit)) => format!(
            "{} block(s) or {} remaining",
            blocks,
            format_runtime_duration(
                limit
                    .checked_duration_since(Instant::now())
                    .unwrap_or_default()
            )
        ),
        (Some(blocks), None) => format!("{blocks} block(s)"),
        (None, Some(limit)) => format!(
            "{} remaining",
            format_runtime_duration(
                limit
                    .checked_duration_since(Instant::now())
                    .unwrap_or_default()
            )
        ),
        (None, None) => "continuous".to_string(),
    };
    let session_progress = match target_blocks {
        Some(total) if total > 0 => format!("{session_blocks}/{total} block(s)"),
        _ => "open-ended".to_string(),
    };

    writeln!(
        &mut output,
        "{} {} {}",
        dashboard_title("ZOKA Mining 1.0", ANSI_CYAN),
        bool_badge(state.is_mining, "LIVE", "IDLE"),
        badge(
            if deadline.is_none() && target_blocks.is_none() {
                "CONTINUOUS"
            } else {
                "LIMITED"
            },
            ANSI_BLUE
        )
    )
    .ok();
    writeln!(&mut output, "{}", dashboard_divider()).ok();
    writeln!(
        &mut output,
        "{}  {}  {}  {}",
        dashboard_line("Session uptime", format_runtime_duration(session_elapsed)),
        dashboard_line("Target", session_target),
        dashboard_line("Progress", session_progress),
        dashboard_line("Height", db_height)
    )
    .ok();
    writeln!(
        &mut output,
        "{} {}",
        dashboard_label("Status"),
        paint(&truncate_dashboard_text(status_line, 120), ANSI_YELLOW)
    )
    .ok();
    writeln!(&mut output, "{}", dashboard_divider()).ok();
    writeln!(&mut output, "{}", dashboard_section("Wallet", ANSI_MAGENTA)).ok();
    if let Some(wallet) = wallet {
        writeln!(
            &mut output,
            "{}",
            dashboard_line("Local label", &wallet.wallet_name)
        )
        .ok();
        writeln!(
            &mut output,
            "{}",
            dashboard_line("Address", &wallet.address)
        )
        .ok();
        writeln!(
            &mut output,
            "{}",
            dashboard_line("Available", format_zka(wallet.balance))
        )
        .ok();
        writeln!(
            &mut output,
            "{}",
            dashboard_line("Locked", format_zka(wallet.locked_balance))
        )
        .ok();
        writeln!(
            &mut output,
            "{}",
            dashboard_line("Total", format_zka(wallet.total_balance))
        )
        .ok();
        writeln!(
            &mut output,
            "{}",
            dashboard_line("Transactions", wallet.tx_count)
        )
        .ok();
        writeln!(
            &mut output,
            "{}",
            dashboard_line("View-only", yes_no(wallet.view_only))
        )
        .ok();
        writeln!(
            &mut output,
            "{}",
            dashboard_line("Restore height", wallet.restore_height)
        )
        .ok();
    } else {
        writeln!(
            &mut output,
            "{}",
            dashboard_line("Snapshot", "No local wallet snapshot available")
        )
        .ok();
    }
    writeln!(&mut output, "{}", dashboard_section("Mining", ANSI_GREEN)).ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line("Engine mode", &state.mode)
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line("Algorithm", &state.algorithm)
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line("Mining active", yes_no(state.is_mining))
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line(
            "CPU power",
            cpu_threads
                .map(mining_power_label_for_threads)
                .unwrap_or_else(|| format!("available {} CPU thread(s)", default_mining_threads()))
        )
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line("Session mined blocks", session_blocks)
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line("Session rewards", format_zka(session_rewards))
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line("Total mined blocks", state.total_blocks)
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line("Total mined rewards", format_zka(state.total_rewards))
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line(
            "Coinbase maturity",
            format!("{} block(s)", COINBASE_MATURITY_BLOCKS)
        )
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line(
            "Hashrate estimate",
            format_hashrate_hps(state.estimated_hashrate_hps as f64)
        )
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line(
            "Difficulty",
            format!("{} bits", state.current_difficulty_bits)
        )
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line(
            "Solve time",
            format!(
                "avg {:.2}s / last {:.2}s",
                state.avg_solve_time_secs, state.last_solve_time_secs
            )
        )
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line(
            "Virtual mining time",
            format!("{:.2}s", state.virtual_time_elapsed_secs)
        )
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_section("Economics", ANSI_YELLOW)
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line("Next block reward", format_zka(next_reward))
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line(
            "Emitted supply",
            format!(
                "{} / {} ZKA",
                format_zka_amount(state.total_supply_emitted),
                format_zka_amount(EMISSION_MAX_SUPPLY)
            )
        )
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line("Remaining supply", format_zka(remaining_supply))
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line("Next halving", format!("{} block(s)", halving_in))
    )
    .ok();
    writeln!(&mut output, "{}", dashboard_section("Network", ANSI_BLUE)).ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line("Current known height", db_height)
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line("Pending private txs", fee_snapshot.pending_private_txs)
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line(
            "Recent avg block txs",
            format!("{:.2}", fee_snapshot.avg_recent_block_txs)
        )
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        paint("Press Ctrl+C to stop.", &format!("{ANSI_DIM}{ANSI_CYAN}"))
    )
    .ok();
    output
}

fn render_fullnode_dashboard(
    wallet: &WalletRuntimeSnapshot,
    state: &UserMiningState,
    fee_snapshot: &FeeMarketSnapshot,
    sync_height: u64,
    session_elapsed: Duration,
    phase: &str,
    warmup_remaining: Option<Duration>,
    announced: &str,
    seed_peer: Option<&str>,
    node: &Node,
    auto_mine: bool,
    mine_interval_secs: u64,
    mine_blocks: u64,
    cpu_threads: Option<usize>,
    next_mine_in: Option<Duration>,
    session_blocks: u64,
    session_rewards: u64,
    last_event: &str,
) -> String {
    let mut output = String::new();
    let transport = node.privacy_transport_status();
    let next_reward = next_block_reward(state.chain_height, state.total_supply_emitted, None);
    let remaining_supply = EMISSION_MAX_SUPPLY.saturating_sub(state.total_supply_emitted);
    let halving_in = if next_reward == 0 {
        0
    } else {
        EMISSION_HALVING_INTERVAL - (state.chain_height % EMISSION_HALVING_INTERVAL)
    };

    writeln!(
        &mut output,
        "{} {} {}",
        dashboard_title("ZOKA Fullnode 1.0", ANSI_CYAN),
        phase_badge(phase),
        bool_badge(auto_mine, "AUTO-MINING", "NO-MINING")
    )
    .ok();
    writeln!(&mut output, "{}", dashboard_divider()).ok();
    writeln!(
        &mut output,
        "{}  {}  {}",
        dashboard_line("Session uptime", format_runtime_duration(session_elapsed)),
        dashboard_line("Synced height", sync_height),
        dashboard_line("Mining height", state.chain_height)
    )
    .ok();
    if let Some(remaining) = warmup_remaining {
        writeln!(
            &mut output,
            "{}",
            dashboard_line("Warmup remaining", format_runtime_duration(remaining))
        )
        .ok();
    }
    writeln!(
        &mut output,
        "{} {}",
        dashboard_label("Last event"),
        paint(&truncate_dashboard_text(last_event, 140), ANSI_YELLOW)
    )
    .ok();
    writeln!(&mut output, "{}", dashboard_divider()).ok();
    writeln!(&mut output, "{}", dashboard_section("Wallet", ANSI_MAGENTA)).ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line("Local label", &wallet.wallet_name)
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line("Address", &wallet.address)
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line("Available", format_zka(wallet.balance))
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line("Locked", format_zka(wallet.locked_balance))
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line("Total", format_zka(wallet.total_balance))
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line("Transactions", wallet.tx_count)
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line("View-only", yes_no(wallet.view_only))
    )
    .ok();
    writeln!(&mut output, "{}", dashboard_section("Network", ANSI_BLUE)).ok();
    writeln!(&mut output, "{}", dashboard_line("Peer ID", node.peer_id())).ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line("Listen address", announced)
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line("Seed peer", seed_peer.unwrap_or("none"))
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line("Transport", transport_status_summary(&transport))
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line("Connected peers", node.connected_peer_count())
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line("Incompatible peers", node.incompatible_peer_count())
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line("PBFT peers", node.pbft_subscribed_peer_count())
    )
    .ok();
    writeln!(&mut output, "{}", dashboard_section("Chain", ANSI_YELLOW)).ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line("Pending private txs", fee_snapshot.pending_private_txs)
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line(
            "Recent avg block txs",
            format!("{:.2}", fee_snapshot.avg_recent_block_txs)
        )
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line("Next block reward", format_zka(next_reward))
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line(
            "Emitted supply",
            format!(
                "{} / {} ZKA",
                format_zka_amount(state.total_supply_emitted),
                format_zka_amount(EMISSION_MAX_SUPPLY)
            )
        )
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line("Remaining supply", format_zka(remaining_supply))
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line("Next halving", format!("{} block(s)", halving_in))
    )
    .ok();
    writeln!(&mut output, "{}", dashboard_section("Mining", ANSI_GREEN)).ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line("Auto-mining", yes_no(auto_mine))
    )
    .ok();
    if auto_mine {
        writeln!(
            &mut output,
            "{}",
            dashboard_line("Algorithm", &state.algorithm)
        )
        .ok();
        writeln!(
            &mut output,
            "{}",
            dashboard_line(
                "CPU power",
                cpu_threads
                    .map(mining_power_label_for_threads)
                    .unwrap_or_else(|| {
                        format!("available {} CPU thread(s)", default_mining_threads())
                    })
            )
        )
        .ok();
        writeln!(
            &mut output,
            "{}",
            dashboard_line(
                "Mining cadence",
                if mine_interval_secs == 0 {
                    format!("continuous ({} block batch)", mine_blocks)
                } else {
                    format!("{} block(s) every {}s", mine_blocks, mine_interval_secs)
                }
            )
        )
        .ok();
        writeln!(
            &mut output,
            "{}",
            dashboard_line(
                "Next mining window",
                if mine_interval_secs == 0 {
                    "active".to_string()
                } else {
                    format_runtime_duration(next_mine_in.unwrap_or_default())
                }
            )
        )
        .ok();
    }
    writeln!(
        &mut output,
        "{}",
        dashboard_line("Session mined blocks", session_blocks)
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line("Session rewards", format_zka(session_rewards))
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line(
            "Coinbase maturity",
            format!("{} block(s)", COINBASE_MATURITY_BLOCKS)
        )
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line(
            "Hashrate estimate",
            format_hashrate_hps(state.estimated_hashrate_hps as f64)
        )
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line(
            "Difficulty",
            format!("{} bits", state.current_difficulty_bits)
        )
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line(
            "Solve time",
            format!(
                "avg {:.2}s / last {:.2}s",
                state.avg_solve_time_secs, state.last_solve_time_secs
            )
        )
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        paint("Press Ctrl+C to stop.", &format!("{ANSI_DIM}{ANSI_CYAN}"))
    )
    .ok();
    output
}

fn draw_fullnode_runtime_screen(
    node: &Node,
    network: NetworkProfile,
    wallet_storage: &WalletStorage,
    wallet_selector: &str,
    password: Option<&str>,
    db_path: Option<&str>,
    avoid_db_reads: bool,
    session_elapsed: Duration,
    phase: &str,
    warmup_remaining: Option<Duration>,
    announced: &str,
    seed_peer: Option<&str>,
    auto_mine: bool,
    mine_interval_secs: u64,
    mine_blocks: u64,
    cpu_threads: Option<usize>,
    next_mine_in: Option<Duration>,
    session_mined_blocks: u64,
    session_mined_rewards: u64,
    last_event: &str,
) -> Result<(), String> {
    // Este tablero es de Zenith: lee una wallet de Zenith, que en Quantic no
    // existe, y en cada refresco escribía el error en el diagnóstico (uno por
    // segundo con ZSilent). ZSilent toma el estado de Quantic de la API.
    if network.is_quantic() {
        return Ok(());
    }
    let wallet_snapshot = match load_wallet_runtime_snapshot_for_dashboard(
        wallet_storage,
        wallet_selector,
        password,
        network,
    ) {
        Ok(snapshot) => snapshot,
        Err(err) => {
            mining_log(&format!(
                "dashboard wallet load transient error (non-fatal): {err}"
            ));
            return Ok(());
        }
    };
    let state = match load_user_mining_state_for_network(network) {
        Ok(state) => state,
        Err(err) => {
            mining_log(&format!(
                "dashboard state load transient error (non-fatal): {err}"
            ));
            return Ok(());
        }
    };
    let fee_snapshot = if avoid_db_reads {
        FeeMarketSnapshot {
            pending_private_txs: 0,
            avg_recent_block_txs: 0.0,
            updated_at: now_secs(),
        }
    } else {
        refresh_fee_market_snapshot(db_path, password)
    };
    let known_height = if avoid_db_reads {
        state.chain_height
    } else {
        current_known_height_for_network(db_path, network)
    };
    let screen = render_fullnode_dashboard(
        &wallet_snapshot,
        &state,
        &fee_snapshot,
        known_height,
        session_elapsed,
        phase,
        warmup_remaining,
        announced,
        seed_peer,
        node,
        auto_mine,
        mine_interval_secs,
        mine_blocks,
        cpu_threads,
        next_mine_in,
        session_mined_blocks,
        session_mined_rewards,
        last_event,
    );
    set_terminal_title("ZOKA Fullnode 1.0")?;
    clear_runtime_screen()?;
    print!("{}", screen);
    io::stdout()
        .flush()
        .map_err(|e| format!("stdout flush failed: {e}"))?;
    Ok(())
}

fn run_mining_batch_with_progress(
    config: &MiningEngineConfig,
    mut progress: Option<&mut dyn FnMut(&MiningProgressSnapshot) -> Result<(), String>>,
) -> Result<MiningBatchSummary, String> {
    let network = NetworkProfile::from_chain_id(&config.chain_id)?;
    if config.blocks == 0 {
        return Err("--blocks must be >= 1".to_string());
    }
    if config.team_solution.is_some() && config.blocks != 1 {
        return Err("a mining-team solution must assemble exactly one block".to_string());
    }
    if config.mode != MAINNET_MINING_MODE && config.mode != TESTNET_MINING_MODE {
        return Err(format!(
            "invalid mining mode; expected {} or {}",
            MAINNET_MINING_MODE, TESTNET_MINING_MODE
        ));
    }
    if let Some(path) = config.chain_db_path.as_deref() {
        let storage = open_storage(path)?;
        zoka::state::chain::validate_mainnet_checkpoint_history(&storage, &config.chain_id)
            .map_err(|err| {
                format!(
                    "mining quarantined: canonical history failed checkpoint validation ({err}); synchronize the recovery branch before mining"
                )
            })?;
    }

    mining_log(&format!(
        "batch starting (chain={}, threads={}, blocks={})",
        config.chain_id, config.threads, config.blocks
    ));
    let mut state = load_user_mining_state_for_network(network)?;
    if !state.is_mining {
        state.is_mining = true;
    }
    state.mining_process_id = Some(std::process::id());
    state.last_updated = now_secs();
    save_user_mining_state_for_network(network, &state)?;
    mining_log("state file initialized");

    let storage = wallet_storage_for_chain_id(&config.chain_id)?;
    let mut private_manager =
        load_private_wallet_manager_for_network(&storage, config.password.as_deref(), network)?;
    mining_log("wallet manager loaded");
    let (wallet_name, _) = resolve_network_wallet_address_required(
        &private_manager,
        &storage,
        &config.wallet_selector,
        "--address",
    )?;
    let wallet_address = private_manager
        .get_private_wallet(&wallet_name)
        .map(|wallet| user_private_address_display(&wallet.commitment_key))
        .ok_or_else(|| format!("private wallet not found: {wallet_name}"))?;
    let wallet_commitment = private_manager
        .get_private_wallet(&wallet_name)
        .map(|wallet| wallet.commitment_key.clone())
        .ok_or_else(|| format!("private wallet not found: {wallet_name}"))?;
    if let Some(path) = config.chain_db_path.as_deref() {
        let storage = open_storage(path)?;
        ensure_atomic_economics_db(&storage)?;
    }

    let mut mined_reward = 0u64;
    let mut mined_blocks_effective = 0u64;
    let mut total_solve_secs = 0.0f64;
    let mut last_solve_secs = 0.0f64;
    let mut total_hashes = 0u64;
    let mut produced_blocks: Vec<zoka::types::Block> = Vec::new();
    let mut last_difficulty = state
        .current_difficulty_bits
        .max(config.base_difficulty)
        .clamp(POW_MIN_DIFFICULTY_BITS, POW_MAX_DIFFICULTY_BITS);
    // chain_height is the height of the last mined block (0 = no blocks
    // yet, i.e. the implicit genesis). The next candidate block is
    // therefore at chain_height + 1.
    //
    // Source-of-truth for the chain tip when a chain DB is configured is
    // the storage layer, not the local state file. This keeps wallet-CLI
    // mining and fullnode auto-mining converged on the same chain. When
    // the chain DB is absent we fall back to the legacy local state.
    let (chain_tip_height, chain_tip_hash, chain_tip_supply) =
        if let Some(path) = config.chain_db_path.as_deref() {
            let storage = open_storage(path)?;
            match zoka::state::chain::read_chain_head(&storage)
                .map_err(|e| format!("failed to read chain head: {e}"))?
            {
                Some(head) => (head.height, head.hash, head.total_supply_emitted),
                None => (0, genesis_pow_hash_for_chain(&config.chain_id), 0),
            }
        } else {
            (
                state.chain_height,
                if state.last_block_hash.is_empty() {
                    genesis_pow_hash_for_chain(&config.chain_id)
                } else {
                    state.last_block_hash.clone()
                },
                state.total_supply_emitted,
            )
        };
    state.chain_height = chain_tip_height;
    state.last_block_hash = chain_tip_hash.clone();
    state.total_supply_emitted = chain_tip_supply;
    mining_log(&format!(
        "chain head height={} hash={}",
        chain_tip_height,
        &chain_tip_hash[..chain_tip_hash.len().min(16)]
    ));

    let pow_epoch_blocks = economics::POW_KEY_EPOCH_BLOCKS;
    let mut next_height = chain_tip_height.saturating_add(1);
    let mut current_prev_hash = chain_tip_hash;
    let team_solution = config.team_solution.as_ref();
    if let Some(callback) = progress.as_deref_mut() {
        callback(&MiningProgressSnapshot {
            height: next_height,
            difficulty_bits: last_difficulty,
            nonce: 0,
            hashes_attempted: 0,
            elapsed_secs: 0.0,
            estimated_hashrate_hps: config.hashrate_hps as f64,
        })?;
    }
    for _ in 0..config.blocks {
        if zoka::api::global_network_runtime().checkpoint_quarantined() {
            return Err(
                "mining stopped: mainnet checkpoint quarantine is active; synchronization remains enabled"
                    .to_string(),
            );
        }
        if let Some(path) = config.chain_db_path.as_deref() {
            let storage = open_storage(path)?;
            zoka::state::chain::validate_mainnet_checkpoint_history(&storage, &config.chain_id)
                .map_err(|err| {
                    format!(
                        "mining stopped: canonical history failed checkpoint validation ({err})"
                    )
                })?;
        }
        // Import any private txs queued in the sidecar directory by other
        // processes that could not write to the DB while we held the lock.
        if let Some(path) = config.chain_db_path.as_deref() {
            let staged = pull_pending_private_txs_from_rpc(path, &config.rpc_sync_endpoints);
            if staged > 0 {
                mining_log(&format!(
                    "pulled {staged} pending private tx(s) from RPC before PoW"
                ));
            }
            if let Ok(storage) = open_storage(path) {
                let imported = import_sidecar_private_txs(path, &storage);
                if imported > 0 {
                    mining_log(&format!(
                        "imported {imported} sidecar private tx(s) before PoW"
                    ));
                }
            }
        }

        let height = next_height;
        // Closes a small gap in a multi-block batch: iteration 2+ advances
        // height/prev_hash from the block THIS batch just mined, not a
        // fresh storage read, so it could start already stale if an
        // external block landed in between. solve_pow_nonce's own check
        // only fires on its first 800ms timeout tick, so check up front too.
        if zoka::api::global_network_runtime().local_chain_height() >= height {
            return Err(format!(
                "stale parent: chain already at or past height {height} before PoW solve started"
            ));
        }
        if config.honor_local_mining_control && local_mining_stop_requested() {
            return Err(format!(
                "mining stopped: local mining control disabled before PoW solve at height {height}"
            ));
        }
        let reward_for_block = next_block_reward(height, state.total_supply_emitted, None);
        if reward_for_block == 0 {
            break;
        }

        let _epoch = height / pow_epoch_blocks;
        // Source-of-truth for the next block's difficulty is the chain
        // DB, not the local state file. Every node, miner or verifier,
        // calls the same helper, so the miner cannot pick a number out
        // of thin air. Falls back to the local-state value when no
        // chain DB is configured (legacy CLI-only mining).
        let expected_difficulty_bits = match config.unsafe_rehearsal_difficulty_bits {
            Some(bits) => bits,
            None => match config.chain_db_path.as_deref() {
                Some(path) => {
                    let storage = open_storage(path)?;
                    zoka::mining::difficulty::expected_for_next_block(&storage)?
                }
                None => last_difficulty.clamp(POW_MIN_DIFFICULTY_BITS, POW_MAX_DIFFICULTY_BITS),
            },
        };
        let (difficulty_bits, timestamp, prev_hash, payout_address) =
            if let Some(solution) = team_solution {
                let template = &solution.template;
                (
                    template.network_difficulty_bits,
                    template.timestamp,
                    template.prev_hash.clone(),
                    template.recipient_address.clone(),
                )
            } else {
                (
                    expected_difficulty_bits,
                    now_secs(),
                    current_prev_hash.clone(),
                    wallet_address.clone(),
                )
            };
        if let Some(callback) = progress.as_deref_mut() {
            callback(&MiningProgressSnapshot {
                height,
                difficulty_bits,
                nonce: 0,
                hashes_attempted: 0,
                elapsed_secs: 0.0,
                estimated_hashrate_hps: config.hashrate_hps as f64,
            })?;
        }
        let (pow_hash_bytes, found_nonce, block_hashes, solve_secs) = if let Some(solution) =
            team_solution
        {
            let recomputed = verify_team_solution_for_candidate(
                solution,
                &config.chain_id,
                height,
                &prev_hash,
                expected_difficulty_bits,
            )?;
            mining_log(&format!(
                "assembling verified team solution for height={} recipient={} difficulty_bits={}",
                height, payout_address, difficulty_bits
            ));
            (recomputed, solution.share.nonce, 0, 0.0)
        } else {
            mining_log(&format!(
                "starting PoW solve for height={} difficulty_bits={} threads={}",
                height, difficulty_bits, config.threads
            ));
            solve_pow_nonce(
                &config.chain_id,
                height,
                difficulty_bits,
                &prev_hash,
                &wallet_address,
                timestamp,
                config.threads,
                config.hashrate_hps,
                config.honor_local_mining_control,
                |snapshot| {
                    if let Some(callback) = progress.as_deref_mut() {
                        callback(snapshot)?;
                    }
                    Ok(())
                },
            )?
        };
        mining_log(&format!(
            "solved height={} in {:.2}s ({} hashes, ~{:.0} H/s)",
            height,
            solve_secs,
            block_hashes,
            block_hashes as f64 / solve_secs.max(0.001)
        ));
        total_hashes = total_hashes.saturating_add(block_hashes);

        // Construct the canonical Block this miner just produced and run
        // it through `validate_pow_block`. Any node receiving this block
        // would call the same validator; failing here means the miner
        // produced something the network would reject, which must never
        // happen, so we treat it as a hard error.
        let coinbase_tx = zoka::mining::block_validator::build_coinbase_tx(
            payout_address.clone(),
            reward_for_block,
            height,
            timestamp,
        );
        // A private transfer can arrive while PoW is already being solved.
        // Import the sidecar queue again here so the block we just solved can
        // carry those txs instead of making the user wait for the next block.
        if let Some(path) = config.chain_db_path.as_deref() {
            let staged = pull_pending_private_txs_from_rpc(path, &config.rpc_sync_endpoints);
            if staged > 0 {
                mining_log(&format!(
                    "pulled {staged} pending private tx(s) from RPC before block assembly"
                ));
            }
            if let Ok(storage) = open_storage(path) {
                let imported = import_sidecar_private_txs(path, &storage);
                if imported > 0 {
                    mining_log(&format!(
                        "imported {imported} sidecar private tx(s) before block assembly"
                    ));
                }
            }
        }
        let private_selection_diagnostics = match config.chain_db_path.as_deref() {
            Some(path) => Some(local_pending_private_tx_selection_diagnostics_at(
                path,
                LOCAL_PRIVATE_TXS_PER_BLOCK_LIMIT,
                Some(height),
            )?),
            None => None,
        };
        if let Some(diagnostics) = private_selection_diagnostics.as_ref() {
            if diagnostics.total_candidates > 0
                || diagnostics.storage_locked
                || !diagnostics.selected.is_empty()
            {
                mining_log(&format!(
                    "private tx selection: {}",
                    summarize_pending_private_tx_selection(diagnostics)
                ));
            }
        }
        let mut private_tx_hashes_for_block = match private_selection_diagnostics {
            Some(diagnostics) => diagnostics.selected,
            None => Vec::new(),
        };
        let private_tx_bodies_for_block = if economics::private_tx_canonical_bundle_required(height)
        {
            match config.chain_db_path.as_deref() {
                Some(path) if !private_tx_hashes_for_block.is_empty() => {
                    match open_storage(path).ok().and_then(|storage| {
                        collect_canonical_private_tx_bodies_for_hashes(
                            &storage,
                            &private_tx_hashes_for_block,
                        )
                    }) {
                        Some(bodies) => bodies,
                        None => {
                            private_tx_hashes_for_block.clear();
                            Vec::new()
                        }
                    }
                }
                _ => Vec::new(),
            }
        } else {
            Vec::new()
        };
        // Derive the spent-nullifier set committed in this block from the
        // selected tx bodies, so every node enforces no-double-spend from
        // the block alone (the bodies are not guaranteed to propagate).
        let private_nullifiers_for_block = match config.chain_db_path.as_deref() {
            Some(path) if !private_tx_hashes_for_block.is_empty() => match open_storage(path) {
                Ok(storage) => {
                    collect_private_nullifiers_for_hashes(&storage, &private_tx_hashes_for_block)
                }
                Err(_) => {
                    private_tx_hashes_for_block.clear();
                    Vec::new()
                }
            },
            _ => Vec::new(),
        };
        let payout_commitment = zoka::private_wallet::canonical_payment_address(&payout_address)
            .map_err(|e| format!("invalid mining payout address: {e}"))?;
        let private_fee_claims_for_block = private_manager
            .create_local_private_fee_claims_for_block(
                &private_tx_hashes_for_block,
                &payout_commitment,
            )
            .map_err(|e| format!("failed to build private fee claims: {e}"))?;
        let private_coinbase = zoka::private_wallet::create_private_coinbase_output_for_recipient(
            reward_for_block,
            height,
            &payout_address,
            height.saturating_add(COINBASE_MATURITY_BLOCKS),
        )
        .map_err(|e| format!("failed to build private coinbase output: {e}"))?;
        if private_coinbase.recipient_commitment != payout_commitment {
            return Err("private coinbase recipient mismatch".to_string());
        }
        let local_coinbase_recipient = payout_commitment == wallet_commitment;
        let mut block = zoka::types::Block {
            header: zoka::types::BlockHeader {
                hash: String::new(),
                prev_hash: prev_hash.clone(),
                merkle_root: String::new(),
                state_root: String::from("0"),
                timestamp,
                nonce: found_nonce,
                block_height: height,
                validator: zoka::types::PublicKey(Vec::new()),
                seal: zoka::types::Signature(pow_hash_bytes.clone()),
            },
            transactions: vec![coinbase_tx],
            private_tx_hashes: private_tx_hashes_for_block.clone(),
            private_tx_bodies: private_tx_bodies_for_block.clone(),
            private_nullifiers: private_nullifiers_for_block.clone(),
            private_fee_claims: private_fee_claims_for_block.clone(),
            private_coinbase: Some(private_coinbase.clone()),
            quantic_transactions: Vec::new(),
        };
        block.header.merkle_root = block.compute_merkle_root();
        block.header.hash = block.compute_hash();

        // Parent stub: only `header.hash` and `header.block_height` are
        // consulted by the chain-link rules, so we don't need the full
        // ancestor here. The full chain plumbing (read parent from
        // storage) is the next roadmap item; for now the miner is
        // self-consistent against the height/prev_hash it advanced from.
        let parent_stub = zoka::types::Block {
            header: zoka::types::BlockHeader {
                hash: prev_hash.clone(),
                prev_hash: String::new(),
                merkle_root: String::from("0"),
                state_root: String::from("0"),
                timestamp: 0,
                nonce: 0,
                block_height: height.saturating_sub(1),
                validator: zoka::types::PublicKey(Vec::new()),
                seal: zoka::types::Signature(Vec::new()),
            },
            transactions: Vec::new(),
            private_tx_hashes: Vec::new(),
            private_tx_bodies: Vec::new(),
            private_nullifiers: Vec::new(),
            private_fee_claims: Vec::new(),
            private_coinbase: None,
            quantic_transactions: Vec::new(),
        };

        zoka::mining::block_validator::validate_pow_block(
            &block,
            &zoka::mining::block_validator::PowBlockContext {
                chain_id: &config.chain_id,
                parent: &parent_stub,
                parent_total_supply_emitted: state.total_supply_emitted,
                expected_difficulty_bits: difficulty_bits,
            },
        )
        .map_err(|e| format!("self-validation of mined block failed: {e}"))?;

        let header_hash = block.header.hash.clone();

        // Persist the block to the canonical chain DB when configured.
        // The canonical `apply_block` re-derives the expected difficulty
        // from `mining::difficulty::expected_for_next_block` so the miner
        // and any peer apply the same rule. Failing here means the miner
        // produced something the network would reject; escalate as a
        // hard error.
        if let Some(path) = config.chain_db_path.as_deref() {
            let chain_storage = open_storage(path)?;
            match config.unsafe_rehearsal_difficulty_bits {
                Some(bits) => zoka::state::chain::apply_block_with_expected_difficulty(
                    &chain_storage,
                    &config.chain_id,
                    &block,
                    bits,
                )
                .map_err(|e| {
                    format!("chain::apply_block rejected freshly mined rehearsal block: {e}")
                })?,
                None => zoka::state::chain::apply_block(&chain_storage, &config.chain_id, &block)
                    .map_err(|e| {
                    format!("chain::apply_block rejected freshly mined block: {e}")
                })?,
            };
            clear_committed_local_private_txs(
                &chain_storage,
                &private_tx_hashes_for_block,
                config.chain_db_path.as_deref(),
            )?;
            if !private_tx_hashes_for_block.is_empty() {
                zoka::api::global_network_runtime()
                    .record_private_tx_included_in_block(private_tx_hashes_for_block.len() as u64);
            }
            let mut wallet_state_changed = private_manager
                .mark_private_transactions_confirmed_locally(&private_tx_hashes_for_block)
                > 0
                || (local_coinbase_recipient && !private_fee_claims_for_block.is_empty());
            // Scan the private tx bodies included in this block and credit any
            // outputs destined for the miner's own wallet.  This handles the
            // case where a third party (e.g. another local wallet) submitted a
            // tx while the miner batch was already running so the wallet was
            // loaded before the send, and also the normal case where the miner
            // is the intended recipient of an incoming transfer.
            for hash in &private_tx_hashes_for_block {
                let body = chain_storage
                    .get_private_transaction_bytes(hash)
                    .unwrap_or(None);
                if let Some(bytes) = body {
                    if let Ok(incoming_tx) =
                        serde_json::from_slice::<zoka::private_wallet::PrivateTransaction>(&bytes)
                    {
                        match private_manager.apply_relevant_private_transaction_for_wallet(
                            &wallet_name,
                            &incoming_tx,
                        ) {
                            Ok(true) => {
                                wallet_state_changed = true;
                            }
                            Ok(false) | Err(_) => {}
                        }
                    }
                }
            }
            if local_coinbase_recipient && !private_fee_claims_for_block.is_empty() {
                wallet_state_changed = true;
            }
            if wallet_state_changed {
                // Re-load the on-disk wallet manager so that any concurrent
                // updates (e.g. another local wallet sending while this batch
                // was running) are not silently overwritten.  Then replay the
                // confirmed-status updates, incoming-output credits, and fee
                // claims before flushing back to disk.
                // El lock cubre recarga + escritura como una sola operacion. Sin
                // el, un `tx-send` podia colarse entre ambas y su reserva moria
                // igual, aunque la recarga hubiera salido bien.
                let _wallet_lock = storage
                    .lock_private_wallets()
                    .map_err(|e| format!("failed to lock wallet directory: {e}"))?;
                // La recarga que falla NO puede degradar a la copia en memoria.
                // Antes era `.unwrap_or(private_manager)`: si el disco fallaba
                // un instante, el nodo seguia con su vista vieja y la persistia
                // encima, borrando las reservas que otro proceso —el `tx-send`
                // de un envio— acababa de escribir. Esa nota volvia a figurar
                // como disponible y la transaccion quedaba sin poder minarse.
                // Perder un guardado es mucho mas barato que perder una reserva.
                let mut fresh_manager = load_private_wallet_manager_for_network(
                    &storage,
                    config.password.as_deref(),
                    network,
                )
                .map_err(|error| {
                    format!(
                        "refusing to persist wallets from a stale in-memory copy: \
                         reloading them from disk failed ({error})"
                    )
                })?;
                fresh_manager
                    .mark_private_transactions_confirmed_locally(&private_tx_hashes_for_block);
                for hash in &private_tx_hashes_for_block {
                    let body = chain_storage
                        .get_private_transaction_bytes(hash)
                        .unwrap_or(None);
                    if let Some(bytes) = body {
                        if let Ok(incoming_tx) = serde_json::from_slice::<
                            zoka::private_wallet::PrivateTransaction,
                        >(&bytes)
                        {
                            let _ = fresh_manager.apply_relevant_private_transaction_for_wallet(
                                &wallet_name,
                                &incoming_tx,
                            );
                        }
                    }
                }
                if local_coinbase_recipient && !private_fee_claims_for_block.is_empty() {
                    fresh_manager
                        .claim_private_fees_from_block_claims(
                            &wallet_name,
                            &private_fee_claims_for_block,
                        )
                        .map_err(|e| {
                            format!("failed to claim private fees for mined block: {e}")
                        })?;
                }
                private_manager = fresh_manager;
                persist_all_private_wallets(
                    &storage,
                    &private_manager,
                    config.password.as_deref(),
                )?;
            }
        }

        if local_coinbase_recipient {
            mined_reward = mined_reward
                .checked_add(reward_for_block)
                .ok_or_else(|| "reward overflow".to_string())?;
        }
        mined_blocks_effective = mined_blocks_effective.saturating_add(1);
        total_solve_secs += solve_secs;
        last_solve_secs = solve_secs;
        // After persisting the block, the canonical "next expected
        // difficulty" is what storage reports. We mirror it into the
        // local state file so the dashboards stay in sync, but the chain
        // DB remains authoritative. When no chain DB is configured we
        // fall back to the legacy local retarget heuristic.
        last_difficulty = match config.unsafe_rehearsal_difficulty_bits {
            Some(bits) => bits,
            None => match config.chain_db_path.as_deref() {
                Some(path) => {
                    let storage = open_storage(path)?;
                    zoka::mining::difficulty::expected_for_next_block(&storage)?
                }
                None => {
                    retarget_difficulty(difficulty_bits, solve_secs, config.target_block_time_secs)
                }
            },
        };
        state.chain_height = height;
        state.total_supply_emitted = state.total_supply_emitted.saturating_add(reward_for_block);
        state.last_block_hash = header_hash.clone();
        if local_coinbase_recipient {
            state.pending_rewards.push(PendingMiningReward {
                wallet_address: wallet_address.clone(),
                amount: reward_for_block,
                source_height: height,
                unlock_height: height.saturating_add(COINBASE_MATURITY_BLOCKS),
                block_hash: header_hash.clone(),
                private_coinbase: Some(private_coinbase),
            });
        }
        current_prev_hash = header_hash;
        next_height = height.saturating_add(1);
        produced_blocks.push(block);
    }

    if mined_blocks_effective == 0 {
        return Err("no blocks mined: emission cap reached".to_string());
    }

    state.total_blocks = state
        .total_blocks
        .checked_add(mined_blocks_effective)
        .ok_or_else(|| "mined blocks overflow".to_string())?;
    state.total_rewards = state
        .total_rewards
        .checked_add(mined_reward)
        .ok_or_else(|| "mined rewards overflow".to_string())?;
    state.mode = config.mode.clone();
    state.algorithm = active_pow_algorithm_for_chain(&config.chain_id).to_string();
    state.estimated_hashrate_hps = if total_solve_secs > 0.0 {
        (total_hashes as f64 / total_solve_secs).ceil() as u64
    } else {
        config.hashrate_hps
    };
    state.current_difficulty_bits = last_difficulty;
    state.last_solve_time_secs = last_solve_secs;
    state.virtual_time_elapsed_secs += total_solve_secs;
    if state.total_blocks > 0 {
        state.avg_solve_time_secs = state.virtual_time_elapsed_secs / state.total_blocks as f64;
    }
    state.last_updated = now_secs();
    // `private_manager` se leyó al empezar la tanda (o en el último bloque con
    // cambios) y en el medio pudo haber un envío desde otro proceso. Acreditar
    // sobre esa copia y guardarla le borraba la reserva al envío: sus notas
    // volvían a figurar disponibles, el saldo se inflaba y el envío siguiente
    // las gastaba de nuevo. Se acredita sobre lo que hay en disco.
    with_fresh_private_wallets(&storage, config.password.as_deref(), |fresh| {
        settle_mature_mining_rewards(
            &storage,
            fresh,
            config.password.as_deref(),
            network,
            &mut state,
        )
    })?;
    save_user_mining_state_for_network(network, &state)?;

    Ok(MiningBatchSummary {
        blocks_mined: mined_blocks_effective,
        reward_credited: mined_reward,
        last_difficulty_bits: last_difficulty,
        produced_blocks,
        team_recipient: team_solution.map(|solution| solution.template.recipient_address.clone()),
    })
}

/// If `message` is a `BlockResponse`, deserialize the encoded block and
/// run it through `state::chain::apply_block`. Returns a human-readable
/// verdict for the dashboard. Non-block messages return `None` so the
/// caller falls back to the generic "received" log line.
fn handle_inbound_block_message(
    node: &mut Node,
    message: &zoka::network::message::ZokaMessage,
    source: Option<libp2p::PeerId>,
    chain_id: &str,
    // Reuse the node's already-open chain storage. This is the client-side
    // IBD apply path -- it runs for every block a syncing node receives.
    // Opening a fresh RocksDbStorage per received block (the previous
    // behaviour) capped sync throughput at roughly one DB-open per block
    // (~a block every several seconds), turning a 3k-block sync into hours.
    storage: Option<&RocksDbStorage>,
    peer_view: &mut zoka::state::sync::PeerChainView,
    unsafe_rehearsal_difficulty_bits: Option<u32>,
) -> Option<String> {
    let response = match message {
        zoka::network::message::ZokaMessage::BlockResponse(r) => r,
        _ => return None,
    };

    let height = response.block_height;

    let storage = match storage {
        Some(s) => s,
        None => {
            return Some(format!(
                "Peer block @ height {height} ignored: no chain DB configured"
            ));
        }
    };
    if !peer_view.should_process_block_candidate(source.as_ref(), height, &response.block_hash) {
        return Some(format!(
            "Deferred unrelated block body @ height {height} while recovering a pinned branch."
        ));
    }
    let local_height = storage.get_chain_height().unwrap_or(0);
    if let Some(source) = source {
        peer_view.observe_from(source, height, local_height);
    } else {
        peer_view.observe(height, local_height);
    }

    let outcome = match unsafe_rehearsal_difficulty_bits {
        Some(bits) => zoka::state::sync::try_apply_synced_block_payload_with_expected_difficulty(
            storage, chain_id, response, bits,
        ),
        None => zoka::state::sync::try_apply_synced_block_payload(storage, chain_id, response),
    };
    if matches!(
        &outcome,
        zoka::state::sync::SyncOutcome::BadPayload(_)
            | zoka::state::sync::SyncOutcome::AnnouncedHashMismatch { .. }
            | zoka::state::sync::SyncOutcome::Rejected(_)
    ) {
        if let Some(source) = source {
            node.penalize_block_sync_peer(source);
        }
    }
    peer_view.record_sync_outcome_from(source, &outcome);
    peer_view.remember_block_response_from(source, &outcome, response);
    if let zoka::state::sync::SyncOutcome::Applied(head) = &outcome {
        if let Err(err) = node.publish_block_announce(
            head.height,
            head.hash.clone(),
            head.cumulative_work.to_string(),
            response.difficulty_bits,
        ) {
            tracing::warn!(
                target: "zoka.network",
                height = head.height,
                error = %err,
                "failed to publish block announce after applying peer block"
            );
        }
    }
    Some(sync_outcome_to_event(outcome))
}

fn handle_block_announce(
    node: &mut Node,
    announce: &zoka::network::message::BlockAnnounceMessage,
    source: Option<libp2p::PeerId>,
    // Reuse the node's already-open chain storage instead of opening a fresh
    // RocksDbStorage per gossiped announce.
    storage: Option<&RocksDbStorage>,
    peer_view: &mut zoka::state::sync::PeerChainView,
) -> Option<String> {
    let height = announce.block_height;
    let hash = announce.block_hash.trim();
    if height == 0 || hash.is_empty() {
        return Some("Ignored malformed block announce: missing height/hash.".to_string());
    }

    let announced_work = announce.cumulative_work.parse::<u128>().ok();
    let local_head =
        storage.and_then(|storage| zoka::state::chain::read_chain_head(storage).ok().flatten());
    let local_height = local_head.as_ref().map(|head| head.height).unwrap_or(0);
    if let Some(source) = source {
        peer_view.observe_block_announce_from(source, height, hash, announced_work, local_height);
    } else {
        peer_view.observe(height, local_height);
    }

    if !peer_view.should_process_block_candidate(source.as_ref(), height, hash) {
        return Some(format!(
            "Deferred unrelated block announce @ height {height} while recovering a pinned branch."
        ));
    }

    if !peer_view.recovery_mode() {
        if let Some(head) = &local_head {
            if head.height >= height {
                if head.height == height && head.hash == hash {
                    return Some(format!(
                        "Block announce @ height {height} already matches local head."
                    ));
                }
                if announced_work
                    .map(|work| work <= head.cumulative_work)
                    .unwrap_or(height <= head.height)
                {
                    return Some(format!(
                    "Ignored block announce @ height {height}: local chain has equal or stronger work."
                ));
                }
            }
        }
    }

    match node.publish_block_request(height, Some(hash.to_string())) {
        Ok(()) => {
            node.track_block_announce_retry(height, hash);
            Some(format!(
                "Block announce @ height {height} received; requested body {hash}."
            ))
        }
        Err(err) => Some(format!(
            "Block announce @ height {height} received but request publish failed: {err}"
        )),
    }
}

/// Retries BlockAnnounce-triggered fetches that never got a body, on a
/// short backoff, instead of silently relying on the (much slower) normal
/// headers-first IBD polling to eventually notice the gap. Self-healing:
/// checks local storage first, so a block that already arrived via the
/// normal path just gets its tracking entry dropped, never double-applied
/// in a way that matters (apply_block is idempotent regardless).
async fn sweep_block_announce_retries(
    node: &mut Node,
    chain_id: &str,
    chain_db_path: Option<&str>,
    peer_view: &mut zoka::state::sync::PeerChainView,
) -> Option<String> {
    // A hash-pinned ancestry walk is the only operation that can close an IBD
    // gap when intermediate canonical bodies were pruned. Announcement retry
    // requests can each consume two Tor timeouts and used to starve that walk.
    if peer_view.has_active_branch_backfill() {
        return None;
    }
    let due = node.due_block_announce_retries();
    if due.is_empty() {
        return None;
    }
    let path = chain_db_path?;
    let storage = RocksDbStorage::new(path).ok()?;
    let mut last_event = None;
    // Process at most one opportunistic retry per sweep so normal IBD always
    // gets another scheduling turn, even through a slow onion circuit.
    for (height, hash, attempts) in due.into_iter().take(1) {
        let local_height = storage.get_chain_height().unwrap_or(0);
        if local_height >= height && !peer_view.recovery_mode() {
            node.forget_block_announce_retry(height, &hash);
            continue;
        }
        if !node.bump_block_announce_retry(height, &hash) {
            last_event = Some(format!(
                "BlockAnnounce retry exhausted for height {height}; leaving to normal IBD."
            ));
            continue;
        }
        let preferred_peer = peer_view.preferred_peer_for_height(height);
        match node
            .request_block_with_affinity(
                height,
                Some(hash.clone()),
                preferred_peer,
                false,
                2,
                Duration::from_secs(10),
            )
            .await
        {
            Ok(sourced) => {
                let outcome = zoka::state::sync::try_apply_synced_block_payload(
                    &storage,
                    chain_id,
                    &sourced.response,
                );
                if matches!(
                    &outcome,
                    zoka::state::sync::SyncOutcome::BadPayload(_)
                        | zoka::state::sync::SyncOutcome::AnnouncedHashMismatch { .. }
                        | zoka::state::sync::SyncOutcome::Rejected(_)
                ) {
                    node.penalize_block_sync_peer(sourced.source);
                }
                peer_view.record_sync_outcome_from(Some(sourced.source), &outcome);
                peer_view.remember_block_response_from(
                    Some(sourced.source),
                    &outcome,
                    &sourced.response,
                );
                if matches!(outcome, zoka::state::sync::SyncOutcome::Applied(_)) {
                    node.forget_block_announce_retry(height, &hash);
                }
                last_event = Some(format!(
                    "BlockAnnounce retry {attempts} for height {height}: {}",
                    sync_outcome_to_event(outcome)
                ));
            }
            Err(err) => {
                last_event = Some(format!(
                    "BlockAnnounce retry {attempts} for height {height} failed: {err}"
                ));
            }
        }
    }
    last_event
}

fn block_body_response_from_storage(
    storage: &RocksDbStorage,
    request: &zoka::network::block_request_response::BlockBodyRequest,
) -> zoka::network::block_request_response::BlockBodyResponse {
    let block_height = request.block_height;
    let block = match request.block_hash.as_deref() {
        Some(hash) => match storage.get_block(hash) {
            Ok(Some(block)) if block.header.block_height == block_height => block,
            Ok(Some(_)) => {
                return zoka::network::block_request_response::BlockBodyResponse::NotFound {
                    block_height,
                    block_hash: request.block_hash.clone(),
                    reason: "requested hash is at a different height".to_string(),
                };
            }
            Ok(None) => {
                return zoka::network::block_request_response::BlockBodyResponse::NotFound {
                    block_height,
                    block_hash: request.block_hash.clone(),
                    reason: "requested hash not found".to_string(),
                };
            }
            Err(err) => {
                return zoka::network::block_request_response::BlockBodyResponse::Rejected {
                    block_height,
                    block_hash: request.block_hash.clone(),
                    reason: format!("read hash failed: {err}"),
                };
            }
        },
        None => match storage.get_canonical_block_by_height(block_height) {
            Ok(Some(block)) => block,
            Ok(None) => {
                return zoka::network::block_request_response::BlockBodyResponse::NotFound {
                    block_height,
                    block_hash: None,
                    reason: "canonical block not found".to_string(),
                };
            }
            Err(err) => {
                return zoka::network::block_request_response::BlockBodyResponse::Rejected {
                    block_height,
                    block_hash: None,
                    reason: format!("canonical read failed: {err}"),
                };
            }
        },
    };

    let encrypted_block = match serde_json::to_vec(&block) {
        Ok(bytes) => bytes,
        Err(err) => {
            return zoka::network::block_request_response::BlockBodyResponse::Rejected {
                block_height,
                block_hash: request.block_hash.clone(),
                reason: format!("encode failed: {err}"),
            };
        }
    };
    let bodies = collect_private_tx_bodies_from_storage(storage, &block);
    zoka::network::block_request_response::BlockBodyResponse::Found(
        zoka::network::message::BlockResponseMessage {
            block_height: block.header.block_height,
            encrypted_block,
            block_hash: block.header.hash,
            difficulty_bits: None,
            private_tx_bodies: bodies,
            timestamp: now_secs(),
        },
    )
}

fn serve_pending_block_body_requests(
    node: &mut Node,
    // Reuse the node's already-open chain storage. Opening a fresh
    // RocksDbStorage per request (the previous behaviour) is extremely
    // expensive -- RocksDB replays the WAL and opens every SST on each
    // open, hundreds of ms to seconds on a large DB, and can transiently
    // fail against the primary's lock. Doing that per inbound block request
    // serialized block serving to ~one DB-open apiece, so a peer doing IBD
    // would receive a block or two and then have its requests time out (the
    // node was stuck opening the DB instead of answering). Sharing the open
    // handle makes serving effectively instant.
    storage: Option<&RocksDbStorage>,
) -> Result<Option<String>, String> {
    let mut served = 0u64;
    let mut failed = 0u64;
    let mut last_height = 0u64;
    while let Some(pending) = node.take_pending_block_body_request() {
        let block_height = pending.request.block_height;
        last_height = block_height;
        let response = match storage {
            Some(storage) => block_body_response_from_storage(storage, &pending.request),
            None => zoka::network::block_request_response::BlockBodyResponse::Rejected {
                block_height,
                block_hash: pending.request.block_hash.clone(),
                reason: "no chain DB configured".to_string(),
            },
        };
        match node.respond_block_body_request(pending, response) {
            Ok(()) => served = served.saturating_add(1),
            Err(_) => failed = failed.saturating_add(1),
        }
    }

    if served == 0 && failed == 0 {
        Ok(None)
    } else if failed == 0 {
        Ok(Some(format!(
            "Served {served} direct block body request(s), latest height {last_height}."
        )))
    } else {
        Ok(Some(format!(
            "Served {served} direct block body request(s), {failed} response failure(s), latest height {last_height}."
        )))
    }
}

fn private_tx_body_response_from_storage(
    storage: &RocksDbStorage,
    request: &zoka::network::private_tx_request_response::PrivateTxBodyRequest,
) -> zoka::network::private_tx_request_response::PrivateTxBodyResponse {
    let tx_hash = request.tx_hash.trim();
    match storage.get_private_transaction_bytes(tx_hash) {
        Ok(Some(body)) if !body.is_empty() => {
            zoka::network::private_tx_request_response::PrivateTxBodyResponse::Body {
                tx_hash: tx_hash.to_string(),
                body,
            }
        }
        Ok(Some(_)) => {
            zoka::network::private_tx_request_response::PrivateTxBodyResponse::NotFound {
                tx_hash: tx_hash.to_string(),
                reason: "private tx body is empty".to_string(),
            }
        }
        Ok(None) => zoka::network::private_tx_request_response::PrivateTxBodyResponse::NotFound {
            tx_hash: tx_hash.to_string(),
            reason: "private tx body not found".to_string(),
        },
        Err(_) => zoka::network::private_tx_request_response::PrivateTxBodyResponse::RateLimited {
            tx_hash: tx_hash.to_string(),
            retry_after_secs: 2,
        },
    }
}

fn serve_pending_private_tx_body_requests(
    node: &mut Node,
    // Reuse the node's already-open chain storage (see
    // serve_pending_block_body_requests) instead of opening a fresh
    // RocksDbStorage per request.
    storage: Option<&RocksDbStorage>,
) -> Result<Option<String>, String> {
    let mut served = 0u64;
    let mut failed = 0u64;
    let mut last_hash = String::new();
    while let Some(pending) = node.take_pending_private_tx_body_request() {
        let tx_hash = pending.request.tx_hash.clone();
        last_hash = tx_hash.clone();
        let response = if let Some(body) = node.cached_private_tx_body(&tx_hash) {
            zoka::network::private_tx_request_response::PrivateTxBodyResponse::Body {
                tx_hash,
                body,
            }
        } else {
            match storage {
                Some(storage) => private_tx_body_response_from_storage(storage, &pending.request),
                None => {
                    zoka::network::private_tx_request_response::PrivateTxBodyResponse::NotFound {
                        tx_hash,
                        reason: "no chain DB configured".to_string(),
                    }
                }
            }
        };
        match node.respond_private_tx_body_request(pending, response) {
            Ok(()) => served = served.saturating_add(1),
            Err(_) => failed = failed.saturating_add(1),
        }
    }

    if served == 0 && failed == 0 {
        Ok(None)
    } else if failed == 0 {
        Ok(Some(format!(
            "Served {served} direct private tx body request(s), latest {}.",
            truncate_dashboard_text(&last_hash, 16)
        )))
    } else {
        Ok(Some(format!(
            "Served {served} direct private tx body request(s), {failed} response failure(s), latest {}.",
            truncate_dashboard_text(&last_hash, 16)
        )))
    }
}

/// Build a `HeadersResponse` for an inbound `GetHeaders` request from local storage.
/// Sirve resumenes de bloque a wallets livianas.
///
/// Un resumen lleva lo que hace falta para escanear un saldo y omite lo que
/// solo sirve para gastar -- sobre todo la prueba de rango del coinbase, que es
/// el 56% de cada bloque. Quien reconozca un coinbase propio pide ESE bloque
/// entero por el protocolo de siempre.
fn private_tx_bodies_from_storage(
    storage: &RocksDbStorage,
    request: &zoka::network::private_tx_bodies_request_response::PrivateTxBodiesRequest,
) -> zoka::network::private_tx_bodies_request_response::PrivateTxBodiesResponse {
    use zoka::network::private_tx_bodies_request_response::{
        PrivateTxBodiesResponse, PrivateTxBodyEntry, MAX_BODIES_PER_REQUEST,
    };

    let mut bodies = Vec::new();
    let mut missing = Vec::new();
    // Se acota lo que un par puede hacer trabajar a otro de una sola vez. Lo
    // que sobra del pedido se informa como faltante, no se descarta en
    // silencio: quien pregunta tiene que saber que le quedo pendiente para
    // volver a pedirlo.
    for (indice, hash) in request.tx_hashes.iter().enumerate() {
        let hash = hash.trim();
        if hash.is_empty() {
            continue;
        }
        if indice >= MAX_BODIES_PER_REQUEST {
            missing.push(hash.to_string());
            continue;
        }
        // Misma fuente que el protocolo de a uno, para que las dos vias no
        // puedan discrepar sobre que cuerpos existen.
        match storage.get_private_transaction_bytes(hash) {
            Ok(Some(body)) if !body.is_empty() => bodies.push(PrivateTxBodyEntry {
                tx_hash: hash.to_string(),
                body,
            }),
            _ => missing.push(hash.to_string()),
        }
    }
    PrivateTxBodiesResponse { bodies, missing }
}

/// Resúmenes de transacciones privadas para wallets livianas, desde la base.
///
/// Misma fuente que los cuerpos (`get_private_transaction_bytes`), así que las
/// dos vías no pueden discrepar sobre qué transacciones existen. Un cuerpo que
/// no se puede leer se informa como faltante: el resumen tiene que salir del
/// cuerpo de verdad, nunca de una suposición.
fn private_tx_digests_from_storage(
    storage: &RocksDbStorage,
    request: &zoka::network::private_tx_digests_request_response::PrivateTxDigestsRequest,
) -> zoka::network::private_tx_digests_request_response::PrivateTxDigestsResponse {
    zoka::network::private_tx_digests_request_response::answer_request(request, |hash| {
        let bytes = storage.get_private_transaction_bytes(hash).ok()??;
        let tx: PrivateTransaction = serde_json::from_slice(&bytes).ok()?;
        Some(zoka::private_wallet::digest::PrivateTxDigest::from_tx(&tx))
    })
}

fn wallet_digests_from_storage(
    storage: &RocksDbStorage,
    request: &zoka::network::wallet_digest_request_response::GetWalletDigests,
) -> zoka::network::wallet_digest_request_response::WalletDigestsResponse {
    use zoka::network::wallet_digest_request_response::{
        BlockDigest, WalletDigestsResponse, MAX_DIGESTS_PER_RESPONSE,
    };

    if request.from_height > request.to_height {
        return WalletDigestsResponse::NotFound {
            reason: "rango invalido".to_string(),
        };
    }
    // Se acota lo que un par puede hacer trabajar a otro de una sola vez.
    let hasta = request.to_height.min(
        request
            .from_height
            .saturating_add(MAX_DIGESTS_PER_RESPONSE - 1),
    );

    let mut digests = Vec::new();
    for altura in request.from_height.max(1)..=hasta {
        match storage.get_block_by_height(altura) {
            Ok(Some(block)) => digests.push(BlockDigest::from_block(&block)),
            // Un hueco corta la respuesta en vez de devolverla con faltantes:
            // quien pregunta necesita saber hasta donde llego de verdad.
            _ => break,
        }
    }
    if digests.len() != (hasta.saturating_sub(request.from_height.max(1)) + 1) as usize {
        return WalletDigestsResponse::NotFound {
            reason: "el nodo no tiene el rango completo".to_string(),
        };
    }
    WalletDigestsResponse::Digests { digests }
}

fn headers_response_from_storage(
    storage: &RocksDbStorage,
    request: &zoka::network::header_request_response::GetHeaders,
) -> zoka::network::header_request_response::HeadersResponse {
    use zoka::network::header_request_response::{HeaderSummary, HeadersResponse};

    let tip = match storage.get_chain_height() {
        Ok(h) => h,
        Err(e) => {
            return HeadersResponse::NotFound {
                reason: format!("get chain height failed: {e}"),
            }
        }
    };

    // Find the common ancestor by scanning the locator list.
    let mut start_height: Option<u64> = None;
    let mut common_ancestor = String::new();
    for hash in &request.locator {
        let hash = hash.trim();
        if hash.is_empty() {
            continue;
        }
        if let Ok(Some(block)) = storage.get_block(hash) {
            start_height = Some(block.header.block_height.saturating_add(1));
            common_ancestor = hash.to_string();
            break;
        }
    }

    // No common ancestor found — instruct the caller to start from genesis.
    let start = start_height.unwrap_or(1).max(1);
    if start > tip {
        return HeadersResponse::Headers {
            headers: Vec::new(),
            common_ancestor,
        };
    }

    let max = (request.max as u64)
        .min(512)
        .min(tip.saturating_sub(start) + 1);
    let mut headers: Vec<HeaderSummary> = Vec::with_capacity(max as usize);

    let mut cumulative_work = if start > 1 {
        storage
            .get_block_by_height(start - 1)
            .ok()
            .flatten()
            .and_then(|_| storage.get_cumulative_work().ok())
            .unwrap_or(0)
    } else {
        0
    };

    let mut difficulty_bits = storage
        .get_current_difficulty_bits()
        .unwrap_or(zoka::economics::POW_INITIAL_DIFFICULTY_BITS);

    for h in start..=(start + max - 1) {
        if let Some(stop) = &request.stop_hash {
            if !stop.is_empty() {
                if headers
                    .last()
                    .map(|s| &s.header.hash == stop)
                    .unwrap_or(false)
                {
                    break;
                }
            }
        }
        let block = match storage.get_canonical_block_by_height(h) {
            Ok(Some(b)) => b,
            _ => break,
        };
        // Approximate cumulative work from PoW bits when exact value is unavailable.
        let block_work = 1u128 << difficulty_bits.min(127);
        cumulative_work = cumulative_work.saturating_add(block_work);

        headers.push(HeaderSummary {
            header: block.header.clone(),
            difficulty_bits,
            cumulative_work,
        });

        if block.header.block_height == tip {
            // Update difficulty for the next block window.
            if let Ok(bits) = storage.get_current_difficulty_bits() {
                difficulty_bits = bits;
            }
        }
    }

    HeadersResponse::Headers {
        headers,
        common_ancestor,
    }
}

/// Drain pending inbound GetHeaders requests and serve them from local storage.
fn serve_pending_header_requests(
    node: &mut zoka::network::p2p::Node,
    // Reuse the node's already-open chain storage (see
    // serve_pending_block_body_requests) instead of opening a fresh
    // RocksDbStorage per request.
    storage: Option<&RocksDbStorage>,
) -> Result<Option<String>, String> {
    let mut served = 0u32;
    let mut failed = 0u32;
    while let Some(pending) = node.take_pending_header_request() {
        let response = match storage {
            Some(storage) => headers_response_from_storage(storage, &pending.request),
            None => zoka::network::header_request_response::HeadersResponse::NotFound {
                reason: "no chain DB configured".to_string(),
            },
        };
        match node.respond_header_request(pending, response) {
            Ok(()) => served = served.saturating_add(1),
            Err(_) => failed = failed.saturating_add(1),
        }
    }
    if served == 0 && failed == 0 {
        Ok(None)
    } else {
        Ok(Some(format!(
            "Served {served} GetHeaders request(s), {failed} response failure(s)."
        )))
    }
}

/// Absolute floor before a snapshot pull is even considered, so a node a few
/// blocks behind never does anything but sync normally.
const PEER_SNAPSHOT_PULL_MIN_GAP: u64 = 1_000;

/// A snapshot carries the *whole* chain, so pulling one to close a partial gap
/// wastes bandwidth: measured on mainnet a snapshot runs ~14 KB per block, so
/// closing a 1 000-block gap costs ~143 MB as a snapshot versus ~15 MB fetched
/// incrementally -- ten times the traffic for the same result. It only pays off
/// when we're missing most of the chain anyway, which is the case this feature
/// actually exists for: letting a fresh node bootstrap from any peer instead of
/// depending on a seed being reachable.
///
/// So require the offer to be at least this multiple of our own height. At 2,
/// a node holding less than half the chain snapshots; anything further along
/// stays on incremental sync.
const PEER_SNAPSHOT_PULL_MIN_HEIGHT_RATIO: u64 = 2;

/// Whether pulling a whole snapshot beats syncing the difference. Used both to
/// decide if it's worth asking peers at all and to filter the offers they send
/// back, so the two can't drift apart.
fn peer_snapshot_would_help(local_height: u64, network_height: u64) -> bool {
    network_height > local_height.saturating_add(PEER_SNAPSHOT_PULL_MIN_GAP)
        && network_height >= local_height.saturating_mul(PEER_SNAPSHOT_PULL_MIN_HEIGHT_RATIO)
}

/// Peers asked per discovery round. Small: the responses are tiny, and any
/// honest peer's answer is enough to find the heaviest chain on offer.
const PEER_SNAPSHOT_MANIFEST_FANOUT: usize = 4;

/// Ask connected peers what chain they can serve. Returns how many were asked.
fn request_snapshot_manifests_from_peers(
    node: &mut zoka::network::p2p::Node,
    chain_id: &str,
) -> usize {
    use zoka::network::snapshot_request_response::GetSnapshotManifests;

    let peers: Vec<_> = node
        .connected_peer_ids()
        .into_iter()
        .take(PEER_SNAPSHOT_MANIFEST_FANOUT)
        .collect();
    let mut sent = 0usize;
    for peer in peers {
        let request = GetSnapshotManifests {
            chain_id: chain_id.to_string(),
            max_height: None,
        };
        if node
            .send_snapshot_manifest_request(&peer, request)
            .is_some()
        {
            sent = sent.saturating_add(1);
        }
    }
    sent
}

/// Drain peer replies and pick the heaviest chain we could actually pull.
///
/// Selection is by cumulative work, the same rule consensus uses to compare
/// chains — not by height, which a peer could inflate by mining a long low
/// difficulty branch. Nothing here is trusted: an offer only decides *who we
/// ask*, and every block is re-validated against consensus (including the
/// canonical 3127 checkpoint) on import, so a dishonest offer costs bandwidth
/// and nothing else.
fn best_peer_snapshot_offer(
    node: &mut zoka::network::p2p::Node,
    chain_id: &str,
    local_height: u64,
) -> Option<zoka::network::snapshot_request_response::SnapshotManifest> {
    use zoka::network::snapshot_request_response::SnapshotManifestsResponse;

    let mut offers = Vec::new();
    while let Some(received) = node.take_received_snapshot_manifests() {
        if let SnapshotManifestsResponse::Manifests { manifests } = received.response {
            offers.extend(manifests);
        }
    }
    choose_best_snapshot_offer(offers, chain_id, local_height)
}

/// Selection rule for [`best_peer_snapshot_offer`], split out so it can be
/// tested without a live swarm.
fn choose_best_snapshot_offer(
    offers: impl IntoIterator<Item = zoka::network::snapshot_request_response::SnapshotManifest>,
    chain_id: &str,
    local_height: u64,
) -> Option<zoka::network::snapshot_request_response::SnapshotManifest> {
    use zoka::network::snapshot_request_response::SnapshotManifest;

    let mut best: Option<SnapshotManifest> = None;
    for manifest in offers {
        if manifest.chain_id != chain_id {
            continue;
        }
        // Older peers advertise no endpoint id; there's no way to pull from
        // them, so their offers are informational only.
        if manifest.iroh_endpoint_id.is_none() {
            continue;
        }
        if !peer_snapshot_would_help(local_height, manifest.head_height) {
            continue;
        }
        if best
            .as_ref()
            .map_or(true, |b| manifest.cumulative_work > b.cumulative_work)
        {
            best = Some(manifest);
        }
    }
    best
}

/// Pull a snapshot from a peer's iroh endpoint. Runs off the main loop: the
/// transfer is slow and purely network-bound, and stalling the loop on it
/// would drop gossip and peer keepalives.
/// El preset de iroh del nodo: el de n0 y, con `pq-experimental`, TLS híbrido
/// postcuántico (X25519MLKEM768) cuando el proceso sigue una red de Quantic.
#[cfg(feature = "pq-experimental")]
fn iroh_preset() -> zoka::network::pq_tls::NodePreset {
    zoka::network::pq_tls::NodePreset
}

#[cfg(not(feature = "pq-experimental"))]
fn iroh_preset() -> iroh::endpoint::presets::N0 {
    iroh::endpoint::presets::N0
}

async fn fetch_peer_snapshot(endpoint_id: iroh::EndpointId) -> Result<(String, Vec<u8>), String> {
    let fetch = zoka::network::iroh_snapshot_transfer::fetch_snapshot(
        endpoint_id.into(),
        iroh_preset(),
    );
    match tokio::time::timeout(Duration::from_secs(180), fetch).await {
        Ok(Ok(bytes)) => Ok((endpoint_id.to_string(), bytes)),
        Ok(Err(e)) => Err(format!("transfer from {endpoint_id} failed: {e}")),
        Err(_) => Err(format!(
            "transfer from {endpoint_id} timed out after 180s (relay/hole-punch unreachable)"
        )),
    }
}

/// Import a snapshot that came from an arbitrary peer.
///
/// Deliberately does *not* require a trusted signature, unlike the
/// manifest-seed bootstrap path. That path uses the signature to authorize a
/// specific operator-run seed; here the whole point is that *any* peer can be
/// the source, so authorization can't come from an allowlist. Safety comes
/// from `import_into_storage` replaying every block through the full consensus
/// apply path — proof of work, difficulty, and the canonical 3127 checkpoint —
/// so a hostile peer cannot introduce an invalid or side-branch chain. The
/// worst it can do is waste our bandwidth.
fn import_peer_snapshot_bytes(
    storage: &Arc<RocksDbStorage>,
    chain_id: &str,
    bytes: &[u8],
) -> Result<u64, String> {
    let snapshot = zoka::snapshot::deserialize(bytes).map_err(|e| format!("parse failed: {e}"))?;
    if snapshot.header.chain_id != chain_id {
        return Err(format!(
            "snapshot is for chain {}, not {chain_id}",
            snapshot.header.chain_id
        ));
    }
    zoka::snapshot::import_into_storage(storage, chain_id, &snapshot)
        .map_err(|e| format!("import rejected by consensus: {e}"))
}

/// Describe the chain this node can serve over iroh *right now*, so peers can
/// pull from us without us having exported a snapshot file first.
///
/// `size_bytes` and `sha256_hex` are deliberately empty: the snapshot is built
/// on demand, so neither exists until the transfer happens. They are advisory
/// fields only — selection uses cumulative work, and correctness comes from
/// re-validating every block on import.
fn live_chain_manifest(
    storage: &Arc<RocksDbStorage>,
    chain_id: &str,
    iroh_endpoint_id: &str,
) -> Result<zoka::network::snapshot_request_response::SnapshotManifest, String> {
    use zoka::network::snapshot_request_response::SnapshotManifest;

    let head_height = storage
        .get_chain_height()
        .map_err(|e| format!("chain height: {e}"))?;
    if head_height == 0 {
        return Err("chain is empty; nothing to advertise".to_string());
    }
    let head_hash = storage
        .get_last_block_hash()
        .map_err(|e| format!("head hash: {e}"))?;
    let cumulative_work = storage
        .get_cumulative_work()
        .map_err(|e| format!("cumulative work: {e}"))?;

    Ok(SnapshotManifest {
        chain_id: chain_id.to_string(),
        schema: zoka::snapshot::SNAPSHOT_SCHEMA_ID.to_string(),
        head_height,
        head_hash,
        cumulative_work,
        block_count: head_height,
        generated_unix_secs: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        size_bytes: 0,
        sha256_hex: String::new(),
        download_urls: Vec::new(),
        iroh_endpoint_id: Some(iroh_endpoint_id.to_string()),
        signature_hex: None,
        signer_pubkey_hex: None,
    })
}

fn serve_pending_snapshot_manifest_requests(
    node: &mut zoka::network::p2p::Node,
    storage: Option<&Arc<RocksDbStorage>>,
    our_chain_id: &str,
    iroh_endpoint_id: Option<&str>,
) -> Result<Option<String>, String> {
    use zoka::network::snapshot_request_response::{SnapshotManifest, SnapshotManifestsResponse};

    let mut served = 0u32;
    let mut failed = 0u32;
    while let Some(pending) = node.take_pending_snapshot_manifest_request() {
        let dir = zoka::snapshot::default_snapshot_dir();
        let file_manifests = zoka::snapshot::scan_snapshot_manifests(&dir);
        let chain_id = &pending.request.chain_id;
        let max_height = pending.request.max_height;

        let mut network_manifests: Vec<SnapshotManifest> = file_manifests
            .into_iter()
            .filter(|m| m.chain_id == *chain_id && max_height.map_or(true, |h| m.head_height <= h))
            .map(|m| SnapshotManifest {
                chain_id: m.chain_id,
                schema: m.schema,
                head_height: m.head_height,
                head_hash: m.head_hash,
                cumulative_work: m.cumulative_work,
                block_count: m.block_count,
                generated_unix_secs: m.generated_unix_secs,
                size_bytes: m.size_bytes,
                sha256_hex: m.sha256_hex,
                download_urls: m.download_urls,
                // Snapshot *files* on disk aren't what the iroh endpoint
                // serves (it serves the live chain, advertised separately
                // below), so they carry no endpoint id.
                iroh_endpoint_id: None,
                signature_hex: m.signature_hex,
                signer_pubkey_hex: m.signer_pubkey_hex,
            })
            .collect();

        // Advertise the live chain we serve over iroh. This is what makes the
        // mesh self-healing: every ZSilent node already runs an iroh endpoint
        // (`--serve-iroh-snapshot`), so once each one says "I'm at height H,
        // pull from endpoint X", a node that falls behind can bootstrap from
        // whichever peer is furthest ahead instead of depending on a seed.
        //
        // size_bytes/sha256 are left empty on purpose: this snapshot is
        // generated on demand, so neither is known until it's transferred.
        // Nothing trusts them — the receiver picks by cumulative work and then
        // re-validates every block against consensus on import.
        if let (Some(storage), Some(endpoint_id)) = (storage, iroh_endpoint_id) {
            if chain_id == our_chain_id {
                match live_chain_manifest(storage, chain_id, endpoint_id) {
                    Ok(live) if max_height.map_or(true, |h| live.head_height <= h) => {
                        network_manifests.push(live)
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::debug!(target: "zoka.snapshot", error = %e, "live chain manifest unavailable")
                    }
                }
            }
        }

        let response = if network_manifests.is_empty() {
            SnapshotManifestsResponse::NotAvailable {
                reason: "no snapshots available for the requested chain/height".to_string(),
            }
        } else {
            SnapshotManifestsResponse::Manifests {
                manifests: network_manifests,
            }
        };

        match node.respond_snapshot_manifest_request(pending, response) {
            Ok(()) => served = served.saturating_add(1),
            Err(_) => failed = failed.saturating_add(1),
        }
    }
    if served == 0 && failed == 0 {
        Ok(None)
    } else {
        Ok(Some(format!(
            "Served {served} GetSnapshotManifests request(s), {failed} response failure(s)."
        )))
    }
}

/// Cuanto atraso tolera el nodo antes de dejar de buscar cuerpos de tx
/// privadas y reservar el canal para sincronizar.
///
/// El fetch de un cuerpo existe para que el minero local lo incluya en un
/// bloque; un nodo atrasado no mina, asi que ese trabajo no tiene destino y
/// compite por los mismos circuitos Tor que los bloques. Por encima de este
/// margen la sincronizacion tiene prioridad.
///
/// El valor es generoso a proposito: cubre de sobra la variacion normal de
/// unos pocos bloques cerca de la punta -- donde el nodo si mina y si
/// necesita los cuerpos -- y solo corta cuando el atraso es real.
const PRIVATE_TX_FETCH_MAX_LAG: u64 = 32;

async fn handle_fullnode_network_message(
    node: &mut Node,
    message: &zoka::network::message::ZokaMessage,
    source: Option<libp2p::PeerId>,
    team_mining_runtime: &Arc<zoka::team_mining::TeamMiningRuntime>,
    chain_id: &str,
    chain_db_path: Option<&str>,
    storage: Option<&RocksDbStorage>,
    peer_view: &mut zoka::state::sync::PeerChainView,
    unsafe_rehearsal_difficulty_bits: Option<u32>,
) -> Option<String> {
    match message {
        zoka::network::message::ZokaMessage::SignedCheckpoint(checkpoint) => {
            let storage = storage?;
            let Some(authority) = zoka::checkpoint_authority::authority_keys_for_chain(chain_id) else {
                return None;
            };
            match zoka::checkpoint_authority::accept_signed_checkpoint(
                storage,
                chain_id,
                checkpoint,
                &authority,
            ) {
                Ok(zoka::checkpoint_authority::Acceptance::Accepted) => Some(format!(
                    "Signed checkpoint accepted: {} at {}.",
                    &checkpoint.block_hash[..12],
                    checkpoint.height
                )),
                Ok(zoka::checkpoint_authority::Acceptance::AlreadyKnown) => None,
                Err(error @ zoka::checkpoint_authority::CheckpointError::Conflict { .. }) => {
                    Some(format!("ALERT: {error}"))
                }
                // La autoridad publica las dos versiones mientras haya nodos
                // viejos; la que no es la de esta cadena se descarta sin ruido.
                Err(
                    zoka::checkpoint_authority::CheckpointError::HybridRequired
                    | zoka::checkpoint_authority::CheckpointError::NoHybridAuthority,
                ) => None,
                Err(error) => Some(format!("Ignored signed checkpoint: {error}")),
            }
        }
        zoka::network::message::ZokaMessage::TeamAdvertise(advertisement) => {
            let Some(source) = source else {
                return Some(
                    "Ignored mining-team advertisement without a signed source peer.".to_string(),
                );
            };
            match team_mining_runtime
                .observe_advertisement(advertisement.clone(), source.to_string(), now_secs())
                .await
            {
                Ok(()) => Some(format!("Mining team {} is online.", advertisement.team_id)),
                Err(error) => Some(format!(
                    "Ignored mining team {}: {error}",
                    advertisement.team_id
                )),
            }
        }
        zoka::network::message::ZokaMessage::TeamJoin(membership) => {
            let Some(source) = source else {
                return Some(
                    "Ignored mining-team membership request without a signed source peer."
                        .to_string(),
                );
            };
            match team_mining_runtime
                .ingest_membership(membership.clone(), source.to_string(), now_secs())
                .await
            {
                Ok(()) => Some(format!(
                    "Mining-team membership updated for {}.",
                    membership.member_address
                )),
                Err(error) => Some(format!("Ignored mining-team membership request: {error}")),
            }
        }
        zoka::network::message::ZokaMessage::ShareSubmit(share) => {
            let Some(source) = source else {
                return Some("Ignored mining share without a signed source peer.".to_string());
            };
            match team_mining_runtime
                .ingest_share(share.clone(), source.to_string(), now_secs())
                .await
            {
                Ok(validated) => Some(format!(
                    "Accepted mining share from {} ({} leading bits{}).",
                    share.miner_address,
                    validated.actual_difficulty_bits,
                    if validated.network_solution {
                        ", full solution"
                    } else {
                        ""
                    }
                )),
                Err(error) => Some(format!("Ignored mining share: {error}")),
            }
        }
        zoka::network::message::ZokaMessage::TurnUpdate(update) => {
            let Some(source) = source else {
                return Some(
                    "Ignored mining-team turn update without a signed source peer.".to_string(),
                );
            };
            match team_mining_runtime
                .ingest_turn_update(update.clone(), source.to_string(), now_secs())
                .await
            {
                Ok(()) => Some(format!(
                    "Mining team {} state synchronized.",
                    update.state.team_id
                )),
                Err(error) => Some(format!("Ignored mining-team turn update: {error}")),
            }
        }
        zoka::network::message::ZokaMessage::BlockResponse(response) => {
            let source_peer = source.as_ref().map(ToString::to_string);
            let verdict = handle_inbound_block_message(
                node,
                message,
                source,
                chain_id,
                storage,
                peer_view,
                unsafe_rehearsal_difficulty_bits,
            );
            let canonical = storage
                .and_then(|storage| zoka::state::chain::read_chain_head(storage).ok().flatten())
                .is_some_and(|head| {
                    head.height == response.block_height && head.hash == response.block_hash
                });
            if canonical {
                if let Ok(block) =
                    serde_json::from_slice::<zoka::types::Block>(&response.encrypted_block)
                {
                    if let Some(recipient) = block
                        .transactions
                        .first()
                        .map(|coinbase| coinbase.receiver.as_str())
                    {
                        let status = team_mining_runtime.status().await;
                        let tracks_template = status.team.as_ref().is_some_and(|team| {
                            team.coordinator == status.local_address
                                && team.active_template.as_ref().is_some_and(|template| {
                                    template.height == block.header.block_height
                                        && template.prev_hash == block.header.prev_hash
                                })
                        });
                        if tracks_template {
                            if let Err(error) = team_mining_runtime
                                .record_confirmed_block(
                                    block.header.block_height,
                                    &block.header.prev_hash,
                                    recipient,
                                    source_peer,
                                    now_secs(),
                                )
                                .await
                            {
                                return Some(format!(
                                    "{} Team turn accounting warning: {error}.",
                                    verdict.unwrap_or_else(|| {
                                        "Canonical peer block applied.".to_string()
                                    })
                                ));
                            }
                        }
                    }
                }
            }
            verdict
        }
        zoka::network::message::ZokaMessage::BlockAnnounce(announce) => {
            handle_block_announce(node, announce, source, storage, peer_view)
        }
        zoka::network::message::ZokaMessage::PrivateTxAnnounce(announce) => {
            let hash = announce.tx_hash.trim();
            let Some(path) = chain_db_path else {
                return Some(format!(
                    "Private tx announce {} received but no chain DB is configured.",
                    truncate_dashboard_text(hash, 16)
                ));
            };
            let storage = match RocksDbStorage::new(path) {
                Ok(storage) => storage,
                Err(err) => {
                    return Some(format!(
                        "Private tx announce {} ignored: storage open failed ({err})",
                        truncate_dashboard_text(hash, 16)
                    ))
                }
            };
            match storage.get_private_transaction_bytes(hash) {
                Ok(Some(_)) => match storage.save_pending_private_transaction_hash(hash) {
                    Ok(()) => Some(format!(
                        "Private tx announce {} matched local body; marked pending for miner.",
                        truncate_dashboard_text(hash, 16)
                    )),
                    Err(err) => Some(format!(
                        "Private tx announce {} matched local body but pending-marker failed: {err}",
                        truncate_dashboard_text(hash, 16)
                    )),
                },
                Ok(None) => {
                    // Un nodo atrasado no gasta el canal en cuerpos que no
                    // puede usar.
                    //
                    // Este fetch existe para que el MINERO local pueda incluir
                    // la tx en un bloque. Pero un nodo atrasado no mina --
                    // `ready_to_mine` es false justamente hasta alcanzar la
                    // punta -- asi que el cuerpo no tiene a quien servirle
                    // todavia. Mientras tanto compite por el mismo camino
                    // directo y los mismos circuitos Tor que los bloques que
                    // si hacen falta: hasta 3 reintentos de 4s cada uno, por
                    // anuncio.
                    //
                    // Medido en la PC con ~250 bloques de atraso: 2.585
                    // eventos de tx privada en el log mientras la latencia de
                    // los pedidos de bloque subia de 2.180 ms a 6.020 ms.
                    //
                    // No se pierde nada: el anuncio queda registrado, y la tx
                    // llega igual dentro del bloque que la incluya cuando la
                    // cadena se ponga al dia. Si el nodo esta cerca de la
                    // punta -- el caso normal -- el fetch corre como siempre.
                    let local_height = storage.get_chain_height().unwrap_or(0);
                    let atraso = peer_view
                        .perceived_max_height
                        .saturating_sub(local_height);
                    if atraso > PRIVATE_TX_FETCH_MAX_LAG {
                        return Some(format!(
                            "Private tx announce {} diferido: el nodo esta {atraso} bloques \
                             atrasado y el canal se reserva para sincronizar.",
                            truncate_dashboard_text(hash, 16)
                        ));
                    }
                    drop(storage);
                    zoka::api::global_network_runtime()
                        .record_private_tx_announced_without_body();
                    let fetch_started_at = std::time::Instant::now();
                    match node
                        .request_private_tx_body_with_retry(hash, 3, Duration::from_secs(4))
                        .await
                    {
                        Ok(body) => {
                            if fetch_started_at.elapsed() > Duration::from_millis(2000) {
                                zoka::api::global_network_runtime()
                                    .record_private_tx_body_received_late();
                            }
                            let tx: zoka::private_wallet::PrivateTransaction =
                                match serde_json::from_slice(&body) {
                                    Ok(tx) => tx,
                                    Err(err) => {
                                        return Some(format!(
                                            "Fetched private tx {} but decode failed: {err}",
                                            truncate_dashboard_text(hash, 16)
                                        ))
                                    }
                                };
                            if tx.tx_hash.trim() != hash {
                                return Some(format!(
                                    "Fetched private tx body hash mismatch: expected {}, got {}.",
                                    truncate_dashboard_text(hash, 16),
                                    truncate_dashboard_text(&tx.tx_hash, 16)
                                ));
                            }
                            if let Err(err) = validate_private_tx_for_pending_pool(&tx) {
                                return Some(format!(
                                    "Fetched private tx {} but validation failed: {err}",
                                    truncate_dashboard_text(hash, 16)
                                ));
                            }
                            node.cache_private_tx_body(hash, body.clone());
                            let storage = match RocksDbStorage::new(path) {
                                Ok(storage) => storage,
                                Err(err) => {
                                    return Some(format!(
                                        "Fetched private tx {} but storage reopen failed: {err}",
                                        truncate_dashboard_text(hash, 16)
                                    ))
                                }
                            };
                            if let Err(err) = storage.save_private_transaction_bytes(hash, &body) {
                                return Some(format!(
                                    "Fetched private tx {} but persist failed: {err}",
                                    truncate_dashboard_text(hash, 16)
                                ));
                            }
                            if let Err(err) = storage.save_pending_private_transaction_hash(hash) {
                                return Some(format!(
                                    "Fetched private tx {} but pending-marker failed: {err}",
                                    truncate_dashboard_text(hash, 16)
                                ));
                            }
                            Some(format!(
                                "Fetched private tx {} by direct request/response and marked pending.",
                                truncate_dashboard_text(hash, 16)
                            ))
                        }
                        Err(err) => {
                            node.penalize_private_tx_announcers(hash);
                            Some(format!(
                                "Private tx announce {} received but direct body fetch failed: {err}",
                                truncate_dashboard_text(hash, 16)
                            ))
                        }
                    }
                }
                Err(err) => Some(format!(
                    "Private tx announce {} ignored: body lookup failed ({err})",
                    truncate_dashboard_text(hash, 16)
                )),
            }
        }
        zoka::network::message::ZokaMessage::SyncState(sync) => {
            // A directly-connected peer announcing its chain head is a
            // legitimate IBD sync target, so treat it like a trusted height
            // (no local-relative MAX_PENDING_BLOCK_AHEAD cap). The capped
            // observe() is meant for untrusted one-off block announces; using
            // it here meant a fresh node more than 512 blocks behind could
            // never learn the real head over P2P and crawled one block at a
            // time. Block payloads are still fully validated before commit, so
            // a dishonest height only wastes bounded fetch attempts. (This
            // used to open a throwaway RocksDbStorage just to read local
            // height for the cap; the trusted path needs neither.)
            if let Some(source) = source {
                peer_view.observe_trusted_height_from(source, sync.last_height);
            } else {
                peer_view.observe_trusted_height(sync.last_height);
            }
            Some(format!(
                "Observed peer sync state at height {}.",
                sync.last_height
            ))
        }
        zoka::network::message::ZokaMessage::BlockRequest {
            block_height,
            block_hash,
        } => {
            let Some(path) = chain_db_path else {
                return Some(format!(
                    "Ignored block request for height {block_height}: no chain DB configured"
                ));
            };
            let storage = match RocksDbStorage::new(path) {
                Ok(s) => s,
                Err(err) => {
                    return Some(format!(
                        "Ignored block request for height {block_height}: storage open failed ({err})"
                    ));
                }
            };
            let block = match block_hash.as_deref() {
                Some(hash) => match storage.get_block(hash) {
                    Ok(Some(block)) if block.header.block_height == *block_height => block,
                    Ok(Some(_)) => {
                        return Some(format!(
                            "Ignored block request for height {block_height}: requested hash is at a different height"
                        ));
                    }
                    Ok(None) => {
                        return Some(format!(
                            "Ignored block request for height {block_height}: requested hash not found"
                        ));
                    }
                    Err(err) => {
                        return Some(format!(
                            "Ignored block request for height {block_height}: read hash failed ({err})"
                        ));
                    }
                },
                None => match storage.get_canonical_block_by_height(*block_height) {
                    Ok(Some(block)) => block,
                    Ok(None) => {
                        return Some(format!(
                            "Ignored block request for height {block_height}: canonical block not found"
                        ));
                    }
                    Err(err) => {
                        return Some(format!(
                            "Ignored block request for height {block_height}: canonical read failed ({err})"
                        ));
                    }
                },
            };
            let payload = match serde_json::to_vec(&block) {
                Ok(bytes) => bytes,
                Err(err) => {
                    return Some(format!(
                        "Ignored block request for height {block_height}: encode failed ({err})"
                    ));
                }
            };
            let bodies = collect_private_tx_bodies_from_storage(&storage, &block);
            match node.publish_block_response_with_bodies(
                block.header.block_height,
                block.header.hash,
                payload,
                bodies,
            ) {
                Ok(()) => Some(format!(
                    "Served block request for height {block_height} from local chain DB."
                )),
                Err(err) => Some(format!(
                    "Failed to serve block request for height {block_height}: {err}"
                )),
            }
        }
        #[cfg(feature = "quantic")]
        zoka::network::message::ZokaMessage::QuanticTransaction { tx_hex } => {
            zoka::quantic::runtime::accept_network_transaction(storage?, chain_id, tx_hex, source)
        }
        #[cfg(feature = "quantic")]
        zoka::network::message::ZokaMessage::QuanticAddressRecord { address, stamp } => {
            zoka::quantic::directory::accept_network_record(
                storage?,
                chain_id,
                zoka::quantic::directory::AddressRecord {
                    address: address.clone(),
                    stamp: *stamp,
                },
                source,
            )
        }
        zoka::network::message::ZokaMessage::PrivateTransaction(message_tx) => {
            // Persist the gossiped private tx body + mark it pending in
            // local storage so the local miner picks it up at its next
            // block-assembly cycle. Without this hop the tx propagates
            // through the gossip layer but never enters any miner's pool.
            // Bodies that are already on disk (we sent them ourselves, or
            // got them via a different route) are no-ops by design.
            let Some(path) = chain_db_path else {
                return None;
            };
            let storage = match RocksDbStorage::new(path) {
                Ok(s) => s,
                Err(err) => {
                    return Some(format!(
                        "Ignored gossiped private tx: storage open failed ({err})"
                    ))
                }
            };
            let tx_value = match serde_json::to_value(message_tx) {
                Ok(v) => v,
                Err(err) => {
                    return Some(format!(
                        "Ignored gossiped private tx: serialize failed ({err})"
                    ))
                }
            };
            let tx: zoka::private_wallet::PrivateTransaction =
                match serde_json::from_value(tx_value.clone()) {
                    Ok(t) => t,
                    Err(err) => {
                        return Some(format!(
                            "Ignored gossiped private tx: deserialize as wallet tx failed ({err})"
                        ))
                    }
                };
            let hash = tx.tx_hash.trim();
            if hash.is_empty() {
                return Some("Ignored gossiped private tx: missing tx_hash".to_string());
            }
            // Skip if we already have this body — avoids overwriting a
            // copy we built ourselves with a peer-supplied one.
            if matches!(storage.get_private_transaction_bytes(hash), Ok(Some(_))) {
                return Some(format!(
                    "Gossiped private tx {} already on disk; ignoring duplicate.",
                    truncate_dashboard_text(hash, 16)
                ));
            }
            if let Err(err) = validate_private_tx_for_pending_pool(&tx) {
                return Some(format!(
                    "Ignored gossiped private tx {}: {err}",
                    truncate_dashboard_text(hash, 16)
                ));
            }
            let body = match serde_json::to_vec(&tx) {
                Ok(b) => b,
                Err(err) => {
                    return Some(format!(
                        "Ignored gossiped private tx {}: encode failed ({err})",
                        truncate_dashboard_text(hash, 16)
                    ))
                }
            };
            node.cache_private_tx_body(hash, body.clone());
            if let Err(err) = storage.save_private_transaction_bytes(hash, &body) {
                return Some(format!(
                    "Failed to persist gossiped private tx {}: {err}",
                    truncate_dashboard_text(hash, 16)
                ));
            }
            // Always mark pending — the miner's block-assembly path
            // dedupes against the canonical chain when including, so a
            // stale pending marker for an already-confirmed tx is a
            // harmless no-op rather than a double-spend risk.
            if let Err(err) = storage.save_pending_private_transaction_hash(hash) {
                return Some(format!(
                    "Persisted gossiped private tx {} but pending-marker failed: {err}",
                    truncate_dashboard_text(hash, 16)
                ));
            }
            let announce = zoka::network::message::PrivateTxAnnounceMessage::from_private_tx(
                message_tx,
                Some(body.len() as u64),
            );
            let announce_status = match node.publish_message(
                &zoka::network::message::ZokaMessage::PrivateTxAnnounce(announce),
            ) {
                Ok(()) => " availability announced",
                Err(_) => " availability announce failed",
            };
            let _ = chain_id; // chain_id retained for future per-chain dedupe
            Some(format!(
                "Imported gossiped private tx {} into local pool (marked pending;{}).",
                truncate_dashboard_text(hash, 16),
                announce_status
            ))
        }
        _ => None,
    }
}

async fn publish_team_mining_messages(
    node: &mut Node,
    runtime: &Arc<zoka::team_mining::TeamMiningRuntime>,
) -> Result<usize, String> {
    runtime
        .tick(now_secs())
        .await
        .map_err(|error| format!("team-mining tick failed: {error}"))?;
    let outbound = runtime.drain_outbound_messages().await;
    let mut published = 0usize;
    let mut outbound = outbound.into_iter();
    while let Some(message) = outbound.next() {
        if let Err(error) = node.publish_message(&message) {
            runtime.queue_message(message).await;
            for pending in outbound {
                runtime.queue_message(pending).await;
            }
            return Err(format!("team-mining gossip failed: {error}"));
        }
        published = published.saturating_add(1);
    }
    Ok(published)
}

async fn current_team_mining_template(
    runtime: &Arc<zoka::team_mining::TeamMiningRuntime>,
    chain_id: &str,
    storage: Option<&RocksDbStorage>,
    unsafe_rehearsal_difficulty_bits: Option<u32>,
) -> Result<Option<zoka::team_mining::MiningTemplate>, String> {
    let status = runtime.status().await;
    if !status.enabled {
        return Ok(None);
    }
    let Some(team) = status.team else {
        return Ok(None);
    };
    let local_is_active = team
        .members
        .get(&status.local_address)
        .is_some_and(|member| member.active && !member.expelled);
    if !local_is_active {
        return Ok(None);
    }
    if team.coordinator != status.local_address {
        return Ok(team.active_template);
    }
    let storage = storage
        .ok_or_else(|| "team mining requires the fullnode canonical chain database".to_string())?;
    let head = zoka::state::chain::read_chain_head(storage)
        .map_err(|error| format!("team mining failed to read chain head: {error}"))?;
    let (height, prev_hash) = match head {
        Some(head) => (head.height.saturating_add(1), head.hash),
        None => (1, genesis_pow_hash_for_chain(chain_id)),
    };
    let difficulty_bits = match unsafe_rehearsal_difficulty_bits {
        Some(bits) => bits,
        None => zoka::mining::difficulty::expected_for_next_block(storage)
            .map_err(|error| format!("team mining failed to read difficulty: {error}"))?,
    };
    if let Some(template) = team.active_template {
        if template.chain_id == chain_id
            && template.height == height
            && template.prev_hash == prev_hash
            && template.network_difficulty_bits == difficulty_bits
            && template.validate_id()
        {
            return Ok(Some(template));
        }
    }
    runtime
        .set_template(
            chain_id.to_string(),
            prev_hash,
            height,
            now_secs(),
            difficulty_bits,
        )
        .await
        .map(Some)
        .map_err(|error| format!("team mining failed to publish template: {error}"))
}

fn publish_local_sync_state(
    node: &mut Node,
    // Reuse the node's already-open chain storage. Opening a fresh
    // RocksDbStorage here per publish (the previous behaviour) was expensive
    // and could transiently fail against the primary's lock -- and when it
    // failed the node silently skipped announcing its height. That broke peer
    // height discovery: a syncing peer never learned this node's real tip, so
    // it treated the last block it happened to fetch as the chain head and
    // stopped IBD early (observed: a fresh node stalling at height ~15 with
    // target_height=15 while the server sat at 3126).
    storage: Option<&RocksDbStorage>,
) -> Result<Option<String>, String> {
    let Some(storage) = storage else {
        return Ok(None);
    };
    let head = match zoka::state::chain::read_chain_head(storage) {
        Ok(head) => head,
        Err(err) => {
            return Ok(Some(format!(
                "Skipped local sync-state publish: read head failed ({err})"
            )));
        }
    };
    let Some(head) = head else {
        return Ok(None);
    };
    match node.publish_sync_state(head.height, head.hash.clone()) {
        Ok(()) => Ok(Some(format!(
            "Published local sync state at height {}.",
            head.height
        ))),
        Err(err) => Ok(Some(format!(
            "Skipped local sync-state publish at height {}: {}",
            head.height, err
        ))),
    }
}

fn publish_chain_head_block_announce(
    node: &mut Node,
    chain_db_path: Option<&str>,
) -> Result<Option<String>, String> {
    let Some(path) = chain_db_path else {
        return Ok(None);
    };
    let storage = match RocksDbStorage::new(path) {
        Ok(storage) => storage,
        Err(err) => {
            return Ok(Some(format!(
                "Skipped block announce publish: storage open failed ({err})"
            )));
        }
    };
    let head = match zoka::state::chain::read_chain_head(&storage) {
        Ok(Some(head)) => head,
        Ok(None) => return Ok(None),
        Err(err) => {
            return Ok(Some(format!(
                "Skipped block announce publish: read head failed ({err})"
            )));
        }
    };
    match node.publish_block_announce(
        head.height,
        head.hash.clone(),
        head.cumulative_work.to_string(),
        None,
    ) {
        Ok(()) => Ok(Some(format!(
            "Published block announce at height {}.",
            head.height
        ))),
        Err(err) => Ok(Some(format!(
            "Skipped block announce at height {}: {}",
            head.height, err
        ))),
    }
}

fn publish_recent_chain_blocks(
    node: &mut Node,
    chain_db_path: Option<&str>,
    max_blocks: u64,
) -> Result<Option<String>, String> {
    if max_blocks == 0 || node.connected_peer_count() == 0 {
        return Ok(None);
    }
    let Some(path) = chain_db_path else {
        return Ok(None);
    };
    let storage = match RocksDbStorage::new(path) {
        Ok(storage) => storage,
        Err(err) => {
            return Ok(Some(format!(
                "Skipped recent block rebroadcast: storage open failed ({err})"
            )));
        }
    };
    let head = match storage.get_chain_height() {
        Ok(height) => height,
        Err(err) => {
            return Ok(Some(format!(
                "Skipped recent block rebroadcast: read height failed ({err})"
            )));
        }
    };
    if head == 0 {
        return Ok(None);
    }

    let start = head.saturating_sub(max_blocks.saturating_sub(1)).max(1);
    let mut published = 0u64;
    let mut failures = 0u64;
    for height in start..=head {
        let block = match storage.get_block_by_height(height) {
            Ok(Some(block)) => block,
            Ok(None) => {
                failures = failures.saturating_add(1);
                continue;
            }
            Err(_) => {
                failures = failures.saturating_add(1);
                continue;
            }
        };
        let payload = match serde_json::to_vec(&block) {
            Ok(bytes) => bytes,
            Err(_) => {
                failures = failures.saturating_add(1);
                continue;
            }
        };
        let bodies = collect_private_tx_bodies_from_storage(&storage, &block);
        match node.publish_block_response_with_bodies(
            block.header.block_height,
            block.header.hash,
            payload,
            bodies,
        ) {
            Ok(()) => published = published.saturating_add(1),
            Err(_) => failures = failures.saturating_add(1),
        }
    }

    let event = if failures == 0 {
        format!("Rebroadcast {published} recent block(s) through height {head}.")
    } else {
        format!(
            "Rebroadcast {published} recent block(s) through height {head} ({failures} failure(s))."
        )
    };
    Ok(Some(event))
}

fn post_block_to_rpc_targets(block: &zoka::types::Block, targets: &[&str]) -> (u64, u64) {
    if targets.is_empty() {
        return (0, 0);
    }
    let body = match serde_json::to_value(block) {
        Ok(value) => value,
        Err(_) => return (0, targets.len() as u64),
    };
    let mut submitted = 0u64;
    let mut failures = 0u64;
    for rpc in targets {
        match rpc_http_post_json(rpc, "/sync/block", &body, 5) {
            Ok(_) => submitted = submitted.saturating_add(1),
            Err(_) => failures = failures.saturating_add(1),
        }
    }
    (submitted, failures)
}

fn post_recent_chain_blocks_to_rpcs(
    chain_db_path: Option<&str>,
    rpc_endpoints: &[String],
    max_blocks: u64,
) -> Result<Option<String>, String> {
    if max_blocks == 0 || rpc_endpoints.is_empty() {
        return Ok(None);
    }
    let Some(path) = chain_db_path else {
        return Ok(None);
    };
    let storage = match RocksDbStorage::new(path) {
        Ok(storage) => storage,
        Err(err) => {
            return Ok(Some(format!(
                "Skipped RPC block autopush retry: storage open failed ({err})"
            )));
        }
    };
    let head = match storage.get_chain_height() {
        Ok(height) => height,
        Err(err) => {
            return Ok(Some(format!(
                "Skipped RPC block autopush retry: read height failed ({err})"
            )));
        }
    };
    if head == 0 {
        return Ok(None);
    }

    let (samples, _) = sample_rpc_heights(rpc_endpoints, 3);
    let rpc_heights = samples
        .into_iter()
        .map(|sample| (sample.rpc, sample.height))
        .collect::<HashMap<_, _>>();
    let start = head.saturating_sub(max_blocks.saturating_sub(1)).max(1);
    let mut submitted = 0u64;
    let mut failures = 0u64;
    let mut skipped = 0u64;

    for height in start..=head {
        let targets = rpc_endpoints
            .iter()
            .filter_map(|rpc| {
                rpc_heights
                    .get(rpc.as_str())
                    .copied()
                    .filter(|remote_height| *remote_height < height)
                    .map(|_| rpc.as_str())
            })
            .collect::<Vec<_>>();
        if targets.is_empty() {
            continue;
        }
        let block = match storage.get_block_by_height(height) {
            Ok(Some(block)) => block,
            Ok(None) => {
                skipped = skipped.saturating_add(1);
                continue;
            }
            Err(_) => {
                skipped = skipped.saturating_add(1);
                continue;
            }
        };
        let (ok, failed) = post_block_to_rpc_targets(&block, &targets);
        submitted = submitted.saturating_add(ok);
        failures = failures.saturating_add(failed);
    }

    if submitted == 0 && failures == 0 && skipped == 0 {
        return Ok(None);
    }
    let event = if failures == 0 && skipped == 0 {
        format!("RPC block autopush retry submitted {submitted} block delivery attempt(s).")
    } else {
        format!(
            "RPC block autopush retry submitted {submitted} block delivery attempt(s) ({failures} failure(s), {skipped} skipped block(s))."
        )
    };
    Ok(Some(event))
}

fn append_unique_peer_addrs(
    target: &mut Vec<String>,
    addrs: impl IntoIterator<Item = String>,
) -> usize {
    let mut known = target.iter().cloned().collect::<HashSet<_>>();
    let mut added = 0usize;
    for addr in addrs {
        let addr = addr.trim().to_string();
        if addr.is_empty() || !known.insert(addr.clone()) {
            continue;
        }
        target.push(addr);
        added += 1;
    }
    added
}

/// Orden en que se intentan las direcciones al reconectar.
///
/// Se baraja para no golpear siempre a la misma, pero **primero se descartan
/// las propias**: marcar la dirección de uno mismo falla siempre con "tried to
/// dial local peer id", y como el presupuesto es de dos intentos por barrido,
/// una dirección propia se lleva la mitad de la ronda sin ninguna posibilidad.
/// Un nodo con onion publicada la tiene en su propio manifiesto, así que el
/// caso es la norma y no la excepción.
fn shuffled_peer_addrs_excluding(seed_addrs: &[String], propias: &[String]) -> Vec<String> {
    let mut shuffled = seed_addrs
        .iter()
        .filter(|addr| {
            !propias
                .iter()
                .any(|propia| direcciones_equivalentes(addr, propia))
        })
        .cloned()
        .collect::<Vec<_>>();
    shuffled.shuffle(&mut rand::thread_rng());
    shuffled
}

/// Dos multiaddr apuntan al mismo sitio.
///
/// Comparación textual tras normalizar: la misma onion puede venir del
/// manifiesto y de la línea de comandos con distinto formato de sufijo, y
/// bastaría esa diferencia para que el filtro no la reconociera.
fn direcciones_equivalentes(a: &str, b: &str) -> bool {
    fn nucleo(addr: &str) -> String {
        addr.trim()
            .trim_end_matches('/')
            .to_ascii_lowercase()
            .replace("/p2p/", "|")
            .split('|')
            .next()
            .unwrap_or("")
            .to_string()
    }
    !a.trim().is_empty() && nucleo(a) == nucleo(b)
}

/// Orden de marcado que respeta la salud que el peerstore ya calculó.
///
/// `PeerStore::dial_addrs` ordena por score, última conexión exitosa y fallos
/// — es decir, ya sabe cuáles direcciones sirven. Ese orden se estaba tirando:
/// la lista se barajaba entera, así que una semilla muerta con seis fallos y
/// score −100 salía sorteada tan seguido como un par sano de score 100. Con un
/// presupuesto de dos intentos por barrido, eso es media ronda regalada.
///
/// El barajado igual hace falta: sin él, dos nodos con el mismo peerstore
/// marcarían siempre en el mismo orden y se amontonarían sobre la misma
/// dirección. La solución es barajar **dentro** de cada nivel de salud, no
/// entre niveles.
///
/// Una dirección que el peerstore no conoce —una semilla recién agregada a
/// mano— se coloca en el medio: no se la premia por encima de un par probado,
/// pero tampoco se la castiga como a uno que ya falló.
fn ranked_dial_candidates(
    seed_addrs: &[String],
    own_addrs: &[String],
    healthiest_first: &[String],
) -> Vec<String> {
    let candidatos = shuffled_peer_addrs_excluding(seed_addrs, own_addrs);
    let desconocida = healthiest_first.len() / 2;
    let rango = |addr: &String| {
        healthiest_first
            .iter()
            .position(|conocida| direcciones_equivalentes(addr, conocida))
            .unwrap_or(desconocida)
    };
    let mut ordenados = candidatos;
    // Estable sobre una lista ya barajada: el orden dentro de cada nivel de
    // salud queda al azar, y entre niveles manda la salud.
    ordenados.sort_by_key(rango);
    ordenados
}

fn shuffled_peer_addrs(seed_addrs: &[String]) -> Vec<String> {
    shuffled_peer_addrs_excluding(seed_addrs, &[])
}

fn reconnect_delay_with_jitter(
    connected_peers: usize,
    target_peers: usize,
    failure_streak: u32,
) -> Duration {
    let base_secs = if connected_peers == 0 {
        2u64
    } else if connected_peers < target_peers {
        5u64
    } else {
        30u64
    };
    let backoff_multiplier = 1u64 << failure_streak.min(4);
    let backoff_secs = base_secs.saturating_mul(backoff_multiplier).min(60);
    let jitter_ms = rand::thread_rng().gen_range(250..=2_500);
    Duration::from_secs(backoff_secs) + Duration::from_millis(jitter_ms)
}

fn persist_observed_peerstore(
    node: &Node,
    peer_store: &mut zoka::network::peer_store::PeerStore,
) -> Result<usize, String> {
    let observed = node.observed_peer_records();
    if observed.is_empty() {
        return Ok(0);
    }
    let count = observed.len();
    for record in observed {
        peer_store.mark_identified(
            &record.addr,
            Some(&record.peer_id),
            record.protocol_version.as_deref(),
            &record.protocols,
            "identify",
        );
    }
    peer_store.save().map_err(|e| {
        format!(
            "failed to save peerstore {}: {e}",
            peer_store.path().display()
        )
    })?;
    Ok(count)
}

async fn publish_runtime_sync_status(
    node: &Node,
    storage: Option<&Arc<RocksDbStorage>>,
    peer_view: &zoka::state::sync::PeerChainView,
    min_peers: usize,
    rpc_fallback_active: bool,
    last_sync_source: Option<&str>,
    last_sync_event: Option<&str>,
    last_sync_error: Option<&str>,
) {
    let local_height = storage
        .and_then(|storage| storage.get_chain_height().ok())
        .unwrap_or(0);
    let target_height = peer_view.perceived_max_height.max(local_height);
    let connected_peers = node.connected_peer_count();
    let mode = if peer_view.has_active_branch_backfill() {
        "backfill"
    } else if target_height > local_height {
        "syncing"
    } else if connected_peers < min_peers {
        "isolated"
    } else {
        "caught_up"
    };
    zoka::api::global_network_runtime()
        .set_sync_status(zoka::api::SyncRuntimeUpdate {
            mode: mode.to_string(),
            local_height,
            target_height,
            min_peers: min_peers as u64,
            rpc_fallback_active,
            last_sync_source: last_sync_source.map(str::to_string),
            last_sync_event: last_sync_event.map(str::to_string),
            last_sync_error: last_sync_error.map(str::to_string),
        })
        .await;
}

fn remember_sync_event(
    source: &str,
    event: &str,
    last_sync_source: &mut Option<String>,
    last_sync_event: &mut Option<String>,
    last_sync_error: &mut Option<String>,
    rpc_fallback_active: &mut bool,
) {
    *last_sync_source = Some(source.to_string());
    *last_sync_event = Some(event.to_string());
    *rpc_fallback_active = source == "rpc";

    let lowered = event.to_ascii_lowercase();
    if lowered.contains("failed")
        || lowered.contains("error")
        || lowered.contains("rejected")
        || lowered.contains("unavailable")
        || lowered.contains("timeout")
        || lowered.contains("pending")
    {
        *last_sync_error = Some(event.to_string());
    } else {
        *last_sync_error = None;
    }
}

fn sync_outcome_to_event(outcome: zoka::state::sync::SyncOutcome) -> String {
    match outcome {
        zoka::state::sync::SyncOutcome::Applied(head) => format!(
            "Accepted peer block @ height {} (chain head now @ {} / {} cumulative work)",
            head.height, head.height, head.cumulative_work
        ),
        zoka::state::sync::SyncOutcome::AlreadyKnown { height, .. } => {
            format!("Peer block @ height {height} already known; ignored")
        }
        zoka::state::sync::SyncOutcome::GapAhead {
            local_head,
            candidate,
            ..
        } => format!(
            "Peer is ahead at height {candidate}; local head is {local_head}. IBD will request missing blocks."
        ),
        zoka::state::sync::SyncOutcome::BehindOrSibling {
            local_head,
            candidate,
            ..
        } => {
            format!("Ignored peer block @ height {candidate}; local head is already {local_head}.")
        }
        zoka::state::sync::SyncOutcome::MissingParent {
            local_head,
            candidate,
            ..
        } => format!(
            "Peer block @ height {candidate} belongs to a branch with an unknown parent; local head is {local_head}. IBD will backfill the branch."
        ),
        zoka::state::sync::SyncOutcome::BadPayload(err) => {
            format!("Discarded peer block: payload not a valid Block ({err})")
        }
        zoka::state::sync::SyncOutcome::AnnouncedHashMismatch { announced, header } => {
            format!("Discarded peer block: announced hash {announced} != header hash {header}")
        }
        zoka::state::sync::SyncOutcome::Rejected(err) => {
            format!("Rejected peer block: {err}")
        }
    }
}

/// Fase 4 parallel IBD body download: fires several direct block-body
/// requests to different peers up front instead of `run_sync_step`'s
/// one-at-a-time wait/apply/wait cycle. Results are stashed into
/// `peer_view`'s pending-response cache; `run_sync_step`'s existing
/// `take_pending_block_response` picks them up in height order on
/// subsequent ticks, so the apply path itself is unchanged - this only
/// parallelizes the network fetch.
const IBD_PARALLEL_BODY_WORKERS: usize = 8;

/// Manda un `GetHeaders` cuando hay un paseo activo que todavia no lo pidio.
///
/// El paseo hacia atras descubria el ancestro comun de a un bloque: cada
/// body completo que bajaba solo revelaba el hash del padre, el dato que
/// hace falta para pedir el siguiente. Medido en la PC: una bifurcacion de
/// ~60 bloques tardo horas en resolverse asi, con una tasa de fallo de
/// pedidos directos del 74% sobre Tor.
///
/// Un `GetHeaders` devuelve hasta 512 alturas -- livianas, sin los bodies --
/// en una sola vuelta. Con eso alcanza para cubrir de punta a punta el
/// paseo mas largo que este nodo esta dispuesto a caminar
/// (`MAX_BACKWARD_WALK_DISTANCE` = 64) en un solo pedido.
///
/// Es una aceleracion, nunca un requisito: si no hay pares, si el pedido
/// falla, o si la respuesta no sirve, el paseo de a un bloque -- ya probado
/// en produccion -- sigue funcionando igual que antes. Esta funcion nunca
/// bloquea ese camino, solo intenta acortarlo.
async fn run_branch_header_probe_step(
    node: &mut zoka::network::p2p::Node,
    chain_db_path: Option<&str>,
    peer_view: &mut zoka::state::sync::PeerChainView,
) -> Result<Option<String>, String> {
    if !peer_view.needs_header_probe() {
        return Ok(None);
    }
    let Some(path) = chain_db_path else {
        return Ok(None);
    };
    let storage = match RocksDbStorage::new(path) {
        Ok(storage) => storage,
        Err(_) => return Ok(None),
    };
    let locator = zoka::state::sync::build_chain_locator(&storage);
    if locator.is_empty() {
        return Ok(None);
    }

    // Preferir al par que origino la rama -- ya sabemos que tiene la
    // ancestria -- pero cualquier par conectado puede responder un
    // GetHeaders: la respuesta se valida estructuralmente igual, y si no
    // conecta con lo que ya sabiamos simplemente se descarta.
    let connected = node.connected_peer_ids();
    if connected.is_empty() {
        return Ok(None);
    }
    let peer = peer_view
        .active_branch_source()
        .filter(|source| connected.contains(source))
        .unwrap_or(connected[0]);

    let request = zoka::network::header_request_response::GetHeaders {
        locator,
        max: 512,
        stop_hash: None,
    };
    match node.send_header_request(&peer, request) {
        Some(_) => {
            peer_view.mark_header_probe_sent();
            Ok(Some(format!(
                "Pedido de cabeceras enviado a {peer} para acelerar el paseo hacia atras."
            )))
        }
        None => {
            // El par elegido no esta conectado despues de todo (se cayo
            // entre el chequeo y el pedido); se reintenta en la proxima
            // vuelta con quien este disponible entonces.
            Ok(None)
        }
    }
}

/// Drena las `HeadersResponse` recibidas y, si hay un paseo activo, las
/// aplica para resolver el rango entero de una vez.
fn drain_header_responses(
    node: &mut zoka::network::p2p::Node,
    chain_db_path: Option<&str>,
    peer_view: &mut zoka::state::sync::PeerChainView,
) -> Result<Option<String>, String> {
    use zoka::network::header_request_response::HeadersResponse;
    use zoka::state::sync::HeaderChainOutcome;

    let mut last_event = None;
    while let Some(received) = node.take_received_headers_response() {
        let headers = match received.response {
            HeadersResponse::Headers {
                headers,
                common_ancestor,
            } => (headers, common_ancestor),
            HeadersResponse::NotFound { reason } => {
                last_event = Some(format!(
                    "Cabeceras no disponibles en {}: {reason}. Sigue el paseo de a un bloque.",
                    received.peer
                ));
                continue;
            }
        };
        let (headers, common_ancestor_hash) = headers;
        if common_ancestor_hash.is_empty() {
            last_event = Some(
                "El par no reconocio ningun hash del locator; sigue el paseo de a un bloque."
                    .to_string(),
            );
            continue;
        }
        let Some(path) = chain_db_path else {
            continue;
        };
        let storage = match RocksDbStorage::new(path) {
            Ok(storage) => storage,
            Err(_) => continue,
        };
        // El locator salio de nuestro propio storage, asi que el hash que el
        // par reconocio ya es un hash que tenemos: buscamos su altura ahi.
        let common_ancestor_height = match storage.get_block(&common_ancestor_hash) {
            Ok(Some(block)) => block.header.block_height,
            _ => {
                last_event = Some(
                    "El ancestro comun que devolvio el par no esta en nuestro storage; sigue \
                     el paseo de a un bloque."
                        .to_string(),
                );
                continue;
            }
        };
        let local_work = storage.get_cumulative_work().unwrap_or(0);

        let outcome = peer_view.apply_header_chain(
            common_ancestor_height,
            &common_ancestor_hash,
            &headers,
            local_work,
        );
        last_event = Some(match outcome {
            HeaderChainOutcome::Adopted {
                from_height,
                through_height,
            } => format!(
                "Cabeceras aplicadas: rango {from_height}..{through_height} resuelto de una \
                 vez (antes: un body por altura)."
            ),
            HeaderChainOutcome::NotHeavier => {
                "La rama competidora pesa menos que la local; paseo abandonado sin bajar \
                 bodies."
                    .to_string()
            }
            HeaderChainOutcome::Invalid(reason) => {
                format!("Cabeceras invalidas de {}: {reason}", received.peer)
            }
            HeaderChainOutcome::NotActive => continue,
        });
    }
    Ok(last_event)
}

/// Que alturas conviene bajar en paralelo, dado donde esta la cabeza local y
/// donde va el paseo hacia atras (si hay uno activo).
///
/// Devuelve `None` cuando no hay nada util que adelantar.
fn ibd_prefetch_window(
    local_height: u64,
    backfill_height: Option<u64>,
    perceived_max_height: u64,
    ventana: u64,
) -> Option<(u64, u64)> {
    if ventana == 0 || local_height >= perceived_max_height {
        return None;
    }
    let primera = local_height.saturating_add(1);
    match backfill_height {
        // El paseo ya esta pidiendo su altura actual; adelantamos las de
        // abajo, que son las que va a necesitar a continuacion.
        Some(paseo) if paseo > primera => {
            let tope = paseo.saturating_sub(1);
            Some((tope.saturating_sub(ventana - 1).max(primera), tope))
        }
        // El paseo esta por engancharse con la cabeza local: no queda nada
        // por debajo que adelantar.
        Some(_) => None,
        None => Some((
            primera,
            perceived_max_height.min(local_height.saturating_add(ventana)),
        )),
    }
}

async fn run_ibd_body_prefetch_step(
    node: &mut Node,
    chain_db_path: Option<&str>,
    peer_view: &mut zoka::state::sync::PeerChainView,
) -> Result<Option<String>, String> {
    let Some(path) = chain_db_path else {
        return Ok(None);
    };
    if node.connected_peer_count() == 0 {
        return Ok(None);
    }
    let storage = match RocksDbStorage::new(path) {
        Ok(storage) => storage,
        Err(_) => return Ok(None),
    };
    let local_height = storage
        .get_chain_height()
        .map_err(|err| format!("read chain height failed: {err}"))?;
    if local_height >= peer_view.perceived_max_height {
        return Ok(None);
    }
    // Que un nodo este atrasado no puede apagarle la descarga en paralelo.
    //
    // Este paso se salteaba entero mientras hubiera un branch backfill
    // activo. Pero backfill no significa "bifurcacion": arranca cada vez que
    // llega un bloque por encima de la cabeza local, o sea SIEMPRE que el
    // nodo va atrasado. El efecto medido: el prefetch corria cero veces y la
    // sincronizacion quedaba en un pedido por bloque, secuencial, ~50s cada
    // uno sobre Tor. Eso da ~1,2 bloques por minuto contra los ~2 que produce
    // la red: el nodo se atrasaba mas rapido de lo que avanzaba y no alcanzaba
    // la punta nunca. Un nodo estuvo 16 horas sin aplicar un solo bloque.
    //
    // Ahora la ventana acompaña al paseo hacia atras en vez de apagarse. El
    // paseo sigue siendo de a un bloque y sigue anclado por hash -- lo unico
    // que cambia es que encuentra el bloque ya descargado en vez de esperar
    // una vuelta de red.
    //
    // Se piden por altura, sin hash, porque el hash del abuelo todavia no se
    // conoce. Eso es seguro: el cache esta indexado por hash y el paseo lo
    // consulta con el hash que espera, asi que un bloque que no sea el de su
    // rama simplemente no se encuentra y se pide por red igual que hoy. En una
    // bifurcacion real degrada sola, sin aceptar nada que no aceptaria ahora.
    let Some((desde, hasta)) = ibd_prefetch_window(
        local_height,
        peer_view.branch_backfill_height,
        peer_view.perceived_max_height,
        IBD_PARALLEL_BODY_WORKERS as u64,
    ) else {
        return Ok(None);
    };
    let ceiling = hasta;
    let heights: Vec<u64> = (desde..=hasta)
        .filter(|h| !peer_view.has_pending_block_response(*h))
        .collect();
    if heights.len() < 2 {
        // Not enough of a window to make pipelining worthwhile; let
        // run_sync_step's normal single-height path handle it.
        return Ok(None);
    }
    // Presupuesto medido para Tor, no para clearnet.
    //
    // Estaba en 8s, y el protocolo de peticion/respuesta ya usa 30 por este
    // mismo motivo: un circuito Tor agrega varios segundos de ida y vuelta, y
    // cada peticion abre un substream nuevo. Con 8s, las respuestas llegaban
    // DESPUES de que la peticion venciera -- el contador de respuestas
    // recibidas subia de a miles mientras la altura local no se movia ni un
    // bloque. Descargar y descartar, en bucle.
    //
    // El comentario que documenta los 30s en la configuracion del behaviour
    // dice textualmente que con presupuestos de 8-15s "IBD block fetches would
    // time out and retry forever". Esta llamada se los volvia a imponer.
    let results = node
        .request_blocks_batch_direct(&heights, Duration::from_secs(30))
        .await;
    let requested = heights.len();
    let mut fetched = 0u64;
    for (_, result) in results {
        if let Ok(sourced) = result {
            peer_view.stash_prefetched_block_response_from(
                Some(sourced.source),
                local_height,
                sourced.response,
            );
            fetched = fetched.saturating_add(1);
        }
    }
    if fetched == 0 {
        return Ok(None);
    }
    Ok(Some(format!(
        "IBD prefetch: {fetched}/{requested} block bodies fetched in parallel for heights {desde}..{ceiling}"
    )))
}

/// Prefer syncing from the P2P peer mesh over centralized RPC endpoints
/// once there's a healthy-enough mesh to do it, so load distributes across
/// the network instead of concentrating on whichever RPC endpoints happen
/// to be configured (typically the operator's own seed nodes). Matches the
/// mesh_n_low=2 threshold already used to tune gossipsub for this network's
/// scale.
const MIN_PEERS_TO_PREFER_P2P_SYNC: usize = 2;

fn should_prefer_p2p_sync(node: &Node) -> bool {
    node.connected_peer_count() >= MIN_PEERS_TO_PREFER_P2P_SYNC
}

/// Cuánto espera el tanteo del bloque siguiente cuando ningún par anunció su
/// altura: un nodo completo que lo tiene contesta en segundos (también por
/// Tor), y una wallet nueva contesta al toque que no lo tiene.
const SPECULATIVE_PROBE_WAIT: Duration = Duration::from_secs(10);
/// Pausa después de un tanteo sin bloque. Un anuncio de altura no la espera.
const SPECULATIVE_PROBE_BACKOFF: Duration = Duration::from_secs(60);

async fn run_sync_step(
    node: &mut Node,
    chain_id: &str,
    chain_db_path: Option<&str>,
    peer_view: &mut zoka::state::sync::PeerChainView,
    unsafe_rehearsal_difficulty_bits: Option<u32>,
) -> Result<Option<String>, String> {
    let Some(path) = chain_db_path else {
        return Ok(None);
    };
    let storage = match RocksDbStorage::new(path) {
        Ok(storage) => storage,
        Err(err) => return Ok(Some(format!("IBD storage open failed ({err})"))),
    };
    let mut speculative = false;
    let next_request = match zoka::state::sync::next_required_block(&storage, peer_view)? {
        Some(request) => request,
        None if node.connected_peer_count() > 0
            && peer_view.perceived_max_height == 0
            && peer_view.speculative_probe_due() =>
        {
            speculative = true;
            let local_height = storage
                .get_chain_height()
                .map_err(|err| format!("read chain height failed: {err}"))?;
            // Speculatively probe for the next block when peers are connected but
            // no gossip height announcement has arrived yet (e.g. RPC is down).
            // If the peer doesn't have it we'll get a "pending" error and retry.
            zoka::state::sync::SyncRequest {
                height: local_height.saturating_add(1).max(1),
                expected_hash: None,
                preferred_peer: None,
                strict_peer_affinity: false,
            }
        }
        None => return Ok(None),
    };
    let next_height = next_request.height;

    let (response, response_peer, response_source) = if let Some((source, cached)) =
        peer_view.take_pending_sourced_block_response(&next_request)
    {
        (cached, source, "cached".to_string())
    } else if speculative {
        // Tanteo sin altura anunciada: solo directo, con plazo corto, y una
        // pausa si nadie lo tiene. Antes caía también al gossip y esperaba dos
        // plazos de 20 s en cada vuelta: con un teléfono como único par (no
        // guarda la cadena) el bucle entero, minería incluida, quedaba frenado.
        match node.probe_block_direct(next_height, SPECULATIVE_PROBE_WAIT).await {
            Ok(sourced) => {
                let source_label = format!("peer {}", sourced.source);
                (sourced.response, Some(sourced.source), source_label)
            }
            Err(err) => {
                peer_view.defer_speculative_probe(SPECULATIVE_PROBE_BACKOFF);
                return Ok(Some(format!(
                    "No connected peer has block {next_height} yet ({err}); probing again in {}s.",
                    SPECULATIVE_PROBE_BACKOFF.as_secs()
                )));
            }
        }
    } else {
        match node
            .request_block_with_affinity(
                next_height,
                next_request.expected_hash.clone(),
                next_request.preferred_peer,
                next_request.strict_peer_affinity,
                // Race every connected peer for this one missing height, not a
                // fixed 3 — with only 1-2 blocks left to fetch there is no
                // batching to parallelize across heights, so peer fan-out is
                // the only lever. `take(max_attempts)` inside the direct-request
                // path already bounds this to however many peers actually are
                // connected, so this never over-requests.
                node.connected_peer_count().max(3) as u32,
                // 20s/attempt (was 8s): sized for Tor onion round trips so IBD
                // block fetches don't spuriously time out and re-request the
                // same height forever. Bounded by the 30s request/response
                // behaviour timeout.
                Duration::from_secs(20),
            )
            .await
        {
            Ok(sourced) => {
                let source_label = format!("peer {}", sourced.source);
                (sourced.response, Some(sourced.source), source_label)
            }
            Err(err) => {
                if next_request.expected_hash.is_none() {
                    // Pedido hacia adelante que no llego. Si se repite, es la
                    // senal de que los pares podaron esta altura y recien ahi
                    // conviene caminar la ancestria hacia atras.
                    peer_view.record_forward_gap_failure();
                }
                let reset_note = if next_request.expected_hash.is_some()
                    && peer_view.record_branch_backfill_request_failure()
                {
                    " Cleared unavailable branch backfill after repeated misses; will retry from available peers."
                } else {
                    ""
                };
                tracing::warn!(
                    target: "zoka.sync",
                    height = next_height,
                    expected_hash = ?next_request.expected_hash,
                    error = %err,
                    "IBD block request pending"
                );
                return Ok(Some(format!(
                    "IBD request for block {next_height} pending: {err}.{reset_note}"
                )));
            }
        }
    };
    let outcome = match unsafe_rehearsal_difficulty_bits {
        Some(bits) => zoka::state::sync::try_apply_synced_block_payload_with_expected_difficulty(
            &storage, chain_id, &response, bits,
        ),
        None => zoka::state::sync::try_apply_synced_block_payload(&storage, chain_id, &response),
    };
    if matches!(
        &outcome,
        zoka::state::sync::SyncOutcome::BadPayload(_)
            | zoka::state::sync::SyncOutcome::AnnouncedHashMismatch { .. }
            | zoka::state::sync::SyncOutcome::Rejected(_)
    ) {
        if let Some(source) = response_peer {
            node.penalize_block_sync_peer(source);
        }
    }
    peer_view.record_sync_outcome_from(response_peer, &outcome);
    peer_view.remember_block_response_from(response_peer, &outcome, &response);
    if matches!(
        &outcome,
        zoka::state::sync::SyncOutcome::BadPayload(_)
            | zoka::state::sync::SyncOutcome::AnnouncedHashMismatch { .. }
            | zoka::state::sync::SyncOutcome::Rejected(_)
    ) {
        tracing::warn!(
            target: "zoka.sync",
            height = next_height,
            source = response_source.as_str(),
            outcome = ?outcome,
            "IBD block apply rejected"
        );
    }
    Ok(Some(format!(
        "IBD {response_source} block {next_height}: {}",
        sync_outcome_to_event(outcome)
    )))
}

fn fetch_rpc_block_response(
    rpc: &str,
    height: u64,
) -> Result<zoka::network::message::BlockResponseMessage, String> {
    let body = rpc_http_get_json(rpc, &format!("/blocks/{height}/raw"), 5)?;
    let difficulty_bits = rpc_difficulty_bits_from_json(&body).or_else(|| {
        rpc_http_get_json(rpc, &format!("/blocks/{height}"), 5)
            .ok()
            .and_then(|metadata| rpc_difficulty_bits_from_json(&metadata))
    });
    let block: zoka::types::Block = serde_json::from_value(body)
        .map_err(|e| format!("RPC block {height} decode failed: {e}"))?;
    block_to_response(block, difficulty_bits)
}

fn block_to_response(
    block: zoka::types::Block,
    difficulty_bits: Option<u32>,
) -> Result<zoka::network::message::BlockResponseMessage, String> {
    let block_hash = block.header.hash.clone();
    let block_height = block.header.block_height;
    let encrypted_block = serde_json::to_vec(&block)
        .map_err(|e| format!("RPC block {block_height} encode failed: {e}"))?;
    Ok(zoka::network::message::BlockResponseMessage {
        block_height,
        encrypted_block,
        block_hash,
        difficulty_bits,
        private_tx_bodies: Vec::new(),
        timestamp: now_secs(),
    })
}

fn rpc_difficulty_bits_from_json(body: &serde_json::Value) -> Option<u32> {
    body.get("difficulty_bits")
        .or_else(|| body.get("difficulty"))
        .and_then(|value| value.as_u64())
        .and_then(|value| u32::try_from(value).ok())
}

fn fetch_rpc_block_range(
    rpc: &str,
    from_height: u64,
    to_height: u64,
) -> Result<Vec<zoka::network::message::BlockResponseMessage>, String> {
    let body = rpc_http_get_json(
        rpc,
        &format!("/blocks/range/{from_height}/{to_height}/raw"),
        10,
    )?;
    let blocks = body
        .get("blocks")
        .and_then(|value| value.as_array())
        .ok_or_else(|| "RPC raw block range response did not include blocks".to_string())?;
    let mut responses = Vec::with_capacity(blocks.len());
    for value in blocks {
        let difficulty_bits = rpc_difficulty_bits_from_json(value).ok_or_else(|| {
            "RPC raw block range response did not include difficulty_bits; falling back to single-block sync"
                .to_string()
        })?;
        let block: zoka::types::Block = serde_json::from_value(value.clone())
            .map_err(|e| format!("RPC block range decode failed: {e}"))?;
        responses.push(block_to_response(block, Some(difficulty_bits))?);
    }
    Ok(responses)
}

#[derive(Debug, Clone)]
struct RpcHeightSample {
    rpc: String,
    height: u64,
}

fn rpc_chain_height(rpc: &str, timeout_secs: u64) -> Result<u64, String> {
    let height_body = rpc_http_get_json(rpc, "/chain/height", timeout_secs)?;
    rpc_json_u64(&height_body, &["height", "chain_height"])
        .ok_or_else(|| format!("{rpc}: height response did not include height"))
}

fn sample_rpc_heights(
    rpc_endpoints: &[String],
    timeout_secs: u64,
) -> (Vec<RpcHeightSample>, Vec<String>) {
    // Query all endpoints in parallel so n failing endpoints take timeout_secs
    // total rather than n * timeout_secs, avoiding Tokio thread starvation.
    let handles: Vec<_> = rpc_endpoints
        .iter()
        .map(|rpc| {
            let rpc = rpc.clone();
            std::thread::spawn(move || match rpc_chain_height(&rpc, timeout_secs) {
                Ok(height) => Ok(RpcHeightSample { rpc, height }),
                Err(err) => Err(format!("{rpc}: {err}")),
            })
        })
        .collect();
    let mut samples = Vec::new();
    let mut errors = Vec::new();
    for (i, handle) in handles.into_iter().enumerate() {
        let rpc = &rpc_endpoints[i];
        match handle
            .join()
            .unwrap_or_else(|_| Err(format!("{rpc}: sampling thread panicked")))
        {
            Ok(sample) => samples.push(sample),
            Err(err) => errors.push(err),
        }
    }
    samples.sort_by(|left, right| {
        right
            .height
            .cmp(&left.height)
            .then_with(|| left.rpc.cmp(&right.rpc))
    });
    (samples, errors)
}

fn best_rpc_height_sample(rpc_endpoints: &[String], timeout_secs: u64) -> Option<RpcHeightSample> {
    sample_rpc_heights(rpc_endpoints, timeout_secs)
        .0
        .into_iter()
        .next()
}

fn run_rpc_sync_batch(
    chain_id: &str,
    chain_db_path: Option<&str>,
    rpc_endpoints: &[String],
    peer_view: &mut zoka::state::sync::PeerChainView,
    unsafe_rehearsal_difficulty_bits: Option<u32>,
    max_blocks: u64,
) -> Result<Option<String>, String> {
    if rpc_endpoints.is_empty() || max_blocks == 0 {
        return Ok(None);
    }
    let Some(path) = chain_db_path else {
        return Ok(None);
    };
    let storage = match RocksDbStorage::new(path) {
        Ok(storage) => storage,
        Err(err) => return Ok(Some(format!("RPC sync storage open failed ({err})"))),
    };
    let local_height = storage
        .get_chain_height()
        .map_err(|err| format!("read chain height failed: {err}"))?;

    let (samples, height_errors) = sample_rpc_heights(rpc_endpoints, 3);
    let mut last_error = if height_errors.is_empty() {
        None
    } else {
        Some(height_errors.join("; "))
    };
    let reachable_rpc_count = samples.len();
    let candidates = samples
        .into_iter()
        .filter(|sample| sample.height > local_height)
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return if reachable_rpc_count == 0 && last_error.is_some() && rpc_endpoints.len() > 0 {
            if let Some(err) = last_error.as_deref() {
                tracing::warn!(
                    target: "zoka.sync",
                    error = %err,
                    "RPC sync unavailable"
                );
            }
            // Return None so the P2P fallback runs; the error is already logged above.
            Ok(None)
        } else {
            Ok(None)
        };
    }

    for candidate in candidates {
        let rpc = candidate.rpc.as_str();
        let remote_height = candidate.height;

        peer_view.observe_trusted_height(remote_height);
        let mut applied = 0u64;
        let mut validated_side_branch = 0u64;
        let mut last_event = String::new();
        let mut endpoint_rejected = false;
        let mut range_sync_supported = true;
        let mut range_cache: VecDeque<zoka::network::message::BlockResponseMessage> =
            VecDeque::new();
        for _ in 0..max_blocks {
            let request = match zoka::state::sync::next_required_block(&storage, peer_view)? {
                Some(request) if request.height <= remote_height => request,
                Some(request) => {
                    last_event = format!(
                        "RPC sync waiting for height {} beyond {rpc} height {remote_height}.",
                        request.height
                    );
                    break;
                }
                None => break,
            };
            let response = if let Some(cached) = peer_view.take_pending_block_response(&request) {
                cached
            } else if request.expected_hash.is_none() && range_sync_supported {
                if range_cache.front().map(|response| response.block_height) != Some(request.height)
                {
                    range_cache.clear();
                    let to_height = request.height.saturating_add(127).min(remote_height);
                    match fetch_rpc_block_range(rpc, request.height, to_height) {
                        Ok(responses) if !responses.is_empty() => range_cache.extend(responses),
                        Ok(_) => {}
                        Err(err) => {
                            range_sync_supported = false;
                            last_error = Some(format!(
                                "{rpc}: failed to fetch block range {}..{} ({err})",
                                request.height, to_height
                            ));
                        }
                    }
                }

                match range_cache.pop_front() {
                    Some(response) if response.block_height == request.height => response,
                    Some(response) => {
                        last_error = Some(format!(
                            "{rpc}: raw block range returned height {}, expected {}",
                            response.block_height, request.height
                        ));
                        endpoint_rejected = true;
                        break;
                    }
                    None => match fetch_rpc_block_response(rpc, request.height) {
                        Ok(response) => response,
                        Err(err) => {
                            last_error = Some(format!(
                                "{rpc}: failed to fetch block {} ({err})",
                                request.height
                            ));
                            endpoint_rejected = true;
                            break;
                        }
                    },
                }
            } else {
                match fetch_rpc_block_response(rpc, request.height) {
                    Ok(response) => response,
                    Err(err) => {
                        last_error = Some(format!(
                            "{rpc}: failed to fetch block {} ({err})",
                            request.height
                        ));
                        endpoint_rejected = true;
                        break;
                    }
                }
            };
            if let Some(expected_hash) = request.expected_hash.as_deref() {
                if response.block_hash != expected_hash {
                    last_error = Some(format!(
                        "{rpc}: fork backfill hash mismatch at height {}: expected {}, got {}",
                        request.height, expected_hash, response.block_hash
                    ));
                    endpoint_rejected = true;
                    break;
                }
            }
            let height = response.block_height;
            let outcome = match unsafe_rehearsal_difficulty_bits {
                Some(bits) => {
                    zoka::state::sync::try_apply_synced_block_payload_with_expected_difficulty(
                        &storage, chain_id, &response, bits,
                    )
                }
                None => {
                    zoka::state::sync::try_apply_synced_block_payload(&storage, chain_id, &response)
                }
            };
            peer_view.record_sync_outcome(&outcome);
            peer_view.remember_block_response(&outcome, &response);
            let imported = matches!(
                &outcome,
                zoka::state::sync::SyncOutcome::Applied(_)
                    | zoka::state::sync::SyncOutcome::AlreadyKnown { .. }
            );
            let side_branch_validated = matches!(
                &outcome,
                zoka::state::sync::SyncOutcome::BehindOrSibling { .. }
            );
            let rejected = matches!(
                &outcome,
                zoka::state::sync::SyncOutcome::BadPayload(_)
                    | zoka::state::sync::SyncOutcome::AnnouncedHashMismatch { .. }
                    | zoka::state::sync::SyncOutcome::Rejected(_)
            );
            last_event = sync_outcome_to_event(outcome);
            if imported {
                applied = applied.saturating_add(1);
            }
            if side_branch_validated {
                validated_side_branch = validated_side_branch.saturating_add(1);
            }
            if rejected {
                last_error = Some(format!(
                    "RPC sync rejected block {height} from {rpc}: {last_event}"
                ));
                tracing::warn!(
                    target: "zoka.sync",
                    rpc = %rpc,
                    height,
                    event = %last_event,
                    "RPC sync rejected block"
                );
                endpoint_rejected = true;
                break;
            }
            let current_height = storage.get_chain_height().unwrap_or(local_height);
            if current_height >= remote_height && !peer_view.has_active_branch_backfill() {
                break;
            }
        }

        if endpoint_rejected {
            continue;
        }

        if applied > 0 || validated_side_branch > 0 {
            let new_height = storage.get_chain_height().unwrap_or(local_height);
            tracing::info!(
                target: "zoka.sync",
                rpc = %rpc,
                applied,
                validated_side_branch,
                local_height = new_height,
                remote_height,
                "RPC sync imported blocks"
            );
            return Ok(Some(format!(
                "RPC sync imported {applied} block(s), validated {validated_side_branch} fork block(s) from {rpc}: local height {new_height}/{remote_height}. {last_event}"
            )));
        }
        last_error = Some(format!(
            "RPC sync saw remote height {remote_height} from {rpc}, but no block was imported. {last_event}"
        ));
    }

    Ok(last_error.map(|err| format!("RPC sync unavailable: {err}")))
}

fn ibd_retry_delay(event: &str) -> Duration {
    if event.contains("pending") || event.contains("storage open failed") {
        Duration::from_secs(3)
    } else {
        Duration::from_millis(250)
    }
}

fn is_transient_storage_lock_error(err: &str) -> bool {
    err.contains("Failed to create lock file")
        || err.contains("/LOCK")
        || err.contains("\\LOCK")
        || err.contains("being used by another process")
        || err.contains("utilizado por otro proceso")
        || err.contains("no tiene acceso al archivo")
        || err.contains("siendo utilizado por otro proceso")
}

fn is_nonrecoverable_mining_batch_error(err: &str) -> bool {
    err.contains("chain DB has no atomic economics marker")
        || err.contains("chain DB was created with economics version")
}

/// Not a real failure: another peer's block already claimed the height this
/// batch was mining, so it aborted early instead of grinding stale work.
fn is_stale_parent_error(err: &str) -> bool {
    err.contains("stale parent:")
}

/// True once POST /local/mining/stop has explicitly flipped the control off.
/// `updated_at == 0` means the control was never touched in this process
/// (standalone CLI miners), so those are never asked to stop through it.
fn local_mining_stop_requested() -> bool {
    let control = zoka::api::global_local_mining_control().snapshot();
    !control.enabled && control.updated_at > 0
}

/// Not a real failure either: the user switched mining off while a batch
/// was in flight, and the batch aborted instead of finishing the solve.
fn is_mining_stopped_error(err: &str) -> bool {
    err.contains("mining stopped:")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GenesisAccount {
    address: String,
    balance: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GenesisSpec {
    version: u32,
    network: String,
    timestamp: u64,
    initial_accounts: Vec<GenesisAccount>,
}

fn genesis_digest_hex(genesis: &GenesisSpec) -> Result<String, String> {
    let bytes = serde_json::to_vec(genesis).map_err(|e| format!("json error: {e}"))?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(hex::encode(hasher.finalize()))
}

async fn persist_state_snapshot(
    manager: &Arc<StateManager>,
    storage: &Arc<RocksDbStorage>,
) -> Result<(), String> {
    let state = manager.get_state().await;
    for account in state.accounts.values() {
        storage
            .save_account_state(&account.address, account)
            .map_err(|e| format!("failed to persist account '{}': {e}", account.address))?;
    }
    storage
        .set_chain_height(state.current_height)
        .map_err(|e| format!("failed to persist chain height: {e}"))?;
    storage
        .set_last_block_hash(&state.current_hash)
        .map_err(|e| format!("failed to persist last block hash: {e}"))?;
    storage
        .set_state_root(&state.current_state_root)
        .map_err(|e| format!("failed to persist state root: {e}"))?;
    storage
        .set_private_commitment_root(&state.private_commitment_root)
        .map_err(|e| format!("failed to persist private commitment root: {e}"))?;
    storage
        .set_private_commitments(&state.private_commitments)
        .map_err(|e| format!("failed to persist private commitments: {e}"))?;
    Ok(())
}

async fn apply_genesis_to_db(
    genesis: &GenesisSpec,
    db_path: &str,
    force: bool,
) -> Result<String, String> {
    let digest = genesis_digest_hex(genesis)?;
    let storage = open_storage(db_path)?;
    ensure_atomic_economics_db(&storage)?;
    let manager = Arc::new(StateManager::new(storage.clone()));
    manager
        .load_from_storage()
        .await
        .map_err(|e| format!("failed to load state from db: {e}"))?;

    let existing_digest = storage
        .get_raw("chain:genesis:digest")
        .map_err(|e| format!("failed to read genesis metadata: {e}"))?
        .and_then(|raw| String::from_utf8(raw).ok());
    if let Some(current) = existing_digest {
        if current == digest {
            println!("Genesis already applied to {}", db_path);
            return Ok(digest);
        }
        if !force {
            return Err(format!(
                "db already has a different genesis digest (current={}, incoming={}); use --force to bypass digest gate",
                current, digest
            ));
        }
    }

    for account in &genesis.initial_accounts {
        let balance_atoms = zka_to_atoms_checked(account.balance, "genesis account balance")?;
        match manager.get_account(&account.address).await {
            Some(existing) => {
                if existing.balance != balance_atoms {
                    return Err(format!(
                        "genesis/account mismatch for '{}': db balance={}, genesis balance={}",
                        account.address,
                        format_zka(existing.balance),
                        format_zka(balance_atoms)
                    ));
                }
            }
            None => {
                manager
                    .create_account(account.address.clone(), balance_atoms)
                    .await
                    .map_err(|e| {
                        format!(
                            "failed to create genesis account '{}': {e}",
                            account.address
                        )
                    })?;
            }
        }
    }

    persist_state_snapshot(&manager, &storage).await?;
    storage
        .put_raw("chain:genesis:digest", digest.as_bytes().to_vec())
        .map_err(|e| format!("failed to write genesis digest metadata: {e}"))?;
    storage
        .put_raw("chain:genesis:network", genesis.network.as_bytes().to_vec())
        .map_err(|e| format!("failed to write genesis network metadata: {e}"))?;
    storage
        .put_raw(
            "chain:genesis:timestamp",
            genesis.timestamp.to_string().as_bytes().to_vec(),
        )
        .map_err(|e| format!("failed to write genesis timestamp metadata: {e}"))?;

    Ok(digest)
}

async fn apply_mainnet_genesis_to_db(
    genesis: &zoka::genesis::MainnetGenesisV1,
    db_path: &str,
    force: bool,
) -> Result<String, String> {
    let report = zoka::genesis::GenesisVerifyReport::from_genesis(genesis)
        .map_err(|e| format!("mainnet genesis rejected: {e}"))?;
    let digest = report.digest_hex;
    let storage = open_storage(db_path)?;
    ensure_atomic_economics_db(&storage)?;
    let manager = Arc::new(StateManager::new(storage.clone()));
    manager
        .load_from_storage()
        .await
        .map_err(|e| format!("failed to load state from db: {e}"))?;

    let existing_digest = storage
        .get_raw("chain:genesis:digest")
        .map_err(|e| format!("failed to read genesis metadata: {e}"))?
        .and_then(|raw| String::from_utf8(raw).ok());
    if let Some(current) = existing_digest {
        if current == digest {
            println!("Genesis already applied to {}", db_path);
            return Ok(digest);
        }
        if !force {
            return Err(format!(
                "db already has a different genesis digest (current={}, incoming={}); use --force to bypass digest gate",
                current, digest
            ));
        }
    }

    for account in &genesis.initial_accounts {
        let balance_atoms =
            zka_to_atoms_checked(account.balance_zka, "genesis initial account balance_zka")?;
        match manager.get_account(&account.address).await {
            Some(existing) => {
                if existing.balance != balance_atoms {
                    return Err(format!(
                        "genesis initial account mismatch for '{}': db balance={}, genesis balance={}",
                        account.address,
                        format_zka(existing.balance),
                        format_zka(balance_atoms)
                    ));
                }
            }
            None => {
                manager
                    .create_account(account.address.clone(), balance_atoms)
                    .await
                    .map_err(|e| {
                        format!(
                            "failed to create genesis initial account '{}': {e}",
                            account.address
                        )
                    })?;
            }
        }
    }

    for entry in &genesis.pre_mine {
        let amount_atoms = zka_to_atoms_checked(entry.amount_zka, "genesis pre-mine amount_zka")?;
        match manager.get_account(&entry.recipient_address).await {
            Some(existing) => {
                if existing.balance != amount_atoms {
                    return Err(format!(
                        "genesis pre-mine mismatch for '{}': db balance={}, genesis amount={}",
                        entry.recipient_address,
                        format_zka(existing.balance),
                        format_zka(amount_atoms)
                    ));
                }
            }
            None => {
                manager
                    .create_account(entry.recipient_address.clone(), amount_atoms)
                    .await
                    .map_err(|e| {
                        format!(
                            "failed to create genesis pre-mine account '{}': {e}",
                            entry.recipient_address
                        )
                    })?;
            }
        }
    }

    persist_state_snapshot(&manager, &storage).await?;
    storage
        .put_raw("chain:genesis:digest", digest.as_bytes().to_vec())
        .map_err(|e| format!("failed to write genesis digest metadata: {e}"))?;
    storage
        .put_raw("chain:genesis:schema", genesis.schema.as_bytes().to_vec())
        .map_err(|e| format!("failed to write genesis schema metadata: {e}"))?;
    storage
        .put_raw(
            "chain:genesis:network",
            genesis.chain_id.as_bytes().to_vec(),
        )
        .map_err(|e| format!("failed to write genesis network metadata: {e}"))?;
    storage
        .put_raw(
            "chain:genesis:timestamp",
            genesis.genesis_timestamp.to_string().as_bytes().to_vec(),
        )
        .map_err(|e| format!("failed to write genesis timestamp metadata: {e}"))?;
    storage
        .set_total_supply_emitted(genesis.pre_mine.iter().try_fold(0u64, |acc, entry| {
            let amount = zka_to_atoms_checked(entry.amount_zka, "genesis pre-mine amount_zka")?;
            acc.checked_add(amount)
                .ok_or_else(|| "pre-mine emitted supply overflow".to_string())
        })?)
        .map_err(|e| format!("failed to persist pre-mine emitted supply: {e}"))?;

    Ok(digest)
}

async fn apply_genesis_file_to_db(
    path: &str,
    db_path: &str,
    force: bool,
) -> Result<String, String> {
    let raw =
        fs::read_to_string(path).map_err(|e| format!("failed to read genesis file {path}: {e}"))?;

    if let Ok(mainnet_genesis) = serde_json::from_str::<zoka::genesis::MainnetGenesisV1>(&raw) {
        return apply_mainnet_genesis_to_db(&mainnet_genesis, db_path, force).await;
    }

    let legacy_genesis: GenesisSpec =
        serde_json::from_str(&raw).map_err(|e| format!("invalid genesis json: {e}"))?;
    apply_genesis_to_db(&legacy_genesis, db_path, force).await
}

fn open_storage(db_path: &str) -> Result<Arc<RocksDbStorage>, String> {
    Ok(Arc::new(
        RocksDbStorage::new(db_path).map_err(|e| format!("failed to open storage: {e}"))?,
    ))
}

const CHAIN_ECONOMICS_VERSION_KEY: &str = "chain:economics:version";

fn read_chain_economics_marker(storage: &RocksDbStorage) -> Result<Option<String>, String> {
    storage
        .get_raw(CHAIN_ECONOMICS_VERSION_KEY)
        .map_err(|e| format!("failed to read chain economics metadata: {e}"))?
        .map(|raw| {
            String::from_utf8(raw)
                .map(|value| value.trim().to_string())
                .map_err(|_| "chain economics metadata is not valid UTF-8".to_string())
        })
        .transpose()
}

fn ensure_atomic_economics_db(storage: &RocksDbStorage) -> Result<(), String> {
    if let Some(version) = read_chain_economics_marker(storage)? {
        if version == economics::ECONOMICS_VERSION {
            return Ok(());
        }
        return Err(format!(
            "chain DB was created with economics version '{}', but this binary uses '{}'",
            version,
            economics::ECONOMICS_VERSION
        ));
    }

    let height = storage
        .get_chain_height()
        .map_err(|e| format!("failed to read chain height: {e}"))?;
    let emitted = storage
        .get_total_supply_emitted()
        .map_err(|e| format!("failed to read emitted supply: {e}"))?;
    let has_genesis = storage
        .get_raw("chain:genesis:digest")
        .map_err(|e| format!("failed to read genesis digest metadata: {e}"))?
        .is_some();
    if height > 0 || emitted > 0 || has_genesis {
        return Err(format!(
            "chain DB has no atomic economics marker. Start a new genesis/DB for '{}' or run an explicit migration; refusing to mix whole-ZKA and atomic-unit accounting.",
            economics::ECONOMICS_VERSION
        ));
    }

    storage
        .put_raw(
            CHAIN_ECONOMICS_VERSION_KEY,
            economics::ECONOMICS_VERSION.as_bytes().to_vec(),
        )
        .map_err(|e| format!("failed to write chain economics metadata: {e}"))
}

#[derive(Debug, Clone, Serialize)]
struct EconomicsMigrationReport {
    db_path: String,
    chain_id: String,
    economics_version: String,
    height: u64,
    total_supply_emitted_atoms: u64,
    computed_emission_atoms: u64,
    marker_before: Option<String>,
    marker_written: bool,
    supply_repaired: bool,
    dry_run: bool,
}

fn read_utf8_metadata(storage: &RocksDbStorage, key: &str) -> Result<Option<String>, String> {
    storage
        .get_raw(key)
        .map_err(|e| format!("failed to read {key}: {e}"))?
        .map(|raw| {
            String::from_utf8(raw)
                .map(|value| value.trim().to_string())
                .map_err(|_| format!("{key} is not valid UTF-8"))
        })
        .transpose()
}

fn validate_genesis_network_marker(
    storage: &RocksDbStorage,
    network: NetworkProfile,
) -> Result<(), String> {
    let Some(genesis_network) = read_utf8_metadata(storage, "chain:genesis:network")? else {
        return Ok(());
    };
    if genesis_network == network.chain_id() || genesis_network == network.label() {
        return Ok(());
    }
    Err(format!(
        "DB genesis network '{}' does not match requested {} ({})",
        genesis_network,
        network.display_name(),
        network.chain_id()
    ))
}

fn audit_atomic_economics_chain(
    storage: &RocksDbStorage,
    network: NetworkProfile,
) -> Result<(u64, u64), String> {
    validate_genesis_network_marker(storage, network)?;
    let height = storage
        .get_chain_height()
        .map_err(|e| format!("failed to read chain height: {e}"))?;
    let mut computed_emission = 0u64;
    for height_i in 1..=height {
        let block = storage
            .get_block_by_height(height_i)
            .map_err(|e| format!("failed to read block {height_i}: {e}"))?
            .ok_or_else(|| format!("missing canonical block at height {height_i}"))?;
        if block.header.block_height != height_i {
            return Err(format!(
                "canonical height index mismatch at {height_i}: block says {}",
                block.header.block_height
            ));
        }
        let coinbase_amount = block.transactions.first().map(|tx| tx.amount).unwrap_or(0);
        let expected = economics::block_reward_for_height(height_i, computed_emission);
        if coinbase_amount != expected {
            return Err(format!(
                "block {height_i} coinbase amount is {} atoms, expected {} atoms for {}; refusing to mark this DB as atomic economics",
                coinbase_amount,
                expected,
                network.chain_id()
            ));
        }
        computed_emission = computed_emission
            .checked_add(coinbase_amount)
            .ok_or_else(|| format!("computed emission overflow at block {height_i}"))?;
    }
    Ok((height, computed_emission))
}

fn migrate_atomic_economics_marker(
    storage: &RocksDbStorage,
    db_path: &str,
    network: NetworkProfile,
    dry_run: bool,
    yes: bool,
    repair_supply: bool,
) -> Result<EconomicsMigrationReport, String> {
    let marker_before = read_chain_economics_marker(storage)?;
    let height = storage
        .get_chain_height()
        .map_err(|e| format!("failed to read chain height: {e}"))?;
    let total_supply_emitted_atoms = storage
        .get_total_supply_emitted()
        .map_err(|e| format!("failed to read emitted supply: {e}"))?;
    let (audited_height, computed_emission_atoms) = audit_atomic_economics_chain(storage, network)?;
    if audited_height != height {
        return Err("internal migration audit height mismatch".to_string());
    }

    if let Some(existing) = marker_before.as_deref() {
        if existing != economics::ECONOMICS_VERSION {
            return Err(format!(
                "DB already has economics marker '{}', expected '{}'",
                existing,
                economics::ECONOMICS_VERSION
            ));
        }
        return Ok(EconomicsMigrationReport {
            db_path: db_path.to_string(),
            chain_id: network.chain_id().to_string(),
            economics_version: economics::ECONOMICS_VERSION.to_string(),
            height,
            total_supply_emitted_atoms,
            computed_emission_atoms,
            marker_before,
            marker_written: false,
            supply_repaired: false,
            dry_run,
        });
    }

    let mut supply_repaired = false;
    if total_supply_emitted_atoms != computed_emission_atoms {
        if repair_supply && yes && !dry_run {
            storage
                .set_total_supply_emitted(computed_emission_atoms)
                .map_err(|e| format!("failed to repair emitted supply metadata: {e}"))?;
            supply_repaired = true;
        } else {
            return Err(format!(
                "emitted supply metadata mismatch: stored={} atoms, computed={} atoms. Re-run with --repair-supply --yes only if this computed value is expected.",
                total_supply_emitted_atoms,
                computed_emission_atoms
            ));
        }
    }

    if !dry_run && !yes {
        return Err(
            "refusing to write economics marker without --yes; run --dry-run first to inspect"
                .to_string(),
        );
    }

    let mut marker_written = false;
    if !dry_run {
        storage
            .put_raw(
                CHAIN_ECONOMICS_VERSION_KEY,
                economics::ECONOMICS_VERSION.as_bytes().to_vec(),
            )
            .map_err(|e| format!("failed to write chain economics metadata: {e}"))?;
        marker_written = true;
    }

    Ok(EconomicsMigrationReport {
        db_path: db_path.to_string(),
        chain_id: network.chain_id().to_string(),
        economics_version: economics::ECONOMICS_VERSION.to_string(),
        height,
        total_supply_emitted_atoms: if supply_repaired {
            computed_emission_atoms
        } else {
            total_supply_emitted_atoms
        },
        computed_emission_atoms,
        marker_before,
        marker_written,
        supply_repaired,
        dry_run,
    })
}

async fn connect_and_warmup(
    node: &mut Node,
    dial_addr: &str,
    connect_timeout: Duration,
) -> Result<(), String> {
    tracing::info!(target: "zoka.network", peer = %dial_addr, "seed dial started");
    timeout(connect_timeout, node.connect_to_peer(dial_addr))
        .await
        .map_err(|_| format!("dial timeout after {}s", connect_timeout.as_secs()))?
        .map_err(|e| format!("dial error: {e}"))?;
    timeout(connect_timeout, node.wait_for_any_connection())
        .await
        .map_err(|_| "connection timeout".to_string())?
        .map_err(|e| format!("connection error: {e}"))?;
    node.wait_for_min_connected_peers(1, connect_timeout)
        .await
        .map_err(|e| format!("peer warmup error: {e}"))?;
    tracing::info!(target: "zoka.network", peer = %dial_addr, "seed dial succeeded");
    Ok(())
}

/// Cuántos vecinos intentar mantener.
///
/// Antes salía de la cantidad de semillas del manifiesto, topado en 3. Eso
/// mezclaba dos cosas que no tienen relación: **cuántos puntos de entrada
/// conozco** y **cuántos vecinos necesito**. Con una sola semilla activa el
/// objetivo quedaba en 1, así que el nodo dejaba de marcar apenas conseguía un
/// par, aunque tuviera decenas de direcciones descubiertas esperando en el
/// peerstore.
///
/// A quien tiene el puerto abierto no le importaba: junta vecinos por
/// conexiones entrantes. A quien no puede abrirlo —detrás de un router que no
/// controla, que es la mayoría— salir a buscar es la única vía, y era
/// justamente lo que se apagaba. De ahí que unos vieran 3 o más pares y otros
/// se quedaran clavados en 1 o 2.
///
/// El objetivo ahora es el que pide una malla sana de gossipsub. Y sigue
/// acotado por las direcciones que realmente se conocen: prometer 6 con 2
/// direcciones en el pozo solo produciría reintentos contra las mismas.
///
/// Esto NO dispara una avalancha de marcados: `maintain_seed_connections`
/// mantiene su presupuesto de dos intentos por barrido, así que la diferencia
/// es que sigue intentando en barridos sucesivos en vez de rendirse.
const HEALTHY_PEER_TARGET: usize = 6;

fn seed_connection_target(unique_seed_nodes: usize, reconnect_addr_count: usize) -> usize {
    if reconnect_addr_count == 0 {
        return 0;
    }
    let _ = unique_seed_nodes;
    HEALTHY_PEER_TARGET.min(reconnect_addr_count)
}

async fn maintain_seed_connections(
    node: &mut Node,
    seed_addrs: &[String],
    target_peers: usize,
    own_addrs: &[String],
    healthiest_first: &[String],
) -> Option<String> {
    const MAX_DIAL_ATTEMPTS_PER_SWEEP: usize = 2;

    if seed_addrs.is_empty() {
        return None;
    }
    let target_peers = target_peers.clamp(1, seed_addrs.len().max(1));
    if node.connected_peer_count() >= target_peers {
        return None;
    }

    let initial_peers = node.connected_peer_count();
    let mut failures = Vec::new();
    let mut attempted = 0usize;
    // Nunca gastar un intento en un par que ya esta conectado. El marcado
    // "funciona" al instante porque la conexion existe, pero la cuenta de
    // vecinos no se mueve, la espera posterior vence, y se anota como fallo.
    // Con dos intentos por barrido, un unico par conectado se lleva media
    // ronda para siempre y las direcciones a las que todavia NO estamos
    // conectados no reciben ninguno: el nodo queda clavado en un vecino.
    let mut excluidas = own_addrs.to_vec();
    excluidas.extend(node.connected_peer_addrs());
    let shuffled_seed_addrs = ranked_dial_candidates(seed_addrs, &excluidas, healthiest_first);
    if shuffled_seed_addrs.is_empty() {
        return None;
    }
    // A peerstore may contain dozens of old onion addresses. Never walk the
    // entire set in one maintenance call: even bounded per-address timeouts
    // would add up and starve the libp2p event loop long enough to lose the
    // healthy connections we are trying to supplement.
    for seed in shuffled_seed_addrs.iter().take(MAX_DIAL_ATTEMPTS_PER_SWEEP) {
        attempted += 1;
        let before = node.connected_peer_count();
        // A dead onion endpoint can otherwise hold the single fullnode event
        // loop inside the SOCKS handshake for roughly a minute. While blocked,
        // existing libp2p connections are not pumped and healthy peers time
        // out as collateral damage. Keep every maintenance dial bounded.
        // Presupuestos medidos para Tor, no para clearnet.
        //
        // Estaban en 6s para marcar y 4s para confirmar. Un circuito Tor tarda
        // varios segundos solo en armarse, asi que por onion practicamente
        // todo intento vencia antes de completarse y se contaba como fallo
        // aunque la conexion terminara estableciendose despues. El nodo
        // marcaba sin descanso y no sumaba vecinos.
        //
        // Este mismo archivo ya habia aprendido la leccion para las peticiones
        // de bloques -- ahi los presupuestos se subieron a 30s citando
        // exactamente este motivo -- pero el camino de reconexion quedo con
        // los numeros viejos.
        //
        // Sigue acotado: una onion muerta no puede retener el bucle mas de lo
        // que dice este numero, que es la razon por la que el limite existe.
        let dial_budget = if seed.contains("/onion3/") || seed.contains("/onion/") {
            Duration::from_secs(20)
        } else {
            Duration::from_secs(6)
        };
        let warmup_budget = if seed.contains("/onion3/") || seed.contains("/onion/") {
            Duration::from_secs(15)
        } else {
            Duration::from_secs(4)
        };
        match timeout(dial_budget, node.connect_to_peer(seed)).await {
            Err(_) => {
                let err = "dial timeout";
                tracing::warn!(
                    target: "zoka.network",
                    peer = %seed,
                    error = err,
                    "seed reconnect dial timed out"
                );
                failures.push(format!("{seed}: {err}"));
            }
            Ok(Ok(())) => {
                let next_floor = before.saturating_add(1).min(target_peers).max(1);
                match node
                    .wait_for_min_connected_peers(next_floor, warmup_budget)
                    .await
                {
                    Ok(()) if node.connected_peer_count() >= target_peers => {
                        tracing::info!(
                            target: "zoka.network",
                            peer = %seed,
                            connected_peers = node.connected_peer_count(),
                            target_peers,
                            "seed reconnect target reached"
                        );
                        return Some(format!(
                            "Seed peer target reached via {seed}: active peers {}/{}.",
                            node.connected_peer_count(),
                            target_peers
                        ));
                    }
                    Ok(()) => {}
                    Err(err) => {
                        tracing::warn!(
                            target: "zoka.network",
                            peer = %seed,
                            error = %err,
                            "seed reconnect warmup failed"
                        );
                        failures.push(format!("{seed}: {err}"));
                    }
                }
            }
            Ok(Err(err)) => {
                tracing::warn!(
                    target: "zoka.network",
                    peer = %seed,
                    error = %err,
                    "seed reconnect dial failed"
                );
                failures.push(format!("{seed}: {err}"));
            }
        }
        if node.connected_peer_count() >= target_peers {
            return Some(format!(
                "Seed peer target reached: active peers {}/{}.",
                node.connected_peer_count(),
                target_peers
            ));
        }
    }

    if node.connected_peer_count() > initial_peers {
        return Some(format!(
            "Connected to seed peer(s); active peers {}/{} after {} attempt(s).",
            node.connected_peer_count(),
            target_peers,
            attempted
        ));
    }

    Some(format!(
        "Seed connection maintenance pending; active peers {}/{}, attempted {} seed(s): {}",
        node.connected_peer_count(),
        target_peers,
        seed_addrs.len(),
        failures.join("; ")
    ))
}

async fn connect_first_seed_candidate(
    node: &mut Node,
    candidates: &[SeedCandidate],
) -> Result<Option<SeedCandidate>, String> {
    let mut failures = Vec::new();
    for candidate in candidates {
        match connect_and_warmup(node, &candidate.address, Duration::from_secs(20)).await {
            Ok(()) => return Ok(Some(candidate.clone())),
            Err(err) => failures.push(format!(
                "{} ({}, {}): {}",
                candidate.label, candidate.transport, candidate.address, err
            )),
        }
    }

    if failures.is_empty() {
        Ok(None)
    } else {
        Err(format!(
            "all configured seed nodes failed: {}",
            failures.join("; ")
        ))
    }
}

fn select_or_create_private_node_wallet(
    storage: &WalletStorage,
    password: Option<&str>,
    wallet_selector: Option<&str>,
    create_if_missing: bool,
    network: NetworkProfile,
) -> Result<(PrivateWalletManager, String, bool), String> {
    let mut manager = load_private_wallet_manager_for_network(storage, password, network)?;
    if let Some(selector) = wallet_selector {
        let commitment = require_private_network_address(selector, "--address")?;
        if let Some(existing_name) = find_private_wallet_name_by_commitment(&manager, &commitment) {
            return Ok((manager, existing_name, false));
        }
        let display_address = user_private_address_display(&commitment);
        if let Some(wallet_name) =
            find_private_wallet_metadata_name_by_commitment(storage, &commitment)
        {
            return Err(format!(
                "--address: wallet address '{}' belongs to local wallet '{}' but it could not be unlocked with the provided password",
                display_address, wallet_name
            ));
        }
        let create_hint = if create_if_missing {
            "; --create-if-missing only applies when no --address is provided"
        } else {
            ""
        };
        return Err(format!(
            "--address: wallet address '{}' not found locally; import it with 'wallet-restore-from-seed' or 'wallet-restore-from-keys' first{}",
            display_address, create_hint
        ));
    }

    let mut existing = manager.get_private_wallet_names();
    existing.sort();
    if let Some(first) = existing.into_iter().next() {
        return Ok((manager, first, false));
    }
    if !create_if_missing {
        return Err("no local wallet found; create one first".to_string());
    }
    let Some(password) = password else {
        return Err(
            "no local wallet found; --password is required to auto-create an encrypted node wallet"
                .to_string(),
        );
    };

    let generated = generate_available_private_wallet_name(storage, "node-wallet");
    manager
        .create_private_wallet(&generated)
        .map_err(|e| format!("private wallet create failed: {e}"))?;
    persist_private_wallet(storage, &manager, &generated, Some(password))?;
    Ok((manager, generated, true))
}

fn cmd_node_wallet_info(args: &[String]) -> Result<(), String> {
    let (network, storage) = wallet_storage_for_args(args)?;
    let wallet_selector = wallet_selector_value(args);
    let password = password_value(args)?;
    let create_if_missing = has_flag(args, "--create-if-missing");
    let (manager, wallet_name, created) = select_or_create_private_node_wallet(
        &storage,
        password.as_deref(),
        wallet_selector.as_deref(),
        create_if_missing,
        network,
    )?;
    let wallet = manager
        .get_private_wallet(&wallet_name)
        .ok_or_else(|| format!("wallet not found after selection: {wallet_name}"))?;
    let export = private_wallet_secrets_export(wallet);

    if created {
        println!("Node wallet created automatically: {}", wallet_name);
    } else {
        println!("Node wallet loaded: {}", wallet_name);
    }
    println!("Network: {}", network.display_name());
    println!(
        "Wallet file: {}",
        private_wallet_file_path_text(&storage, &wallet_name)
    );
    println!("Wallet address: {}", export.user_address);
    if let Some(mnemonic) = export.mnemonic {
        println!("Recovery phrase: {}", mnemonic);
    } else {
        println!("Recovery phrase: unavailable");
    }
    println!("Scan private key (hex): {}", export.scan_private_key_hex);
    println!("Spend private key (hex): {}", export.spend_private_key_hex);
    Ok(())
}

/// Accept both `"threads": 3` and `"threads": "3"` in the JSON config so a
/// hand-edited file is forgiving about number-vs-string.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ConfigScalar {
    Str(String),
    Num(u64),
}

impl ConfigScalar {
    fn as_arg(&self) -> String {
        match self {
            ConfigScalar::Str(s) => s.clone(),
            ConfigScalar::Num(n) => n.to_string(),
        }
    }
}

/// Editable node config, à la `bitcoin.conf`/`config.toml` on other networks.
/// Every field is optional and maps onto an existing CLI flag; the command
/// line always overrides the file (see `expand_node_config_into_args`).
#[derive(Debug, Default, Deserialize)]
struct NodeConfigFile {
    network: Option<String>,
    address: Option<String>,
    /// Plaintext wallet password (convenience). Leave empty to be prompted.
    password: Option<String>,
    /// Name of an environment variable holding the password (safer than
    /// plaintext). Ignored if `password` is set.
    password_env: Option<String>,
    threads: Option<ConfigScalar>,
    power: Option<String>,
    /// false → run node without mining (injects --no-mine). Default: mine.
    mine: Option<bool>,
    mining_console: Option<bool>,
    allow_isolated_start: Option<bool>,
    network_config: Option<String>,
    db: Option<String>,
    seeds: Option<Vec<String>>,
}

/// Resolve the node config path: explicit `--config <path>` or `$ZOKA_CONFIG`
/// (error if named but missing), otherwise an optional `zoka.config.json` next
/// to the executable or in the current directory.
fn find_node_config_path(args: &[String]) -> Result<Option<PathBuf>, String> {
    if let Some(p) = option_value(args, "--config") {
        let path = PathBuf::from(p.trim());
        if !path.is_file() {
            return Err(format!("--config: file not found: {}", path.display()));
        }
        return Ok(Some(path));
    }
    if let Ok(p) = std::env::var("ZOKA_CONFIG") {
        let p = p.trim();
        if !p.is_empty() {
            let path = PathBuf::from(p);
            if !path.is_file() {
                return Err(format!("ZOKA_CONFIG: file not found: {}", path.display()));
            }
            return Ok(Some(path));
        }
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("zoka.config.json"));
        }
    }
    candidates.push(PathBuf::from("zoka.config.json"));
    for cand in candidates {
        if cand.is_file() {
            return Ok(Some(cand));
        }
    }
    Ok(None)
}

/// Load the optional JSON node config and expand its fields into CLI-style
/// args. Config-derived args are appended AFTER the user's args so the command
/// line always wins (`option_value` returns the first match); `--seed` values
/// from both the file and the CLI are unioned. Returns the (possibly enlarged)
/// argument list; when no config is present the args are returned unchanged.
fn expand_node_config_into_args(args: &[String]) -> Result<Vec<String>, String> {
    let mut out = args.to_vec();
    let path = match find_node_config_path(args)? {
        Some(p) => p,
        None => return Ok(out),
    };
    let raw = fs::read_to_string(&path)
        .map_err(|e| format!("failed to read config {}: {e}", path.display()))?;
    let cfg: NodeConfigFile = serde_json::from_str(&raw)
        .map_err(|e| format!("invalid config {}: {e}", path.display()))?;
    println!("Loaded node config: {}", path.display());

    // `main()` chdir's the process into the Zoka home directory (so the
    // fullnode's own relative paths like ".zoka/mainnet" resolve correctly)
    // before this ever runs -- so by the time we get here, the process's
    // current directory is NOT the folder the user's zoka.config.json lives
    // in. A bare relative value like "network_config": "mainnet-network.v1.json"
    // would then resolve against the wrong directory and fail to load. Resolve
    // relative file paths from the config against the config file's own
    // directory instead, so they work regardless of that chdir.
    let config_dir = path.parent().map(std::path::Path::to_path_buf);
    let resolve_relative_path = |value: &str| -> String {
        let candidate = PathBuf::from(value);
        if candidate.is_absolute() {
            return value.to_string();
        }
        match &config_dir {
            Some(dir) => dir.join(&candidate).to_string_lossy().into_owned(),
            None => value.to_string(),
        }
    };

    let mut cfg_args: Vec<String> = Vec::new();
    let push = |a: &mut Vec<String>, flag: &str, value: &str| {
        a.push(flag.to_string());
        a.push(value.to_string());
    };
    let non_empty = |o: &Option<String>| {
        o.as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };

    if let Some(v) = non_empty(&cfg.network) {
        push(&mut cfg_args, "--network", &v);
    }
    if let Some(v) = non_empty(&cfg.address) {
        push(&mut cfg_args, "--address", &v);
    }
    if let Some(v) = non_empty(&cfg.network_config) {
        push(
            &mut cfg_args,
            "--network-config",
            &resolve_relative_path(&v),
        );
    }
    if let Some(v) = non_empty(&cfg.power) {
        push(&mut cfg_args, "--power", &v);
    }
    if let Some(v) = non_empty(&cfg.db) {
        push(&mut cfg_args, "--db", &resolve_relative_path(&v));
    }
    if let Some(threads) = &cfg.threads {
        push(&mut cfg_args, "--threads", &threads.as_arg());
    }
    if let Some(seeds) = &cfg.seeds {
        for seed in seeds {
            let seed = seed.trim();
            if !seed.is_empty() {
                push(&mut cfg_args, "--seed", seed);
            }
        }
    }
    if cfg.mine == Some(false) {
        cfg_args.push("--no-mine".to_string());
    }
    // Console mode is the default now, so `mining_console` in the config only
    // needs to matter when explicitly set to false (opt back into the
    // full-screen dashboard). `true` is accepted too for explicitness/older
    // config files, but is a no-op against the default.
    if cfg.mining_console == Some(false) {
        cfg_args.push("--dashboard".to_string());
    }
    if cfg.allow_isolated_start == Some(true) {
        cfg_args.push("--allow-isolated-start".to_string());
    }

    // Password: only sourced from the config when the command line didn't
    // already supply one (checked via flag presence so we never consume stdin
    // twice). Plaintext `password` wins; otherwise `password_env`; otherwise,
    // if this run will actually mine, prompt for it (the "flexible" default).
    let cli_has_password = option_value(args, "--password").is_some()
        || option_value(args, "--wallet-password").is_some()
        || option_value(args, "--password-env").is_some()
        || option_value(args, "--wallet-password-env").is_some()
        || has_flag(args, "--password-stdin")
        || has_flag(args, "--wallet-password-stdin");
    if !cli_has_password {
        let plaintext = cfg.password.clone().unwrap_or_default();
        if !plaintext.is_empty() {
            std::env::set_var("ZOKA_CONFIG_PASSWORD", plaintext);
            push(&mut cfg_args, "--password-env", "ZOKA_CONFIG_PASSWORD");
        } else if let Some(env_name) = non_empty(&cfg.password_env) {
            push(&mut cfg_args, "--password-env", &env_name);
        } else if cfg.mine != Some(false) && non_empty(&cfg.address).is_some() {
            let pw = prompt_required("Wallet password: ")?;
            std::env::set_var("ZOKA_CONFIG_PASSWORD", pw);
            push(&mut cfg_args, "--password-env", "ZOKA_CONFIG_PASSWORD");
        }
    }

    out.extend(cfg_args);
    Ok(out)
}

/// El runtime de Quantic del nodo, si el perfil es de Quantic. Con
/// `--quantic-mine-to <dirección>` también mina (`--quantic-mine-threads` o
/// `--threads`, `--mine-interval-secs` y `--min-mine-peers` como en Zenith).
/// Con `--no-mine` además, espera a `/mining/start`, como hace ZSilent.
#[cfg(feature = "quantic")]
fn quantic_runtime_for(
    args: &[String],
    network: NetworkProfile,
    chain_id: &str,
    min_mine_peers: usize,
) -> Result<Option<zoka::quantic::runtime::QuanticRuntime>, String> {
    if !network.is_quantic() {
        return Ok(None);
    }
    let quantic_network = zoka::quantic::network_for_chain(chain_id)
        .ok_or_else(|| format!("{chain_id} is not a Quantic chain"))?;
    let mining = match option_value(args, "--quantic-mine-to") {
        Some(text) => {
            let address = zoka::quantic::protocol::QuanticAddress::decode_for(text.trim(), quantic_network)
                .map_err(|e| format!("--quantic-mine-to: {e}"))?;
            let threads = parse_u64_arg(
                option_value(args, "--quantic-mine-threads").or_else(|| option_value(args, "--threads")),
                default_mining_threads() as u64,
                "--quantic-mine-threads",
            )?
            .clamp(1, 64) as usize;
            let interval = parse_u64_arg(option_value(args, "--mine-interval-secs"), 0, "--mine-interval-secs")?;
            Some(zoka::quantic::runtime::QuanticMiningConfig {
                address,
                threads,
                interval: Duration::from_secs(interval),
                min_peers: min_mine_peers,
                start_enabled: !has_flag(args, "--no-mine"),
            })
        }
        None => None,
    };
    zoka::quantic::runtime::QuanticRuntime::new(chain_id, mining).map(Some)
}

/// El archivo que lee `mine-status --network quantic-…` (y ZSilent cuando no
/// llega a la API): lo que el minero de Quantic lleva en esta sesión sumado a
/// `base`, lo de sesiones anteriores. Las recompensas sin madurar son las de
/// bloques propios que siguen en la cadena canónica.
#[cfg(feature = "quantic")]
fn save_quantic_mining_state(
    network: NetworkProfile,
    chain_id: &str,
    storage: &RocksDbStorage,
    status: &zoka::quantic::runtime::QuanticMiningStatus,
    base: &UserMiningState,
) -> Result<(), String> {
    use zoka::quantic::difficulty;
    let chain_height = storage.get_chain_height().unwrap_or(0);
    let current_difficulty_bits = zoka::quantic::compact_target_for_tip(storage, chain_id)
        .map(|compact| difficulty::bits_of_target(&difficulty::from_compact(compact)))
        .unwrap_or(0);
    let still_ours = |height: u64, hash: &str| {
        storage
            .get_canonical_block_hash_at_height(height)
            .ok()
            .flatten()
            .is_some_and(|canonical| canonical == hash)
    };
    let address = status.address.clone().unwrap_or_default();
    let mut pending_rewards: Vec<PendingMiningReward> = base
        .pending_rewards
        .iter()
        .filter(|reward| !status.recent_blocks.iter().any(|block| block.hash == reward.block_hash))
        .cloned()
        .chain(status.recent_blocks.iter().map(|block| PendingMiningReward {
            wallet_address: address.clone(),
            amount: block.reward_atoms,
            source_height: block.height,
            unlock_height: block.unlock_height,
            block_hash: block.hash.clone(),
            private_coinbase: None,
        }))
        .filter(|reward| reward.unlock_height > chain_height && still_ours(reward.source_height, &reward.block_hash))
        .collect();
    pending_rewards.sort_by_key(|reward| reward.source_height);
    let mined_now = status.blocks_mined > 0;
    let state = UserMiningState {
        total_blocks: base.total_blocks.saturating_add(status.blocks_mined),
        total_rewards: base.total_rewards.saturating_add(status.rewards_atoms),
        is_mining: status.enabled,
        chain_height,
        current_difficulty_bits,
        estimated_hashrate_hps: if status.enabled { status.hashrate_hps } else { 0 },
        avg_solve_time_secs: if mined_now { status.avg_solve_secs } else { base.avg_solve_time_secs },
        last_solve_time_secs: if mined_now { status.last_solve_secs } else { base.last_solve_time_secs },
        virtual_time_elapsed_secs: 0.0,
        total_supply_emitted: storage.get_total_supply_emitted().unwrap_or(0),
        mode: network.mining_mode().to_string(),
        algorithm: active_pow_algorithm_for_chain(chain_id).to_string(),
        last_block_hash: status
            .recent_blocks
            .last()
            .map(|block| block.hash.clone())
            .unwrap_or_else(|| base.last_block_hash.clone()),
        pending_rewards,
        last_updated: now_secs(),
        mining_process_id: status.enabled.then(std::process::id),
    };
    save_user_mining_state_for_network(network, &state)
}

async fn cmd_fullnode_auto(args: &[String]) -> Result<(), String> {
    let expanded = expand_node_config_into_args(args)?;
    let args: &[String] = &expanded;
    let network = NetworkProfile::from_args(args)?;
    let chain_id = network.chain_id().to_string();
    // Zoka Quantic: su propio protocolo y sus tópicos, antes de crear el nodo
    // de red, para que nunca se mezcle con Zenith.
    if let Some(prefix) = network.topic_prefix() {
        if !zoka::network::p2p::set_network_protocol(network.protocol_id(), prefix) {
            return Err("this process already joined another network".to_string());
        }
    }
    zoka::api::global_startup_status().reset(
        "starting",
        Some(format!("starting {} fullnode", network.display_name())),
    );
    zoka::api::global_startup_status().set_chain_context(network.label(), chain_id.as_str());
    let wallet_selector = wallet_selector_value(args);
    let password = password_value(args)?;
    let mut manual_seed_peers = option_values(args, "--seed");
    manual_seed_peers.extend(option_values(args, "--dial"));
    let network_config_arg = option_value(args, "--network-config");
    let use_network_config = !has_flag(args, "--no-network-config");
    let allow_clearnet_seeds = has_flag(args, "--allow-clearnet-seeds");
    let allow_isolated_start = has_flag(args, "--allow-isolated-start");
    // Bajar bloques sin abrir ninguna wallet.
    //
    // Existe para que ZSilent empiece a sincronizar apenas se abre el
    // programa, mientras el usuario todavia esta en la pantalla de login: sin
    // esto el nodo no arranca hasta que se escribe la contrasena, y esos
    // minutos se pierden.
    //
    // El nodo normal abre (o crea) una wallet privada al arrancar para la
    // direccion de recompensas y el escaneo de tx privadas, y sin
    // contrasena eso falla. En este modo no se toca ninguna wallet: no se
    // desbloquea, no se crea y no se escribe nada. Solo se descargan y validan
    // bloques, que es trabajo que no depende de tener claves.
    //
    // Cuando el usuario inicia sesion, ZSilent reinicia el nodo en modo
    // completo. Para entonces la cadena ya avanzo.
    // En Quantic no hay wallet de Zenith que abrir ni minería de Zenith: el
    // nodo baja y valida bloques como en --sync-only, y lo de Quantic (mempool
    // y minería con --quantic-mine-to) lo lleva `zoka::quantic::runtime`.
    let sync_only = has_flag(args, "--sync-only") || network.is_quantic();
    let tor_proxy = option_value(args, "--tor-proxy")
        .or_else(|| std::env::var("ZOKA_TOR_PROXY").ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let tor_only = has_flag(args, "--tor-only")
        || has_flag(args, "--anonymous-only")
        || std::env::var("ZOKA_TOR_ONLY").as_deref() == Ok("1");
    if tor_only && tor_proxy.is_none() {
        return Err(
            "--tor-only requires --tor-proxy <host:port> or ZOKA_TOR_PROXY=host:port".to_string(),
        );
    }
    let db_path =
        option_value(args, "--db").or_else(|| Some(network.default_db_path().to_string()));
    let genesis_path = option_value(args, "--genesis")
        .or_else(|| Some(network.default_genesis_path().to_string()));
    let ready_file = option_value(args, "--ready-file");
    let external_addrs = option_values(args, "--external-addr")
        .into_iter()
        .map(|addr| addr.trim().to_string())
        .filter(|addr| !addr.is_empty())
        .collect::<Vec<_>>();
    if let Some(path) = ready_file.as_deref() {
        write_fullnode_ready_file(
            path,
            "starting",
            serde_json::json!({
                "network": network.label(),
                "network_role": network.role(),
                "network_name": network.display_name(),
                "chain_id": chain_id.as_str(),
                "db": db_path,
            }),
        )?;
    }
    let listen_addr =
        option_value(args, "--listen").unwrap_or_else(|| format!("/ip4/0.0.0.0/tcp/{}", network.default_p2p_port()));
    let seconds = parse_u64_arg(option_value(args, "--seconds"), 0, "--seconds")?;
    let sync_warmup_secs = parse_u64_arg(
        option_value(args, "--sync-warmup-secs"),
        5,
        "--sync-warmup-secs",
    )?
    .clamp(1, 300);
    let mine_interval_secs = parse_u64_arg(
        option_value(args, "--mine-interval-secs"),
        0,
        "--mine-interval-secs",
    )?
    .clamp(0, 300);
    let mine_blocks =
        parse_u64_arg(option_value(args, "--mine-blocks"), 1, "--mine-blocks")?.clamp(1, 32);
    // Sin wallet abierta no hay direccion a la que pagarle la recompensa, asi
    // que minar en modo solo-sincronizar seria tirar el trabajo. Se apaga acá
    // en vez de confiar en que el llamador pase tambien --no-mine.
    let auto_mine = !has_flag(args, "--no-mine") && !sync_only;
    let min_mine_peers_arg = option_value(args, "--min-mine-peers");
    let mut min_mine_peers =
        parse_u64_arg(min_mine_peers_arg.clone(), 0, "--min-mine-peers")?.clamp(0, 128) as usize;
    let sync_rebroadcast_recent_blocks = parse_u64_arg(
        option_value(args, "--sync-rebroadcast-recent-blocks"),
        0,
        "--sync-rebroadcast-recent-blocks",
    )?
    .clamp(0, 64);
    let unsafe_rehearsal_difficulty_bits = match option_value(
        args,
        "--unsafe-rehearsal-difficulty-bits",
    ) {
        Some(_) if std::env::var("ZOKA_ALLOW_UNSAFE_REHEARSAL").as_deref() != Ok("1") => {
            return Err(
                "--unsafe-rehearsal-difficulty-bits requires ZOKA_ALLOW_UNSAFE_REHEARSAL=1"
                    .to_string(),
            );
        }
        Some(raw) => {
            let bits = parse_u32_arg(
                Some(raw),
                POW_INITIAL_DIFFICULTY_BITS,
                "--unsafe-rehearsal-difficulty-bits",
            )?;
            if bits == 0 || bits > POW_MAX_DIFFICULTY_BITS {
                return Err(format!(
                    "--unsafe-rehearsal-difficulty-bits must be between 1 and {POW_MAX_DIFFICULTY_BITS}"
                ));
            }
            Some(bits)
        }
        None => None,
    };
    let show_keys = has_flag(args, "--show-keys");
    let verbose = has_flag(args, "--verbose");
    // Fluid streaming console is the default: a scrolling status/event log
    // instead of the full-screen dashboard, which clears the screen on every
    // redraw and fights the mining progress lines (the terminal "goes crazy"
    // when blocks arrive). Pass --dashboard (or ZOKA_DASHBOARD=1) to opt back
    // into the old full-screen view; --mining-console/ZOKA_MINING_CONSOLE=1
    // are still accepted for explicitness/back-compat but no longer needed.
    let mining_console =
        !has_flag(args, "--dashboard") && std::env::var("ZOKA_DASHBOARD").as_deref() != Ok("1");
    if mining_console {
        println!(
            "{}",
            paint(
                "Mining console — streaming view. Ctrl+C to stop.",
                ANSI_CYAN
            )
        );
    }
    let mining_threads = parse_thread_count_arg(args)?;
    let mining_target_block_time_secs = POW_TARGET_BLOCK_TIME_SECS;
    let mut mining_hashrate_hps = 0;
    let mining_base_difficulty = POW_INITIAL_DIFFICULTY_BITS;
    let current_mining_state = load_user_mining_state_for_network(network)?;
    if auto_mine && legacy_pow_sim_state_detected(&current_mining_state) {
        return Err(legacy_pow_sim_reset_message());
    }

    let storage = wallet_storage_for_network(network)?;
    let (mut private_wallet_manager, wallet_name, created, export) = if sync_only {
        // Ni desbloquear ni crear: el gestor arranca vacio a proposito. Bajar
        // bloques no necesita claves, y sin contrasena cualquier intento de
        // abrir la wallet fallaria (o, peor, pediria crear una nueva).
        (PrivateWalletManager::new(), String::new(), false, None)
    } else {
        let (mut manager, name, created) = select_or_create_private_node_wallet(
            &storage,
            password.as_deref(),
            wallet_selector.as_deref(),
            true,
            network,
        )?;
        let wallet = manager
            .get_private_wallet(&name)
            .ok_or_else(|| format!("wallet not found after selection: {name}"))?;
        let export = private_wallet_secrets_export(wallet);
        let _ = &mut manager;
        (manager, name, created, Some(export))
    };
    let wallet_address = export
        .as_ref()
        .map(|export| export.user_address.clone())
        .unwrap_or_default();
    if let Some(path) = ready_file.as_deref() {
        write_fullnode_ready_file(
            path,
            "wallet_loaded",
            serde_json::json!({
                "network": network.label(),
                "network_role": network.role(),
                "network_name": network.display_name(),
                "chain_id": chain_id.as_str(),
                "db": db_path,
                "wallet": wallet_name,
                "wallet_address": wallet_address,
            }),
        )?;
    }

    if let Some(export) = export.as_ref() {
        if created {
            println!("Node wallet created automatically.");
        } else {
            println!("Node wallet loaded.");
        }
        println!("Wallet address: {}", wallet_address);
        println!(
            "Wallet file: {}",
            private_wallet_file_path_text(&storage, &wallet_name)
        );
        if created {
            println!(
                "Wallet saved encrypted. Back up the recovery phrase from the Wallets menu before relying on large balances."
            );
        }
        if show_keys {
            if let Some(mnemonic) = export.mnemonic.as_deref() {
                println!("Recovery phrase: {}", mnemonic);
            } else {
                println!("Recovery phrase: unavailable");
            }
            println!("Scan private key (hex): {}", export.scan_private_key_hex);
            println!("Spend private key (hex): {}", export.spend_private_key_hex);
        }
    } else {
        println!("Sync-only mode: no wallet opened. Blocks download and validate; mining and private scanning stay off until the wallet session starts.");
    }
    let mut startup_notes = vec![
        format!(
            "Network profile: {} ({}).",
            network.display_name(),
            chain_id
        ),
        if created {
            format!("Node wallet created automatically for {}.", wallet_address)
        } else {
            format!("Node wallet loaded for {}.", wallet_address)
        },
    ];
    if let Some(bits) = unsafe_rehearsal_difficulty_bits {
        startup_notes.push(format!(
            "UNSAFE local rehearsal difficulty override active: {bits} bits."
        ));
    }
    if auto_mine {
        startup_notes.push(format!(
            "Mining CPU power: {}.",
            mining_power_label_for_threads(mining_threads)
        ));
    }

    // El P2P Exchange se retiro en octubre de 2026. Las ofertas que quedaron
    // abiertas dejaron notas reservadas que ya nadie va a liberar, y sin esto
    // el saldo gastable se veria menor de lo que es. Se sueltan al arrancar.
    //
    // Este camino LIBERA notas, asi que es el que mas cuidado necesita: si
    // corre sobre una vista vieja de la wallet puede soltar una reserva que
    // otro proceso acaba de crear. Por eso relee las wallets con el lock
    // tomado y cubre leer-decidir-escribir como una sola operacion. Con
    // `--sync-only` el gestor de wallets esta vacio a proposito y no hay nada
    // que liberar (ni contrasena con que leerlas).
    if !sync_only {
        let (released, fresh) =
            with_fresh_private_wallets(&storage, password.as_deref(), |fresh| {
                let released = fresh.release_retired_exchange_reservations();
                if released > 0 {
                    persist_all_private_wallets(&storage, fresh, password.as_deref())?;
                }
                Ok(released)
            })?;
        private_wallet_manager = fresh;
        if released > 0 {
            startup_notes.push(format!(
                "Released {released} note reservation(s) left by retired P2P exchange offers."
            ));
        }
    }
    let private_wallet_manager = Arc::new(tokio::sync::RwLock::new(private_wallet_manager));
    // Sin wallet (--sync-only) el runtime solo observa y trabaja con un almacen
    // en memoria: el team.json pertenece a la sesion de la wallet y este modo
    // no la abre. Con la direccion vacia `new` fallaba y el nodo no arrancaba.
    let team_mining_runtime = if sync_only {
        Arc::new(zoka::team_mining::TeamMiningRuntime::without_local_member(
            zoka::team_mining::TeamStore::memory(),
        ))
    } else {
        let team_store_path = PathBuf::from(network.local_dir())
            .join("team_mining")
            .join("team.json");
        let team_store = zoka::team_mining::TeamStore::load(&team_store_path)
            .map_err(|error| format!("failed to open team-mining store: {error}"))?;
        Arc::new(
            zoka::team_mining::TeamMiningRuntime::new(team_store, wallet_address.clone())
                .map_err(|error| format!("failed to initialize team-mining runtime: {error}"))?,
        )
    };

    // Modo autoridad: este nodo firma checkpoints si tiene la clave del equipo.
    let checkpoint_signer =
        load_checkpoint_authority(network.local_dir(), &chain_id, &mut startup_notes);

    // Receiver half of the API → fullnode-main-loop channel; populated
    // only when the HTTP API is spun up (i.e. `--db` path present).
    let mut private_tx_recv_outer: Option<
        tokio::sync::mpsc::UnboundedReceiver<zoka::private_wallet::PrivateTransaction>,
    > = None;
    let mut chain_storage_outer: Option<Arc<RocksDbStorage>> = None;
    let mut checkpoint_quarantine = false;

    if let Some(db) = db_path.as_ref() {
        if let Some(genesis_file) = genesis_path.as_ref() {
            if std::path::Path::new(genesis_file).exists() {
                zoka::api::global_startup_status().set_phase(
                    "applying_genesis",
                    Some("checking genesis state".to_string()),
                );
                let digest = apply_genesis_file_to_db(genesis_file, db, false).await?;
                startup_notes.push(format!("Genesis ready on {} (digest={}).", db, digest));
            }
        }
        zoka::api::global_startup_status()
            .set_phase("opening_db", Some("opening RocksDB".to_string()));
        let storage = open_storage(db)?;

        // Sellar la base recien creada como contabilidad atomica.
        //
        // Este es el unico instante en que la base esta vacia, y por lo tanto
        // el unico en que el marcador se puede poner sin riesgo de mezclar dos
        // contabilidades. Si se deja pasar, la base se llena de bloques -- por
        // IBD o por snapshot, da igual, ninguno de los dos lo escribe -- y a
        // partir de ahi minar queda rechazado para siempre con "chain DB has
        // no atomic economics marker".
        //
        // Antes esto solo ocurria al aplicar el genesis, y el genesis solo se
        // aplica si existe `docs/genesis.mainnet.v1.json`, que el instalador de
        // ZSilent no incluye. O sea que le pasaba a TODO usuario que instalara
        // y sincronizara: nunca podia minar. Los nodos que si podian eran los
        // que corrian desde el repo, donde ese archivo existe.
        //
        // El error se ignora a proposito, igual que en `private-send`. En una
        // base que ya tiene bloques y no tiene marcador esto falla, y asi debe
        // ser: esa base necesita `chain-economics-migrate`, que audita bloque
        // por bloque antes de sellarla. Convertir eso en un fallo de arranque
        // dejaria sin nodo a quien hoy al menos puede sincronizar y operar.
        let _ = ensure_atomic_economics_db(&storage);

        // Fase 5 fast-bootstrap: if a local signed snapshot is well ahead of
        // what's already on disk, replay it (every block still goes through
        // apply_block inside import_into_storage, so PoW/coinbase/fork-link
        // rules are re-checked exactly as if it came from the network)
        // instead of waiting for block-by-block P2P/RPC catch-up. This only
        // ever reads a snapshot file already present locally - it does not
        // fetch anything. Skipped unless the network config (file or
        // embedded fallback) has at least one snapshot_signers key the
        // snapshot's signature verifies against; importing an unverifiable
        // snapshot into a production chain is worse than a slow catch-up.
        if use_network_config {
            let local_height = storage.get_chain_height().unwrap_or(0);
            const MIN_SNAPSHOT_LEAD_BLOCKS: u64 = 100;
            let manifest =
                zoka::snapshot::scan_snapshot_manifests(&zoka::snapshot::default_snapshot_dir())
                    .into_iter()
                    .find(|m| {
                        m.chain_id == chain_id
                            && m.head_height
                                >= local_height.saturating_add(MIN_SNAPSHOT_LEAD_BLOCKS)
                    });
            if let Some(manifest) = manifest {
                let config_path = network_config_arg
                    .as_deref()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from(network.default_network_config_path()));
                let trusted_signers = load_network_config_for(&config_path, network)
                    .map(|cfg| cfg.snapshot_signers)
                    .unwrap_or_default();
                if trusted_signers.is_empty() {
                    startup_notes.push(format!(
                        "Local snapshot at height {} found but skipped: no snapshot_signers configured.",
                        manifest.head_height
                    ));
                } else {
                    match fs::read(&manifest.snap_path) {
                        Ok(bytes) => {
                            let mut hasher = Sha256::new();
                            hasher.update(&bytes);
                            let file_digest = hex::encode(hasher.finalize());
                            if file_digest != manifest.sha256_hex {
                                startup_notes.push(format!(
                                    "Local snapshot at {} skipped: sha256 mismatch against its manifest (file may be corrupt or tampered).",
                                    manifest.snap_path
                                ));
                            } else {
                                match zoka::snapshot::deserialize(&bytes) {
                                    Ok(snapshot) => {
                                        let (verified, last_verify_err) =
                                            match zoka::snapshot::verify_against_trusted_signers(
                                                &snapshot,
                                                chain_id.as_str(),
                                                &trusted_signers,
                                            ) {
                                                Ok(_) => (true, String::new()),
                                                Err(e) => (false, e.to_string()),
                                            };
                                        if verified {
                                            zoka::api::global_startup_status().set_phase(
                                                "importing_snapshot",
                                                Some(format!(
                                                    "importing local snapshot at height {}",
                                                    manifest.head_height
                                                )),
                                            );
                                            match zoka::snapshot::import_into_storage(
                                                &storage, &chain_id, &snapshot,
                                            ) {
                                                Ok(applied) => startup_notes.push(format!(
                                                    "Fast-bootstrapped from local snapshot: {} block(s) applied, height {} -> {}.",
                                                    applied,
                                                    local_height,
                                                    storage.get_chain_height().unwrap_or(local_height)
                                                )),
                                                Err(e) => startup_notes.push(format!(
                                                    "Local snapshot import failed, continuing with normal sync: {e}"
                                                )),
                                            }
                                        } else {
                                            startup_notes.push(format!(
                                                "Local snapshot at {} skipped: signature did not verify against any configured snapshot_signers ({last_verify_err}).",
                                                manifest.snap_path
                                            ));
                                        }
                                    }
                                    Err(e) => startup_notes.push(format!(
                                        "Local snapshot at {} skipped: parse failed ({e}).",
                                        manifest.snap_path
                                    )),
                                }
                            }
                        }
                        Err(e) => startup_notes.push(format!(
                            "Local snapshot manifest found but file read failed ({e}): {}",
                            manifest.snap_path
                        )),
                    }
                }
            }
        }

        match zoka::state::chain::validate_mainnet_checkpoint_history(&storage, &chain_id) {
            Ok(()) => {}
            Err(zoka::state::chain::ChainError::MainnetCheckpointMismatch {
                height,
                expected,
                actual,
            }) => {
                checkpoint_quarantine = true;
                zoka::api::global_local_mining_control().disable();
                startup_notes.push(format!(
                    "CHECKPOINT QUARANTINE: canonical block {height} is {actual}, expected {expected}. Mining disabled; P2P recovery sync remains active."
                ));
            }
            Err(err) => {
                return Err(format!(
                    "failed to validate mainnet checkpoint history: {err}"
                ));
            }
        }
        zoka::api::global_network_runtime().set_checkpoint_quarantined(checkpoint_quarantine);

        let manager = Arc::new(StateManager::new(storage));
        chain_storage_outer = Some(manager.raw_storage());
        let api_mempool = Arc::new(zoka::mempool::Mempool::new(10_000));
        let api_bind_addr = configured_api_bind_addr(args, network)?;
        let api_public_url = format!("http://{api_bind_addr}");
        // Channel for API private-tx submission handlers to hand freshly
        // built PrivateTransactions back to the fullnode
        // main loop. The main loop drains the receiver below, inserts
        // each tx into the in-memory private pool, AND gossips it via
        // libp2p so any connected mining peer can include it in their
        // next block. Without this hop, txs built by the API stay
        // marooned in `priv_tx:<hash>` storage and no miner ever sees
        // them.
        let (private_tx_send, mut private_tx_recv) =
            tokio::sync::mpsc::unbounded_channel::<zoka::private_wallet::PrivateTransaction>();
        let api_broadcaster = Some(private_tx_send);
        // Keep `private_tx_recv` alive across the main loop below by
        // shadowing the binding name when we move it into the select.
        let _ = &mut private_tx_recv;
        tokio::spawn({
            let api_broadcaster = api_broadcaster.clone();
            let manager = Arc::clone(&manager);
            let private_wallet_manager = Arc::clone(&private_wallet_manager);
            let team_mining_runtime = Arc::clone(&team_mining_runtime);
            async move {
                if let Err(e) = zoka::api::start_api_on_bind_with_runtime(
                    manager,
                    api_mempool,
                    api_bind_addr,
                    api_broadcaster,
                    private_wallet_manager,
                    team_mining_runtime,
                )
                .await
                {
                    eprintln!("API server error: {e}");
                }
            }
        });
        startup_notes.push(format!("HTTP API listening on {api_public_url}"));
        zoka::api::global_startup_status()
            .set_phase("loading_state", Some("loading chain state".to_string()));
        manager.load_from_storage_blocking().await.map_err(|e| {
            zoka::api::global_startup_status()
                .mark_failed(Some(format!("failed to load chain state: {e}")));
            format!("failed to load state: {e}")
        })?;
        let state = manager.get_state().await;
        startup_notes.push(format!(
            "State ready from {}: height={}, state_root={}, accounts={}",
            db,
            state.current_height,
            state.current_state_root,
            state.accounts.len()
        ));
        zoka::api::global_startup_status().set_phase(
            "state_ready",
            Some(format!(
                "chain state loaded at height {}",
                state.current_height
            )),
        );
        if let Some(path) = ready_file.as_deref() {
            write_fullnode_ready_file(
                path,
                "state_ready",
                serde_json::json!({
                    "network": network.label(),
                    "network_role": network.role(),
                    "network_name": network.display_name(),
                    "chain_id": chain_id.as_str(),
                    "db": db_path,
                    "wallet": wallet_name,
                    "wallet_address": wallet_address,
                    "height": state.current_height,
                    "state_root": state.current_state_root,
                }),
            )?;
        }
        // Bubble the receiver out of this scope so the main fullnode
        // loop further down can poll it alongside its own P2P events.
        // Stash into the outer-scope option below.
        private_tx_recv_outer = Some(private_tx_recv);
    } else if genesis_path.is_some() {
        return Err("--genesis requires --db in fullnode".to_string());
    }

    zoka::api::global_startup_status()
        .set_phase("starting_p2p", Some("starting libp2p node".to_string()));
    let mut node = Node::new_with_tor_proxy(0, tor_proxy.clone())
        .await
        .map_err(|e| {
            zoka::api::global_startup_status().mark_failed(Some(format!("node init error: {e}")));
            format!("node init error: {e}")
        })?;
    if let Some(proxy) = tor_proxy.as_ref() {
        node.configure_tor_proxy(proxy.clone(), tor_only);
        startup_notes.push(if tor_only {
            format!("Tor-only transport policy active via SOCKS5 {proxy}.")
        } else {
            format!("Tor SOCKS5 proxy configured at {proxy}.")
        });
    }
    // Wire immediate responders so inbound block/header requests are answered
    // the instant they arrive (from the already-open chain storage), instead
    // of waiting on the main loop's serve_pending_* pass. This is what makes
    // IBD serving fast: a peer syncing from us no longer waits behind whatever
    // outbound sync request our own loop happens to be blocked on.
    if let Some(storage) = chain_storage_outer.clone() {
        let block_storage = storage.clone();
        node.set_block_body_responder(move |request| {
            block_body_response_from_storage(&block_storage, request)
        });
        let header_storage = storage.clone();
        node.set_header_responder(move |request| {
            headers_response_from_storage(&header_storage, request)
        });
        let digest_storage = storage.clone();
        node.set_wallet_digest_responder(Box::new(move |request| {
            wallet_digests_from_storage(&digest_storage, request)
        }));
        let bodies_storage = storage.clone();
        node.set_private_tx_bodies_responder(Box::new(move |request| {
            private_tx_bodies_from_storage(&bodies_storage, request)
        }));
        let digests_storage = storage.clone();
        node.set_private_tx_digests_responder(Box::new(move |request| {
            private_tx_digests_from_storage(&digests_storage, request)
        }));
        // Zoka Quantic: los teléfonos piden por P2P lo mismo que la API HTTP.
        #[cfg(feature = "quantic")]
        if network.is_quantic() {
            let light_storage = storage.clone();
            let light_chain_id = chain_id.clone();
            node.set_quantic_light_responder(Box::new(move |request| {
                zoka::quantic::light::answer(&light_storage, &light_chain_id, request)
            }));
        }
    }
    // Optionally also serve this node's chain as an iroh snapshot so fresh
    // nodes can bootstrap in a single transfer (QUIC + n0 hole-punching, no
    // open port, no Tor) instead of block-by-block IBD. Opt-in via
    // --serve-iroh-snapshot and fully guarded: any failure just logs and the
    // node continues. Held for the node's lifetime so the endpoint stays up;
    // its EndpointId is printed for advertising in the manifest 'iroh' field.
    // A fresh node: try a one-shot snapshot bootstrap over iroh before IBD.
    let iroh_bootstrap_config_path = network_config_arg
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(network.default_network_config_path()));
    maybe_bootstrap_from_iroh(
        chain_storage_outer.as_ref(),
        &chain_id,
        network,
        &iroh_bootstrap_config_path,
        use_network_config && !has_flag(args, "--no-iroh-bootstrap"),
        has_flag(args, "--iroh-require-signed-snapshot"),
    )
    .await;
    let _iroh_snapshot_endpoint = maybe_serve_iroh_snapshot(
        chain_storage_outer.as_ref(),
        &chain_id,
        has_flag(args, "--serve-iroh-snapshot"),
    )
    .await;
    node.listen(&listen_addr).await.map_err(|e| {
        zoka::api::global_startup_status().mark_failed(Some(format!("listen error: {e}")));
        format!("listen error: {e}")
    })?;
    for addr in &external_addrs {
        node.add_external_address(addr).map_err(|e| {
            zoka::api::global_startup_status()
                .mark_failed(Some(format!("external address error: {e}")));
            format!("external address error: {e}")
        })?;
        startup_notes.push(format!("External P2P address advertised: {addr}."));
    }
    let announced = timeout(Duration::from_secs(10), node.wait_for_listen_addr())
        .await
        .map_err(|_| {
            zoka::api::global_startup_status()
                .mark_failed(Some("listen address timeout".to_string()));
            "listen address timeout".to_string()
        })?
        .map_err(|e| {
            zoka::api::global_startup_status()
                .mark_failed(Some(format!("listen address error: {e}")));
            format!("listen address error: {e}")
        })?;
    let announced = announced.to_string();
    startup_notes.push(format!("Fullnode listening on {}.", announced));
    zoka::api::global_startup_status().mark_ready(Some(format!(
        "state loaded and P2P listening on {announced}"
    )));
    if let Some(path) = ready_file.as_deref() {
        let peer_id = node.peer_id().to_string();
        let mut publishable_addrs = external_addrs.clone();
        publishable_addrs.push(announced.clone());
        let seed_addresses = seed_addresses_for_peer(&publishable_addrs, &peer_id);
        let seed_address = seed_addresses
            .first()
            .cloned()
            .unwrap_or_else(|| seed_address_for_peer(&announced, &peer_id));
        write_fullnode_ready_file(
            path,
            "ready",
            serde_json::json!({
                "network": network.label(),
                "network_role": network.role(),
                "network_name": network.display_name(),
                "chain_id": chain_id.as_str(),
                "peer_id": peer_id,
                "listen_address": announced,
                "external_addresses": external_addrs,
                "seed_addresses": seed_addresses,
                "seed_address": seed_address,
                "db": db_path,
                "wallet": wallet_name,
                "wallet_address": wallet_address,
            }),
        )?;
    }
    if auto_mine {
        mining_hashrate_hps = benchmark_hashrate_hps_for_chain(&chain_id, 350);
    }

    let mut seed_peer_display = None::<String>;
    let mut reconnect_seed_peers = manual_seed_peers.clone();
    // Count logical seed operators/nodes, not transport addresses. A single
    // manifest seed commonly contributes DNS + onion addresses but can still
    // establish only one PeerId connection.
    let mut configured_seed_nodes = manual_seed_peers.len();
    let mut rpc_sync_endpoints = Vec::<String>::new();
    let peer_store_path = zoka::network::peer_store::default_peer_store_path();
    let mut peer_store =
        match zoka::network::peer_store::PeerStore::load_or_default(peer_store_path.clone()) {
            Ok(store) => {
                if store.is_empty() {
                    startup_notes.push(format!(
                        "PeerStore initialized at {}.",
                        peer_store_path.display()
                    ));
                } else {
                    startup_notes.push(format!(
                        "PeerStore loaded from {} with {} peer(s).",
                        peer_store_path.display(),
                        store.len()
                    ));
                }
                store
            }
            Err(err) => {
                startup_notes.push(format!(
                    "PeerStore load failed at {}: {}. Starting with empty peerstore.",
                    peer_store_path.display(),
                    err
                ));
                zoka::network::peer_store::PeerStore::empty(peer_store_path.clone())
            }
        };
    if !manual_seed_peers.is_empty() {
        for seed in &manual_seed_peers {
            peer_store.upsert_addr(seed, "manual_seed");
        }
        if use_network_config {
            let config_path = network_config_arg
                .as_deref()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(network.default_network_config_path()));
            if config_path.exists() {
                let network_config = load_network_config_for(&config_path, network)?;
                configured_seed_nodes =
                    configured_seed_nodes.max(network_config.active_seed_node_count());
                rpc_sync_endpoints = network_config
                    .public_rpc
                    .iter()
                    .filter(|rpc| rpc.status.eq_ignore_ascii_case("active"))
                    .map(|rpc| rpc.url.clone())
                    .collect();
                // A manual --seed must AUGMENT the manifest's seeds, not
                // replace them. Merge the manifest's active seeds into the
                // reconnect pool so the node dials every seed (e.g. both BOOT
                // and VPS2), not just the single --seed it was handed. Without
                // this, a wallet app that passes one --seed pinned the node to
                // that seed alone and it never dialed the others, capping it at
                // a single P2P peer.
                let seed_candidates = network_config.active_seed_candidates(allow_clearnet_seeds);
                let mut merged_seeds = 0usize;
                let mut skipped_anon_seeds = 0usize;
                for seed in &seed_candidates {
                    // A `tor`/`i2p` seed can only be dialed through a running
                    // SOCKS proxy. Without --tor-proxy the node has no transport
                    // that can reach an /onion3/ or i2p address, so adding it to
                    // the reconnect pool just produces endless "failed to
                    // negotiate transport" churn. Skip anonymous seeds unless a
                    // proxy is configured; the manifest's TCP/DNS seeds still
                    // cover a plain (non-Tor) node.
                    if (seed.transport == "tor" || seed.transport == "i2p") && tor_proxy.is_none() {
                        skipped_anon_seeds += 1;
                        continue;
                    }
                    peer_store.upsert_addr(&seed.address, "manifest_seed");
                    if !reconnect_seed_peers.iter().any(|s| s == &seed.address) {
                        reconnect_seed_peers.push(seed.address.clone());
                        merged_seeds += 1;
                    }
                }
                if skipped_anon_seeds > 0 {
                    startup_notes.push(format!(
                        "Skipped {skipped_anon_seeds} anonymous (tor/i2p) seed(s): no --tor-proxy configured to dial them."
                    ));
                }
                startup_notes.push(format!(
                    "{} network config loaded from {}: status={}, active_rpc={}, manifest_seeds_merged={} (reconnect pool now {}).",
                    network.display_name(),
                    config_path.display(),
                    if network_config.status.is_empty() {
                        "unknown"
                    } else {
                        network_config.status.as_str()
                    },
                    network_config.active_rpc_count(),
                    merged_seeds,
                    reconnect_seed_peers.len(),
                ));
            }
        }
        if auto_mine && min_mine_peers_arg.is_none() {
            min_mine_peers = min_mine_peers.max(1);
        }
        let initial_seed_connect_timeout = if allow_isolated_start {
            Duration::from_secs(4)
        } else {
            Duration::from_secs(20)
        };
        let mut connected_seeds = Vec::new();
        let mut seed_failures = Vec::new();
        for seed in &manual_seed_peers {
            match connect_and_warmup(&mut node, seed, initial_seed_connect_timeout).await {
                Ok(()) => {
                    peer_store.mark_connected(seed, None, "manual_seed");
                    connected_seeds.push(seed.clone());
                }
                Err(err) => {
                    peer_store.mark_failed(seed);
                    seed_failures.push(format!("{seed}: {err}"));
                }
            }
        }
        if let Err(err) = peer_store.save() {
            startup_notes.push(format!("PeerStore save failed: {err}."));
        }
        if connected_seeds.is_empty() {
            if allow_isolated_start {
                startup_notes.push(format!(
                    "Manual seed connection failed; starting isolated and retrying in-session: {}.",
                    seed_failures.join("; ")
                ));
            } else {
                return Err(format!(
                    "failed to connect to manual seed peer(s): {}",
                    seed_failures.join("; ")
                ));
            }
        }
        if !connected_seeds.is_empty() {
            startup_notes.push(format!(
                "Connected to seed peer(s) {}.",
                connected_seeds.join(", ")
            ));
        }
        if !seed_failures.is_empty() {
            startup_notes.push(format!(
                "Manual seed connection failures: {}.",
                seed_failures.join("; ")
            ));
        }
        seed_peer_display = Some(if connected_seeds.is_empty() {
            manual_seed_peers.join(", ")
        } else {
            connected_seeds.join(", ")
        });
    } else if use_network_config {
        let config_path = network_config_arg
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(network.default_network_config_path()));
        if config_path.exists() {
            let network_config = load_network_config_for(&config_path, network)?;
            configured_seed_nodes = network_config.active_seed_node_count();
            let seed_candidates = network_config.active_seed_candidates(allow_clearnet_seeds);
            for seed in &seed_candidates {
                peer_store.upsert_addr(&seed.address, "manifest_seed");
            }
            rpc_sync_endpoints = network_config
                .public_rpc
                .iter()
                .filter(|rpc| rpc.status.eq_ignore_ascii_case("active"))
                .map(|rpc| rpc.url.clone())
                .collect();
            reconnect_seed_peers = seed_candidates
                .iter()
                .map(|seed| seed.address.clone())
                .collect();
            if auto_mine && min_mine_peers_arg.is_none() && !reconnect_seed_peers.is_empty() {
                min_mine_peers = min_mine_peers.max(1);
            }
            startup_notes.push(format!(
                "{} network config loaded from {}: status={}, active_seeds={}, active_rpc={}, snapshot_signers={}, notes={}.",
                network.display_name(),
                config_path.display(),
                if network_config.status.is_empty() {
                    "unknown"
                } else {
                    network_config.status.as_str()
                },
                seed_candidates.len(),
                network_config.active_rpc_count(),
                network_config.snapshot_signers.len(),
                network_config.notes.len()
            ));
            if network_config
                .transport_policy
                .anonymous_transport_preferred
            {
                startup_notes
                    .push("Anonymous transport is preferred by network policy.".to_string());
            }
            match connect_first_seed_candidate(&mut node, &seed_candidates).await {
                Ok(Some(seed)) => {
                    peer_store.mark_connected(&seed.address, None, "manifest_seed");
                    startup_notes.push(format!(
                        "Connected to configured seed {} via {} ({}).",
                        seed.label, seed.transport, seed.address
                    ));
                    seed_peer_display = Some(format!(
                        "{} via {} ({})",
                        seed.label, seed.transport, seed.address
                    ));
                }
                Ok(None) => {
                    let clearnet_note = if network_config
                        .transport_policy
                        .clearnet_requires_explicit_operator_choice
                        && !allow_clearnet_seeds
                    {
                        " Clearnet seeds require --allow-clearnet-seeds."
                    } else {
                        ""
                    };
                    startup_notes.push(format!(
                        "Network config has no usable active seed nodes.{} Running standalone until peers connect.",
                        clearnet_note
                    ));
                }
                Err(err) if allow_isolated_start => {
                    startup_notes.push(format!(
                        "Configured seed connection failed; starting standalone because isolated start is allowed: {err}."
                    ));
                }
                Err(err) => return Err(err),
            }
            if let Err(err) = peer_store.save() {
                startup_notes.push(format!("PeerStore save failed: {err}."));
            }
        } else if network_config_arg.is_some() {
            return Err(format!(
                "{} network config file not found: {}",
                network.display_name(),
                config_path.display()
            ));
        } else {
            startup_notes.push(format!(
                "Default {} network config not found at {}; running standalone until peers connect.",
                network.display_name(),
                config_path.display()
            ));
        }
    } else {
        startup_notes
            .push("No seed peer configured; running standalone until peers connect.".to_string());
    }
    let peerstore_reconnects = peer_store.dial_addrs(32);
    let peerstore_added = append_unique_peer_addrs(&mut reconnect_seed_peers, peerstore_reconnects);
    if peerstore_added > 0 {
        startup_notes.push(format!(
            "PeerStore added {peerstore_added} reconnect candidate(s)."
        ));
    }
    let kademlia_seed_addrs = node.add_kademlia_peer_addrs(&reconnect_seed_peers);
    if kademlia_seed_addrs > 0 {
        match node.bootstrap_kademlia() {
            Ok(_) => startup_notes.push(format!(
                "Kademlia bootstrap started with {kademlia_seed_addrs} known peer address(es)."
            )),
            Err(err) => startup_notes.push(format!(
                "Kademlia bootstrap pending with {kademlia_seed_addrs} known peer address(es): {err}."
            )),
        }
    }

    // Safety net: follower nodes started with --no-network-config and a
    // --seed peer still need RPC endpoints to do bulk IBD, so fall back to
    // the embedded manifest. Seed nodes (no --seed flag) are excluded: they
    // are the canonical source and would otherwise query their own RPC,
    // blocking the single Tokio worker thread on a self-deadlock for 3s
    // every sync poll and starving the API server.
    if rpc_sync_endpoints.is_empty() && !manual_seed_peers.is_empty() {
        if let Ok(embedded) =
            parse_network_config_for(embedded_network_config_for(network), network)
        {
            rpc_sync_endpoints = embedded
                .public_rpc
                .iter()
                .filter(|rpc| rpc.status.eq_ignore_ascii_case("active"))
                .map(|rpc| rpc.url.clone())
                .collect();
        }
    }

    let effective_min_mine_peers =
        authority_min_mine_peers(min_mine_peers, checkpoint_signer.is_some());
    if effective_min_mine_peers != min_mine_peers {
        startup_notes.push("Checkpoint authority: mining does not wait for peers.".to_string());
        min_mine_peers = effective_min_mine_peers;
    }

    let session_started = Instant::now();
    let mut session_mined_blocks = 0u64;
    let mut session_mined_rewards = 0u64;
    let mut last_event = startup_notes.join(" ");
    let mut peer_chain_view = zoka::state::sync::PeerChainView::default();
    peer_chain_view.set_recovery_mode(checkpoint_quarantine);
    let mut last_sync_source: Option<String> = None;
    let mut last_sync_event: Option<String> = None;
    let mut last_sync_error: Option<String> = None;
    let mut rpc_fallback_active = false;

    publish_runtime_sync_status(
        &node,
        chain_storage_outer.as_ref(),
        &peer_chain_view,
        min_mine_peers,
        rpc_fallback_active,
        last_sync_source.as_deref(),
        last_sync_event.as_deref(),
        last_sync_error.as_deref(),
    )
    .await;

    let warmup_deadline = tokio::time::Instant::now() + Duration::from_secs(sync_warmup_secs);
    let mut next_warmup_ibd_at = tokio::time::Instant::now();
    let mut next_warmup_seed_reconnect_at = tokio::time::Instant::now();
    let mut next_warmup_team_tick_at = tokio::time::Instant::now();
    let mut warmup_seed_reconnect_failures = 0u32;
    // --mining-console throttle, shared by the warmup loop and the main loop:
    // emit one compact status line every ~6s (instead of clearing and redrawing
    // the whole dashboard). Not tied to `last_event` — during IBD that changes
    // every block and would spam a line per block; the streaming `mining:
    // progress …` lines already carry the live detail. A newly mined block
    // (session count change) does bypass the throttle so it shows promptly.
    let mut next_console_status_at = tokio::time::Instant::now();
    let mut last_console_session_blocks = 0u64;
    let mut last_console_session_rewards = 0u64;
    while tokio::time::Instant::now() < warmup_deadline {
        let mut now = tokio::time::Instant::now();
        if now >= next_warmup_team_tick_at {
            if let Err(error) = publish_team_mining_messages(&mut node, &team_mining_runtime).await
            {
                last_event = error;
            }
            next_warmup_team_tick_at = tokio::time::Instant::now() + Duration::from_secs(1);
            now = tokio::time::Instant::now();
        }
        if now >= next_warmup_seed_reconnect_at {
            let target_seed_peers =
                seed_connection_target(configured_seed_nodes, reconnect_seed_peers.len());
            if let Some(reconnect_event) = maintain_seed_connections(
                &mut node,
                &reconnect_seed_peers,
                target_seed_peers,
                &external_addrs,
                &peer_store.dial_addrs(32),
            )
            .await
            {
                last_event = reconnect_event;
            }
            if node.connected_peer_count() >= target_seed_peers {
                warmup_seed_reconnect_failures = 0;
            } else {
                warmup_seed_reconnect_failures = warmup_seed_reconnect_failures.saturating_add(1);
            }
            next_warmup_seed_reconnect_at = tokio::time::Instant::now()
                + reconnect_delay_with_jitter(
                    node.connected_peer_count(),
                    target_seed_peers,
                    warmup_seed_reconnect_failures,
                );
            now = tokio::time::Instant::now();
        }
        let remaining = if warmup_deadline > now {
            warmup_deadline - now
        } else {
            Duration::default()
        };
        if mining_console {
            let now_console = tokio::time::Instant::now();
            if now_console >= next_console_status_at {
                let height = current_known_height_for_network(db_path.as_deref(), network);
                let peers = node.connected_peer_count();
                let peers_color = if peers >= 2 {
                    ANSI_GREEN
                } else if peers == 1 {
                    ANSI_YELLOW
                } else {
                    ANSI_RED
                };
                println!(
                    "{}  height={}  peers={}  {} ({}s left)  {} {}",
                    paint("connecting", ANSI_CYAN),
                    paint(&height.to_string(), ANSI_BOLD),
                    paint(&peers.to_string(), peers_color),
                    paint("syncing…", ANSI_YELLOW),
                    remaining.as_secs(),
                    paint("|", ANSI_DIM),
                    paint(&truncate_dashboard_text(&last_event, 80), ANSI_DIM),
                );
                next_console_status_at = now_console + Duration::from_secs(6);
            }
        } else {
            draw_fullnode_runtime_screen(
                &node,
                network,
                &storage,
                &wallet_address,
                password.as_deref(),
                db_path.as_deref(),
                false,
                session_started.elapsed(),
                "warmup",
                Some(remaining),
                &announced,
                seed_peer_display.as_deref(),
                auto_mine,
                mine_interval_secs,
                mine_blocks,
                if auto_mine {
                    Some(mining_threads)
                } else {
                    None
                },
                None,
                session_mined_blocks,
                session_mined_rewards,
                &last_event,
            )?;
        }
        match timeout(Duration::from_millis(500), node.next_sourced_message()).await {
            Ok(Ok(sourced)) => {
                let message = sourced.message;
                last_event = if let Some(verdict) = handle_fullnode_network_message(
                    &mut node,
                    &message,
                    sourced.source,
                    &team_mining_runtime,
                    &chain_id,
                    db_path.as_deref(),
                    chain_storage_outer.as_deref(),
                    &mut peer_chain_view,
                    unsafe_rehearsal_difficulty_bits,
                )
                .await
                {
                    verdict
                } else if verbose {
                    format!(
                        "recv {}",
                        truncate_dashboard_text(
                            &serde_json::to_string(&message)
                                .unwrap_or_else(|_| "<invalid>".to_string()),
                            120
                        )
                    )
                } else {
                    "Warmup traffic received.".to_string()
                };
                if tokio::time::Instant::now() >= next_warmup_ibd_at {
                    let sync_result = if should_prefer_p2p_sync(&node) {
                        match run_sync_step(
                            &mut node,
                            &chain_id,
                            db_path.as_deref(),
                            &mut peer_chain_view,
                            unsafe_rehearsal_difficulty_bits,
                        )
                        .await?
                        {
                            Some(event) => Some(("p2p", event)),
                            None => run_rpc_sync_batch(
                                &chain_id,
                                db_path.as_deref(),
                                &rpc_sync_endpoints,
                                &mut peer_chain_view,
                                unsafe_rehearsal_difficulty_bits,
                                32,
                            )?
                            .map(|event| ("rpc", event)),
                        }
                    } else {
                        match run_rpc_sync_batch(
                            &chain_id,
                            db_path.as_deref(),
                            &rpc_sync_endpoints,
                            &mut peer_chain_view,
                            unsafe_rehearsal_difficulty_bits,
                            32,
                        )? {
                            Some(event) => Some(("rpc", event)),
                            None => run_sync_step(
                                &mut node,
                                &chain_id,
                                db_path.as_deref(),
                                &mut peer_chain_view,
                                unsafe_rehearsal_difficulty_bits,
                            )
                            .await?
                            .map(|event| ("p2p", event)),
                        }
                    };
                    match sync_result {
                        Some((source, sync_event)) => {
                            next_warmup_ibd_at =
                                tokio::time::Instant::now() + ibd_retry_delay(&sync_event);
                            remember_sync_event(
                                source,
                                &sync_event,
                                &mut last_sync_source,
                                &mut last_sync_event,
                                &mut last_sync_error,
                                &mut rpc_fallback_active,
                            );
                            last_event = sync_event;
                        }
                        None => {
                            next_warmup_ibd_at =
                                tokio::time::Instant::now() + Duration::from_secs(1);
                        }
                    }
                }
            }
            Ok(Err(err)) => {
                last_event = format!("receive error: {err}");
                remember_sync_event(
                    "p2p",
                    &last_event,
                    &mut last_sync_source,
                    &mut last_sync_event,
                    &mut last_sync_error,
                    &mut rpc_fallback_active,
                );
            }
            Err(_) => {
                node.pump_for(Duration::from_millis(100))
                    .await
                    .map_err(|e| format!("pump error: {e}"))?;
                if tokio::time::Instant::now() >= next_warmup_ibd_at {
                    let sync_result = if should_prefer_p2p_sync(&node) {
                        match run_sync_step(
                            &mut node,
                            &chain_id,
                            db_path.as_deref(),
                            &mut peer_chain_view,
                            unsafe_rehearsal_difficulty_bits,
                        )
                        .await?
                        {
                            Some(event) => Some(("p2p", event)),
                            None => run_rpc_sync_batch(
                                &chain_id,
                                db_path.as_deref(),
                                &rpc_sync_endpoints,
                                &mut peer_chain_view,
                                unsafe_rehearsal_difficulty_bits,
                                32,
                            )?
                            .map(|event| ("rpc", event)),
                        }
                    } else {
                        match run_rpc_sync_batch(
                            &chain_id,
                            db_path.as_deref(),
                            &rpc_sync_endpoints,
                            &mut peer_chain_view,
                            unsafe_rehearsal_difficulty_bits,
                            32,
                        )? {
                            Some(event) => Some(("rpc", event)),
                            None => run_sync_step(
                                &mut node,
                                &chain_id,
                                db_path.as_deref(),
                                &mut peer_chain_view,
                                unsafe_rehearsal_difficulty_bits,
                            )
                            .await?
                            .map(|event| ("p2p", event)),
                        }
                    };
                    match sync_result {
                        Some((source, sync_event)) => {
                            next_warmup_ibd_at =
                                tokio::time::Instant::now() + ibd_retry_delay(&sync_event);
                            remember_sync_event(
                                source,
                                &sync_event,
                                &mut last_sync_source,
                                &mut last_sync_event,
                                &mut last_sync_error,
                                &mut rpc_fallback_active,
                            );
                            last_event = sync_event;
                        }
                        None => {
                            next_warmup_ibd_at =
                                tokio::time::Instant::now() + Duration::from_secs(1);
                        }
                    }
                }
            }
        }
        publish_runtime_sync_status(
            &node,
            chain_storage_outer.as_ref(),
            &peer_chain_view,
            min_mine_peers,
            rpc_fallback_active,
            last_sync_source.as_deref(),
            last_sync_event.as_deref(),
            last_sync_error.as_deref(),
        )
        .await;
    }
    last_event = if auto_mine {
        if mine_interval_secs == 0 {
            format!(
                "Sync warmup complete. Auto-mining enabled: continuous {} block batch with {}.",
                mine_blocks,
                active_pow_algorithm_for_chain(&chain_id)
            )
        } else {
            format!(
                "Sync warmup complete. Auto-mining enabled: {} block(s) every {}s with {}.",
                mine_blocks,
                mine_interval_secs,
                active_pow_algorithm_for_chain(&chain_id)
            )
        }
    } else {
        "Sync warmup complete. Auto-mining disabled.".to_string()
    };

    let maybe_deadline = if seconds == 0 {
        None
    } else {
        Some(tokio::time::Instant::now() + Duration::from_secs(seconds))
    };
    let mut next_mine_at = tokio::time::Instant::now();
    let mut next_sync_state_at = tokio::time::Instant::now();
    let mut next_team_tick_at = tokio::time::Instant::now();
    let mut next_team_work_at = tokio::time::Instant::now();
    let mut next_checkpoint_tick_at = tokio::time::Instant::now();
    let mut last_checkpoint_broadcast: Option<tokio::time::Instant> = None;
    // publish_recent_chain_blocks re-gossips up to `sync_rebroadcast_recent_blocks`
    // full block payloads. Running that on the same 3s cadence as the cheap
    // height/announce broadcasts turned into a self-inflicted gossip storm: a
    // seed configured with a nonzero rebroadcast count was re-publishing ~10
    // blocks/sec forever, each as a brand-new gossipsub message (so peers can't
    // dedupe it against the identical content they already have), which is what
    // OOM-killed the bootstrap seed every ~15 minutes in production. Recent
    // blocks only need to be nudged back onto the wire occasionally to help a
    // peer that missed one during a brief disconnect -- IBD/RPC sync remains the
    // real catch-up path -- so this gets its own far slower timer.
    let mut next_block_rebroadcast_at = tokio::time::Instant::now();
    let mut next_seed_reconnect_at = tokio::time::Instant::now();
    let mut seed_reconnect_failures = 0u32;
    let mut next_ibd_at = tokio::time::Instant::now();
    let mut next_miner_heartbeat_at = tokio::time::Instant::now();
    let mut next_peer_store_save_at = tokio::time::Instant::now() + Duration::from_secs(15);
    // Backstop for DHT staleness: the peer-store-save branch below already
    // re-bootstraps Kademlia reactively when it observes brand-new
    // addresses, but if nothing new surfaces through that path the routing
    // table never refreshes on its own. This fires unconditionally on a
    // slow cadence regardless of whether new addresses were seen.
    //
    // The cadence is adaptive: a node sitting on very few peers (the
    // reactive path above has nothing to react to) used to wait the full
    // 15 minutes for this backstop, so a single flaky connection could
    // leave it running on one peer for that whole window. Below the
    // healthy-peer threshold it retries every 45s instead; once
    // well-connected it falls back to the slow cadence so a healthy node
    // isn't spamming DHT queries it doesn't need.
    fn kademlia_rebootstrap_interval(connected_peers: usize) -> Duration {
        const LOW_PEER_THRESHOLD: usize = 4;
        const LOW_PEER_INTERVAL: Duration = Duration::from_secs(45);
        const HEALTHY_INTERVAL: Duration = Duration::from_secs(900);
        if connected_peers < LOW_PEER_THRESHOLD {
            LOW_PEER_INTERVAL
        } else {
            HEALTHY_INTERVAL
        }
    }
    let mut next_kad_rebootstrap_at =
        tokio::time::Instant::now() + kademlia_rebootstrap_interval(node.connected_peer_count());
    let mut next_block_announce_retry_sweep_at =
        tokio::time::Instant::now() + Duration::from_secs(2);
    // Periodic RPC pull of pending private txs from public peers. This is the
    // NAT-friendly counterpart to libp2p gossip: a miner that can only open
    // outbound HTTP pulls pending private txs into its own pool so it can mine
    // them, even when inbound P2P gossip never reaches it.
    // Zoka Quantic no tiene transacciones privadas de Zenith: las cuatro tareas
    // de su cola (importar, reenviar, bajar por RPC, completar cuerpos) no
    // corren. Verificarlas es Groth16, y en un nodo de Quantic eso es un `panic!`.
    let zenith_private_pool = !network.is_quantic();
    let mut next_private_sidecar_import_at = tokio::time::Instant::now();
    let mut next_private_pull_at = tokio::time::Instant::now();
    // Arranca corrido un intervalo: al levantar el nodo el gossip todavia no
    // tiene peers suscritos, y reenviar en ese momento es tirar el intento.
    let mut next_private_regossip_at =
        tokio::time::Instant::now() + Duration::from_secs(PRIVATE_TX_REGOSSIP_INTERVAL_SECS);
    let mut next_private_body_pull_at = tokio::time::Instant::now();
    let mut last_private_body_scan_height = 0u64;
    let mut heartbeat_last_status_log = HashMap::<String, bool>::new();
    let mut heartbeat_recent_failures: VecDeque<(String, String)> = VecDeque::with_capacity(8);
    let mut last_runtime_mining_enabled = auto_mine;
    // Incremental iroh tail-sync: keep this node current from the manifest's
    // iroh seeds even with no live libp2p link to the miner (both behind NAT).
    // iroh's n0-relay hole-punching is the transport we already know crosses
    // NAT here (it's how snapshot bootstrap works); this reuses it for the
    // steady-state block tail. Empty when running without the network manifest.
    let iroh_tail_sync_seed_ids: Vec<String> = if use_network_config {
        load_network_config_for(&iroh_bootstrap_config_path, network)
            .map(|c| c.iroh_snapshot_seed_ids())
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let iroh_own_endpoint_id: Option<String> =
        _iroh_snapshot_endpoint.as_ref().map(|e| e.id().to_string());
    let mut next_iroh_tail_sync_at = tokio::time::Instant::now() + Duration::from_secs(20);
    // Peer-to-peer snapshot discovery. The transfer runs off-loop (it is slow
    // and purely network-bound); the *import* is done back on the loop so we
    // never have two writers on the same RocksDB.
    let mut peer_snapshot_fetch: Option<
        tokio::task::JoinHandle<Result<(String, Vec<u8>), String>>,
    > = None;
    let mut next_peer_snapshot_probe_at = tokio::time::Instant::now() + Duration::from_secs(45);
    let mut mining_task: Option<tokio::task::JoinHandle<Result<MiningBatchSummary, String>>> = None;
    let mut team_share_task: Option<TeamShareTask> = None;
    let mut pending_team_solution: Option<zoka::team_mining::NetworkSolution> = None;
    #[cfg(feature = "quantic")]
    if let Some(storage) = chain_storage_outer.as_deref() {
        match zoka::quantic::import_embedded_genesis(storage, &chain_id) {
            Ok(true) => startup_notes.push("Zoka Quantic: genesis block imported from the binary.".to_string()),
            Ok(false) => {}
            Err(error) => return Err(format!("the embedded Quantic genesis does not validate: {error}")),
        }
    }
    #[cfg(feature = "quantic")]
    let mut quantic_runtime = quantic_runtime_for(args, network, &chain_id, min_mine_peers)?;
    // Stratum: XMRig mina contra este nodo (`--stratum 127.0.0.1:3333`). Cobra a
    // la dirección del login del minero, o a la de `--quantic-mine-to`.
    #[cfg(feature = "quantic")]
    if let (true, Some(bind), Some(storage)) =
        (network.is_quantic(), option_value(args, "--stratum"), chain_storage_outer.clone())
    {
        let bind: std::net::SocketAddr = bind
            .trim()
            .parse()
            .map_err(|e| format!("--stratum: expected host:port ({e})"))?;
        let quantic_network = zoka::quantic::network_for_chain(&chain_id)
            .ok_or_else(|| format!("{chain_id} is not a Quantic chain"))?;
        let default_address = match option_value(args, "--quantic-mine-to") {
            Some(text) => Some(
                zoka::quantic::protocol::QuanticAddress::decode_for(text.trim(), quantic_network)
                    .map_err(|e| format!("--quantic-mine-to: {e}"))?,
            ),
            None => None,
        };
        let config = zoka::quantic::stratum::StratumConfig { bind, default_address };
        let stratum_chain = chain_id.clone();
        tokio::spawn(async move {
            if let Err(error) = zoka::quantic::stratum::serve(config, storage, stratum_chain).await {
                tracing::error!(target: "zoka.stratum", "{error}");
            }
        });
        startup_notes.push(format!("Zoka Quantic: Stratum for XMRig (rx/0) on {bind}."));
    }
    #[cfg(feature = "quantic")]
    if let Some(runtime) = quantic_runtime.as_ref() {
        startup_notes.push(if runtime.starts_enabled() {
            "Zoka Quantic: mining to the address of --quantic-mine-to.".to_string()
        } else if runtime.is_mining() {
            "Zoka Quantic: mining address set; waiting for /mining/start.".to_string()
        } else {
            "Zoka Quantic: validating and relaying; pass --quantic-mine-to <address> to mine.".to_string()
        });
    }
    // Lo que `mine-status` ya traía de sesiones anteriores en esta red.
    #[cfg(feature = "quantic")]
    let quantic_mining_base = if network.is_quantic() {
        load_user_mining_state_for_network(network).unwrap_or_default()
    } else {
        UserMiningState::default()
    };
    #[cfg(feature = "quantic")]
    let mut next_quantic_state_save_at = tokio::time::Instant::now();
    loop {
        #[cfg(feature = "quantic")]
        if let (Some(runtime), Some(storage)) = (quantic_runtime.as_mut(), chain_storage_outer.as_ref()) {
            let outcome = runtime.tick(&mut node, storage).await;
            if let Some(event) = outcome.events.into_iter().last() {
                last_event = event;
            }
            if runtime.is_mining()
                && (outcome.mined.is_some() || tokio::time::Instant::now() >= next_quantic_state_save_at)
            {
                if let Err(error) =
                    save_quantic_mining_state(network, &chain_id, storage, runtime.status(), &quantic_mining_base)
                {
                    mining_log(&format!("quantic mining state not saved: {error}"));
                }
                next_quantic_state_save_at = tokio::time::Instant::now() + Duration::from_secs(5);
            }
        }
        if tokio::time::Instant::now() >= next_checkpoint_tick_at {
            if let (Some(signer), Some(storage)) =
                (checkpoint_signer.as_ref(), chain_storage_outer.as_deref())
            {
                if let Some(event) = run_checkpoint_authority(
                    &mut node,
                    storage,
                    &chain_id,
                    signer,
                    &mut last_checkpoint_broadcast,
                )
                .await
                {
                    last_event = event;
                }
            }
            next_checkpoint_tick_at = tokio::time::Instant::now() + Duration::from_secs(10);
        }
        if tokio::time::Instant::now() >= next_team_tick_at {
            if let Err(error) = publish_team_mining_messages(&mut node, &team_mining_runtime).await
            {
                last_event = error;
            }
            next_team_tick_at = tokio::time::Instant::now() + Duration::from_secs(1);
        }
        if let Some(deadline) = maybe_deadline {
            if tokio::time::Instant::now() >= deadline {
                last_event = "Fullnode session time limit reached.".to_string();
                break;
            }
        }

        if let Some(storage) = chain_storage_outer.as_deref() {
            match zoka::state::chain::validate_mainnet_checkpoint_history(storage, &chain_id) {
                Ok(()) if checkpoint_quarantine => {
                    checkpoint_quarantine = false;
                    peer_chain_view.set_recovery_mode(false);
                    zoka::api::global_network_runtime().set_checkpoint_quarantined(false);
                    last_event =
                        "Checkpoint recovery complete; canonical history validated.".to_string();
                }
                Ok(()) => {}
                Err(zoka::state::chain::ChainError::MainnetCheckpointMismatch {
                    height,
                    expected,
                    actual,
                }) => {
                    if !checkpoint_quarantine {
                        last_event = format!(
                            "Checkpoint quarantine at {height}: local {actual}, expected {expected}; mining disabled while sync recovers."
                        );
                    }
                    checkpoint_quarantine = true;
                    peer_chain_view.set_recovery_mode(true);
                    zoka::api::global_local_mining_control().disable();
                    zoka::api::global_network_runtime().set_checkpoint_quarantined(true);
                }
                Err(err) => {
                    if !checkpoint_quarantine {
                        last_event = format!(
                            "Checkpoint validation unavailable ({err}); mining quarantined, sync remains active."
                        );
                    }
                    checkpoint_quarantine = true;
                    peer_chain_view.set_recovery_mode(true);
                    zoka::api::global_local_mining_control().disable();
                    zoka::api::global_network_runtime().set_checkpoint_quarantined(true);
                }
            }
        }

        if !checkpoint_quarantine && tokio::time::Instant::now() >= next_team_work_at {
            if let Err(error) = current_team_mining_template(
                &team_mining_runtime,
                &chain_id,
                chain_storage_outer.as_deref(),
                unsafe_rehearsal_difficulty_bits,
            )
            .await
            {
                last_event = error;
            }
            next_team_work_at = tokio::time::Instant::now() + Duration::from_secs(1);
        }

        let team_status = team_mining_runtime.status().await;
        let team_template = team_status
            .team
            .as_ref()
            .and_then(|team| team.active_template.clone());
        let team_member_difficulty = team_status.team.as_ref().and_then(|team| {
            team.members
                .get(&team_status.local_address)
                .filter(|member| member.active && !member.expelled)
                .map(|member| member.share_difficulty_bits)
        });
        let team_mining_enabled = !checkpoint_quarantine
            && team_status.enabled
            && team_template.is_some()
            && team_member_difficulty.is_some();

        if let Some(task) = team_share_task.as_ref() {
            let assignment_changed = team_template
                .as_ref()
                .is_none_or(|template| template.id != task.template_id);
            if !team_mining_enabled || assignment_changed || mining_task.is_some() {
                task.cancel.store(true, Ordering::Relaxed);
            }
        }

        let team_share_finished = team_share_task
            .as_ref()
            .is_some_and(|task| task.handle.is_finished());
        let mut team_share_completed_this_tick = false;
        if team_share_finished {
            let task = team_share_task
                .take()
                .ok_or_else(|| "team share task finished without a handle".to_string())?;
            team_mining_runtime.set_hashing(false).await;
            match task.handle.await {
                Ok(Ok(summary)) => {
                    match team_mining_runtime
                        .submit_local_share(summary.share.clone(), now_secs())
                        .await
                    {
                        Ok(validated) => {
                            let measured_hashrate = if summary.elapsed_secs > 0.0 {
                                summary.hashes_attempted as f64 / summary.elapsed_secs
                            } else {
                                0.0
                            };
                            last_event = format!(
                                "Team share accepted ({} leading bits{}, ~{}).",
                                validated.actual_difficulty_bits,
                                if validated.network_solution {
                                    ", full network solution"
                                } else {
                                    ""
                                },
                                format_hashrate_hps(measured_hashrate)
                            );
                        }
                        Err(error) => {
                            last_event = format!("Team share rejected: {error}");
                        }
                    }
                }
                Ok(Err(error))
                    if error == "team share work cancelled"
                        || error.starts_with("stale team template:") => {}
                Ok(Err(error)) => {
                    last_event = format!("Team share worker stopped: {error}");
                }
                Err(error) => {
                    last_event = format!("Team share task failed: {error}");
                }
            }
            team_share_completed_this_tick = true;
        }

        if pending_team_solution.is_none() {
            pending_team_solution = team_mining_runtime.take_network_solution().await;
        }

        if team_share_task.is_none()
            && !team_share_completed_this_tick
            && pending_team_solution.is_none()
            && mining_task.is_none()
            && team_mining_enabled
        {
            let template = team_template
                .clone()
                .ok_or_else(|| "team mining lost its active template".to_string())?;
            let difficulty_bits = team_member_difficulty
                .ok_or_else(|| "team mining lost the local member assignment".to_string())?;
            let template_id = template.id.clone();
            let miner_address = team_status.local_address.clone();
            let threads = team_status.threads.clamp(1, 128) as usize;
            let cancel = Arc::new(AtomicBool::new(false));
            let task_cancel = Arc::clone(&cancel);
            let handle = tokio::task::spawn_blocking(move || {
                solve_team_share(
                    template,
                    miner_address,
                    difficulty_bits,
                    threads,
                    task_cancel,
                )
            });
            team_share_task = Some(TeamShareTask {
                template_id,
                cancel,
                handle,
            });
            team_mining_runtime.set_hashing(true).await;
        }

        let team_hashing = team_share_task.is_some();
        let mining_active = mining_task.is_some();
        let mining_control = zoka::api::global_local_mining_control().snapshot();
        // En Quantic el control en caliente lo obedece el minero de Quantic; el
        // de Zenith no tiene nada que minar ahí.
        let runtime_mining_enabled = !network.is_quantic()
            && !checkpoint_quarantine
            && !team_status.enabled
            && (auto_mine || mining_control.enabled);
        let runtime_mining_threads = if mining_control.threads > 0 {
            mining_control.threads.clamp(1, 128) as usize
        } else {
            mining_threads
        };
        if runtime_mining_enabled && !last_runtime_mining_enabled {
            next_mine_at = tokio::time::Instant::now();
            last_event = format!(
                "Runtime mining enabled from local control with {}.",
                mining_power_label_for_threads(runtime_mining_threads)
            );
        } else if !runtime_mining_enabled && last_runtime_mining_enabled {
            last_event =
                "Runtime mining disabled; fullnode remains online for peers and RPC.".to_string();
        }
        last_runtime_mining_enabled = runtime_mining_enabled;

        let mut runtime_mining_state =
            load_user_mining_state_for_network(network).unwrap_or_default();
        if !network.is_quantic()
            && !runtime_mining_enabled
            && !mining_active
            && runtime_mining_state.is_mining
        {
            runtime_mining_state.is_mining = false;
            runtime_mining_state.mining_process_id = None;
            runtime_mining_state.last_updated = now_secs();
            let _ = save_user_mining_state_for_network(network, &runtime_mining_state);
        }
        let runtime_chain_height = chain_storage_outer
            .as_ref()
            .and_then(|storage| storage.get_chain_height().ok())
            .unwrap_or(runtime_mining_state.chain_height);
        zoka::api::global_network_runtime().set_connected_peers(node.connected_peer_count());
        zoka::api::global_network_runtime()
            .set_connected_peer_addrs(node.connected_peer_addr_pairs())
            .await;
        zoka::api::global_network_runtime().set_gossip_stats(node.gossip_stats());
        zoka::api::global_network_runtime().set_topic_peer_counts(node.topic_peer_counts());
        zoka::api::global_network_runtime()
            .set_kademlia_routing_peers(node.kademlia_routing_peer_count());
        zoka::api::global_network_runtime()
            .set_block_request_response_stats(node.block_request_response_stats());
        #[cfg(feature = "quantic")]
        let quantic_local_miner = quantic_runtime
            .as_ref()
            .map(|runtime| (runtime.status().enabled, runtime.status().hashrate_hps));
        #[cfg(not(feature = "quantic"))]
        let quantic_local_miner: Option<(bool, u64)> = None;
        let (local_miner_active, local_miner_hashrate) = match quantic_local_miner {
            Some((enabled, hashrate)) => (enabled, if enabled { hashrate } else { 0 }),
            None => (
                runtime_mining_enabled || mining_active || team_hashing,
                if team_hashing {
                    team_status
                        .team
                        .as_ref()
                        .and_then(|team| team.members.get(&team_status.local_address))
                        .map(|member| member.estimated_hashrate_hps)
                        .unwrap_or(0)
                } else if runtime_mining_enabled || mining_active {
                    runtime_mining_state.estimated_hashrate_hps
                } else {
                    0
                },
            ),
        };
        zoka::api::global_network_runtime()
            .set_local_miner(
                node.peer_id().to_string(),
                local_miner_active,
                runtime_chain_height,
                local_miner_hashrate,
            )
            .await;

        publish_runtime_sync_status(
            &node,
            chain_storage_outer.as_ref(),
            &peer_chain_view,
            min_mine_peers,
            rpc_fallback_active,
            last_sync_source.as_deref(),
            last_sync_event.as_deref(),
            last_sync_error.as_deref(),
        )
        .await;

        if let Some(direct_event) =
            serve_pending_block_body_requests(&mut node, chain_storage_outer.as_deref())?
        {
            last_event = direct_event;
        }
        if let Some(private_tx_event) =
            serve_pending_private_tx_body_requests(&mut node, chain_storage_outer.as_deref())?
        {
            last_event = private_tx_event;
        }
        if let Some(header_event) =
            serve_pending_header_requests(&mut node, chain_storage_outer.as_deref())?
        {
            last_event = header_event;
        }
        if let Some(snapshot_event) = serve_pending_snapshot_manifest_requests(
            &mut node,
            chain_storage_outer.as_ref(),
            &chain_id,
            iroh_own_endpoint_id.as_deref(),
        )? {
            last_event = snapshot_event;
        }

        if tokio::time::Instant::now() >= next_peer_store_save_at {
            match persist_observed_peerstore(&node, &mut peer_store) {
                Ok(observed) if observed > 0 => {
                    let added = append_unique_peer_addrs(
                        &mut reconnect_seed_peers,
                        peer_store.dial_addrs(32),
                    );
                    if added > 0 {
                        let kad_added = node.add_kademlia_peer_addrs(&reconnect_seed_peers);
                        if kad_added > 0 {
                            if let Err(err) = node.bootstrap_kademlia() {
                                tracing::warn!(
                                    target: "zoka.network",
                                    error = %err,
                                    "Kademlia bootstrap refresh pending"
                                );
                            }
                        }
                        last_event = format!(
                            "PeerStore persisted {observed} observed address(es), {added} new reconnect candidate(s), {kad_added} Kademlia address(es)."
                        );
                    }
                }
                Ok(_) => {}
                Err(err) => {
                    last_event = err;
                }
            }
            next_peer_store_save_at = tokio::time::Instant::now() + Duration::from_secs(60);
        }

        if tokio::time::Instant::now() >= next_kad_rebootstrap_at {
            next_kad_rebootstrap_at = tokio::time::Instant::now()
                + kademlia_rebootstrap_interval(node.connected_peer_count());
            if let Err(err) = node.bootstrap_kademlia() {
                tracing::warn!(
                    target: "zoka.network",
                    error = %err,
                    "Periodic Kademlia re-bootstrap failed"
                );
            }
        }

        if tokio::time::Instant::now() >= next_block_announce_retry_sweep_at {
            if let Some(retry_event) = sweep_block_announce_retries(
                &mut node,
                &chain_id,
                db_path.as_deref(),
                &mut peer_chain_view,
            )
            .await
            {
                last_event = retry_event;
            }
            // Schedule from completion, not from the start of a potentially
            // slow Tor round trip, otherwise every loop is immediately due.
            next_block_announce_retry_sweep_at =
                tokio::time::Instant::now() + Duration::from_secs(2);
        }

        if runtime_mining_enabled && tokio::time::Instant::now() >= next_miner_heartbeat_at {
            let (launcher, launcher_version) = miner_launcher_metadata();
            let software = launcher.clone().unwrap_or_else(|| "zoka".to_string());
            let heartbeat = serde_json::json!({
                "peer_id": node.peer_id().to_string(),
                "address": wallet_address.as_str(),
                "chain_height": runtime_chain_height,
                "hashrate_hps": runtime_mining_state.estimated_hashrate_hps,
                "mining": true,
                "client_version": env!("CARGO_PKG_VERSION"),
                "software": software,
                "launcher": launcher,
                "launcher_version": launcher_version,
            });
            if rpc_sync_endpoints.is_empty() {
                let key = "__no_endpoints__".to_string();
                if heartbeat_last_status_log.insert(key, false) != Some(false) {
                    eprintln!(
                        "miner heartbeat: no public RPC endpoints configured; the explorer will not detect this miner."
                    );
                }
            } else {
                for rpc in &rpc_sync_endpoints {
                    let ok = rpc_http_post_json(rpc, "/network/miners/heartbeat", &heartbeat, 2);
                    let prev = heartbeat_last_status_log.insert(rpc.clone(), ok.is_ok());
                    match (&ok, prev) {
                        (Ok(_), Some(false)) | (Ok(_), None) => {
                            eprintln!(
                                "miner heartbeat: explorer link OK via {rpc} (height={}, hps={}).",
                                runtime_chain_height, runtime_mining_state.estimated_hashrate_hps
                            );
                        }
                        (Err(err), Some(true)) | (Err(err), None) => {
                            let msg = format!("miner heartbeat: failed via {rpc}: {err}");
                            eprintln!("{msg}");
                            heartbeat_recent_failures.push_back((rpc.clone(), err.clone()));
                            while heartbeat_recent_failures.len() > 8 {
                                heartbeat_recent_failures.pop_front();
                            }
                        }
                        _ => {}
                    }
                }
            }
            next_miner_heartbeat_at = tokio::time::Instant::now() + Duration::from_secs(15);
        }

        if !mining_active && tokio::time::Instant::now() >= next_ibd_at {
            if let Some(header_event) =
                drain_header_responses(&mut node, db_path.as_deref(), &mut peer_chain_view)?
            {
                last_event = header_event;
            }
            if let Some(probe_event) =
                run_branch_header_probe_step(&mut node, db_path.as_deref(), &mut peer_chain_view)
                    .await?
            {
                last_event = probe_event;
            }
            if let Some(prefetch_event) =
                run_ibd_body_prefetch_step(&mut node, db_path.as_deref(), &mut peer_chain_view)
                    .await?
            {
                last_event = prefetch_event;
            }
            let sync_result = if should_prefer_p2p_sync(&node) {
                match run_sync_step(
                    &mut node,
                    &chain_id,
                    db_path.as_deref(),
                    &mut peer_chain_view,
                    unsafe_rehearsal_difficulty_bits,
                )
                .await?
                {
                    Some(event) => Some(("p2p", event)),
                    None => run_rpc_sync_batch(
                        &chain_id,
                        db_path.as_deref(),
                        &rpc_sync_endpoints,
                        &mut peer_chain_view,
                        unsafe_rehearsal_difficulty_bits,
                        64,
                    )?
                    .map(|event| ("rpc", event)),
                }
            } else {
                match run_rpc_sync_batch(
                    &chain_id,
                    db_path.as_deref(),
                    &rpc_sync_endpoints,
                    &mut peer_chain_view,
                    unsafe_rehearsal_difficulty_bits,
                    64,
                )? {
                    Some(event) => Some(("rpc", event)),
                    None => run_sync_step(
                        &mut node,
                        &chain_id,
                        db_path.as_deref(),
                        &mut peer_chain_view,
                        unsafe_rehearsal_difficulty_bits,
                    )
                    .await?
                    .map(|event| ("p2p", event)),
                }
            };
            match sync_result {
                Some((source, sync_event)) => {
                    next_ibd_at = tokio::time::Instant::now() + ibd_retry_delay(&sync_event);
                    remember_sync_event(
                        source,
                        &sync_event,
                        &mut last_sync_source,
                        &mut last_sync_event,
                        &mut last_sync_error,
                        &mut rpc_fallback_active,
                    );
                    last_event = sync_event;
                }
                None => {
                    next_ibd_at = tokio::time::Instant::now() + Duration::from_secs(1);
                }
            }
        }

        if tokio::time::Instant::now() >= next_seed_reconnect_at {
            let target_seed_peers =
                seed_connection_target(configured_seed_nodes, reconnect_seed_peers.len());
            if let Some(reconnect_event) = maintain_seed_connections(
                &mut node,
                &reconnect_seed_peers,
                target_seed_peers,
                &external_addrs,
                &peer_store.dial_addrs(32),
            )
            .await
            {
                last_event = reconnect_event;
            }
            if node.connected_peer_count() >= target_seed_peers {
                seed_reconnect_failures = 0;
            } else {
                seed_reconnect_failures = seed_reconnect_failures.saturating_add(1);
            }
            next_seed_reconnect_at = tokio::time::Instant::now()
                + reconnect_delay_with_jitter(
                    node.connected_peer_count(),
                    target_seed_peers,
                    seed_reconnect_failures,
                );
        }

        if tokio::time::Instant::now() >= next_sync_state_at {
            // Always broadcast local height so peers behind us can discover
            // they need to catch up — even while this node is actively mining.
            if let Some(sync_event) =
                publish_local_sync_state(&mut node, chain_storage_outer.as_deref())?
            {
                last_event = sync_event;
            }
            if let Some(announce_event) =
                publish_chain_head_block_announce(&mut node, db_path.as_deref())?
            {
                last_event = announce_event;
            }
            next_sync_state_at = tokio::time::Instant::now() + Duration::from_secs(3);
        }

        if tokio::time::Instant::now() >= next_block_rebroadcast_at {
            // Block rebroadcast is skipped during mining to avoid flooding
            // the network with redundant payloads the miner just produced.
            if !mining_active {
                if let Some(rebroadcast_event) = publish_recent_chain_blocks(
                    &mut node,
                    db_path.as_deref(),
                    sync_rebroadcast_recent_blocks,
                )? {
                    last_event = rebroadcast_event;
                }
            }
            if let Some(rpc_rebroadcast_event) = post_recent_chain_blocks_to_rpcs(
                db_path.as_deref(),
                &rpc_sync_endpoints,
                sync_rebroadcast_recent_blocks,
            )? {
                last_event = rpc_rebroadcast_event;
            }
            next_block_rebroadcast_at = tokio::time::Instant::now() + Duration::from_secs(120);
        }

        // Incremental iroh tail-sync. Only for a non-mining follower (a miner is
        // producing the tip, and gating on `!mining_active` also avoids applying
        // a seed's block concurrently with the mining task writing the same DB).
        // This is what actually propagates new blocks to a NAT'd node when there
        // is no live libp2p link to whoever mined them.
        if !mining_active
            && !iroh_tail_sync_seed_ids.is_empty()
            && tokio::time::Instant::now() >= next_iroh_tail_sync_at
        {
            next_iroh_tail_sync_at = tokio::time::Instant::now() + Duration::from_secs(20);
            if let Some(storage) = chain_storage_outer.as_ref() {
                if let Some(event) = maybe_iroh_head_sync(
                    storage,
                    &chain_id,
                    &iroh_tail_sync_seed_ids,
                    iroh_own_endpoint_id.as_deref(),
                )
                .await
                {
                    // Re-announce the new head to any libp2p peers we do have.
                    let announced =
                        publish_local_sync_state(&mut node, chain_storage_outer.as_deref())?;
                    last_event = match announced {
                        Some(sync_event) => format!("{event} {sync_event}"),
                        None => event,
                    };
                }
            }
        }

        // Peer-to-peer snapshot recovery: when this node has fallen far behind
        // the network, pull a snapshot straight from whichever *peer* offers
        // the heaviest chain instead of depending on a configured seed. Every
        // ZSilent node already serves its chain over iroh, so any of them can
        // be the source -- the network heals itself as long as anyone is up.
        //
        // Skipped while mining for the same reason as tail-sync above: the
        // mining task is writing the same DB.
        if !mining_active {
            if let Some(storage) = chain_storage_outer.as_ref() {
                // A transfer we started earlier has landed -> verify + import.
                let finished = peer_snapshot_fetch
                    .as_ref()
                    .map_or(false, |handle| handle.is_finished());
                if finished {
                    if let Some(handle) = peer_snapshot_fetch.take() {
                        match handle.await {
                            Ok(Ok((source, bytes))) => {
                                match import_peer_snapshot_bytes(storage, &chain_id, &bytes) {
                                    Ok(applied) => {
                                        last_event = format!(
                                            "Peer snapshot from {source}: imported {applied} block(s); now at height {}.",
                                            storage.get_chain_height().unwrap_or(0)
                                        );
                                        println!("{last_event}");
                                        let _ = publish_local_sync_state(
                                            &mut node,
                                            chain_storage_outer.as_deref(),
                                        )?;
                                    }
                                    Err(e) => {
                                        println!("Peer snapshot from {source} rejected: {e}");
                                    }
                                }
                            }
                            Ok(Err(e)) => println!("Peer snapshot fetch failed: {e}"),
                            Err(e) => println!("Peer snapshot fetch task panicked: {e}"),
                        }
                    }
                } else if peer_snapshot_fetch.is_none()
                    && tokio::time::Instant::now() >= next_peer_snapshot_probe_at
                {
                    next_peer_snapshot_probe_at =
                        tokio::time::Instant::now() + Duration::from_secs(60);
                    let local_height = storage.get_chain_height().unwrap_or(0);

                    // Act on the previous round's replies first.
                    if let Some(offer) =
                        best_peer_snapshot_offer(&mut node, &chain_id, local_height)
                    {
                        if let Some(id_str) = offer.iroh_endpoint_id.clone() {
                            match id_str.parse::<iroh::EndpointId>() {
                                Ok(endpoint_id) => {
                                    println!(
                                        "Peer snapshot: {} offers height {} (we are at {}); pulling over iroh from {endpoint_id} ...",
                                        endpoint_id, offer.head_height, local_height
                                    );
                                    peer_snapshot_fetch =
                                        Some(tokio::spawn(fetch_peer_snapshot(endpoint_id)));
                                }
                                Err(e) => {
                                    tracing::debug!(target: "zoka.snapshot", id = %id_str, error = %e, "peer advertised an unparseable iroh id")
                                }
                            }
                        }
                    } else if peer_snapshot_would_help(
                        local_height,
                        peer_chain_view.perceived_max_height,
                    ) {
                        // We're far behind and hold no usable offer yet: ask.
                        let asked = request_snapshot_manifests_from_peers(&mut node, &chain_id);
                        if asked > 0 {
                            tracing::debug!(
                                target: "zoka.snapshot",
                                asked,
                                local_height,
                                network_height = peer_chain_view.perceived_max_height,
                                "asked peers for snapshot offers"
                            );
                        }
                    }
                }
            }
        }

        // Drain private tx sidecars from the normal fullnode loop, not only
        // from the mining loop. ZSilent wallet nodes often run with --no-mine;
        // without this path, private sends can sit forever in
        // pending_private_queue while /private/pending reports zero.
        if zenith_private_pool && tokio::time::Instant::now() >= next_private_sidecar_import_at {
            if let (Some(path), Some(storage)) =
                (db_path.as_deref(), chain_storage_outer.as_deref())
            {
                let pruned = prune_confirmed_pending_private_txs(storage, Some(path));
                let summary = import_sidecar_private_txs_with_summary(path, storage);
                if pruned > 0 || summary.total_activity() > 0 {
                    last_event = format!(
                        "Private tx queue drained: imported {}, removed confirmed/spent {}, pruned pending {}, skipped invalid {}.",
                        summary.imported,
                        summary.removed_confirmed,
                        pruned,
                        summary.skipped_invalid
                    );
                }
            }
            next_private_sidecar_import_at = tokio::time::Instant::now() + Duration::from_secs(2);
        }

        // Reintento de publicacion. Ver `pending_private_tx_bodies_to_regossip`:
        // sin esto, un unico gossip perdido deja la transaccion viva localmente
        // y muerta para la red, y el emisor no se entera nunca.
        if zenith_private_pool && tokio::time::Instant::now() >= next_private_regossip_at {
            if let Some(storage) = chain_storage_outer.as_deref() {
                let bodies =
                    pending_private_tx_bodies_to_regossip(storage, PRIVATE_TX_REGOSSIP_BATCH);
                let mut republished = 0usize;
                for (hash, bytes) in bodies {
                    let Ok(tx) = serde_json::from_slice::<PrivateTransaction>(&bytes) else {
                        continue;
                    };
                    node.cache_private_tx_body(&hash, bytes.clone());
                    let converted = serde_json::to_value(&tx).and_then(|v| {
                        serde_json::from_value::<zoka::network::message::PrivateTransactionMessage>(
                            v,
                        )
                    });
                    let Ok(msg) = converted else { continue };
                    let announce =
                        zoka::network::message::PrivateTxAnnounceMessage::from_private_tx(
                            &msg,
                            Some(bytes.len() as u64),
                        );
                    let _ = node.publish_message(
                        &zoka::network::message::ZokaMessage::PrivateTxAnnounce(announce),
                    );
                    if node
                        .publish_message(&zoka::network::message::ZokaMessage::PrivateTransaction(
                            msg,
                        ))
                        .is_ok()
                    {
                        republished = republished.saturating_add(1);
                    }
                }
                if republished > 0 {
                    last_event =
                        format!("Re-gossiped {republished} pending private tx(s) still unmined");
                }
            }
            next_private_regossip_at = tokio::time::Instant::now()
                + Duration::from_secs(PRIVATE_TX_REGOSSIP_INTERVAL_SECS);
        }

        // Pull pending private txs from public RPC peers so a NAT'd miner can
        // include txs that were published to a droplet but never gossiped to
        // it. Bodies are dropped into the sidecar queue, which the mining loop
        // already drains into the local pending pool — this avoids fighting the
        // miner for the RocksDB write lock. Runs on every node with RPC
        // endpoints; harmless on the droplets (they re-stage what they have).
        if zenith_private_pool && tokio::time::Instant::now() >= next_private_pull_at {
            if let Some(path) = db_path.as_deref() {
                let staged = pull_pending_private_txs_from_rpc(path, &rpc_sync_endpoints);
                if staged > 0 {
                    zoka::api::global_network_runtime()
                        .record_private_tx_rpc_fallback_pulled(staged as u64);
                    last_event = format!("Pulled {staged} pending private tx(s) from RPC peers");
                }
            }
            next_private_pull_at = tokio::time::Instant::now() + Duration::from_secs(5);
        }

        // Backfill private transaction bodies for hashes that are already
        // committed in canonical blocks. This repairs peers that synced a
        // block before ever seeing the private tx in their pending pool.
        if zenith_private_pool && tokio::time::Instant::now() >= next_private_body_pull_at {
            if let Some(storage) = chain_storage_outer.as_deref() {
                let tip = storage.get_chain_height().unwrap_or(0);
                if tip > 0 {
                    if last_private_body_scan_height > tip {
                        last_private_body_scan_height = 0;
                    }
                    let initial_backfill = last_private_body_scan_height == 0;
                    let from_height = if initial_backfill {
                        tip.saturating_sub(PRIVATE_TX_BODY_BACKFILL_WINDOW_BLOCKS)
                            .saturating_add(1)
                            .max(1)
                    } else {
                        last_private_body_scan_height.saturating_add(1)
                    };
                    if from_height <= tip {
                        let hashes =
                            collect_private_tx_hashes_in_height_range(storage, from_height, tip);
                        let missing_hashes = missing_private_tx_body_hashes(storage, &hashes);
                        let fetched = pull_missing_committed_private_tx_bodies(
                            storage,
                            &rpc_sync_endpoints,
                            &missing_hashes,
                        );
                        if fetched > 0 {
                            zoka::api::global_network_runtime()
                                .record_private_tx_rpc_fallback_pulled(fetched as u64);
                            last_event = format!(
                                "Backfilled {fetched} committed private tx body(s) from RPC peers ({from_height}..{tip})"
                            );
                        }
                        if should_advance_private_body_scan(
                            !rpc_sync_endpoints.is_empty(),
                            initial_backfill,
                            missing_hashes.len(),
                            fetched,
                        ) {
                            last_private_body_scan_height = tip;
                        } else if rpc_sync_endpoints.is_empty() && !missing_hashes.is_empty() {
                            last_event = format!(
                                "Private tx body backfill waiting for RPC endpoints ({from_height}..{tip}, {} missing)",
                                missing_hashes.len()
                            );
                        } else if initial_backfill && !missing_hashes.is_empty() {
                            last_event = format!(
                                "Private tx body backfill will retry ({from_height}..{tip}, {} missing)",
                                missing_hashes.len()
                            );
                        }
                    } else {
                        last_private_body_scan_height = tip;
                    }
                }
            }
            next_private_body_pull_at = tokio::time::Instant::now()
                + Duration::from_secs(PRIVATE_TX_BODY_PULL_INTERVAL_SECS);
        }

        if mining_task
            .as_ref()
            .map(|task| task.is_finished())
            .unwrap_or(false)
        {
            let task = mining_task
                .take()
                .ok_or_else(|| "auto-mining task finished without a handle".to_string())?;
            let batch = match task.await {
                Ok(Ok(batch)) => batch,
                Ok(Err(err)) if is_stale_parent_error(&err) => {
                    // Not a failure - someone else's block won the height we
                    // were mining. Restart immediately against the real tip
                    // instead of the normal 5s backoff, so we don't add
                    // artificial delay on top of losing the race.
                    last_event =
                        "Restarting mining: chain tip advanced (another block was applied)."
                            .to_string();
                    next_mine_at = tokio::time::Instant::now();
                    continue;
                }
                Ok(Err(err)) if is_mining_stopped_error(&err) => {
                    // Not a failure - the user switched mining off mid-solve.
                    // The spawn gate (runtime_mining_enabled) is already off,
                    // so nothing respawns until mining is re-enabled.
                    mining_log("batch stopped by local mining control");
                    last_event =
                        "Mining stopped by local control; fullnode remains online for peers and RPC."
                            .to_string();
                    continue;
                }
                Ok(Err(err)) if is_transient_storage_lock_error(&err) => {
                    last_event = format!("Auto-mining retry pending: storage busy ({err})");
                    next_mine_at = tokio::time::Instant::now() + Duration::from_secs(2);
                    continue;
                }
                Ok(Err(err)) => {
                    if is_nonrecoverable_mining_batch_error(&err) {
                        zoka::api::global_local_mining_control().disable();
                        mining_log(&format!("batch error (mining disabled): {err}"));
                        last_event = format!(
                            "Auto-mining disabled after non-recoverable batch error; fullnode stayed online: {err}"
                        );
                        next_mine_at = tokio::time::Instant::now() + Duration::from_secs(5);
                        continue;
                    }
                    last_event =
                        format!("Auto-mining batch failed but the fullnode stayed online: {err}");
                    mining_log(&format!("batch error (will retry): {err}"));
                    next_mine_at = tokio::time::Instant::now() + Duration::from_secs(5);
                    continue;
                }
                Err(err) => {
                    last_event =
                        format!("Auto-mining task failed but the fullnode stayed online: {err}");
                    mining_log(&format!("task join error (will retry): {err}"));
                    next_mine_at = tokio::time::Instant::now() + Duration::from_secs(5);
                    continue;
                }
            };
            let mut team_accounting_warning = None;
            if let Some(recipient) = batch.team_recipient.as_deref() {
                let team_block = batch.produced_blocks.first().ok_or_else(|| {
                    "team mining batch completed without a produced block".to_string()
                })?;
                if let Err(error) = team_mining_runtime
                    .record_confirmed_block(
                        team_block.header.block_height,
                        &team_block.header.prev_hash,
                        recipient,
                        None,
                        now_secs(),
                    )
                    .await
                {
                    team_accounting_warning = Some(format!(
                        "team block persisted but turn accounting could not advance: {error}"
                    ));
                }
            }
            session_mined_blocks = session_mined_blocks.saturating_add(batch.blocks_mined);
            session_mined_rewards = session_mined_rewards.saturating_add(batch.reward_credited);
            // here are non-fatal; the block is already persisted locally.
            let mut publish_failures = 0u32;
            for block in &batch.produced_blocks {
                let payload = match serde_json::to_vec(block) {
                    Ok(bytes) => bytes,
                    Err(_) => {
                        publish_failures = publish_failures.saturating_add(1);
                        continue;
                    }
                };
                let bodies = collect_private_tx_bodies_for_block_at_path(db_path.as_deref(), block);
                if let Err(_err) = node.publish_block_response_with_bodies(
                    block.header.block_height,
                    block.header.hash.clone(),
                    payload,
                    bodies,
                ) {
                    publish_failures = publish_failures.saturating_add(1);
                }
            }
            // Autopush: also POST each mined block to the configured public RPC
            // endpoints (/sync/block). P2P gossip alone is unreliable for a
            // NAT'd local miner, so this HTTP push delivers the block straight
            // to the cloud nodes for efficient sync.
            let mut rpc_submit_failures = 0u64;
            if !rpc_sync_endpoints.is_empty() {
                let rpc_targets = rpc_sync_endpoints
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>();
                for block in &batch.produced_blocks {
                    let (_, failed) = post_block_to_rpc_targets(block, &rpc_targets);
                    rpc_submit_failures = rpc_submit_failures.saturating_add(failed);
                }
            }
            last_event = if let Some(recipient) = batch.team_recipient.as_deref() {
                format!(
                    "Team-mined {} block(s) for {} ({} broadcast failure(s)); next difficulty {} bits.",
                    batch.blocks_mined,
                    recipient,
                    publish_failures,
                    batch.last_difficulty_bits,
                )
            } else if publish_failures == 0 {
                format!(
                    "Auto-mined {} block(s), broadcast to peers, scheduled {} for maturity, next difficulty {} bits.",
                    batch.blocks_mined,
                    format_zka(batch.reward_credited),
                    batch.last_difficulty_bits
                )
            } else {
                format!(
                    "Auto-mined {} block(s) ({} broadcast failure(s)), scheduled {} for maturity, next difficulty {} bits.",
                    batch.blocks_mined,
                    publish_failures,
                    format_zka(batch.reward_credited),
                    batch.last_difficulty_bits,
                )
            };
            if let Some(warning) = team_accounting_warning {
                last_event = format!("{last_event} {warning}");
            }
            if rpc_submit_failures > 0 {
                last_event = format!(
                    "{last_event} RPC autopush had {rpc_submit_failures} endpoint failure(s); announce/request sync will keep peers aware of the new head."
                );
            }
            if let Some(sync_event) =
                publish_local_sync_state(&mut node, chain_storage_outer.as_deref())?
            {
                last_event = format!("{last_event} {sync_event}");
            }
            if let Some(announce_event) =
                publish_chain_head_block_announce(&mut node, db_path.as_deref())?
            {
                last_event = format!("{last_event} {announce_event}");
            }
            if let Some(rebroadcast_event) = publish_recent_chain_blocks(
                &mut node,
                db_path.as_deref(),
                sync_rebroadcast_recent_blocks,
            )? {
                last_event = format!("{last_event} {rebroadcast_event}");
            }
            next_mine_at = tokio::time::Instant::now() + Duration::from_secs(mine_interval_secs);
        }

        if mining_task.is_none() {
            if let Some(solution) = pending_team_solution.take() {
                let recipient = solution.template.recipient_address.clone();
                let height = solution.template.height;
                let mining_config = MiningEngineConfig {
                    chain_id: chain_id.clone(),
                    wallet_selector: wallet_address.clone(),
                    password: password.clone(),
                    blocks: 1,
                    mode: network.mining_mode().to_string(),
                    hashrate_hps: 0,
                    threads: 1,
                    base_difficulty: mining_base_difficulty,
                    unsafe_rehearsal_difficulty_bits,
                    target_block_time_secs: mining_target_block_time_secs,
                    chain_db_path: db_path.clone(),
                    rpc_sync_endpoints: rpc_sync_endpoints.clone(),
                    honor_local_mining_control: false,
                    team_solution: Some(solution),
                };
                mining_task = Some(tokio::task::spawn_blocking(move || {
                    run_mining_batch_with_progress(&mining_config, None)
                }));
                last_event = format!(
                    "Assembling verified team solution at height {height} for {recipient}."
                );
            }
        }

        if runtime_mining_enabled
            && mining_task.is_none()
            && tokio::time::Instant::now() >= next_mine_at
        {
            let connected_peers = node.connected_peer_count();
            let rpc_checkpoint_allows_peerless_mining =
                if connected_peers < min_mine_peers && !rpc_sync_endpoints.is_empty() {
                    chain_storage_outer
                        .as_ref()
                        .and_then(|storage| storage.get_chain_height().ok())
                        .filter(|height| *height > 0)
                        .and_then(|local_height| {
                            best_rpc_height_sample(&rpc_sync_endpoints, 3).and_then(|sample| {
                                let matches_rpc_height = sample.height == local_height;
                                let matches_rpc_chain = matches_rpc_height
                                    && ensure_local_chain_matches_rpc_at_height(
                                        &sample.rpc,
                                        db_path.as_deref(),
                                        local_height,
                                    )
                                    .is_ok();
                                matches_rpc_chain.then_some((sample.rpc, local_height))
                            })
                        })
                } else {
                    None
                };
            if connected_peers < min_mine_peers && rpc_checkpoint_allows_peerless_mining.is_none() {
                last_event = format!(
                    "Auto-mining waiting for peers: connected {}, required {}.",
                    connected_peers, min_mine_peers
                );
                next_mine_at = tokio::time::Instant::now() + Duration::from_secs(1);
                continue;
            }
            if let Some((rpc, local_height)) = rpc_checkpoint_allows_peerless_mining {
                // Log instead of assigning to last_event: every path from
                // here overwrites last_event before the dashboard reads it,
                // so the assignment silently discarded this diagnostic.
                mining_log(&format!(
                    "peer gate satisfied by RPC checkpoint at height {local_height} via {rpc}"
                ));
            }
            if let Some(path) = db_path.as_deref() {
                match RocksDbStorage::new(path).and_then(|storage| storage.get_chain_height()) {
                    Ok(local_height) => {
                        if let Some(sample) = best_rpc_height_sample(&rpc_sync_endpoints, 3) {
                            if let Err(err) = ensure_local_chain_matches_rpc_at_height(
                                &sample.rpc,
                                Some(path),
                                local_height.min(sample.height),
                            ) {
                                last_event = err;
                                next_mine_at = tokio::time::Instant::now() + Duration::from_secs(2);
                                continue;
                            }
                            if sample.height > local_height {
                                last_event = format!(
                                    "Auto-mining paused for RPC sync: local height {local_height}, network height {} via {}.",
                                    sample.height, sample.rpc
                                );
                                next_mine_at = tokio::time::Instant::now() + Duration::from_secs(1);
                                continue;
                            }
                            if local_height > sample.height {
                                last_event = format!(
                                    "Auto-mining paused: local height {local_height} is ahead of network bootstrap height {} via {}; waiting for network agreement.",
                                    sample.height, sample.rpc
                                );
                                next_mine_at = tokio::time::Instant::now() + Duration::from_secs(2);
                                continue;
                            }
                        }
                        if let Some(target_height) =
                            peer_chain_view.mining_pause_target(local_height)
                        {
                            last_event = format!(
                                "Auto-mining paused for sync: local height {local_height}, target height {target_height}."
                            );
                            next_mine_at = tokio::time::Instant::now() + Duration::from_secs(1);
                            continue;
                        }
                        // mining_pause_target returned None: either there was
                        // never a pause, or a previously-announced height aged
                        // out without anyone ever delivering it. Clear it now
                        // so a stale value can't resurface on the next peer
                        // announcement at the same height.
                        peer_chain_view.clear_stale_perceived_height(local_height);
                    }
                    Err(err) if is_transient_storage_lock_error(&err.to_string()) => {
                        last_event = format!("Auto-mining paused: storage busy ({err})");
                        next_mine_at = tokio::time::Instant::now() + Duration::from_secs(1);
                        continue;
                    }
                    Err(_err) => {}
                }
            }
            let mining_config = MiningEngineConfig {
                chain_id: chain_id.clone(),
                wallet_selector: wallet_address.clone(),
                password: password.clone(),
                blocks: mine_blocks,
                mode: network.mining_mode().to_string(),
                hashrate_hps: mining_hashrate_hps,
                threads: runtime_mining_threads,
                base_difficulty: mining_base_difficulty,
                unsafe_rehearsal_difficulty_bits,
                target_block_time_secs: mining_target_block_time_secs,
                chain_db_path: db_path.clone(),
                rpc_sync_endpoints: rpc_sync_endpoints.clone(),
                // With --mine the CLI flag owns mining and the HTTP control
                // cannot stop it (runtime_mining_enabled stays true), so the
                // batch must not abort on it either or it would churn
                // spawn/abort forever.
                honor_local_mining_control: !auto_mine,
                team_solution: None,
            };
            let progress_network = network;
            mining_task = Some(tokio::task::spawn_blocking(move || {
                let mut last_state_write = Instant::now();
                let mut last_progress_log = Instant::now() - Duration::from_secs(10);
                let mut publish_progress =
                    |snapshot: &MiningProgressSnapshot| -> Result<(), String> {
                        if last_progress_log.elapsed() >= Duration::from_secs(3) {
                            mining_progress_log(snapshot);
                            last_progress_log = Instant::now();
                        }
                        if last_state_write.elapsed() < Duration::from_secs(2) {
                            return Ok(());
                        }
                        last_state_write = Instant::now();
                        let mut live_state = load_user_mining_state_for_network(progress_network)
                            .unwrap_or_default();
                        live_state.is_mining = true;
                        live_state.mining_process_id = Some(std::process::id());
                        live_state.current_difficulty_bits = snapshot.difficulty_bits;
                        live_state.estimated_hashrate_hps = snapshot.estimated_hashrate_hps as u64;
                        live_state.last_updated = now_secs();
                        let _ = save_user_mining_state_for_network(progress_network, &live_state);
                        Ok(())
                    };
                run_mining_batch_with_progress(&mining_config, Some(&mut publish_progress))
            }));
            last_event = format!(
                "Auto-mining solving {} block(s) with {} using {}; network loop remains active.",
                mine_blocks,
                active_pow_algorithm_for_chain(&chain_id),
                mining_power_label_for_threads(runtime_mining_threads)
            );
            next_mine_at = tokio::time::Instant::now() + Duration::from_secs(1);
            // here are non-fatal; the block is already persisted
        }

        let now = tokio::time::Instant::now();
        let next_mine_in = if runtime_mining_enabled {
            Some(if next_mine_at > now {
                next_mine_at - now
            } else {
                Duration::default()
            })
        } else {
            None
        };
        if mining_console {
            // Fluid streaming console: no full-screen clear/redraw. The
            // `mining: progress …` lines already stream; here we add a compact
            // status line at most every 6s, but immediately when the last event
            // changes so block arrivals / peer / height changes surface without
            // wiping the scrollback.
            if session_mined_blocks > last_console_session_blocks {
                print_block_found_banner(
                    runtime_chain_height,
                    session_mined_rewards.saturating_sub(last_console_session_rewards),
                );
                last_console_session_rewards = session_mined_rewards;
            }
            let now_console = tokio::time::Instant::now();
            if now_console >= next_console_status_at
                || session_mined_blocks != last_console_session_blocks
            {
                if network.is_quantic() {
                    print_quantic_console_status(
                        runtime_chain_height,
                        node.connected_peer_count(),
                        &last_event,
                    );
                } else {
                    // Reuses the dashboard's own 30s-TTL wallet snapshot cache, so
                    // this doesn't re-run the (Argon2-backed) wallet decrypt on
                    // every console tick -- same fix as the earlier CPU-pegging
                    // dashboard bug, just shared rather than re-triggered here.
                    let wallet_snapshot_result = load_wallet_runtime_snapshot_for_dashboard(
                        &storage,
                        &wallet_address,
                        password.as_deref(),
                        network,
                    );
                    let wallet_balance = wallet_snapshot_result
                        .as_ref()
                        .ok()
                        .map(|snapshot| snapshot.total_balance);
                    let wallet_balance_error =
                        wallet_snapshot_result.as_ref().err().map(String::as_str);
                    print_mining_console_status(
                        &wallet_address,
                        runtime_chain_height,
                        node.connected_peer_count(),
                        runtime_mining_enabled || mining_active,
                        runtime_mining_state.estimated_hashrate_hps,
                        wallet_balance,
                        wallet_balance_error,
                        session_mined_blocks,
                        session_mined_rewards,
                        &last_event,
                    );
                }
                next_console_status_at = now_console + Duration::from_secs(6);
                last_console_session_blocks = session_mined_blocks;
            }
        } else if let Err(err) = draw_fullnode_runtime_screen(
            &node,
            network,
            &storage,
            &wallet_address,
            password.as_deref(),
            db_path.as_deref(),
            mining_task.is_some(),
            session_started.elapsed(),
            "running",
            None,
            &announced,
            seed_peer_display.as_deref(),
            runtime_mining_enabled,
            mine_interval_secs,
            mine_blocks,
            if runtime_mining_enabled {
                Some(runtime_mining_threads)
            } else {
                None
            },
            next_mine_in,
            session_mined_blocks,
            session_mined_rewards,
            &last_event,
        ) {
            mining_log(&format!("dashboard render error (non-fatal): {err}"));
        }

        // Drain anything the HTTP API pushed onto the private-tx channel
        // (e.g. `/private/wallet/send-from-seed` calls). For each tx we
        // received, gossip it over libp2p so connected miners pick it up
        // and include it in their next block. Best-effort: a serialize
        // or publish failure is logged but never aborts the main loop.
        if let Some(rx) = private_tx_recv_outer.as_mut() {
            while let Ok(broadcast_tx) = rx.try_recv() {
                let hash_short: String = broadcast_tx.tx_hash.chars().take(16).collect();
                let broadcast_body = serde_json::to_vec(&broadcast_tx).ok();
                if let Some(bytes) = broadcast_body.as_ref() {
                    node.cache_private_tx_body(&broadcast_tx.tx_hash, bytes.clone());
                }
                // Write to the sidecar so the local mining loop, which
                // cannot open the DB while StateManager holds the write
                // lock, picks up the TX on its next block attempt.
                if let Some(db) = db_path.as_deref() {
                    if let Some(bytes) = broadcast_body.as_ref() {
                        let sidecar = private_tx_sidecar_dir(db);
                        let _ = std::fs::create_dir_all(&sidecar);
                        let _ = std::fs::write(
                            sidecar.join(format!("{}.privtx", broadcast_tx.tx_hash)),
                            bytes,
                        );
                    }
                }
                match serde_json::to_value(&broadcast_tx).and_then(|v| {
                    serde_json::from_value::<zoka::network::message::PrivateTransactionMessage>(v)
                }) {
                    Ok(msg) => {
                        let announce =
                            zoka::network::message::PrivateTxAnnounceMessage::from_private_tx(
                                &msg,
                                broadcast_body.as_ref().map(|bytes| bytes.len() as u64),
                            );
                        if let Err(e) = node.publish_message(
                            &zoka::network::message::ZokaMessage::PrivateTxAnnounce(announce),
                        ) {
                            mining_log(&format!(
                                "p2p announce private tx {hash_short}...: publish failed: {e}"
                            ));
                        }
                        if let Err(e) = node.publish_message(
                            &zoka::network::message::ZokaMessage::PrivateTransaction(msg),
                        ) {
                            mining_log(&format!(
                                "p2p gossip private tx {hash_short}…: publish failed: {e}"
                            ));
                        } else if verbose {
                            mining_log(&format!("p2p gossip private tx {hash_short}… emitted"));
                        }
                    }
                    Err(e) => mining_log(&format!(
                        "p2p gossip private tx {hash_short}…: convert failed: {e}"
                    )),
                }
            }
        }

        match timeout(Duration::from_millis(800), node.next_sourced_message()).await {
            Ok(Ok(sourced)) => {
                let message = sourced.message;
                last_event = match handle_fullnode_network_message(
                    &mut node,
                    &message,
                    sourced.source,
                    &team_mining_runtime,
                    &chain_id,
                    db_path.as_deref(),
                    chain_storage_outer.as_deref(),
                    &mut peer_chain_view,
                    unsafe_rehearsal_difficulty_bits,
                )
                .await
                {
                    Some(verdict) => {
                        remember_sync_event(
                            "p2p",
                            &verdict,
                            &mut last_sync_source,
                            &mut last_sync_event,
                            &mut last_sync_error,
                            &mut rpc_fallback_active,
                        );
                        verdict
                    }
                    None => {
                        if verbose {
                            format!(
                                "recv {}",
                                truncate_dashboard_text(
                                    &serde_json::to_string(&message)
                                        .unwrap_or_else(|_| "<invalid>".to_string()),
                                    120
                                )
                            )
                        } else {
                            "Network message received.".to_string()
                        }
                    }
                };
            }
            Ok(Err(err)) => {
                last_event = format!("receive error: {err}");
                remember_sync_event(
                    "p2p",
                    &last_event,
                    &mut last_sync_source,
                    &mut last_sync_event,
                    &mut last_sync_error,
                    &mut rpc_fallback_active,
                );
            }
            Err(_) => {
                node.pump_for(Duration::from_millis(100))
                    .await
                    .map_err(|e| format!("pump error: {e}"))?;
            }
        }
        publish_runtime_sync_status(
            &node,
            chain_storage_outer.as_ref(),
            &peer_chain_view,
            min_mine_peers,
            rpc_fallback_active,
            last_sync_source.as_deref(),
            last_sync_event.as_deref(),
            last_sync_error.as_deref(),
        )
        .await;
    }

    let next_mine_in = if auto_mine {
        Some(Duration::default())
    } else {
        None
    };
    if mining_console {
        let height = current_known_height_for_network(db_path.as_deref(), network);
        println!(
            "{}  height={}  peers={}  session={} block(s) / {}  {} {}",
            paint("stopped", ANSI_RED),
            paint(&height.to_string(), ANSI_BOLD),
            node.connected_peer_count(),
            paint(&session_mined_blocks.to_string(), ANSI_GREEN),
            paint(&format_zka_amount(session_mined_rewards), ANSI_GREEN),
            paint("|", ANSI_DIM),
            paint(&truncate_dashboard_text(&last_event, 80), ANSI_DIM),
        );
    } else {
        draw_fullnode_runtime_screen(
            &node,
            network,
            &storage,
            &wallet_address,
            password.as_deref(),
            db_path.as_deref(),
            false,
            session_started.elapsed(),
            "stopped",
            None,
            &announced,
            seed_peer_display.as_deref(),
            auto_mine,
            mine_interval_secs,
            mine_blocks,
            if auto_mine {
                Some(mining_threads)
            } else {
                None
            },
            next_mine_in,
            session_mined_blocks,
            session_mined_rewards,
            &last_event,
        )?;
    }
    Ok(())
}

/// Keep the very large fullnode state machine off the caller's stack.
///
/// The fullnode future owns the networking, sync, solo-mining and team-mining
/// loop state. Embedding it directly in the top-level command dispatcher can
/// overflow the default Windows main-thread stack in debug builds before the
/// first startup message is printed.
async fn cmd_fullnode_auto_boxed(args: &[String]) -> Result<(), String> {
    Box::pin(cmd_fullnode_auto(args)).await
}

// Zenith: no se llama en una build de Quantic (Zenith se retiró el 21/10/2026).
#[cfg_attr(feature = "quantic", allow(dead_code))]
async fn run_zokahd_guided_default() -> Result<(), String> {
    println!("{}", dashboard_title("ZOKA Node Autopilot", ANSI_CYAN));
    println!("{}", dashboard_divider());
    println!(
        "{}",
        paint(
            "Automatic startup: wallet, node sync, then mining when the local chain is ready.",
            ANSI_DIM
        )
    );
    let storage = wallet_storage_for_network(NetworkProfile::Zenith)?;
    let wallet_names = local_private_wallet_file_names(&storage);
    let wallet_addresses = local_private_wallet_address_choices(&storage);
    println!(
        "{}",
        dashboard_line("Wallet storage", wallet_storage_path_text())
    );
    if wallet_names.is_empty() {
        println!("No local wallet files found. ZOKA will create an encrypted node wallet now.");
    } else {
        println!("{}", dashboard_section("Local Wallets", ANSI_MAGENTA));
        for (name, address) in wallet_addresses.iter().take(8) {
            println!("  {}  {}", paint(name, ANSI_WHITE), address);
        }
        if wallet_addresses.len() > 8 {
            println!("  ... {} more", wallet_addresses.len().saturating_sub(8));
        }
    }

    let default_wallet_address = wallet_addresses.first().map(|(_, address)| address.clone());
    if let Some(wallet) = default_wallet_address.as_deref() {
        println!("{}", dashboard_line("Selected wallet", wallet));
        println!("To use a different wallet, run: zokahd --address <wallet-address>");
    } else {
        println!(
            "{}",
            dashboard_line("Selected wallet", "auto-create node-wallet")
        );
    }
    println!("{}", dashboard_line("Node DB", DEFAULT_CHAIN_DB_PATH));
    println!(
        "{}",
        dashboard_line("Network", "configured seeds, sync-first")
    );
    println!("{}", dashboard_line("Mining", "automatic after sync"));
    println!(
        "{}",
        dashboard_line(
            "CPU power",
            mining_power_label_for_threads(mining_threads_for_power_preset("normal")?)
        )
    );
    println!("{}", dashboard_divider());

    let password = if default_wallet_address.is_some() {
        prompt_required("Wallet password: ")?
    } else {
        prompt_required("Create wallet password: ")?
    };
    let auto_mine = std::env::var("ZOKA_ZOKAHD_AUTOPILOT_NO_MINE").as_deref() != Ok("1");
    let use_network_config =
        std::env::var("ZOKA_ZOKAHD_AUTOPILOT_NO_NETWORK_CONFIG").as_deref() != Ok("1");
    let seconds = std::env::var("ZOKA_ZOKAHD_AUTOPILOT_SECONDS").ok();
    let power =
        std::env::var("ZOKA_ZOKAHD_AUTOPILOT_POWER").unwrap_or_else(|_| "normal".to_string());
    let threads = if auto_mine {
        Some(mining_threads_for_power_preset(&power)?)
    } else {
        None
    };

    let mut args = vec![
        "--network".to_string(),
        "mainnet".to_string(),
        "--password".to_string(),
        password,
        "--db".to_string(),
        DEFAULT_CHAIN_DB_PATH.to_string(),
        "--allow-isolated-start".to_string(),
        "--sync-warmup-secs".to_string(),
        "10".to_string(),
    ];
    push_opt_arg(&mut args, "--address", default_wallet_address);
    push_opt_arg(&mut args, "--seconds", seconds);
    if !auto_mine {
        args.push("--no-mine".to_string());
    }
    if let Some(threads) = threads {
        args.push("--threads".to_string());
        args.push(threads.to_string());
    }
    if !use_network_config {
        args.push("--no-network-config".to_string());
    }
    cmd_fullnode_auto_boxed(&args).await
}

fn cmd_wallet_restore_from_keys(args: &[String]) -> Result<(), String> {
    let password = required_password_value(args)?;
    let (_, storage) = wallet_storage_for_args(args)?;
    let wallet_name = option_value(args, "--name")
        .unwrap_or_else(|| generate_available_private_wallet_name(&storage, "wallet"));
    let scan_key_hex = required_option_value(args, "--scan-key-hex")?;
    let spend_key_hex = option_value(args, "--spend-key-hex");
    let spend_public_key_hex = option_value(args, "--spend-view-key-hex")
        .or_else(|| option_value(args, "--spend-public-key-hex"));
    let view_only = args.iter().any(|a| a == "--view-only");
    let restore_height = parse_u64_arg(
        option_value(args, "--restore-height"),
        0,
        "--restore-height",
    )?;

    if private_wallet_exists_on_disk(&storage, &wallet_name) {
        return Err(format!("private wallet already exists: {}", wallet_name));
    }

    let scan_key = hex::decode(&scan_key_hex)
        .map_err(|_| "invalid --scan-key-hex (hex expected)".to_string())?;
    if scan_key.len() != 32 {
        return Err("invalid --scan-key-hex length (expected 32 bytes)".to_string());
    }

    let (spending_key, spend_public_hex, effective_view_only) =
        match (spend_key_hex, spend_public_key_hex) {
            (Some(spend_hex), _) => {
                let sk = hex::decode(&spend_hex)
                    .map_err(|_| "invalid --spend-key-hex (hex expected)".to_string())?;
                if sk.len() != 32 {
                    return Err("invalid --spend-key-hex length (expected 32 bytes)".to_string());
                }
                let pub_hex = public_key_hex_from_secret(&sk);
                (sk, pub_hex, view_only)
            }
            (None, Some(spend_pub)) => {
                if !view_only {
                    return Err(
                        "using a spend view key requires --view-only (no spend secret present)"
                            .to_string(),
                    );
                }
                let bytes = hex::decode(&spend_pub)
                    .map_err(|_| "invalid --spend-view-key-hex (hex expected)".to_string())?;
                if bytes.len() != 32 {
                    return Err(
                        "invalid --spend-view-key-hex length (expected 32 bytes compressed point)"
                            .to_string(),
                    );
                }
                (Vec::new(), spend_pub, true)
            }
            (None, None) => {
                return Err(
                    "missing spend key material: provide --spend-key-hex or --spend-view-key-hex"
                        .to_string(),
                );
            }
        };

    let commitment_key = private_payment_address(&scan_key, &spend_public_hex);
    let mut wallet = PrivateWallet::new(
        wallet_name.clone(),
        spending_key,
        scan_key,
        commitment_key.clone(),
    );
    wallet.view_only = effective_view_only;
    wallet.restore_height = restore_height;
    wallet.anonymity_public_key = spend_public_hex;

    // Toma el lock del directorio: este proceso corre en paralelo con el nodo,
    // y sin coordinacion el ultimo en escribir pisa lo del otro.
    let _wallet_lock = storage
        .lock_private_wallets()
        .map_err(|e| format!("failed to lock wallet directory: {e}"))?;
    storage
        .save_private_wallet(&wallet, Some(&password))
        .map_err(|e| format!("failed to save private wallet: {e}"))?;

    println!("Private wallet restored from keys");
    println!("Name: {}", wallet_name);
    println!("Address: {}", user_private_address_display(&commitment_key));
    print_wallet_ready_notice(&storage, &wallet_name);
    println!("View-only: {}", wallet.view_only);
    println!("Restore height: {}", restore_height);
    Ok(())
}

fn cmd_private_wallet_create(args: &[String]) -> Result<(), String> {
    let password = required_password_value(args)?;
    let export_path = option_value(args, "--export-secrets");
    let show_keys = has_flag(args, "--show-keys");
    let seed_words = parse_u64_arg(option_value(args, "--seed-words"), 24, "--seed-words")?;
    let entropy_bytes = seed_words_to_entropy_bytes(seed_words)?;
    let (network, storage) = wallet_storage_for_args(args)?;
    let wallet_name = option_value(args, "--name")
        .unwrap_or_else(|| generate_available_private_wallet_name(&storage, "wallet"));
    if private_wallet_exists_on_disk(&storage, &wallet_name) {
        return Err(format!("private wallet already exists: {}", wallet_name));
    }
    let mut manager = load_private_wallet_manager_for_network(&storage, Some(&password), network)?;
    let commitment = manager
        .create_private_wallet_with_entropy_bytes(&wallet_name, entropy_bytes)
        .map_err(|e| format!("private wallet create failed: {e}"))?;
    persist_private_wallet(&storage, &manager, &wallet_name, Some(&password))?;
    let wallet = manager
        .get_private_wallet(&wallet_name)
        .ok_or_else(|| format!("private wallet not found after create: {}", wallet_name))?;
    let secrets = private_wallet_secrets_export(wallet);
    println!("Private wallet created");
    println!("Name: {}", wallet_name);
    println!("Address: {}", user_private_address_display(&commitment));
    print_wallet_ready_notice(&storage, &wallet_name);
    if let Some(mnemonic) = secrets.mnemonic.as_deref() {
        println!("Recovery phrase: {}", mnemonic);
        println!("Keep the recovery phrase offline. Anyone with it can restore this wallet.");
    } else {
        println!("Recovery phrase: unavailable");
    }
    if show_keys {
        println!("Scan private key (hex): {}", secrets.scan_private_key_hex);
        println!("Spend private key (hex): {}", secrets.spend_private_key_hex);
    }
    if let Some(path) = export_path {
        write_secrets_export(&path, &secrets)?;
        println!("Secrets exported to {}", path);
    } else {
        println!(
            "Tip: use wallet-export-secrets when you need a full encrypted/offline backup of keys."
        );
    }
    Ok(())
}

fn cmd_wallet_generate_seed(args: &[String]) -> Result<(), String> {
    let seed_words = parse_u64_arg(option_value(args, "--seed-words"), 24, "--seed-words")?;
    let entropy_bytes = seed_words_to_entropy_bytes(seed_words)?;
    let as_json = has_flag(args, "--json");
    let seed = HDWallet::generate_seed(entropy_bytes)
        .map_err(|e| format!("failed to generate recovery seed: {e}"))?;
    let mnemonic = HDWallet::new(seed)
        .and_then(|wallet| wallet.seed_to_mnemonic())
        .map_err(|e| format!("failed to encode recovery phrase: {e}"))?;
    let phrase = mnemonic.join(" ");
    if as_json {
        println!(
            "{}",
            serde_json::json!({
                "seed_words": seed_words,
                "recovery_phrase": phrase,
            })
        );
    } else {
        println!("Recovery phrase: {}", phrase);
        println!("Seed words: {}", seed_words);
    }
    Ok(())
}

fn cmd_wallet_export_secrets(args: &[String]) -> Result<(), String> {
    let wallet_selector = required_wallet_selector(args)?;
    let password = password_value(args)?;
    let out = option_value(args, "--out");
    let as_json = has_flag(args, "--json");

    let (network, storage) = wallet_storage_for_args(args)?;
    let mut manager =
        load_private_wallet_manager_for_network(&storage, password.as_deref(), network)?;
    let wallet_name = resolve_private_wallet_name_required(&manager, &wallet_selector)?;
    let wallet = manager
        .get_private_wallet_mut(&wallet_name)
        .ok_or_else(|| format!("wallet not found: {wallet_name}"))?;
    wallet.normalize_key_material();
    let export = private_wallet_secrets_export(wallet);

    if let Some(path) = out {
        write_secrets_export(&path, &export)?;
        println!("Secrets exported to {}", path);
    }

    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&export)
                .map_err(|e| format!("failed to encode secrets json: {e}"))?
        );
    } else {
        println!("Wallet secrets");
        println!("Name: {}", export.wallet_name);
        println!("Address: {}", export.user_address);
        if let Some(mnemonic) = export.mnemonic {
            println!("Recovery phrase: {}", mnemonic);
        } else {
            println!("Recovery phrase: unavailable");
        }
        println!("Scan private key (hex): {}", export.scan_private_key_hex);
        println!("Spend private key (hex): {}", export.spend_private_key_hex);
    }

    Ok(())
}

fn cmd_private_wallet_list(args: &[String]) -> Result<(), String> {
    let password = password_value(args)?;
    let as_json = has_flag(args, "--json");
    let (network, storage) = wallet_storage_for_args(args)?;
    // --rpc-only: skip the local chain DB entirely and sync straight from a
    // network RPC. Callers that also run their own fullnode pointed at the
    // same local DB (e.g. ZSilent's embedded node) would otherwise hit a
    // RocksDB lock conflict on every call, fall back to RPC anyway, and pay
    // for a wasted local-open attempt first.
    let db_path = if has_flag(args, "--rpc-only") {
        None
    } else {
        option_value(args, "--db").or_else(|| Some(network.default_db_path().to_string()))
    };
    let disk_wallets_raw = storage
        .load_all_private_wallet_metadata()
        .map_err(|e| format!("failed to read private wallet files: {e}"))?;
    let mut disk_wallets: Vec<PrivateWalletMetadata> = Vec::new();
    for wallet in disk_wallets_raw {
        if let Some(existing) = disk_wallets
            .iter_mut()
            .find(|existing| existing.commitment_key == wallet.commitment_key)
        {
            let preferred_name = preferred_private_wallet_name(&existing.name, &wallet.name);
            let existing_score = (existing.private_transaction_count, existing.private_balance);
            let candidate_score = (wallet.private_transaction_count, wallet.private_balance);
            if candidate_score > existing_score {
                *existing = wallet;
            }
            existing.name = preferred_name;
            continue;
        }
        disk_wallets.push(wallet);
    }
    // Con contraseña este comando sincroniza y guarda: el candado va antes de
    // leer (ver `persist_private_wallet`). ZSilent lo llama seguido para
    // refrescar el saldo, así que es el que más se cruza con un envío.
    let _wallet_lock = if password.is_some() {
        Some(
            storage
                .lock_private_wallets()
                .map_err(|e| format!("failed to lock wallet directory: {e}"))?,
        )
    } else {
        None
    };
    let mut manager =
        load_private_wallet_manager_for_network(&storage, password.as_deref(), network)?;
    if password.is_some() {
        let wallet_names: Vec<String> = manager
            .list_private_wallets()
            .into_iter()
            .map(|(name, _, _, _)| name)
            .collect();
        for wallet_name in wallet_names {
            let storage_ref = &storage;
            let manager_ref: &mut PrivateWalletManager = &mut manager;
            let pwd = password.as_deref();
            let db = db_path.as_deref();
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                sync_private_coinbase_outputs_from_chain_db(
                    storage_ref,
                    manager_ref,
                    &wallet_name,
                    pwd,
                    network,
                    db,
                )
            }));
        }
    }
    let mining_state = std::panic::catch_unwind(|| {
        load_user_mining_state_for_network(network).unwrap_or_default()
    })
    .unwrap_or_default();
    let unlocked_by_name: std::collections::HashMap<_, _> = manager
        .list_private_wallets()
        .into_iter()
        .map(|(name, commitment, balance, tx_count)| (name, (commitment, balance, tx_count)))
        .collect();
    let rows: Vec<_> = disk_wallets
        .iter()
        .map(|wallet| {
            let unlocked = unlocked_by_name.get(&wallet.name);
            let (commitment, balance, tx_count, is_unlocked) =
                if let Some((commitment, balance, tx_count)) = unlocked {
                    (commitment.clone(), *balance, *tx_count, true)
                } else {
                    (
                        wallet.commitment_key.clone(),
                        wallet.private_balance,
                        wallet.private_transaction_count,
                        false,
                    )
                };
            let address = user_private_address_display(&commitment);
            let locked_balance = locked_rewards_for_address(&mining_state, &address);
            (
                wallet.name.clone(),
                address,
                balance,
                locked_balance,
                tx_count,
                wallet.encrypted,
                is_unlocked,
            )
        })
        .collect();
    if as_json {
        let json_rows: Vec<_> = rows
            .into_iter()
            .map(
                |(name, address, balance, locked_balance, tx_count, encrypted, unlocked)| {
                    serde_json::json!({
                        "name": name,
                        "address": address,
                        "balance_atoms": balance,
                        "balance": format_zka_amount(balance),
                        "locked_balance_atoms": locked_balance,
                        "locked_balance": format_zka_amount(locked_balance),
                        "total_balance_atoms": balance.saturating_add(locked_balance),
                        "total_balance": format_zka_amount(balance.saturating_add(locked_balance)),
                        "tx_count": tx_count,
                        "encrypted": encrypted,
                        "unlocked": unlocked,
                    })
                },
            )
            .collect();
        println!(
            "{}",
            serde_json::json!({
                "network": network.label(),
                "network_role": network.role(),
                "network_name": network.display_name(),
                "wallet_storage": wallet_storage_path_text_for(&storage),
                "private_wallets": json_rows
            })
        );
        return Ok(());
    }
    if rows.is_empty() {
        println!("No private wallets found");
        return Ok(());
    }
    println!("Wallets ({})", network.display_name());
    println!("Storage: {}", wallet_storage_path_text_for(&storage));
    let any_unlocked = rows.iter().any(|row| row.6);
    if password.is_some() && !any_unlocked {
        println!("No private wallets could be unlocked with the provided password.");
        println!("The local wallet files are still listed below so you can confirm their names.");
    }
    for (name, address, balance, locked_balance, tx_count, encrypted, unlocked) in rows {
        let status = if unlocked {
            "unlocked"
        } else if encrypted {
            "locked"
        } else {
            "available"
        };
        if unlocked {
            println!(
                "- {} | {} | {} | available={} | locked={} | total={} | tx_count={}",
                name,
                address,
                status,
                format_zka(balance),
                format_zka(locked_balance),
                format_zka(balance.saturating_add(locked_balance)),
                tx_count
            );
        } else {
            println!(
                "- {} | {} | {} | add --password to unlock balance/send",
                name, address, status
            );
        }
    }
    Ok(())
}

fn cmd_private_balance(args: &[String]) -> Result<(), String> {
    let (network, storage) = wallet_storage_for_args(args)?;
    let wallet_selector = required_wallet_selector(args)?;
    let password = password_value(args)?;
    let db_path = if has_flag(args, "--rpc-only") {
        None
    } else {
        option_value(args, "--db").or_else(|| Some(network.default_db_path().to_string()))
    };
    let as_json = has_flag(args, "--json");
    // Sincroniza y guarda: el candado va antes de leer (ver
    // `persist_private_wallet`).
    let _wallet_lock = storage
        .lock_private_wallets()
        .map_err(|e| format!("failed to lock wallet directory: {e}"))?;
    let mut manager =
        load_private_wallet_manager_for_network(&storage, password.as_deref(), network)?;
    let wallet_name = resolve_private_wallet_name_required(&manager, &wallet_selector)?;
    sync_private_wallet_history_from_chain_db(
        &storage,
        &mut manager,
        &wallet_name,
        password.as_deref(),
        network,
        db_path.as_deref(),
    )?;
    sync_private_coinbase_outputs_from_chain_db(
        &storage,
        &mut manager,
        &wallet_name,
        password.as_deref(),
        network,
        db_path.as_deref(),
    )?;
    let wallet = manager
        .get_private_wallet(&wallet_name)
        .ok_or_else(|| format!("private wallet not found: {wallet_name}"))?;
    let address = user_private_address_display(&wallet.commitment_key);
    let mining_state = load_user_mining_state_for_network(network).unwrap_or_default();
    let locked_balance = locked_rewards_for_address(&mining_state, &address);
    if as_json {
        println!(
            "{}",
            serde_json::json!({
                "wallet": wallet_name,
                "address": address,
                "balance_atoms": wallet.private_balance,
                "balance": format_zka_amount(wallet.private_balance),
                "locked_balance_atoms": locked_balance,
                "locked_balance": format_zka_amount(locked_balance),
                "total_balance_atoms": wallet.private_balance.saturating_add(locked_balance),
                "total_balance": format_zka_amount(wallet.private_balance.saturating_add(locked_balance)),
                "view_only": wallet.view_only,
            })
        );
        return Ok(());
    }
    println!("Wallet: {}", wallet_name);
    println!("Address: {}", address);
    println!("Available balance: {}", format_zka(wallet.private_balance));
    println!("Locked balance: {}", format_zka(locked_balance));
    println!(
        "Total balance: {}",
        format_zka(wallet.private_balance.saturating_add(locked_balance))
    );
    println!("View-only: {}", wallet.view_only);
    Ok(())
}

fn cmd_receive_address(args: &[String]) -> Result<(), String> {
    let wallet_selector = required_wallet_selector(args)?;
    let password = password_value(args)?;
    let as_json = has_flag(args, "--json");
    let (network, storage) = wallet_storage_for_args(args)?;
    let manager = load_private_wallet_manager_for_network(&storage, password.as_deref(), network)?;
    let wallet_name = resolve_private_wallet_name_required(&manager, &wallet_selector)?;
    let wallet = manager
        .get_private_wallet(&wallet_name)
        .ok_or_else(|| format!("private wallet not found: {wallet_name}"))?;
    let address = user_private_address_display(&wallet.commitment_key);
    // La dirección v2 (zkq1) solo se muestra si se pide: hasta que la red fije
    // su activación nadie puede pagarle, y la que hay que compartir es la zka1.
    let hybrid_address = if has_flag(args, "--hybrid") {
        Some(
            wallet
                .hybrid_payment_address()
                .map_err(|e| format!("hybrid address unavailable: {e}"))?,
        )
    } else {
        None
    };
    let hybrid_activation = economics::pq_hybrid_outputs_activation_height();
    if as_json {
        let mut out = serde_json::json!({
            "wallet": wallet_name,
            "address": address,
            "view_only": wallet.view_only,
        });
        if let Some(hybrid) = hybrid_address.as_deref() {
            out["hybrid_address"] = serde_json::json!(hybrid);
            out["hybrid_activation_height"] = serde_json::json!(hybrid_activation);
        }
        println!("{out}");
        return Ok(());
    }
    println!("Receive ZKA");
    println!("Wallet: {}", wallet_name);
    println!("Address: {}", address);
    println!("Share this address to receive private ZKA.");
    if let Some(hybrid) = hybrid_address.as_deref() {
        println!();
        println!(
            "Hybrid address (zkq1, post-quantum, {} chars):",
            hybrid.len()
        );
        println!("{hybrid}");
        match hybrid_activation {
            Some(height) => println!("Senders can pay it from block {height} on."),
            None => println!(
                "Not active on this network yet: nobody can pay it. Keep sharing the zka1 address."
            ),
        }
    }
    Ok(())
}

fn cmd_private_history(args: &[String]) -> Result<(), String> {
    let (network, storage) = wallet_storage_for_args(args)?;
    let wallet_selector = required_wallet_selector(args)?;
    let password = password_value(args)?;
    let db_path = if has_flag(args, "--rpc-only") {
        None
    } else {
        option_value(args, "--db").or_else(|| Some(network.default_db_path().to_string()))
    };
    let as_json = has_flag(args, "--json");
    let verbose = has_flag(args, "--verbose");
    let limit = parse_u64_arg(option_value(args, "--limit"), 20, "--limit")? as usize;
    // Sincroniza y guarda: el candado va antes de leer (ver
    // `persist_private_wallet`).
    let _wallet_lock = storage
        .lock_private_wallets()
        .map_err(|e| format!("failed to lock wallet directory: {e}"))?;
    let mut manager =
        load_private_wallet_manager_for_network(&storage, password.as_deref(), network)?;
    let wallet_name = resolve_private_wallet_name_required(&manager, &wallet_selector)?;
    sync_private_wallet_history_from_chain_db(
        &storage,
        &mut manager,
        &wallet_name,
        password.as_deref(),
        network,
        db_path.as_deref(),
    )?;
    sync_private_coinbase_outputs_from_chain_db(
        &storage,
        &mut manager,
        &wallet_name,
        password.as_deref(),
        network,
        db_path.as_deref(),
    )?;
    let wallet = manager
        .get_private_wallet(&wallet_name)
        .ok_or_else(|| format!("private wallet not found: {wallet_name}"))?;
    let txs: Vec<_> = wallet
        .private_transactions
        .iter()
        .rev()
        .take(limit)
        .cloned()
        .collect();
    if as_json {
        if verbose {
            println!(
                "{}",
                serde_json::json!({
                    "wallet": wallet_name,
                    "count": txs.len(),
                    "privacy": {
                        "amounts_hidden": false,
                        "full_private_transaction_bodies": true
                    },
                    "transactions": txs
                })
            );
            return Ok(());
        }
        let summaries: Vec<_> = txs
            .iter()
            .map(|tx| {
                serde_json::json!({
                    "tx_hash": tx.tx_hash,
                    "status": tx.status.to_string(),
                    "timestamp": tx.timestamp,
                    "outputs": tx.outputs.len(),
                    "amount_hidden": true,
                    "fee_hidden": true,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::json!({
                "wallet": wallet_name,
                "count": txs.len(),
                "privacy": {
                    "amounts_hidden": true,
                    "fees_hidden": true,
                    "full_private_transaction_bodies": false
                },
                "transactions": summaries
            })
        );
        return Ok(());
    }
    if txs.is_empty() {
        println!("No private transactions for {}", wallet_name);
        return Ok(());
    }
    println!("Private transactions for {}:", wallet_name);
    for tx in txs {
        println!(
            "- {} | status={:?} | fee={} | outputs={}",
            tx.tx_hash,
            tx.status,
            format_zka(tx.fee),
            tx.outputs.len()
        );
    }
    Ok(())
}

fn sync_private_wallet_history_from_chain_db(
    storage: &WalletStorage,
    manager: &mut PrivateWalletManager,
    wallet_name: &str,
    password: Option<&str>,
    network: NetworkProfile,
    db_path: Option<&str>,
) -> Result<usize, String> {
    let Some(db_path) = db_path else {
        return sync_private_wallet_history_from_available_rpc(
            storage,
            manager,
            wallet_name,
            password,
            network,
        );
    };
    let wallet = manager
        .get_private_wallet(wallet_name)
        .ok_or_else(|| format!("private wallet not found: {wallet_name}"))?;
    let known_hashes: std::collections::HashSet<String> = wallet
        .private_transactions
        .iter()
        .map(|tx| tx.tx_hash.clone())
        .collect();

    let chain_storage = match open_storage(db_path) {
        Ok(storage) => storage,
        Err(e) if is_storage_lock_error(&e) => {
            return sync_private_wallet_history_from_available_rpc(
                storage,
                manager,
                wallet_name,
                password,
                network,
            );
        }
        Err(e) => return Err(e),
    };
    let height = chain_storage
        .get_chain_height()
        .map_err(|e| format!("failed to read chain height: {e}"))?;
    if height == 0 {
        return sync_private_wallet_history_from_available_rpc(
            storage,
            manager,
            wallet_name,
            password,
            network,
        );
    }
    let mut confirmed = Vec::new();
    let mut imported = 0usize;
    for h in 1..=height {
        let Some(block) = chain_storage
            .get_block_by_height(h)
            .map_err(|e| format!("failed to read block at height {h}: {e}"))?
        else {
            continue;
        };
        for hash in block.private_tx_hashes {
            confirmed.push(hash.clone());
            if known_hashes.contains(&hash) {
                continue;
            }
            let Some(body) = chain_storage
                .get_private_transaction_bytes(&hash)
                .map_err(|e| format!("failed to read private tx body {hash}: {e}"))?
            else {
                continue;
            };
            let tx: zoka::private_wallet::PrivateTransaction = serde_json::from_slice(&body)
                .map_err(|e| format!("failed to parse private tx body {hash}: {e}"))?;
            let changed = manager
                .apply_relevant_private_transaction_for_wallet(wallet_name, &tx)
                .map_err(|e| {
                    format!("failed to scan private tx {hash} for wallet '{wallet_name}': {e}")
                })?;
            if changed {
                imported += 1;
            }
        }
    }
    confirmed.sort();
    confirmed.dedup();
    let updated = manager.mark_private_transactions_confirmed_locally(&confirmed);
    if imported > 0 || updated > 0 {
        persist_private_wallet(storage, manager, wallet_name, password)?;
    }
    Ok(imported + updated)
}

fn sync_private_wallet_history_from_available_rpc(
    storage: &WalletStorage,
    manager: &mut PrivateWalletManager,
    wallet_name: &str,
    password: Option<&str>,
    network: NetworkProfile,
) -> Result<usize, String> {
    let mut errors = Vec::new();
    for rpc in wallet_sync_rpc_urls(network) {
        match sync_private_wallet_history_from_rpc(storage, manager, wallet_name, password, &rpc) {
            Ok(imported) => return Ok(imported),
            Err(err) => errors.push(format!("{rpc}: {err}")),
        }
    }
    if errors.is_empty() {
        Err(format!(
            "no wallet sync RPC endpoint configured for {}",
            network.display_name()
        ))
    } else {
        Err(format!(
            "wallet history RPC failed for every configured endpoint: {}",
            errors.join("; ")
        ))
    }
}

fn sync_private_wallet_history_from_rpc(
    storage: &WalletStorage,
    manager: &mut PrivateWalletManager,
    wallet_name: &str,
    password: Option<&str>,
    rpc: &str,
) -> Result<usize, String> {
    let height_body = rpc_http_get_json(rpc, "/chain/height", 3)?;
    let height = rpc_json_u64(&height_body, &["height", "chain_height"])
        .ok_or_else(|| "RPC height response did not include height".to_string())?;

    let wallet = manager
        .get_private_wallet(wallet_name)
        .ok_or_else(|| format!("private wallet not found: {wallet_name}"))?;
    let known_hashes: std::collections::HashSet<String> = wallet
        .private_transactions
        .iter()
        .map(|tx| tx.tx_hash.clone())
        .collect();

    let mut confirmed = Vec::new();
    let mut imported = 0usize;
    let from_height = wallet.restore_height.max(1);
    let mut range_from = from_height;
    while range_from <= height {
        let range_to = range_from.saturating_add(255).min(height);
        let range_private = rpc_http_get_json(
            rpc,
            &format!("/blocks/range/{range_from}/{range_to}/private_tx_hashes"),
            5,
        )?;
        let blocks = range_private
            .get("blocks")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        for block_private in blocks {
            let hashes = block_private
                .get("private_tx_hashes")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            for value in hashes {
                let Some(hash) = value.as_str() else {
                    continue;
                };
                confirmed.push(hash.to_string());
                if known_hashes.contains(hash) {
                    continue;
                }
                let tx_json = match rpc_http_get_json_with_rate_limit_backoff(
                    rpc,
                    &format!("/private/tx/{hash}"),
                    10,
                ) {
                    Ok(body) => body,
                    Err(_) => continue,
                };
                let tx: zoka::private_wallet::PrivateTransaction = serde_json::from_value(tx_json)
                    .map_err(|e| format!("failed to parse private tx {hash} from RPC: {e}"))?;
                let changed = manager
                    .apply_relevant_private_transaction_for_wallet(wallet_name, &tx)
                    .map_err(|e| {
                        format!("failed to scan private tx {hash} for wallet '{wallet_name}': {e}")
                    })?;
                if changed {
                    imported += 1;
                }
            }
        }
        range_from = range_to.saturating_add(1);
    }

    confirmed.sort();
    confirmed.dedup();
    let updated = manager.mark_private_transactions_confirmed_locally(&confirmed);
    if imported > 0 || updated > 0 {
        persist_private_wallet(storage, manager, wallet_name, password)?;
    }
    Ok(imported + updated)
}

fn cmd_private_tx_import(args: &[String]) -> Result<(), String> {
    let wallet_selector = required_wallet_selector(args)?;
    let password = password_value(args)?;
    let db_path = required_option_value(args, "--db")?;
    let tx_hash = required_option_value(args, "--hash")?;
    let (network, storage) = wallet_storage_for_args(args)?;
    // Lee, aplica y guarda: el candado va antes de leer (ver
    // `persist_private_wallet`).
    let _wallet_lock = storage
        .lock_private_wallets()
        .map_err(|e| format!("failed to lock wallet directory: {e}"))?;
    let mut manager =
        load_private_wallet_manager_for_network(&storage, password.as_deref(), network)?;
    let wallet_name = resolve_private_wallet_name_required(&manager, &wallet_selector)?;

    let chain_storage = open_storage(&db_path)?;
    let body = chain_storage
        .get_private_transaction_bytes(&tx_hash)
        .map_err(|e| format!("failed to read private tx body from db: {e}"))?
        .ok_or_else(|| format!("private tx body not found in db for hash {tx_hash}"))?;
    let tx: zoka::private_wallet::PrivateTransaction = serde_json::from_slice(&body)
        .map_err(|e| format!("failed to parse private tx body {tx_hash}: {e}"))?;

    let credited = manager
        .apply_private_transaction_for_wallet(&wallet_name, &tx)
        .map_err(|e| format!("failed to apply private tx to wallet '{wallet_name}': {e}"))?;
    persist_private_wallet(&storage, &manager, &wallet_name, password.as_deref())?;
    let clear_pending = has_flag(args, "--clear-pending");
    if clear_pending {
        chain_storage
            .delete_pending_private_transaction_hash(&tx_hash)
            .map_err(|e| format!("failed to clear pending private tx marker '{tx_hash}': {e}"))?;
    }

    println!("Private transaction imported");
    println!("Wallet: {}", wallet_name);
    println!("Hash: {}", tx_hash);
    println!("Credited: {}", format_zka(credited));
    println!("Pending marker cleared: {}", yes_no(clear_pending));
    Ok(())
}

fn cmd_private_tx_publish(args: &[String]) -> Result<(), String> {
    let (network, storage) = wallet_storage_for_args(args)?;
    let publish_all_wallet_files =
        has_flag(args, "--all-wallet-files") || has_flag(args, "--all-files");
    let wallet_selector = if publish_all_wallet_files {
        None
    } else {
        Some(required_wallet_selector(args)?)
    };
    let password = password_value(args)?;
    let as_json = has_flag(args, "--json");
    let hash_filter = option_value(args, "--hash");
    let rpc_urls = if let Some(rpc) = option_value(args, "--rpc") {
        parse_rpc_http_url(&rpc)?;
        vec![rpc]
    } else {
        private_tx_archive_rpc_urls(network)
    };
    if rpc_urls.is_empty() {
        return Err("no active RPC endpoint configured; pass --rpc http://host:port".to_string());
    }

    let (wallet_name, txs) = if publish_all_wallet_files {
        let wallets = storage
            .load_all_private_wallets(password.as_deref())
            .map_err(|e| format!("failed to load private wallet files: {e}"))?;
        let wallet_file_count = wallets.len();
        let mut seen_hashes = HashSet::new();
        let mut txs = Vec::new();
        for wallet in wallets {
            for tx in wallet.private_transactions.iter().filter(|tx| {
                hash_filter
                    .as_deref()
                    .map(|hash| hash == tx.tx_hash)
                    .unwrap_or(true)
            }) {
                if seen_hashes.insert(tx.tx_hash.clone()) {
                    txs.push(tx.clone());
                }
            }
        }
        if hash_filter.is_some() && txs.is_empty() {
            return Err(format!(
                "private tx {} not found in any unlocked private wallet file",
                hash_filter.unwrap_or_default()
            ));
        }
        (format!("all-wallet-files:{wallet_file_count}"), txs)
    } else {
        let manager =
            load_private_wallet_manager_for_network(&storage, password.as_deref(), network)?;
        let wallet_selector = wallet_selector.as_deref().unwrap_or_default();
        let wallet_name = resolve_private_wallet_name_required(&manager, wallet_selector)?;
        let wallet = manager
            .get_private_wallet(&wallet_name)
            .ok_or_else(|| format!("private wallet not found: {wallet_name}"))?;
        let txs = wallet
            .private_transactions
            .iter()
            .filter(|tx| {
                hash_filter
                    .as_deref()
                    .map(|hash| hash == tx.tx_hash)
                    .unwrap_or(true)
            })
            .cloned()
            .collect::<Vec<_>>();
        if hash_filter.is_some() && txs.is_empty() {
            return Err(format!(
                "private tx {} not found in wallet {}",
                hash_filter.unwrap_or_default(),
                wallet_name
            ));
        }
        (wallet_name, txs)
    };

    let mut published_txs = 0usize;
    let mut attempts = 0usize;
    let mut all_errors = Vec::new();
    let mut tx_results = Vec::new();
    for tx in &txs {
        let mut published_to = Vec::new();
        let mut tx_errors = Vec::new();
        for rpc in &rpc_urls {
            attempts = attempts.saturating_add(1);
            match publish_private_tx_body_to_rpc(rpc, tx) {
                Ok(()) => published_to.push(rpc.clone()),
                Err(err) => {
                    let entry = format!("{rpc}: {err}");
                    tx_errors.push(entry.clone());
                    all_errors.push(format!("{}: {}", tx.tx_hash, entry));
                }
            }
        }
        if !published_to.is_empty() {
            published_txs = published_txs.saturating_add(1);
        }
        tx_results.push(serde_json::json!({
            "hash": tx.tx_hash,
            "published_to": published_to,
            "errors": tx_errors,
        }));
    }

    if as_json {
        println!(
            "{}",
            serde_json::json!({
                "wallet": wallet_name,
                "tx_count": txs.len(),
                "published_txs": published_txs,
                "attempts": attempts,
                "rpc_endpoints": rpc_urls,
                "transactions": tx_results,
            })
        );
        return if txs.is_empty() || published_txs > 0 || all_errors.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "failed to publish private tx bodies: {}",
                all_errors.join("; ")
            ))
        };
    }

    println!("Private tx body publish");
    println!("Wallet: {}", wallet_name);
    println!("Transactions: {}", txs.len());
    println!("Published: {}", published_txs);
    for rpc in &rpc_urls {
        println!("RPC: {}", rpc);
    }
    if !all_errors.is_empty() {
        println!("Errors:");
        for err in all_errors.iter().take(5) {
            println!("- {}", err);
        }
    }
    if !txs.is_empty() && published_txs == 0 && !all_errors.is_empty() {
        return Err("no private tx bodies were published".to_string());
    }
    Ok(())
}

/// Clave del equipo para firmar checkpoints, si este nodo es la autoridad: el
/// archivo existe en la carpeta de la red y su clave pública es la compilada
/// para esta cadena. Si no coincide no firma: nadie aceptaría esas firmas.
fn load_checkpoint_authority(
    local_dir: &str,
    chain_id: &str,
    notes: &mut Vec<String>,
) -> Option<zoka::checkpoint_authority::CheckpointSigner> {
    let path = PathBuf::from(local_dir).join(zoka::checkpoint_authority::AUTHORITY_KEY_FILE);
    if !path.exists() {
        return None;
    }
    let key = match zoka::checkpoint_authority::load_signing_key(&path) {
        Ok(key) => key,
        Err(error) => {
            notes.push(format!("Checkpoint authority key unreadable: {error}"));
            return None;
        }
    };
    match zoka::checkpoint_authority::authority_keys_for_chain(chain_id) {
        Some(expected) if expected.ed25519 == key.verifying_key() => {
            let signer = checkpoint_signer_with_hybrid(local_dir, chain_id, &expected, key, notes)?;
            notes.push(format!(
                "Checkpoint authority: this node signs a checkpoint every {} blocks.",
                zoka::checkpoint_authority::AUTHORITY_SIGN_INTERVAL
            ));
            Some(signer)
        }
        Some(_) => {
            notes.push(
                "Checkpoint authority key does not match the key compiled for this chain; not signing."
                    .to_string(),
            );
            None
        }
        None => {
            notes.push(
                "Checkpoint authority key found, but this chain has no compiled authority; not signing."
                    .to_string(),
            );
            None
        }
    }
}

/// Firma un checkpoint cuando toca (cada `AUTHORITY_SIGN_INTERVAL` bloques,
/// `AUTHORITY_CONFIRMATIONS` por debajo de la punta) y reenvía el vigente cada
/// cinco minutos, para que lo reciba quien se conecte tarde. Solo firma con el
/// nodo al día (ver `checkpoint_signing_allowed`); reenviar el vigente no
/// depende de eso, porque ya está decidido.
async fn run_checkpoint_authority(
    node: &mut Node,
    storage: &RocksDbStorage,
    chain_id: &str,
    signer: &zoka::checkpoint_authority::CheckpointSigner,
    last_broadcast: &mut Option<tokio::time::Instant>,
) -> Option<String> {
    let tip = storage.get_chain_height().ok()?;
    let status = zoka::api::global_network_runtime()
        .sync_status_snapshot(tip, 0)
        .await;
    let latest = zoka::checkpoint_authority::latest_signed_checkpoint(storage)
        .ok()
        .flatten();
    let on_latest_checkpoint = chain_contains_checkpoint(latest.as_ref(), |height| {
        storage.get_canonical_block_hash_at_height(height).ok().flatten()
    });
    let next_height = if checkpoint_signing_allowed(&status) && on_latest_checkpoint {
        zoka::checkpoint_authority::next_height_to_sign(
            tip,
            latest.as_ref().map(|checkpoint| checkpoint.height),
        )
    } else {
        None
    };
    if let Some(height) = next_height {
        let hash = storage.get_canonical_block_hash_at_height(height).ok().flatten()?;
        let authority = zoka::checkpoint_authority::authority_keys_for_chain(chain_id)?;
        let mut versions = signer.sign_all(&authority, chain_id, height, &hash).into_iter();
        let own = versions.next()?;
        return Some(
            match zoka::checkpoint_authority::accept_signed_checkpoint(storage, chain_id, &own, &authority) {
                Ok(_) => {
                    let _ = node.publish_message(
                        &zoka::network::message::ZokaMessage::SignedCheckpoint(own.clone()),
                    );
                    // La otra versión, para los nodos de la otra política.
                    for compat in versions {
                        let _ = zoka::checkpoint_authority::store_compat_checkpoint(storage, &compat);
                        let _ = node.publish_message(
                            &zoka::network::message::ZokaMessage::SignedCheckpoint(compat),
                        );
                    }
                    *last_broadcast = Some(tokio::time::Instant::now());
                    format!(
                        "Signed checkpoint {height} ({}, version {}).",
                        &hash[..12],
                        own.version
                    )
                }
                Err(error) => format!("Checkpoint signing failed: {error}"),
            },
        );
    }
    let latest = latest?;
    let due = last_broadcast
        .map(|at| at.elapsed() >= Duration::from_secs(300))
        .unwrap_or(true);
    if due {
        let compat = zoka::checkpoint_authority::latest_compat_checkpoint(storage)
            .ok()
            .flatten()
            .filter(|compat| compat.height == latest.height);
        let _ = node.publish_message(&zoka::network::message::ZokaMessage::SignedCheckpoint(latest));
        if let Some(compat) = compat {
            let _ = node.publish_message(&zoka::network::message::ZokaMessage::SignedCheckpoint(compat));
        }
        *last_broadcast = Some(tokio::time::Instant::now());
    }
    None
}

/// Suma la mitad ML-DSA a la clave de la autoridad, si su archivo está en la
/// carpeta de la red. Reglas: si la cadena ya tiene compilada la clave ML-DSA,
/// sin el archivo (o con otro) no se firma nada, porque los nodos nuevos solo
/// aceptan la versión 2; si no la tiene, se firma solo la versión 1.
fn checkpoint_signer_with_hybrid(
    local_dir: &str,
    chain_id: &str,
    authority: &zoka::checkpoint_authority::AuthorityKeys,
    key: ed25519_dalek::SigningKey,
    notes: &mut Vec<String>,
) -> Option<zoka::checkpoint_authority::CheckpointSigner> {
    let mut signer = zoka::checkpoint_authority::CheckpointSigner::ed25519_only(key);
    // Zenith tiene nodos de antes de la etapa 6, que solo entienden la versión 1.
    signer.legacy_v1 = chain_id == MAINNET_CHAIN_ID;
    #[cfg(feature = "pq-experimental")]
    {
        let path = PathBuf::from(local_dir).join(zoka::checkpoint_authority::AUTHORITY_MLDSA_KEY_FILE);
        let hybrid = if path.exists() {
            match zoka::hybrid_signature::load_mldsa_seed(&path).and_then(|seed| {
                zoka::hybrid_signature::HybridSigningKey::from_parts(signer.ed25519.clone(), seed.as_ref())
                    .map_err(|e| e.to_string())
            }) {
                Ok(hybrid) => Some(hybrid),
                Err(error) => {
                    notes.push(format!("Checkpoint authority ML-DSA key unreadable: {error}"));
                    None
                }
            }
        } else {
            None
        };
        match (&authority.mldsa, hybrid) {
            (Some(expected), Some(hybrid)) if hybrid.verifying_key().mldsa().to_bytes() == expected.to_bytes() => {
                signer.hybrid = Some(hybrid);
            }
            (Some(_), _) => {
                notes.push(
                    "This chain requires hybrid checkpoints and the local ML-DSA key is missing or does not match; not signing."
                        .to_string(),
                );
                return None;
            }
            (None, Some(_)) => notes.push(
                "Checkpoint authority ML-DSA key found, but this chain has no compiled ML-DSA authority; signing version 1 only."
                    .to_string(),
            ),
            (None, None) => {}
        }
    }
    #[cfg(not(feature = "pq-experimental"))]
    {
        let _ = (local_dir, authority, &notes);
    }
    Some(signer)
}

/// Firmar fija la cadena para todos, así que pide más que `caught_up`: con
/// `--min-mine-peers 0` una PC sin pares también figura al día y firmaría la
/// rama que mina sola, atándose a ella. Exige al menos un par, una punta
/// anunciada por la red (`ready_to_mine`) y ninguna rama en resolución
/// (`mode`, que vale `backfill` mientras se baja una rama competidora).
/// Mínimo de pares para minar. La autoridad de checkpoints mina aunque no tenga
/// ninguno: el mínimo existe para que un nodo aislado no mine una rama propia,
/// pero la cadena de la autoridad es la de referencia, y sin esto una red donde
/// solo queda ella no avanza nunca (Zenith el 13/10/2026: ninguna otra semilla
/// respondía). Sin pares igual no firma (`checkpoint_signing_allowed`), así que
/// lo que mine sola no queda fijado hasta que la red lo vea.
fn authority_min_mine_peers(requested: usize, is_checkpoint_authority: bool) -> usize {
    if is_checkpoint_authority {
        0
    } else {
        requested
    }
}

fn checkpoint_signing_allowed(status: &zoka::api::SyncStatusSnapshot) -> bool {
    status.status == "caught_up"
        && status.mode == "caught_up"
        && status.ready_to_mine
        && status.connected_peers > 0
}

/// La cadena local contiene el último checkpoint aceptado. Si no la contiene
/// (uno firmado a mano para corregir la rama, o uno por encima de la punta),
/// la autoridad no firma: estaría fijando de nuevo, más arriba, la rama que ese
/// checkpoint vino a corregir. Firma otra vez cuando se reorganiza.
fn chain_contains_checkpoint(
    latest: Option<&zoka::checkpoint_authority::SignedCheckpoint>,
    canonical_hash_at: impl Fn(u64) -> Option<String>,
) -> bool {
    match latest {
        None => true,
        Some(checkpoint) => {
            canonical_hash_at(checkpoint.height).as_deref() == Some(checkpoint.block_hash.as_str())
        }
    }
}

/// La dirección corta de una completa (`docs/QUANTIC-DIRECCIONES-CORTAS.md`).
#[cfg(feature = "quantic")]
fn short_address_of(address: &str, network: zoka::quantic::protocol::Network) -> Result<String, String> {
    zoka::quantic::protocol::QuanticAddress::decode_for(address.trim(), network)
        .map(|full| full.short().encode())
        .map_err(|e| e.to_string())
}

/// El destinatario de un envío: una dirección completa tal cual, o una corta
/// resuelta en el directorio del nodo. La completa que devuelve el nodo se
/// comprueba contra el resumen de la corta: un nodo puede no tenerla, pero no
/// cambiarla.
#[cfg(feature = "quantic")]
fn resolve_recipient(
    text: &str,
    network: zoka::quantic::protocol::Network,
    rpc: &dyn Fn() -> Result<String, String>,
) -> Result<zoka::quantic::protocol::QuanticAddress, String> {
    use zoka::quantic::protocol::address::Recipient;
    match Recipient::decode_for(text.trim(), network).map_err(|e| e.to_string())? {
        Recipient::Full(full) => Ok(full),
        Recipient::Short(short) => {
            let reply = rpc_http_get_json(&rpc()?, &format!("/quantic/address/{}", short.encode()), 15).map_err(|e| {
                format!("the node could not resolve that short address ({e}); ask for the full address")
            })?;
            let address = reply["address"]
                .as_str()
                .ok_or_else(|| format!("unexpected /quantic/address reply: {reply}"))?;
            let full = zoka::quantic::protocol::QuanticAddress::decode_for(address, network).map_err(|e| e.to_string())?;
            if !short.matches(&full) {
                return Err("the node answered an address that does not match the short address".to_string());
            }
            Ok(full)
        }
    }
}

/// La contraseña de `--password` o `--password-env`, sin tocar stdin: elegir
/// la wallet no puede consumir lo que después lee el comando.
#[cfg(feature = "quantic")]
fn password_without_stdin(args: &[String]) -> Option<zeroize::Zeroizing<String>> {
    option_value(args, "--password")
        .or_else(|| option_value(args, "--wallet-password"))
        .or_else(|| {
            option_value(args, "--password-env")
                .or_else(|| option_value(args, "--wallet-password-env"))
                .and_then(|var_name| std::env::var(var_name).ok())
        })
        .map(zeroize::Zeroizing::new)
}

/// Entre varias wallets con la misma dirección (la misma frase restaurada dos
/// veces, cada una con su contraseña), la que abre con la contraseña dada. Si
/// ninguna abre, o no hay contraseña, la primera, como antes. Pasó el 22/10:
/// ZSilent elige por dirección y la primera era una copia con otra contraseña.
#[cfg(feature = "quantic")]
fn pick_wallet_for_address(
    dir: &Path,
    network: zoka::quantic::protocol::Network,
    candidates: Vec<String>,
    password: Option<zeroize::Zeroizing<String>>,
) -> Option<String> {
    if candidates.len() > 1 {
        if let Some(password) = password.as_deref() {
            if let Some(name) = candidates
                .iter()
                .find(|name| zoka::quantic::wallet_file::open(dir, name, network, password).is_ok())
            {
                return Some(name.clone());
            }
        }
    }
    candidates.into_iter().next()
}

/// `zoka quantic-wallet <acción> --network quantic-regtest|quantic-testnet`:
/// la wallet de Zoka Quantic de línea de comandos. Habla con el nodo por su API
/// (`--rpc`, o la del manifiesto) y se sincroniza con bloques compactos que
/// verifica contra cada bloque, retomando desde lo que guardó la vez anterior.
/// Acciones: `new`, `restore`, `address`, `list`, `balance`, `history`,
/// `estimate-fee --amount <ZKA>`, `send --to <dirección> --amount <ZKA>`,
/// `show-phrase`, `change-password --new-password-env <VAR>` e
/// `import-regtest-team` (solo regtest: la wallet del premine, de clave pública
/// a propósito).
#[cfg(feature = "quantic")]
fn cmd_quantic_wallet(args: &[String]) -> Result<(), String> {
    // Armar la clave y la prueba de Halo 2 no entra en la pila del hilo
    // principal de Windows (1 MB): se corre en uno con pila amplia.
    let args = args.to_vec();
    std::thread::Builder::new()
        .name("quantic-wallet".to_string())
        .stack_size(64 * 1024 * 1024)
        .spawn(move || cmd_quantic_wallet_inner(&args))
        .map_err(|e| format!("cannot start the wallet thread: {e}"))?
        .join()
        .map_err(|_| "the wallet thread panicked".to_string())?
}

#[cfg(feature = "quantic")]
fn cmd_quantic_wallet_inner(args: &[String]) -> Result<(), String> {
    const USAGE: &str = "usage: zoka quantic-wallet <new|generate-phrase|restore|address|publish-address|list|balance|history|estimate-fee|send|show-phrase|change-password|backup|import-backup|import-regtest-team> --network quantic-mainnet|quantic-testnet|quantic-regtest [--name <wallet> | --address <its address>] [--password <p> | --password-env <VAR> | --password-stdin] [history: --limit <n>] [restore: --phrase-env <VAR> or the phrase on stdin] [estimate-fee: --amount <ZKA>] [address: --short] [send: --to <full or short address> --amount <ZKA> [--memo <text>]] [change-password: --new-password-env <VAR>] [backup: --out <file>] [import-backup: --file <file>] [--rpc <url>] [--json]";
    let Some((action, rest)) = args.split_first() else {
        return Err(USAGE.to_string());
    };
    let network = NetworkProfile::from_args(rest)?;
    if !network.is_quantic() {
        return Err("quantic-wallet needs --network quantic-mainnet, quantic-testnet or quantic-regtest".to_string());
    }
    let quantic_network = zoka::quantic::network_for_chain(network.chain_id())
        .ok_or_else(|| "unknown Quantic network".to_string())?;
    let dir = zoka::quantic::wallet_file::wallets_dir(network.local_dir());
    // ZSilent identifica las wallets por su dirección: `--address` elige la que
    // la tiene, en lugar de `--name`.
    let name = match (option_value(rest, "--name"), option_value(rest, "--address")) {
        (Some(name), _) => name,
        (None, Some(address)) => {
            let candidates = zoka::quantic::wallet_file::list_with_addresses(&dir)
                .into_iter()
                .filter(|(_, own)| own == address.trim())
                .map(|(name, _)| name)
                .collect();
            pick_wallet_for_address(&dir, quantic_network, candidates, password_without_stdin(rest))
                .ok_or_else(|| "no Zoka Quantic wallet here has that address".to_string())?
        }
        (None, None) => "default".to_string(),
    };
    let json = has_flag(rest, "--json");
    let rpc = || -> Result<String, String> {
        option_value(rest, "--rpc")
            .or_else(|| configured_local_rpc_url(network))
            .ok_or_else(|| "no node API: pass --rpc <url>".to_string())
    };
    // Retoma desde lo que guardó la vez anterior (`<nombre>.scan`) en vez de
    // recorrer la cadena desde el génesis en cada consulta.
    let synced = |password: &str| -> Result<(zoka::quantic::protocol::QuanticSpendingKey, zoka::quantic::wallet_sync::Resumed, u64), String> {
        let (key, scan_key) = zoka::quantic::wallet_file::open_for_scan(&dir, &name, quantic_network, password)?;
        let url = rpc()?;
        let info = rpc_http_get_json(&url, "/quantic/info", 15)?;
        let tip = info["height"].as_u64().ok_or_else(|| format!("unexpected /quantic/info reply: {info}"))?;
        let saved = zoka::quantic::wallet_file::load_scan(&dir, &name, &scan_key);
        let resumed = zoka::quantic::wallet_sync::resume_wallet(key.full_viewing_key(), saved, tip, |from, to| {
            let reply = rpc_http_get_json(&url, &format!("/quantic/compact?from={from}&to={to}"), 60)?;
            serde_json::from_value(reply["blocks"].clone()).map_err(|e| format!("unexpected /quantic/compact reply: {e}"))
        })?;
        if !resumed.node_behind {
            // Si no se puede guardar, la próxima vez solo cuesta volver a escanear.
            let _ = zoka::quantic::wallet_file::save_scan(&dir, &name, &scan_key, &resumed.points);
        }
        Ok((key, resumed, tip))
    };
    match action.as_str() {
        "new" => {
            let password = required_password_value(rest)?;
            let (phrase, address) = zoka::quantic::wallet_file::create(&dir, &name, quantic_network, &password)?;
            if json {
                println!(
                    "{}",
                    serde_json::json!({ "name": name, "address": address, "recovery_phrase": phrase.as_str() })
                );
            } else {
                println!("Zoka Quantic wallet '{name}' created ({}).", network.display_name());
                println!("Recovery phrase (write it down offline; it is shown only once):");
                println!("{}", phrase.as_str());
                println!("Address:");
                println!("{address}");
            }
            Ok(())
        }
        "restore" => {
            let password = required_password_value(rest)?;
            // Como en Zenith: la frase nunca tiene por qué ir como argumento del
            // proceso (la ve cualquiera que liste los procesos). ZSilent la pasa
            // por una variable de entorno; sin nada, se lee de stdin.
            let phrase = match option_value(rest, "--phrase") {
                Some(phrase) => zeroize::Zeroizing::new(phrase),
                None => match option_value(rest, "--phrase-env").or_else(|| option_value(rest, "--seed-env")) {
                    Some(var_name) => zeroize::Zeroizing::new(secret_from_env_var(&var_name, "--phrase-env")?),
                    None => zeroize::Zeroizing::new(stdin_secret("--phrase")?),
                },
            };
            let address = zoka::quantic::wallet_file::restore(&dir, &name, quantic_network, &phrase, &password)?;
            if json {
                println!("{}", serde_json::json!({ "name": name, "address": address }));
            } else {
                println!("Zoka Quantic wallet '{name}' restored.");
                println!("{address}");
            }
            Ok(())
        }
        "import-regtest-team" => {
            if quantic_network != zoka::quantic::protocol::Network::Regtest {
                return Err("the team wallet with a public key exists only on quantic-regtest".to_string());
            }
            let password = required_password_value(rest)?;
            let address = zoka::quantic::wallet_file::import_regtest_team(&dir, &name, &password)?;
            println!("{address}");
            Ok(())
        }
        "address" => {
            let address = zoka::quantic::wallet_file::address(&dir, &name)?;
            let short = short_address_of(&address, quantic_network)?;
            if json {
                println!(
                    "{}",
                    serde_json::json!({ "name": name, "address": address, "short_address": short })
                );
            } else if has_flag(rest, "--short") {
                println!("{short}");
            } else {
                println!("{address}");
            }
            Ok(())
        }
        // La dirección completa al directorio del nodo, para que le paguen con
        // la corta. Pública: no pide contraseña. El sello se busca acá.
        "publish-address" => {
            let address = zoka::quantic::wallet_file::address(&dir, &name)?;
            let full = zoka::quantic::protocol::QuanticAddress::decode_for(&address, quantic_network)
                .map_err(|e| format!("wallet '{name}': {e}"))?;
            let record = zoka::quantic::directory::AddressRecord::new(&full);
            let reply = rpc_http_post_json(
                &rpc()?,
                "/quantic/address",
                &serde_json::json!({ "address": record.address, "stamp": record.stamp }),
                60,
            )?;
            let short = full.short().encode();
            if json {
                println!("{}", serde_json::json!({ "name": name, "short_address": short, "node": reply }));
            } else {
                println!("Published. Short address of '{name}':");
                println!("{short}");
            }
            Ok(())
        }
        "generate-phrase" => {
            let phrase = zoka::quantic::wallet_file::generate_phrase()?;
            if json {
                println!("{}", serde_json::json!({ "words": 24, "recovery_phrase": phrase.as_str() }));
            } else {
                println!("{}", phrase.as_str());
            }
            Ok(())
        }
        "list" => {
            let wallets = zoka::quantic::wallet_file::list_with_addresses(&dir);
            // Con contraseña, dice cuáles abre (lo que necesita el login de ZSilent).
            let password = password_value(rest)?;
            if json {
                let names: Vec<&String> = wallets.iter().map(|(name, _)| name).collect();
                let details: Vec<serde_json::Value> = wallets
                    .iter()
                    .map(|(name, address)| {
                        let unlocked = password.as_deref().is_some_and(|password| {
                            zoka::quantic::wallet_file::open(&dir, name, quantic_network, password).is_ok()
                        });
                        let short = short_address_of(address, quantic_network).unwrap_or_default();
                        serde_json::json!({
                            "name": name,
                            "address": address,
                            "short_address": short,
                            "unlocked": unlocked,
                        })
                    })
                    .collect();
                println!("{}", serde_json::json!({ "network": network.label(), "wallets": names, "details": details }));
            } else {
                for (wallet, _) in wallets {
                    println!("{wallet}");
                }
            }
            Ok(())
        }
        "backup" => {
            let out = PathBuf::from(required_option_value(rest, "--out")?);
            zoka::quantic::wallet_file::backup(&dir, &name, &out)?;
            if json {
                println!("{}", serde_json::json!({ "name": name, "backup": out.display().to_string() }));
            } else {
                println!("Wallet '{name}' copied to {} (still encrypted with its password).", out.display());
            }
            Ok(())
        }
        "import-backup" => {
            let password = required_password_value(rest)?;
            let file = PathBuf::from(required_option_value(rest, "--file")?);
            let address = zoka::quantic::wallet_file::import_backup(&dir, &name, &file, quantic_network, &password)?;
            if json {
                println!("{}", serde_json::json!({ "name": name, "address": address }));
            } else {
                println!("Wallet '{name}' imported.");
                println!("{address}");
            }
            Ok(())
        }
        "show-phrase" => {
            let password = required_password_value(rest)?;
            let phrase = zoka::quantic::wallet_file::recovery_phrase(&dir, &name, quantic_network, &password)?;
            if json {
                println!("{}", serde_json::json!({ "name": name, "recovery_phrase": phrase.as_str() }));
            } else {
                println!("{}", phrase.as_str());
            }
            Ok(())
        }
        "change-password" => {
            let password = required_password_value(rest)?;
            let new_password = zeroize::Zeroizing::new(match option_value(rest, "--new-password-env") {
                Some(var_name) => secret_from_env_var(&var_name, "--new-password-env")?,
                None => required_option_value(rest, "--new-password")?,
            });
            if new_password.is_empty() {
                return Err("the new password cannot be empty".to_string());
            }
            zoka::quantic::wallet_file::change_password(&dir, &name, quantic_network, &password, &new_password)?;
            if json {
                println!("{}", serde_json::json!({ "name": name, "changed": true }));
            } else {
                println!("Password of wallet '{name}' changed.");
            }
            Ok(())
        }
        "estimate-fee" => {
            let password = required_password_value(rest)?;
            let amount = economics::parse_zka_amount(&required_option_value(rest, "--amount")?)?;
            let (_, resumed, tip) = synced(&password)?;
            let wallet = &resumed.wallet;
            let plan = wallet.plan_transfer(amount, 1);
            if json {
                let mut reply = serde_json::json!({
                    "name": name,
                    "height": tip,
                    "amount_atoms": amount,
                    "max_sendable_atoms": wallet.max_sendable(),
                    "spendable_atoms": wallet.balance(),
                });
                match &plan {
                    Ok(plan) => {
                        reply["enough"] = true.into();
                        reply["fee_atoms"] = plan.fee.into();
                        reply["fee"] = format_zka_amount(plan.fee).into();
                        reply["inputs"] = plan.inputs.into();
                        reply["actions"] = plan.actions.into();
                        reply["change_atoms"] = plan.change.into();
                    }
                    Err(error) => {
                        reply["enough"] = false.into();
                        reply["reason"] = error.to_string().into();
                    }
                }
                println!("{reply}");
            } else {
                match plan {
                    Ok(plan) => println!(
                        "Fee {} ({} action(s), {} note(s) spent, change {}).",
                        format_zka(plan.fee),
                        plan.actions,
                        plan.inputs,
                        format_zka(plan.change)
                    ),
                    Err(error) => println!("Cannot send {}: {error}", format_zka(amount)),
                }
                println!("Most you can send now: {}", format_zka(wallet.max_sendable()));
            }
            Ok(())
        }
        "balance" => {
            let password = required_password_value(rest)?;
            let (_, resumed, tip) = synced(&password)?;
            let wallet = &resumed.wallet;
            let spendable = wallet.balance();
            let immature = wallet.immature_balance();
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "name": name,
                        "height": tip,
                        "wallet_height": u64::from(wallet.next_height()).saturating_sub(1),
                        "node_behind": resumed.node_behind,
                        "spendable_atoms": spendable,
                        "immature_atoms": immature,
                        "spendable": format_zka_amount(spendable),
                        "immature": format_zka_amount(immature),
                        "max_sendable_atoms": wallet.max_sendable(),
                        "notes": wallet.notes().iter().filter(|n| n.spent_in.is_none()).count(),
                    })
                );
            } else {
                println!("Wallet '{name}' at height {tip}:");
                println!("Spendable: {}", format_zka(spendable));
                println!("Immature (coinbase): {}", format_zka(immature));
            }
            Ok(())
        }
        "history" => {
            let password = required_password_value(rest)?;
            let (_, resumed, tip) = synced(&password)?;
            let total = resumed.wallet.history().len();
            let limit = parse_u64_arg(option_value(rest, "--limit"), total as u64, "--limit")? as usize;
            let history: Vec<_> = resumed.wallet.history().into_iter().take(limit).collect();
            if json {
                // La hora de cada bloque, del nodo propio (no revela nada a terceros).
                let url = rpc()?;
                let block_time = |height: u32| -> Option<u64> {
                    rpc_http_get_json(&url, &format!("/blocks/{height}"), 15).ok()?["timestamp"].as_u64()
                };
                let entries: Vec<serde_json::Value> = history
                    .iter()
                    .map(|entry| {
                        let kind = if entry.coinbase {
                            "coinbase"
                        } else if entry.spent > 0 {
                            "sent"
                        } else {
                            "received"
                        };
                        serde_json::json!({
                            "txid": entry.txid.to_hex(),
                            "height": entry.height,
                            "kind": kind,
                            "received_atoms": entry.received,
                            "spent_atoms": entry.spent,
                            "net_atoms": entry.net().to_string(),
                            "amount_atoms": entry.net().unsigned_abs() as u64,
                            "amount": format_zka_amount(entry.net().unsigned_abs() as u64),
                            "timestamp": block_time(entry.height),
                            "confirmations": tip.saturating_sub(u64::from(entry.height)) + 1,
                        })
                    })
                    .collect();
                println!(
                    "{}",
                    serde_json::json!({ "name": name, "height": tip, "count": total, "transactions": entries })
                );
            } else {
                for entry in history {
                    let net = entry.net();
                    let amount = format_zka(net.unsigned_abs() as u64);
                    println!(
                        "{:>8}  {}{}  {}{}",
                        entry.height,
                        if net < 0 { "-" } else { "+" },
                        amount,
                        &entry.txid.to_hex()[..16],
                        if entry.coinbase { "  (coinbase)" } else { "" }
                    );
                }
            }
            Ok(())
        }
        "send" => {
            let password = required_password_value(rest)?;
            let to = resolve_recipient(&required_option_value(rest, "--to")?, quantic_network, &rpc)
                .map_err(|e| format!("--to: {e}"))?;
            let amount = economics::parse_zka_amount(&required_option_value(rest, "--amount")?)?;
            if amount == 0 {
                return Err("--amount must be positive".to_string());
            }
            let mut payment = zoka::quantic::protocol::builder::Payment::new(to, amount);
            // Texto para el destinatario, cifrado con la nota (hasta 512 bytes).
            if let Some(memo) = option_value(rest, "--memo").filter(|memo| !memo.is_empty()) {
                let bytes = memo.as_bytes();
                if bytes.len() > payment.memo.len() {
                    return Err(format!("--memo has {} bytes; at most {}", bytes.len(), payment.memo.len()));
                }
                payment.memo[..bytes.len()].copy_from_slice(bytes);
            }
            let (key, resumed, tip) = synced(&password)?;
            if resumed.node_behind {
                return Err("the node is still catching up with the chain; send when it is synced".to_string());
            }
            let proving_key = zoka::quantic::runtime::proving_key();
            let tx = resumed
                .wallet
                .create_transfer(
                    &key,
                    vec![payment],
                    0,
                    proving_key,
                    &mut zoka::quantic::protocol::os_rng(),
                )
                .map_err(|e| e.to_string())?;
            let reply = rpc_http_post_json(
                &rpc()?,
                "/quantic/tx",
                &serde_json::json!({ "tx_hex": hex::encode(tx.to_bytes()) }),
                60,
            )?;
            let txid = tx.txid().to_hex();
            if json {
                println!(
                    "{}",
                    serde_json::json!({ "txid": txid, "fee_atoms": tx.fee(), "height": tip, "node": reply })
                );
            } else {
                println!("Sent {} (fee {}).", format_zka(amount), format_zka(tx.fee().unwrap_or(0)));
                println!("Transaction: {txid}");
                println!("Node reply: {reply}");
            }
            Ok(())
        }
        _ => Err(USAGE.to_string()),
    }
}

/// `zoka quantic-genesis --network quantic-testnet --premine-to <dirección>
/// [--out <archivo>]`: arma el génesis de una red pública de Quantic (el bloque
/// 1 con el premine público) y lo escribe. Después se fijan su hash y la
/// dirección en `economics::QUANTIC_*_GENESIS` y el archivo se embebe.
#[cfg(feature = "quantic")]
fn cmd_quantic_genesis(args: &[String]) -> Result<(), String> {
    let args = args.to_vec();
    std::thread::Builder::new()
        .name("quantic-genesis".to_string())
        .stack_size(64 * 1024 * 1024)
        .spawn(move || -> Result<(), String> {
            let network = NetworkProfile::from_args(&args)?;
            if !matches!(network, NetworkProfile::QuanticTestnet | NetworkProfile::QuanticMainnet) {
                return Err("quantic-genesis is for --network quantic-testnet or quantic-mainnet".to_string());
            }
            let chain_id = network.chain_id();
            let quantic_network = zoka::quantic::network_for_chain(chain_id).ok_or("unknown network")?;
            if zoka::quantic::fixed_genesis(quantic_network).is_some() {
                return Err(format!("{chain_id} already has its genesis compiled in"));
            }
            let premine_to = zoka::quantic::protocol::QuanticAddress::decode_for(
                required_option_value(&args, "--premine-to")?.trim(),
                quantic_network,
            )
            .map_err(|e| format!("--premine-to: {e}"))?;
            let block = zoka::quantic::build_genesis(chain_id, &premine_to, zoka::quantic::runtime::proving_key(), now_secs())?;
            let json = serde_json::to_string_pretty(&block).map_err(|e| e.to_string())?;
            match option_value(&args, "--out") {
                Some(path) => fs::write(&path, &json).map_err(|e| format!("cannot write {path}: {e}"))?,
                None => println!("{json}"),
            }
            eprintln!("Genesis of {chain_id}: block 1 {}", block.header.hash);
            eprintln!("Premine: 5,000,000 ZKA to the public output of {}", premine_to.encode());
            Ok(())
        })
        .map_err(|e| format!("cannot start the genesis thread: {e}"))?
        .join()
        .map_err(|_| "the genesis thread panicked".to_string())?
}

/// `zoka checkpoint-keygen [--network testnet]`: crea la clave del equipo en la
/// carpeta de la red e imprime la pública, que se compila en
/// `economics::CHECKPOINT_AUTHORITY_KEYS`. No sobrescribe una existente.
fn cmd_checkpoint_keygen(args: &[String]) -> Result<(), String> {
    let network = NetworkProfile::from_args(args)?;
    if has_flag(args, "--mldsa") {
        return cmd_checkpoint_keygen_mldsa(&network);
    }
    let path = PathBuf::from(network.local_dir()).join(zoka::checkpoint_authority::AUTHORITY_KEY_FILE);
    let public = zoka::checkpoint_authority::generate_signing_key(&path)?;
    let shown = std::env::current_dir()
        .map(|dir| dir.join(&path))
        .unwrap_or_else(|_| path.clone());
    println!("Checkpoint authority key created: {}", shown.display());
    println!("Chain: {}", network.chain_id());
    println!("Public key: {public}");
    println!("Back up that file offline: it is the only copy. Compile the public key into");
    println!("economics::CHECKPOINT_AUTHORITY_KEYS for this chain and release new binaries.");
    // Zoka Quantic exige la clave híbrida desde el génesis: sin la mitad ML-DSA
    // la cadena no tiene autoridad. Se crean las dos mitades juntas.
    if network.is_quantic() {
        println!();
        return cmd_checkpoint_keygen_mldsa(&network);
    }
    Ok(())
}

/// `zoka checkpoint-keygen --mldsa`: crea la mitad ML-DSA-65 de la clave de la
/// autoridad junto a la Ed25519 (etapa 6). Su pública se compila en
/// `economics::CHECKPOINT_AUTHORITY_MLDSA_KEYS`; desde ese binario la cadena
/// solo acepta checkpoints híbridos. No sobrescribe una existente.
fn cmd_checkpoint_keygen_mldsa(network: &NetworkProfile) -> Result<(), String> {
    #[cfg(feature = "pq-experimental")]
    {
        let dir = PathBuf::from(network.local_dir());
        if !dir.join(zoka::checkpoint_authority::AUTHORITY_KEY_FILE).exists() {
            return Err(
                "create the Ed25519 authority key first (zoka checkpoint-keygen): the ML-DSA key is its second half"
                    .to_string(),
            );
        }
        let path = dir.join(zoka::checkpoint_authority::AUTHORITY_MLDSA_KEY_FILE);
        let public = zoka::hybrid_signature::generate_mldsa_seed(&path)?;
        println!("Checkpoint authority ML-DSA-65 key created: {}", path.display());
        println!("Chain: {}", network.chain_id());
        println!("ML-DSA public key ({} bytes):", public.len() / 2);
        println!("{public}");
        println!("Back up that file offline next to the Ed25519 key. Compile the public key into");
        println!("economics::CHECKPOINT_AUTHORITY_MLDSA_KEYS: from that release on, the chain only");
        println!("accepts hybrid (Ed25519 + ML-DSA-65) checkpoints.");
        Ok(())
    }
    #[cfg(not(feature = "pq-experimental"))]
    {
        let _ = network;
        Err("this build has no ML-DSA support; build with --features pq-experimental".to_string())
    }
}

/// `zoka checkpoint-sign --height <h> --hash <hash> [--network testnet]
/// [--post <rpc>] [--json]`: firma a mano el checkpoint indicado con la clave
/// de autoridad de esta PC.
///
/// Es la salida de emergencia si la autoridad firmó una rama equivocada: ella
/// misma queda atada a esa rama. Un checkpoint más alto sobre la rama correcta
/// reemplaza al anterior en todos los nodos, que se reorganizan hacia ella.
/// Con `--post` se entrega a un nodo (`POST /checkpoint`); si ese nodo es la
/// autoridad, lo reenvía a la red en el próximo ciclo.
fn cmd_checkpoint_sign(args: &[String]) -> Result<(), String> {
    let network = NetworkProfile::from_args(args)?;
    let chain_id = network.chain_id();
    let height = parse_u64_arg(
        Some(required_option_value(args, "--height")?),
        0,
        "--height",
    )?;
    let hash = required_option_value(args, "--hash")?;
    let key_path =
        PathBuf::from(network.local_dir()).join(zoka::checkpoint_authority::AUTHORITY_KEY_FILE);
    let checkpoints = sign_manual_checkpoint(
        &key_path,
        chain_id,
        zoka::checkpoint_authority::authority_keys_for_chain(chain_id),
        height,
        &hash,
    )?;
    // Se entregan todas las versiones: el nodo acepta la de su política y
    // rechaza la otra, así que un rechazo no es un fallo del comando.
    let posted = match option_value(args, "--post") {
        Some(rpc) => {
            let mut replies = Vec::new();
            for checkpoint in &checkpoints {
                let body = serde_json::to_value(checkpoint).map_err(|e| e.to_string())?;
                replies.push(match rpc_http_post_json(&rpc, "/checkpoint", &body, 10) {
                    Ok(reply) => format!("version {}: {reply}", checkpoint.version),
                    Err(error) => format!("version {}: {error}", checkpoint.version),
                });
            }
            Some(replies)
        }
        None => None,
    };
    if has_flag(args, "--json") {
        println!(
            "{}",
            serde_json::json!({
                "checkpoint": checkpoints.first(),
                "checkpoints": checkpoints,
                "posted": posted,
            })
        );
        return Ok(());
    }
    for checkpoint in &checkpoints {
        println!(
            "{}",
            serde_json::to_string(checkpoint).map_err(|e| e.to_string())?
        );
    }
    match posted {
        Some(replies) => {
            for reply in replies {
                println!("Node reply, {reply}");
            }
        }
        None => println!(
            "Not delivered: POST it to a node's /checkpoint, or run again with --post <rpc>."
        ),
    }
    Ok(())
}

/// Firma un checkpoint con la clave de `key_path` (y su mitad ML-DSA, si está
/// al lado), solo si es la autoridad de la cadena, y verifica la versión que
/// acepta la cadena antes de devolverla: nunca sale una que los nodos
/// rechazarían. Devuelve primero esa versión y después la de compatibilidad.
fn sign_manual_checkpoint(
    key_path: &Path,
    chain_id: &str,
    authority: Option<zoka::checkpoint_authority::AuthorityKeys>,
    height: u64,
    hash: &str,
) -> Result<Vec<zoka::checkpoint_authority::SignedCheckpoint>, String> {
    if !key_path.exists() {
        return Err(format!(
            "no checkpoint authority key at {}: only the authority node can sign",
            key_path.display()
        ));
    }
    let key = zoka::checkpoint_authority::load_signing_key(key_path)?;
    let authority = authority
        .ok_or_else(|| format!("chain {chain_id} has no compiled checkpoint authority"))?;
    if authority.ed25519 != key.verifying_key() {
        return Err(
            "the local key is not the compiled authority for this chain: every node would reject the checkpoint"
                .to_string(),
        );
    }
    let local_dir = key_path
        .parent()
        .and_then(|dir| dir.to_str())
        .unwrap_or(".")
        .to_string();
    let mut notes = Vec::new();
    let signer = checkpoint_signer_with_hybrid(&local_dir, chain_id, &authority, key, &mut notes)
        .ok_or_else(|| notes.join(" "))?;
    let hash = hash.trim().to_ascii_lowercase();
    let checkpoints = signer.sign_all(&authority, chain_id, height, &hash);
    // Revisa también la forma: altura mayor que cero y hash de 64 hex.
    checkpoints
        .first()
        .ok_or_else(|| "nothing to sign".to_string())?
        .verify(&authority)
        .map_err(|e| format!("refusing to emit an invalid checkpoint: {e}"))?;
    Ok(checkpoints)
}

/// `zoka checkpoint-status [--network testnet] [--db <path>] [--json]`.
fn cmd_checkpoint_status(args: &[String]) -> Result<(), String> {
    let network = NetworkProfile::from_args(args)?;
    let chain_id = network.chain_id();
    let db_path = option_value(args, "--db").unwrap_or_else(|| network.default_db_path().to_string());
    let authority_keys = zoka::checkpoint_authority::authority_keys_for_chain(chain_id);
    let hybrid_required = authority_keys
        .as_ref()
        .map(|keys| keys.requires_hybrid())
        .unwrap_or(false);
    let authority = authority_keys
        .as_ref()
        .map(|keys| hex::encode(keys.ed25519.to_bytes()));
    let key_path = PathBuf::from(network.local_dir()).join(zoka::checkpoint_authority::AUTHORITY_KEY_FILE);
    let local_mldsa_key = PathBuf::from(network.local_dir())
        .join(zoka::checkpoint_authority::AUTHORITY_MLDSA_KEY_FILE)
        .exists();
    let local_key = if key_path.exists() {
        Some(
            zoka::checkpoint_authority::load_signing_key(&key_path)
                .map(|key| hex::encode(key.verifying_key().to_bytes()))?,
        )
    } else {
        None
    };
    let signs = matches!((&authority, &local_key), (Some(a), Some(l)) if a == l);
    let latest = match open_storage(&db_path) {
        Ok(storage) => zoka::checkpoint_authority::latest_signed_checkpoint(&storage)
            .map_err(|e| e.to_string())?,
        Err(error) => return Err(format!("{error} (stop the node or pass --db)")),
    };
    if has_flag(args, "--json") {
        println!(
            "{}",
            serde_json::json!({
                "chain_id": chain_id,
                "authority_public_key": authority,
                "local_key_public": local_key,
                "this_node_signs": signs,
                "hybrid_required": hybrid_required,
                "local_mldsa_key": local_mldsa_key,
                "latest_signed_checkpoint": latest,
            })
        );
        return Ok(());
    }
    println!("Chain: {chain_id}");
    println!(
        "Authority public key: {}",
        authority.as_deref().unwrap_or("none compiled (no signed checkpoints on this chain)")
    );
    match &local_key {
        Some(key) if signs => println!("This node is the authority (key {key})."),
        Some(key) => println!("Local key {key} is NOT the compiled authority; it does not sign."),
        None => println!("This node has no authority key."),
    }
    println!(
        "Hybrid checkpoints (Ed25519 + ML-DSA-65): {}; local ML-DSA key: {}.",
        if hybrid_required { "required on this chain" } else { "not required on this chain" },
        if local_mldsa_key { "present" } else { "absent" }
    );
    match latest {
        Some(checkpoint) => println!(
            "Latest signed checkpoint: height {} hash {} (version {})",
            checkpoint.height, checkpoint.block_hash, checkpoint.version
        ),
        None => println!("Latest signed checkpoint: none accepted yet."),
    }
    Ok(())
}

fn cmd_private_tx_audit(args: &[String]) -> Result<(), String> {
    let db_path = required_option_value(args, "--db")?;
    let tx_hash = required_option_value(args, "--hash")?;
    let as_json = has_flag(args, "--json");
    let chain_storage = open_storage(&db_path)?;
    let body = chain_storage
        .get_private_transaction_bytes(&tx_hash)
        .map_err(|e| format!("failed to read private tx body from db: {e}"))?
        .ok_or_else(|| format!("private tx body not found in db for hash {tx_hash}"))?;
    let tx: zoka::private_wallet::PrivateTransaction = serde_json::from_slice(&body)
        .map_err(|e| format!("failed to parse private tx body {tx_hash}: {e}"))?;

    let report = audit_private_transaction_privacy(&tx);

    if as_json {
        println!(
            "{}",
            serde_json::json!({
                "hash": tx_hash,
                "audit_passed": report.audit_passed,
                "output_count": tx.outputs.len(),
                "ring_size": tx.ring_members.len(),
                "private_fee_commitment": report.private_fee,
                "zero_fee": report.zero_fee,
                "reveals_recipient_commitment": report.reveals_recipient,
                "reveals_output_blind": report.reveals_output_blind,
                "reveals_change_output": report.reveals_change,
                "reveals_clear_fee": report.reveals_clear_fee,
                "reveals_fee_opening": report.reveals_fee_opening,
                "legacy_public_amount_stream": report.legacy_public_amount_stream
            })
        );
    } else {
        println!("Private transaction privacy audit");
        println!("Hash: {}", tx_hash);
        println!("Audit passed: {}", yes_no(report.audit_passed));
        println!("Outputs: {}", tx.outputs.len());
        println!("Ring size: {}", tx.ring_members.len());
        println!("Private fee commitment: {}", yes_no(report.private_fee));
        println!("Zero fee: {}", yes_no(report.zero_fee));
        println!(
            "Reveals recipient commitment: {}",
            yes_no(report.reveals_recipient)
        );
        println!(
            "Reveals output blind: {}",
            yes_no(report.reveals_output_blind)
        );
        println!("Reveals change output: {}", yes_no(report.reveals_change));
        println!("Reveals clear fee: {}", yes_no(report.reveals_clear_fee));
        println!(
            "Reveals fee opening: {}",
            yes_no(report.reveals_fee_opening)
        );
        println!(
            "Legacy public amount stream: {}",
            yes_no(report.legacy_public_amount_stream)
        );
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PrivateTxAuditReport {
    audit_passed: bool,
    private_fee: bool,
    zero_fee: bool,
    reveals_recipient: bool,
    reveals_output_blind: bool,
    reveals_change: bool,
    reveals_clear_fee: bool,
    reveals_fee_opening: bool,
    legacy_public_amount_stream: bool,
}

fn audit_private_transaction_privacy(
    tx: &zoka::private_wallet::PrivateTransaction,
) -> PrivateTxAuditReport {
    let reveals_recipient = !tx.recipient_commitment.trim().is_empty()
        || tx
            .outputs
            .iter()
            .any(|output| !output.recipient_commitment.trim().is_empty());
    let reveals_output_blind = !tx.commitment_blind.is_empty()
        || tx
            .outputs
            .iter()
            .any(|output| !output.commitment_blind.is_empty());
    let reveals_change = tx.outputs.iter().any(|output| output.is_change);
    let reveals_clear_fee = tx.fee > 0;
    let legacy_public_amount_stream = tx.outputs.iter().any(|output| {
        output.encrypted_amount.len() == 8
            && !output.commitment_blind.is_empty()
            && (!output.recipient_commitment.is_empty() || !tx.recipient_commitment.is_empty())
    });
    let private_fee =
        tx.fee == 0 && !tx.fee_commitment.is_empty() && !tx.fee_range_proof.is_empty();
    let zero_fee = tx.fee == 0 && tx.fee_commitment.is_empty() && tx.fee_range_proof.is_empty();
    let reveals_fee_opening = tx.fee_opening.is_some();
    let audit_passed = !reveals_recipient
        && !reveals_output_blind
        && !reveals_change
        && !reveals_clear_fee
        && !reveals_fee_opening
        && !legacy_public_amount_stream
        && (private_fee || zero_fee);

    PrivateTxAuditReport {
        audit_passed,
        private_fee,
        zero_fee,
        reveals_recipient,
        reveals_output_blind,
        reveals_change,
        reveals_clear_fee,
        reveals_fee_opening,
        legacy_public_amount_stream,
    }
}

fn cmd_private_send(args: &[String]) -> Result<(), String> {
    let network = NetworkProfile::from_args(args)?;
    let from_raw = required_option_value(args, "--from")?;
    let to_raw = required_option_value(args, "--to")?;
    let amount = parse_zka_amount_arg(
        Some(required_option_value(args, "--amount")?),
        0,
        "--amount",
    )?;
    let fee = parse_zka_amount_arg(option_value(args, "--fee"), 0, "--fee")?;
    let password = password_value(args)?;
    let db_path =
        option_value(args, "--db").or_else(|| Some(network.default_db_path().to_string()));
    // Best-effort economics version check.  If the DB is locked by the miner
    // (another process) skip the check rather than aborting the send — the tx
    // will land in the sidecar queue and the miner will import it.
    if let Some(path) = db_path.as_deref() {
        if let Ok(storage) = open_storage(path) {
            let _ = ensure_atomic_economics_db(&storage);
        }
    }
    let confirm = has_flag(args, "--yes");
    if !confirm {
        return Err(
            "refusing to broadcast private transfer without explicit confirmation; add --yes"
                .to_string(),
        );
    }

    let storage = wallet_storage_for_network(network)?;
    // Se toma ANTES de leer la wallet y se sostiene hasta que termina el envio.
    // Este proceso corre en paralelo con el nodo -- ZSilent lo lanza aparte, y
    // quien usa la linea de comando lo corre a mano mientras su nodo mina. Sin
    // el lock, el nodo persiste su vista entre que aca se lee y se guarda, y
    // borra la reserva de notas que este envio acaba de crear.
    let _wallet_lock = storage
        .lock_private_wallets()
        .map_err(|e| format!("failed to lock wallet directory: {e}"))?;
    let mut manager =
        load_private_wallet_manager_for_network(&storage, password.as_deref(), network)?;

    // Antes de elegir que gastar, preguntarle a la cadena.
    //
    // La wallet decide con una marca propia que se pone cuando ve pasar la
    // transaccion que consume la nota. Si nunca la vio -- restaurada de un
    // respaldo, escaneo corto, nodo atrasado -- la nota le queda viva para
    // siempre y arma un envio que reusa un nulificador ya gastado. La red lo
    // propaga, ningun minero puede incluirlo, y el usuario espera un envio que
    // jamas va a salir. Paso de verdad.
    //
    // Si la base de la cadena no esta a mano (la tiene tomada el nodo) no se
    // puede comprobar y se sigue igual: es mejor un envio que quizas falle que
    // no poder enviar nunca.
    {
        // Se prueban dos caminos porque en la practica solo uno esta abierto.
        //
        // Leer el indice directo es lo mas rapido, pero la base la tiene tomada
        // el nodo mientras corre -- o sea siempre. La primera version de esto
        // solo intentaba abrirla y seguia de largo si fallaba: quedo codigo
        // muerto que nunca corrio, y la wallet siguio armando envios que
        // gastaban notas inexistentes. Por eso el segundo camino: preguntarle al
        // nodo por RPC, que es quien tiene la base.
        let chain = db_path.as_deref().and_then(|path| open_storage(path).ok());
        let rpc = if chain.is_none() {
            configured_local_rpc_url(network)
        } else {
            None
        };
        if chain.is_some() || rpc.is_some() {
            let corregidas = manager.reconciliar_notas_gastadas(|nullifier| {
                if let Some(chain) = chain.as_ref() {
                    return chain.is_nullifier_spent(nullifier).unwrap_or(false);
                }
                let Some(rpc) = rpc.as_deref() else {
                    return false;
                };
                // Ante la duda, NO marcar gastada: si el nodo no contesta,
                // inventar que una nota se gasto le borraria saldo real al
                // usuario. El error seguro es el otro -- dejarla pasar y que la
                // red la rechace.
                rpc_http_get_json(rpc, &format!("/private/nullifier/{nullifier}"), 5)
                    .ok()
                    .and_then(|v| v.get("spent").and_then(|b| b.as_bool()))
                    .unwrap_or(false)
            });
            if corregidas > 0 {
                println!(
                    "Aviso: {corregidas} nota(s) figuraban disponibles pero ya estaban gastadas \
                     en la cadena. Se corrigieron, asi que tu saldo real puede ser menor que el \
                     que veias."
                );
                // Se guarda con `save_private_wallet` y no con
                // `persist_private_wallet`: esta ultima vuelve a tomar el
                // candado del directorio, que ya lo tenemos agarrado unas
                // lineas mas arriba.
                for (nombre, _, _, _) in manager.list_private_wallets() {
                    if let Some(wallet) = manager.get_private_wallet(&nombre) {
                        if let Err(e) = storage.save_private_wallet(wallet, password.as_deref()) {
                            eprintln!("no pude guardar la correccion en {nombre}: {e}");
                        }
                    }
                }
            }
        }
    }

    let (from_wallet, from_commitment) =
        resolve_network_wallet_address_required(&manager, &storage, &from_raw, "--from")?;
    let sender = manager
        .get_private_wallet(&from_wallet)
        .ok_or_else(|| format!("private wallet not found: {from_wallet}"))?;
    let sender_display = user_private_address_display(&from_commitment);
    if sender.view_only || sender.spending_key.is_empty() {
        return Err("cannot send from view-only private wallet".to_string());
    }
    let required_total = amount
        .checked_add(fee)
        .ok_or_else(|| "amount plus fee overflows atomic ZKA representation".to_string())?;
    if sender.private_balance < required_total {
        let locked_balance = locked_rewards_for_address(
            &load_user_mining_state_for_network(network).unwrap_or_default(),
            &sender_display,
        );
        return Err(format!(
            "not enough spendable ZKA in '{}'. Available: {}, required: {} (amount {} + fee {}). Locked mining rewards: {}. Coinbase rewards unlock after {} blocks.",
            from_wallet,
            format_zka(sender.private_balance),
            format_zka(required_total),
            format_zka(amount),
            format_zka(fee),
            format_zka(locked_balance),
            COINBASE_MATURITY_BLOCKS
        ));
    }

    let to_commitment = require_private_recipient_address(&to_raw, "--to")?;
    let recipient_wallet_name = find_private_wallet_name_by_commitment(&manager, &to_commitment);

    let mut remaining_amount = amount;
    let mut remaining_fee = fee;
    let mut sent_chunks: Vec<(String, u64, u64)> = Vec::new();
    let mut archived_private_tx_bodies = 0usize;
    let mut archive_errors = Vec::new();
    while remaining_amount > 0 {
        let sender = manager
            .get_private_wallet(&from_wallet)
            .ok_or_else(|| format!("private wallet not found: {from_wallet}"))?;
        let max_total = max_private_tx_total_for_wallet(sender);
        if max_total == 0 {
            return Err("private send failed: no spendable private inputs available".to_string());
        }
        if remaining_fee >= max_total {
            return Err(format!(
                "private send failed: fee {} exceeds one-transaction input capacity {}",
                format_zka(remaining_fee),
                format_zka(max_total)
            ));
        }
        let chunk_amount = remaining_amount.min(max_total.saturating_sub(remaining_fee));
        if chunk_amount == 0 {
            return Err(
                "private send failed: unable to create a non-empty private transfer chunk"
                    .to_string(),
            );
        }
        let chunk_fee = remaining_fee;
        let tx_hash = if chunk_fee > 0 {
            manager
                .send_private_transaction_with_fee(
                    &from_wallet,
                    to_commitment.clone(),
                    chunk_amount,
                    chunk_fee,
                )
                .map_err(|e| format!("private send failed: {e}"))?
        } else {
            manager
                .send_private_transaction(&from_wallet, to_commitment.clone(), chunk_amount)
                .map_err(|e| format!("private send failed: {e}"))?
        };

        let tx = manager
            .get_private_transaction(&tx_hash)
            .ok_or_else(|| "private tx not found after creation".to_string())?
            .clone();
        manager
            .validate_inbound_private_transaction(&tx)
            .map_err(|e| format!("private inbound validation failed: {e}"))?;
        if let Some(db_path) = db_path.as_deref() {
            persist_local_private_tx_for_node(db_path, &tx_hash, &tx)?;
        }
        let (published, errors) = publish_private_tx_body_to_active_rpcs(network, &tx);
        if published > 0 {
            archived_private_tx_bodies = archived_private_tx_bodies.saturating_add(1);
        } else {
            archive_errors.extend(errors);
        }

        if let Some(recipient_name) = recipient_wallet_name.as_deref() {
            let decrypted = manager
                .decrypt_private_amount_for_wallet(recipient_name, &tx)
                .map_err(|e| format!("recipient decryption check failed: {e}"))?;
            if decrypted != chunk_amount {
                return Err(format!(
                    "privacy integrity check failed: recipient decrypted amount {} but expected {}",
                    decrypted, chunk_amount
                ));
            }
        }

        for candidate in manager.get_private_wallet_names() {
            if candidate == from_wallet {
                continue;
            }
            let candidate_commitment = manager
                .get_private_wallet(&candidate)
                .map(|wallet| wallet.commitment_key.as_str());
            if candidate_commitment == Some(from_commitment.as_str())
                || candidate_commitment == Some(to_commitment.as_str())
            {
                continue;
            }
            if recipient_wallet_name
                .as_deref()
                .map(|name| name == candidate.as_str())
                .unwrap_or(false)
            {
                continue;
            }
            let can_decrypt = manager
                .can_wallet_decrypt_private_tx(&candidate, &tx)
                .map_err(|e| format!("privacy isolation check failed for '{candidate}': {e}"))?;
            if can_decrypt {
                return Err(format!(
                    "privacy isolation check failed: unrelated wallet '{}' can decrypt private transfer",
                    candidate
                ));
            }
        }

        let wallet_names = manager.get_private_wallet_names();
        for name in wallet_names {
            let _ = manager.receive_private_transaction_for_wallet(&name, &tx);
        }
        sent_chunks.push((tx_hash, chunk_amount, chunk_fee));
        remaining_amount = remaining_amount.saturating_sub(chunk_amount);
        remaining_fee = 0;
    }
    persist_all_private_wallets(&storage, &manager, password.as_deref())?;

    println!("Private transfer queued");
    println!(
        "Hash: {}",
        sent_chunks
            .first()
            .map(|chunk| chunk.0.as_str())
            .unwrap_or("unavailable")
    );
    if sent_chunks.len() > 1 {
        println!("Split transactions: {}", sent_chunks.len());
        for (idx, (hash, chunk_amount, chunk_fee)) in sent_chunks.iter().enumerate() {
            println!(
                "- {} | hash={} | amount={} | fee={}",
                idx + 1,
                hash,
                format_zka(*chunk_amount),
                format_zka(*chunk_fee)
            );
        }
    }
    println!("From: {}", sender_display);
    println!("To: {}", user_private_address_display(&to_commitment));
    println!("Amount: [HIDDEN]");
    println!("Fee: [HIDDEN]");
    if let Some(db_path) = db_path {
        println!("Queued for local node DB: {}", db_path);
    }
    if archived_private_tx_bodies > 0 {
        println!("Archived private tx bodies: {}", archived_private_tx_bodies);
    } else if !archive_errors.is_empty() {
        println!("Archived private tx bodies: 0 (run private-tx-publish after RPC is reachable)");
    }
    Ok(())
}

/// `zoka consolidate --from <addr> --yes [--db <path>] [--password ...]`
///
/// Merges a fragmented private wallet (e.g. thousands of small mining-coinbase
/// notes) into far fewer, larger notes by sending the whole spendable balance
/// to the wallet's OWN address. `cmd_private_send` auto-splits that self-send
/// into chunks of up to PRIVATE_TX_MAX_INPUTS notes, so each chunk fuses ~96
/// small notes into one. One run takes N notes down to ceil(N/96); re-run
/// after the sweep is mined to consolidate further. Large ordinary sends that
/// previously needed dozens of chained txs then need far fewer.
fn cmd_private_consolidate(args: &[String]) -> Result<(), String> {
    let network = NetworkProfile::from_args(args)?;
    let from_raw = required_option_value(args, "--from")?;
    let password = password_value(args)?;
    let db_path =
        option_value(args, "--db").or_else(|| Some(network.default_db_path().to_string()));
    if !has_flag(args, "--yes") {
        return Err("refusing to consolidate without explicit confirmation; add --yes".to_string());
    }

    // Read the wallet's spendable balance + note count, then release the DB
    // before the send reopens it.
    let (own_address, balance, note_count) = {
        let storage = wallet_storage_for_network(network)?;
        let manager =
            load_private_wallet_manager_for_network(&storage, password.as_deref(), network)?;
        let (from_wallet, from_commitment) =
            resolve_network_wallet_address_required(&manager, &storage, &from_raw, "--from")?;
        let sender = manager
            .get_private_wallet(&from_wallet)
            .ok_or_else(|| format!("private wallet not found: {from_wallet}"))?;
        if sender.view_only || sender.spending_key.is_empty() {
            return Err("cannot consolidate a view-only private wallet".to_string());
        }
        let count = sender.private_notes.iter().filter(|n| !n.spent).count();
        (
            user_private_address_display(&from_commitment),
            sender.private_balance,
            count,
        )
    };

    if balance == 0 {
        return Err("nothing to consolidate: zero spendable balance".to_string());
    }
    if note_count <= 1 {
        println!("Already consolidated: {note_count} spendable note(s).");
        return Ok(());
    }

    let expected_after = (note_count + PRIVATE_TX_MAX_INPUTS - 1) / PRIVATE_TX_MAX_INPUTS;
    println!(
        "Consolidating {} across {} note(s) → ~{} note(s) ({} self-transfer chunk(s) of up to {} inputs).",
        format_zka(balance),
        note_count,
        expected_after,
        expected_after,
        PRIVATE_TX_MAX_INPUTS
    );

    // Sweep: send the whole spendable balance back to our own address.
    let mut send_args = vec![
        "--from".to_string(),
        own_address.clone(),
        "--to".to_string(),
        own_address,
        "--amount".to_string(),
        format_zka_amount(balance),
        "--fee".to_string(),
        "0".to_string(),
        "--yes".to_string(),
        "--network".to_string(),
        network.label().to_string(),
    ];
    if let Some(pw) = password.as_deref() {
        send_args.push("--password".to_string());
        send_args.push(pw.to_string());
    }
    push_opt_arg(&mut send_args, "--db", db_path);

    cmd_private_send(&send_args)?;
    println!(
        "Consolidation sweep broadcast. Once mined, re-run to merge the remaining \
         ~{expected_after} note(s) further."
    );
    Ok(())
}

fn max_private_tx_total_for_wallet(wallet: &PrivateWallet) -> u64 {
    let mut amounts = wallet
        .private_notes
        .iter()
        .filter(|note| !note.spent)
        .map(|note| note.amount)
        .collect::<Vec<_>>();
    if amounts.is_empty() && wallet.private_balance > 0 {
        return wallet.private_balance;
    }
    amounts.sort_by(|lhs, rhs| rhs.cmp(lhs));
    amounts
        .into_iter()
        .take(PRIVATE_TX_MAX_INPUTS)
        .fold(0u64, |total, amount| total.saturating_add(amount))
}

fn cmd_wallet_create_unified(args: &[String]) -> Result<(), String> {
    cmd_private_wallet_create(args)
}

fn cmd_wallet_restore_from_seed(args: &[String]) -> Result<(), String> {
    let password = required_password_value(args)?;
    let (_, storage) = wallet_storage_for_args(args)?;
    let wallet_name = option_value(args, "--name")
        .unwrap_or_else(|| generate_available_private_wallet_name(&storage, "wallet"));
    let seed_phrase = required_seed_phrase_value(args)?;
    let restore_height = parse_u64_arg(
        option_value(args, "--restore-height"),
        0,
        "--restore-height",
    )?;

    if private_wallet_exists_on_disk(&storage, &wallet_name) {
        return Err(format!("private wallet already exists: {}", wallet_name));
    }

    let mnemonic = seed_phrase
        .split_whitespace()
        .map(|word| word.to_ascii_lowercase())
        .collect::<Vec<_>>();
    let scan_key = HDWallet::mnemonic_to_seed(&mnemonic)
        .map_err(|e| format!("invalid recovery phrase: {e}"))?;
    if scan_key.len() != 16 && scan_key.len() != 32 {
        return Err("invalid recovery phrase entropy length".to_string());
    }

    let spending_key = derive_spending_key_from_recovery_seed(&scan_key);
    let spend_public_hex = public_key_hex_from_secret(&spending_key);
    let commitment_key = private_payment_address(&scan_key, &spend_public_hex);
    let mut wallet = PrivateWallet::new(
        wallet_name.clone(),
        spending_key,
        scan_key,
        commitment_key.clone(),
    );
    wallet.restore_height = restore_height;
    wallet.anonymity_public_key = spend_public_hex;

    // Toma el lock del directorio: este proceso corre en paralelo con el nodo,
    // y sin coordinacion el ultimo en escribir pisa lo del otro.
    let _wallet_lock = storage
        .lock_private_wallets()
        .map_err(|e| format!("failed to lock wallet directory: {e}"))?;
    storage
        .save_private_wallet(&wallet, Some(&password))
        .map_err(|e| format!("failed to save private wallet: {e}"))?;

    println!("Private wallet restored from seed");
    println!("Name: {}", wallet_name);
    println!("Address: {}", user_private_address_display(&commitment_key));
    print_wallet_ready_notice(&storage, &wallet_name);
    println!("Restore height: {}", restore_height);
    Ok(())
}

fn cmd_wallet_import_disabled(_args: &[String]) -> Result<(), String> {
    Err("wallet-import is disabled in unified mode; use wallet-restore-from-keys".to_string())
}

fn cmd_wallet_restore_from_keys_unified(args: &[String]) -> Result<(), String> {
    cmd_wallet_restore_from_keys(args)
}

fn cmd_wallet_list_unified(args: &[String]) -> Result<(), String> {
    cmd_private_wallet_list(args)
}

fn cmd_wallet_balance_unified(args: &[String]) -> Result<(), String> {
    cmd_private_balance(args)
}

fn cmd_wallet_history_unified(args: &[String]) -> Result<(), String> {
    cmd_private_history(args)
}

fn cmd_wallet_change_passphrase_unified(args: &[String]) -> Result<(), String> {
    let wallet_selector = required_wallet_selector(args)?;
    let old_password = option_value(args, "--old-password")
        .or_else(|| option_value(args, "--password"))
        .map(Ok)
        .or_else(|| {
            option_value(args, "--old-password-env")
                .or_else(|| option_value(args, "--password-env"))
                .map(|var_name| secret_from_env_var(&var_name, "--old-password-env"))
        })
        .transpose()?;
    let new_password = option_value(args, "--new-password")
        .map(Ok)
        .or_else(|| {
            option_value(args, "--new-password-env")
                .map(|var_name| secret_from_env_var(&var_name, "--new-password-env"))
        })
        .transpose()?
        .ok_or_else(|| {
            "missing required argument: --new-password or --new-password-env".to_string()
        })?;
    if new_password.is_empty() {
        return Err("--new-password cannot be empty".to_string());
    }

    let (network, storage) = wallet_storage_for_args(args)?;
    // Sin lock, el nodo puede persistir su copia -- cifrada con la contrasena
    // vieja -- justo despues, y el cambio se pierde en silencio. Y se toma
    // antes de leer: guardar lo leido antes pisaria un envio hecho en el medio.
    let _wallet_lock = storage
        .lock_private_wallets()
        .map_err(|e| format!("failed to lock wallet directory: {e}"))?;
    let mut manager =
        load_private_wallet_manager_for_network(&storage, old_password.as_deref(), network)?;
    let wallet_name = resolve_private_wallet_name_required(&manager, &wallet_selector)?;
    let wallet = manager
        .get_private_wallet_mut(&wallet_name)
        .ok_or_else(|| format!("wallet not found: {wallet_name}"))?;
    wallet.normalize_key_material();
    let address = user_private_address_display(&wallet.commitment_key);
    storage
        .save_private_wallet(wallet, Some(&new_password))
        .map_err(|e| format!("failed to change wallet passphrase: {e}"))?;

    println!("Wallet passphrase changed");
    println!("Name: {}", wallet_name);
    println!("Address: {}", address);
    Ok(())
}

fn cmd_sync_status_unified(args: &[String]) -> Result<(), String> {
    let (network, storage) = wallet_storage_for_args(args)?;
    let wallet_selector = required_wallet_selector(args)?;
    let password = password_value(args)?;
    let db_path =
        option_value(args, "--db").or_else(|| Some(network.default_db_path().to_string()));
    let as_json = has_flag(args, "--json");
    let manager = load_private_wallet_manager_for_network(&storage, password.as_deref(), network)?;
    let wallet_name = resolve_private_wallet_name_required(&manager, &wallet_selector)?;
    let wallet = manager
        .get_private_wallet(&wallet_name)
        .ok_or_else(|| format!("wallet not found: {wallet_name}"))?;
    let chain_height = current_known_height_for_network(db_path.as_deref(), network);
    let scanned_height = chain_height;
    let restore_height = wallet.restore_height;
    let progress = if chain_height == 0 {
        100.0
    } else if scanned_height <= restore_height {
        0.0
    } else {
        let denom = (chain_height.saturating_sub(restore_height)).max(1) as f64;
        let num = (scanned_height.saturating_sub(restore_height)) as f64;
        (num / denom * 100.0).min(100.0)
    };
    if as_json {
        println!(
            "{}",
            serde_json::json!({
                "wallet": wallet_name,
                "chain_height": chain_height,
                "restore_height": restore_height,
                "scanned_height": scanned_height,
                "progress_percent": progress
            })
        );
        return Ok(());
    }
    println!("Wallet: {}", wallet_name);
    println!("Chain height: {}", chain_height);
    println!("Restore height: {}", restore_height);
    println!("Scanned height: {}", scanned_height);
    println!("Progress: {:.2}%", progress);
    Ok(())
}

fn cmd_wallet_backup_unified(args: &[String]) -> Result<(), String> {
    let wallet_selector = required_wallet_selector(args)?;
    let out_path = required_option_value(args, "--out")?;
    let password = password_value(args)?;
    let (network, storage) = wallet_storage_for_args(args)?;
    let manager = load_private_wallet_manager_for_network(&storage, password.as_deref(), network)?;
    let wallet_name = resolve_private_wallet_name_required(&manager, &wallet_selector)?;
    let src = storage
        .private_wallets_dir()
        .join(format!("{}.private.json", wallet_name));
    if !src.exists() {
        return Err(format!("wallet file not found: {}", src.display()));
    }
    let dst = PathBuf::from(&out_path);
    if let Some(parent) = dst.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create backup directory: {e}"))?;
        }
    }
    fs::copy(&src, &dst).map_err(|e| {
        format!(
            "failed to copy backup file from '{}' to '{}': {}",
            src.display(),
            dst.display(),
            e
        )
    })?;
    println!("Backup created at {}", out_path);
    Ok(())
}

fn cmd_wallet_restore_from_backup_unified(args: &[String]) -> Result<(), String> {
    let in_path = required_option_value(args, "--in")?;
    let password = required_password_value(args)?;
    let import_name = option_value(args, "--name");
    let (network, storage) = wallet_storage_for_args(args)?;
    let src = PathBuf::from(&in_path);
    if !src.is_file() {
        return Err(format!("backup file not found: {}", src.display()));
    }

    let raw = fs::read_to_string(&src)
        .map_err(|e| format!("failed to read backup '{}': {e}", src.display()))?;
    let mut backup: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| format!("backup file is not valid encrypted wallet JSON: {e}"))?;

    let wallet = backup
        .get_mut("wallet")
        .and_then(|value| value.as_object_mut())
        .ok_or_else(|| "backup file is missing wallet metadata".to_string())?;
    let original_name = wallet
        .get("name")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let wallet_name = import_name.unwrap_or(original_name);
    if wallet_name.trim().is_empty() {
        return Err("backup wallet name is empty; pass --name <wallet-name>".to_string());
    }
    if wallet_name == "."
        || wallet_name == ".."
        || wallet_name.chars().any(|ch| {
            ch.is_control() || matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        })
    {
        return Err(
            "wallet name contains characters that are not safe for a wallet file".to_string(),
        );
    }
    wallet.insert(
        "name".to_string(),
        serde_json::Value::String(wallet_name.clone()),
    );
    let address = wallet
        .get("commitment_key")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();
    if address.trim().is_empty() {
        return Err("backup file is missing the wallet address".to_string());
    }
    let has_spending_key = backup
        .get("encrypted_spending_key")
        .and_then(|value| value.as_array())
        .map(|items| !items.is_empty())
        .unwrap_or(false);
    let has_scan_key = backup
        .get("encrypted_scan_key")
        .and_then(|value| value.as_array())
        .map(|items| !items.is_empty())
        .unwrap_or(false);
    if !has_spending_key || !has_scan_key {
        return Err("backup file is not an encrypted private wallet backup".to_string());
    }

    fs::create_dir_all(storage.private_wallets_dir())
        .map_err(|e| format!("failed to create private wallet directory: {e}"))?;
    let dst = storage
        .private_wallets_dir()
        .join(format!("{}.private.json", wallet_name));
    if dst.exists() {
        return Err(format!(
            "wallet '{}' already exists locally; choose another --name or remove the existing wallet first",
            wallet_name
        ));
    }

    let encoded = serde_json::to_string_pretty(&backup)
        .map_err(|e| format!("failed to encode restored backup JSON: {e}"))?;
    fs::write(&dst, encoded)
        .map_err(|e| format!("failed to write restored wallet '{}': {e}", dst.display()))?;

    let manager = load_private_wallet_manager_for_network(&storage, Some(&password), network)?;
    let wallet = match manager.get_private_wallet(&wallet_name) {
        Some(wallet) => wallet,
        None => {
            let _ = fs::remove_file(&dst);
            return Err(
                "backup was copied but could not be unlocked with the provided password; import was rolled back"
                    .to_string(),
            );
        }
    };
    let address = user_private_address_display(&wallet.commitment_key);
    println!("Encrypted wallet backup restored");
    println!("Name: {}", wallet_name);
    println!("Address: {}", address);
    print_wallet_ready_notice(&storage, &wallet_name);
    Ok(())
}

fn cmd_wallet_wipe_unified(args: &[String]) -> Result<(), String> {
    let wallet_selector = required_wallet_selector(args)?;
    let password = password_value(args)?;
    let force = args.iter().any(|a| a == "--force");
    if !force {
        return Err(
            "delete local wallet removes only the encrypted wallet file from this computer; back it up first, then pass --force to confirm"
                .to_string(),
        );
    }
    let (network, storage) = wallet_storage_for_args(args)?;
    let manager = load_private_wallet_manager_for_network(&storage, password.as_deref(), network)?;
    let wallet_name = resolve_private_wallet_name_required(&manager, &wallet_selector)?;
    storage
        .delete_wallet(&wallet_name, true)
        .map_err(|e| format!("failed to delete local wallet file: {e}"))?;
    println!("Local wallet deleted: {}", wallet_name);
    Ok(())
}

fn cmd_pending_transfers(args: &[String]) -> Result<(), String> {
    let network = NetworkProfile::from_args(args)?;
    let db_path =
        option_value(args, "--db").unwrap_or_else(|| network.default_db_path().to_string());
    let limit = parse_u64_arg(option_value(args, "--limit"), 20, "--limit")? as usize;
    let as_json = has_flag(args, "--json");
    let hashes = if Path::new(&db_path).exists() {
        local_pending_private_tx_hashes(&db_path, limit)?
    } else {
        Vec::new()
    };
    let sidecar_hashes = list_sidecar_private_tx_hashes(&db_path);
    if as_json {
        println!(
            "{}",
            serde_json::json!({
                "db": db_path,
                "count": hashes.len(),
                "sidecar_count": sidecar_hashes.len(),
                "pending_private_tx_hashes": hashes,
                "sidecar_private_tx_hashes": sidecar_hashes,
            })
        );
        return Ok(());
    }
    println!("Pending private transfers");
    println!("Node DB: {}", db_path);
    if hashes.is_empty() && sidecar_hashes.is_empty() {
        println!("No pending private transfers.");
        println!(
            "When you send from the console, the transfer is queued here for your local node to mine."
        );
        return Ok(());
    }
    for hash in &hashes {
        println!("- {}", hash);
    }
    if !sidecar_hashes.is_empty() {
        println!("Sidecar queue (waiting for node import):");
        for hash in sidecar_hashes {
            println!("- {}", hash);
        }
    }
    Ok(())
}

fn cmd_private_pending_diagnostics(args: &[String]) -> Result<(), String> {
    let network = NetworkProfile::from_args(args)?;
    let db_path =
        option_value(args, "--db").unwrap_or_else(|| network.default_db_path().to_string());
    let storage = open_storage(&db_path)?;
    let target_block_height = match option_value(args, "--height") {
        Some(raw) => parse_u64_arg(Some(raw), 1, "--height")?,
        None => storage
            .get_chain_height()
            .map_err(|e| format!("failed to read chain height: {e}"))?
            .saturating_add(1),
    };
    let limit = parse_u64_arg(
        option_value(args, "--limit"),
        LOCAL_PRIVATE_TXS_PER_BLOCK_LIMIT as u64,
        "--limit",
    )? as usize;
    let hashes = storage
        .iterate_pending_private_transaction_hashes()
        .map_err(|e| format!("failed to read pending private txs: {e}"))?;
    let diagnostics =
        diagnose_pending_private_tx_selection(&storage, hashes, limit, target_block_height);

    if has_flag(args, "--json") {
        println!(
            "{}",
            serde_json::json!({
                "db": db_path,
                "chain_id": network.chain_id(),
                "diagnostics": diagnostics,
            })
        );
        return Ok(());
    }

    println!("Pending private tx diagnostics");
    println!("Node DB: {}", db_path);
    println!("Chain id: {}", network.chain_id());
    println!("{}", summarize_pending_private_tx_selection(&diagnostics));
    for candidate in diagnostics.candidates {
        println!(
            "- {}: {}{}",
            candidate.hash,
            candidate.reason,
            if candidate.tag_count > 0 {
                format!(" ({} tag(s))", candidate.tag_count)
            } else {
                String::new()
            }
        );
    }
    Ok(())
}

fn cmd_chain_economics_migrate(args: &[String]) -> Result<(), String> {
    let network = NetworkProfile::from_args(args)?;
    let db_path =
        option_value(args, "--db").unwrap_or_else(|| network.default_db_path().to_string());
    let dry_run = has_flag(args, "--dry-run");
    let yes = has_flag(args, "--yes");
    let repair_supply = has_flag(args, "--repair-supply");
    let as_json = has_flag(args, "--json");
    // Negarse si la base no existe todavia.
    //
    // La ruta por defecto es RELATIVA al directorio actual, asi que este comando
    // corrido desde la carpeta equivocada abriria una base nueva, la encontraria
    // vacia, la sellaria y diria "Marker written: yes". El usuario se va
    // convencido de haber arreglado su cadena sin haber tocado su cadena.
    //
    // Este es el unico comando donde equivocarse de carpeta parece exito: los
    // demas muestran altura 0 o ninguna wallet, que se ve mal a simple vista.
    if !std::path::Path::new(&db_path).exists() {
        return Err(format!(
            "no chain db at '{db_path}'. This command never creates one: pass --db with the \
             path to the database you mean to migrate, or run it from the directory that \
             contains '{}'. Sealing an empty database would report success without \
             migrating anything.",
            network.default_db_path()
        ));
    }
    let storage = RocksDbStorage::new(&db_path)
        .map_err(|e| format!("failed to open chain db '{db_path}': {e}"))?;
    let report =
        migrate_atomic_economics_marker(&storage, &db_path, network, dry_run, yes, repair_supply)?;

    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report)
                .map_err(|e| format!("failed to encode migration report: {e}"))?
        );
        return Ok(());
    }

    println!("Chain economics migration");
    println!("DB: {}", report.db_path);
    println!("Chain id: {}", report.chain_id);
    println!("Economics version: {}", report.economics_version);
    println!("Height checked: {}", report.height);
    println!(
        "Stored emitted: {}",
        format_zka(report.total_supply_emitted_atoms)
    );
    println!(
        "Computed emitted: {}",
        format_zka(report.computed_emission_atoms)
    );
    println!(
        "Marker before: {}",
        report.marker_before.as_deref().unwrap_or("<missing>")
    );
    println!("Marker written: {}", yes_no(report.marker_written));
    println!("Supply repaired: {}", yes_no(report.supply_repaired));
    println!("Dry run: {}", yes_no(report.dry_run));
    Ok(())
}

fn cmd_tx_send_unified(args: &[String]) -> Result<(), String> {
    let network = NetworkProfile::from_args(args)?;
    let from = required_option_value(args, "--from")?;
    let to = required_option_value(args, "--to")?;
    let from_commitment = require_private_network_address(&from, "--from")?;
    let to_commitment = require_private_recipient_address(&to, "--to")?;
    let amount = required_option_value(args, "--amount")?;
    let password = option_value(args, "--password");
    let password_env = option_value(args, "--password-env")
        .or_else(|| option_value(args, "--wallet-password-env"));
    let password_stdin =
        has_flag(args, "--password-stdin") || has_flag(args, "--wallet-password-stdin");
    let db_path =
        option_value(args, "--db").or_else(|| Some(network.default_db_path().to_string()));
    let confirm = has_flag(args, "--yes");
    if !confirm {
        return Err("refusing to broadcast without explicit confirmation; add --yes".to_string());
    }

    let fee = if let Some(raw) = option_value(args, "--fee") {
        raw
    } else {
        format_zka_amount(resolve_fee(args)?)
    };

    let mut forwarded = vec![
        "--from".to_string(),
        user_private_address_display(&from_commitment),
        "--to".to_string(),
        user_private_address_display(&to_commitment),
        "--amount".to_string(),
        amount,
        "--fee".to_string(),
        fee,
        "--yes".to_string(),
        "--network".to_string(),
        network.label().to_string(),
    ];
    push_opt_arg(&mut forwarded, "--password", password);
    push_opt_arg(&mut forwarded, "--password-env", password_env);
    if password_stdin {
        forwarded.push("--password-stdin".to_string());
    }
    push_opt_arg(&mut forwarded, "--db", db_path);
    cmd_private_send(&forwarded)
}

fn cmd_height(args: &[String]) -> Result<(), String> {
    let (network, db_path) = chain_db_path_for_args(args)?;
    let as_json = has_flag(args, "--json");
    let h = current_known_height_for_network(Some(&db_path), network);
    if as_json {
        println!("{}", serde_json::json!({ "height": h }));
        return Ok(());
    }
    println!("Height: {}", h);
    Ok(())
}

fn cmd_block_info_from_local_rpc(
    height: u64,
    as_json: bool,
    network: NetworkProfile,
) -> Result<(), String> {
    let rpc = required_local_rpc_url(network)?;
    let block = rpc_http_get_json(&rpc, &format!("/blocks/{height}"), 3)?;
    let private = rpc_http_get_json(&rpc, &format!("/blocks/{height}/private_tx_hashes"), 3)
        .unwrap_or_else(|_| serde_json::json!({ "private_tx_hashes": [] }));
    let private_tx_hashes = private
        .get("private_tx_hashes")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    if as_json {
        let mut out = block.as_object().cloned().unwrap_or_default();
        out.insert(
            "private_tx_hashes".to_string(),
            serde_json::Value::Array(private_tx_hashes),
        );
        out.insert(
            "source".to_string(),
            serde_json::Value::String("local_rpc".to_string()),
        );
        println!("{}", serde_json::Value::Object(out));
        return Ok(());
    }

    println!(
        "Height:    {}",
        rpc_json_u64(&block, &["height"]).unwrap_or(height)
    );
    println!(
        "Hash:      {}",
        block
            .get("hash")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
    );
    println!(
        "Prev hash: {}",
        block
            .get("previous_hash")
            .or_else(|| block.get("prev_hash"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
    );
    println!(
        "Timestamp: {}",
        rpc_json_u64(&block, &["timestamp"]).unwrap_or_default()
    );
    println!(
        "Nonce:     {}",
        rpc_json_u64(&block, &["nonce"]).unwrap_or_default()
    );
    println!(
        "Reward:    {}",
        format_zka(rpc_json_u64(&block, &["reward_atoms"]).unwrap_or_default())
    );
    println!(
        "Txs:       {}",
        rpc_json_u64(&block, &["transactions_count", "tx_count"]).unwrap_or_default()
    );
    println!("Private txs: {}", private_tx_hashes.len());
    if !private_tx_hashes.is_empty() {
        println!("Private tx hashes:");
        for hash in &private_tx_hashes {
            if let Some(hash) = hash.as_str() {
                println!("- {hash}");
            }
        }
    }
    println!("Source:    local RPC");
    Ok(())
}

fn cmd_block_info(args: &[String]) -> Result<(), String> {
    let (network, db_path) = chain_db_path_for_args(args)?;
    let height_str = required_option_value(args, "--height")?;
    let height: u64 = height_str
        .parse()
        .map_err(|_| format!("--height must be a non-negative integer (got '{height_str}')"))?;
    let as_json = has_flag(args, "--json");

    let storage = match RocksDbStorage::new(&db_path) {
        Ok(storage) => storage,
        Err(e) => {
            let open_err = format!("failed to open chain db '{db_path}': {e}");
            if is_storage_lock_error(&open_err) {
                return cmd_block_info_from_local_rpc(height, as_json, network).map_err(
                    |rpc_err| format!("{open_err}; local RPC fallback failed: {rpc_err}"),
                );
            }
            return Err(open_err);
        }
    };

    let block = storage
        .get_block_by_height(height)
        .map_err(|e| format!("failed to read block at height {height}: {e}"))?
        .ok_or_else(|| format!("no block found at height {height}"))?;

    let miner = block
        .transactions
        .first()
        .map(|tx| tx.receiver.clone())
        .unwrap_or_default();
    let reward = block.transactions.first().map(|tx| tx.amount).unwrap_or(0);

    if as_json {
        println!(
            "{}",
            serde_json::json!({
                "height": block.header.block_height,
                "hash": block.header.hash,
                "prev_hash": block.header.prev_hash,
                "timestamp": block.header.timestamp,
                "nonce": block.header.nonce,
                "miner": miner,
                "reward_atoms": reward,
                "reward_zka": format_zka_amount(reward),
                "tx_count": block.transactions.len(),
                "private_tx_count": block.private_tx_hashes.len(),
                "private_tx_hashes": block.private_tx_hashes,
                "private_fee_claim_count": block.private_fee_claims.len(),
                "has_private_coinbase": block.private_coinbase.is_some(),
            })
        );
        return Ok(());
    }
    println!("Height:    {}", block.header.block_height);
    println!("Hash:      {}", block.header.hash);
    println!("Prev hash: {}", block.header.prev_hash);
    println!("Timestamp: {}", block.header.timestamp);
    println!("Nonce:     {}", block.header.nonce);
    println!("Miner:     {miner}");
    println!("Reward:    {}", format_zka(reward));
    println!("Txs:       {}", block.transactions.len());
    println!("Private txs: {}", block.private_tx_hashes.len());
    if !block.private_tx_hashes.is_empty() {
        println!("Private tx hashes:");
        for hash in &block.private_tx_hashes {
            println!("- {hash}");
        }
    }
    println!("Private fee claims: {}", block.private_fee_claims.len());
    println!(
        "Private coinbase: {}",
        yes_no(block.private_coinbase.is_some())
    );
    Ok(())
}

fn cmd_chain_stats(args: &[String]) -> Result<(), String> {
    let (network, db_path) = chain_db_path_for_args(args)?;
    let as_json = has_flag(args, "--json");
    let storage = RocksDbStorage::new(&db_path)
        .map_err(|e| format!("failed to open chain db '{db_path}': {e}"))?;

    let height = storage
        .get_chain_height()
        .map_err(|e| format!("failed to read chain height: {e}"))?;
    let head_hash = storage
        .get_last_block_hash()
        .map_err(|e| format!("failed to read head hash: {e}"))?;
    let state_root = storage
        .get_state_root()
        .map_err(|e| format!("failed to read state root: {e}"))?;
    let private_commitment_root = storage
        .get_private_commitment_root()
        .map_err(|e| format!("failed to read private commitment root: {e}"))?;
    let total_supply_emitted = storage
        .get_total_supply_emitted()
        .map_err(|e| format!("failed to read emitted supply: {e}"))?;
    let cumulative_work = storage
        .get_cumulative_work()
        .map_err(|e| format!("failed to read cumulative work: {e}"))?;
    let current_difficulty_bits = storage
        .get_current_difficulty_bits()
        .map_err(|e| format!("failed to read current difficulty: {e}"))?;
    let effective_difficulty_bits = if current_difficulty_bits == 0 {
        POW_INITIAL_DIFFICULTY_BITS
    } else {
        current_difficulty_bits
    };
    let next_height = height.saturating_add(1);
    let next_reward = economics::block_reward_for_height(next_height, total_supply_emitted);
    let remaining_supply = EMISSION_MAX_SUPPLY.saturating_sub(total_supply_emitted);
    let halving_in_blocks = EMISSION_HALVING_INTERVAL - (height % EMISSION_HALVING_INTERVAL);

    if as_json {
        println!(
            "{}",
            serde_json::json!({
                "db": db_path,
                "chain_id": network.chain_id(),
                "height": height,
                "head_hash": head_hash,
                "state_root": state_root,
                "private_commitment_root": private_commitment_root,
                "total_supply_emitted": total_supply_emitted,
                "total_supply_emitted_zka": format_zka_amount(total_supply_emitted),
                "max_supply": EMISSION_MAX_SUPPLY,
                "max_supply_zka": format_zka_amount(EMISSION_MAX_SUPPLY),
                "remaining_supply": remaining_supply,
                "remaining_supply_zka": format_zka_amount(remaining_supply),
                "cumulative_work": cumulative_work.to_string(),
                "current_difficulty_bits": current_difficulty_bits,
                "effective_difficulty_bits": effective_difficulty_bits,
                "next_height": next_height,
                "next_reward": next_reward,
                "next_reward_zka": format_zka_amount(next_reward),
                "amount_unit": "atoms",
                "atoms_per_zka": economics::ATOMS_PER_ZKA,
                "halving_in_blocks": halving_in_blocks,
                "pow_algorithm": active_pow_algorithm_for_chain(network.chain_id()),
                "coinbase_maturity_blocks": COINBASE_MATURITY_BLOCKS,
            })
        );
        return Ok(());
    }

    println!("Chain stats");
    println!("DB:                       {db_path}");
    println!("Chain id:                 {}", network.chain_id());
    println!("Height:                   {height}");
    println!("Head hash:                {head_hash}");
    println!("State root:               {state_root}");
    println!("Private commitment root:  {private_commitment_root}");
    println!(
        "Total supply emitted:     {}",
        format_zka(total_supply_emitted)
    );
    println!("Remaining supply:         {}", format_zka(remaining_supply));
    println!("Cumulative work:          {cumulative_work}");
    println!("Current difficulty bits:  {current_difficulty_bits}");
    println!("Effective difficulty:     {effective_difficulty_bits} bits");
    println!(
        "PoW algorithm:            {}",
        active_pow_algorithm_for_chain(network.chain_id())
    );
    println!("Next height:              {next_height}");
    println!("Next reward:              {}", format_zka(next_reward));
    println!("Halving in:               {halving_in_blocks} block(s)");
    println!("Coinbase maturity:        {COINBASE_MATURITY_BLOCKS} block(s)");
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RpcHttpUrl {
    host: String,
    port: u16,
    base_path: String,
}

fn parse_rpc_http_url(raw: &str) -> Result<RpcHttpUrl, String> {
    let trimmed = raw.trim();
    if trimmed.starts_with("https://") {
        return Err("RPC HTTPS endpoints require a local HTTP proxy or future TLS client support; use --rpc http://host:port for v0.4".to_string());
    }
    let rest = trimmed
        .strip_prefix("http://")
        .ok_or_else(|| "RPC URL must start with http:// for v0.4 read-only client".to_string())?;
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    if authority.is_empty() || authority.contains('@') {
        return Err("RPC URL must not contain credentials".to_string());
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, raw_port)) if !host.is_empty() => {
            let port = raw_port
                .parse::<u16>()
                .map_err(|_| format!("invalid RPC port: {raw_port}"))?;
            (host.to_string(), port)
        }
        _ => (authority.to_string(), 80),
    };
    if host.is_empty()
        || host
            .chars()
            .any(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')))
    {
        return Err("RPC host contains unsupported characters".to_string());
    }
    let base_path = if path.is_empty() {
        String::new()
    } else {
        format!("/{}", path.trim_end_matches('/'))
    };
    if base_path
        .chars()
        .any(|c| !(c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '-' | '_' | '~')))
    {
        return Err("RPC base path contains unsupported characters".to_string());
    }
    Ok(RpcHttpUrl {
        host,
        port,
        base_path,
    })
}

fn rpc_join_path(base: &str, path: &str) -> String {
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    if base.is_empty() {
        path
    } else {
        format!("{base}{path}")
    }
}

fn find_crlf(bytes: &[u8], start: usize) -> Option<usize> {
    bytes
        .get(start..)?
        .windows(2)
        .position(|window| window == b"\r\n")
        .map(|pos| start + pos)
}

fn http_header_has_token(head: &str, header_name: &str, token: &str) -> bool {
    head.lines().skip(1).any(|line| {
        let Some((name, value)) = line.split_once(':') else {
            return false;
        };
        name.trim().eq_ignore_ascii_case(header_name)
            && value
                .split(',')
                .any(|part| part.trim().eq_ignore_ascii_case(token))
    })
}

fn decode_http_chunked_body(body: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < body.len() {
        while matches!(body.get(pos), Some(b'\r' | b'\n')) {
            pos += 1;
        }
        let line_end = find_crlf(body, pos)
            .ok_or_else(|| "HTTP chunked body missing chunk size".to_string())?;
        let size_line = std::str::from_utf8(&body[pos..line_end])
            .map_err(|_| "HTTP chunked size was not UTF-8".to_string())?;
        let size_hex = size_line.split(';').next().unwrap_or_default().trim();
        if size_hex.is_empty() {
            return Err("HTTP chunked body had empty chunk size".to_string());
        }
        let size = usize::from_str_radix(size_hex, 16)
            .map_err(|_| format!("HTTP chunked body had invalid chunk size: {size_hex}"))?;
        pos = line_end + 2;
        if size == 0 {
            return Ok(out);
        }
        let end = pos
            .checked_add(size)
            .ok_or_else(|| "HTTP chunked body size overflow".to_string())?;
        if end > body.len() {
            return Err("HTTP chunked body ended before chunk data".to_string());
        }
        out.extend_from_slice(&body[pos..end]);
        pos = end;
        if body.get(pos..pos + 2) != Some(b"\r\n") {
            return Err("HTTP chunked body missing chunk terminator".to_string());
        }
        pos += 2;
    }
    Err("HTTP chunked body ended before final zero chunk".to_string())
}

fn decode_http_json_response(
    response: &[u8],
    error_prefix: &str,
) -> Result<serde_json::Value, String> {
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| format!("{error_prefix} response was not valid HTTP"))?;
    let head = String::from_utf8(response[..header_end].to_vec())
        .map_err(|_| format!("{error_prefix} response headers were not UTF-8"))?;
    let status = head.lines().next().unwrap_or_default();
    if !status.contains(" 200 ") {
        return Err(format!("{error_prefix} returned non-200 status: {status}"));
    }
    let body_bytes = &response[header_end + 4..];
    let body_bytes = if http_header_has_token(&head, "transfer-encoding", "chunked") {
        decode_http_chunked_body(body_bytes)?
    } else {
        body_bytes.to_vec()
    };
    let body = String::from_utf8(body_bytes)
        .map_err(|_| format!("{error_prefix} response body was not UTF-8"))?;
    serde_json::from_str(body.trim()).map_err(|e| format!("{error_prefix} JSON decode failed: {e}"))
}

fn rpc_http_get_json(
    raw_url: &str,
    path: &str,
    timeout_secs: u64,
) -> Result<serde_json::Value, String> {
    let url = parse_rpc_http_url(raw_url)?;
    let request_path = rpc_join_path(&url.base_path, path);
    let timeout = Duration::from_secs(timeout_secs.clamp(1, 10));
    // Bound the CONNECT, not just read/write. TcpStream::connect uses the OS
    // default connect timeout (~21s on Windows, os error 10060) when the
    // endpoint is unreachable. The node's sync loop calls this synchronously,
    // so a single dead RPC endpoint (e.g. a decommissioned seed, or a DNS name
    // that resolves but whose port isn't reachable) stalls the whole loop for
    // ~21s every cycle. During that stall the libp2p swarm isn't pumped, so
    // peer handshakes and block request/response substreams time out -- nodes
    // then fail to connect and IBD gets stuck. Resolving + connect_timeout
    // makes an unreachable endpoint fail within `timeout` instead.
    let socket_addr = (url.host.as_str(), url.port)
        .to_socket_addrs()
        .map_err(|e| format!("RPC connect failed for {}:{}: {e}", url.host, url.port))?
        .next()
        .ok_or_else(|| {
            format!(
                "RPC connect failed for {}:{}: no address resolved",
                url.host, url.port
            )
        })?;
    let mut stream = TcpStream::connect_timeout(&socket_addr, timeout)
        .map_err(|e| format!("RPC connect failed for {}:{}: {e}", url.host, url.port))?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|e| format!("failed to set RPC read timeout: {e}"))?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|e| format!("failed to set RPC write timeout: {e}"))?;
    let request = format!(
        "GET {request_path} HTTP/1.1\r\nHost: {}\r\nAccept: application/json\r\nConnection: close\r\nUser-Agent: zoka-cli-rpc/0.4\r\n\r\n",
        url.host
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("RPC request write failed: {e}"))?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .map_err(|e| format!("RPC response read failed: {e}"))?;
    decode_http_json_response(&response, &format!("RPC GET {request_path}"))
}

fn is_rpc_rate_limit_error(err: &str) -> bool {
    err.contains(" 429 ") || err.contains("429 Too Many Requests")
}

fn rpc_http_get_json_with_rate_limit_backoff(
    raw_url: &str,
    path: &str,
    timeout_secs: u64,
) -> Result<serde_json::Value, String> {
    let delays = [0u64, 8, 20];
    let mut last_err = None;
    for delay in delays {
        if delay > 0 {
            thread::sleep(Duration::from_secs(delay));
        }
        match rpc_http_get_json(raw_url, path, timeout_secs) {
            Ok(value) => return Ok(value),
            Err(err) if is_rpc_rate_limit_error(&err) => last_err = Some(err),
            Err(err) => return Err(err),
        }
    }
    Err(last_err.unwrap_or_else(|| "RPC request failed after backoff".to_string()))
}

fn rpc_http_post_json(
    raw_url: &str,
    path: &str,
    body: &serde_json::Value,
    timeout_secs: u64,
) -> Result<serde_json::Value, String> {
    let url = parse_rpc_http_url(raw_url)?;
    let request_path = rpc_join_path(&url.base_path, path);
    let timeout = Duration::from_secs(timeout_secs.clamp(1, 10));
    let payload = serde_json::to_vec(body).map_err(|e| format!("RPC JSON encode failed: {e}"))?;
    let mut stream = TcpStream::connect((url.host.as_str(), url.port))
        .map_err(|e| format!("RPC connect failed for {}:{}: {e}", url.host, url.port))?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|e| format!("failed to set RPC read timeout: {e}"))?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|e| format!("failed to set RPC write timeout: {e}"))?;
    let request = format!(
        "POST {request_path} HTTP/1.1\r\nHost: {}\r\nAccept: application/json\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nUser-Agent: zoka-cli-rpc/0.4\r\n\r\n",
        url.host,
        payload.len()
    );
    stream
        .write_all(request.as_bytes())
        .and_then(|_| stream.write_all(&payload))
        .map_err(|e| format!("RPC request write failed: {e}"))?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .map_err(|e| format!("RPC response read failed: {e}"))?;
    decode_http_json_response(&response, &format!("RPC POST {request_path}"))
}

fn active_rpc_endpoints_from_config(
    args: &[String],
    network: NetworkProfile,
) -> Result<Vec<MainnetRpcEndpoint>, String> {
    let path = network_config_path_from_args(args, network);
    let config = load_network_config_for(&path, network)?;
    Ok(config
        .public_rpc
        .into_iter()
        .filter(|rpc| {
            rpc.status.eq_ignore_ascii_case("active") || rpc.status.eq_ignore_ascii_case("fallback")
        })
        .collect())
}

fn resolve_rpc_url(args: &[String], network: NetworkProfile) -> Result<String, String> {
    if let Some(url) = option_value(args, "--rpc") {
        parse_rpc_http_url(&url)?;
        return Ok(url);
    }
    let active = active_rpc_endpoints_from_config(args, network)?;
    if active.is_empty() {
        return Err("no active RPC endpoint configured; pass --rpc http://host:port".to_string());
    }
    let endpoints = active.iter().map(|rpc| rpc.url.clone()).collect::<Vec<_>>();
    let (samples, errors) = sample_rpc_heights(&endpoints, 3);
    let Some(best) = samples.into_iter().next() else {
        return Err(format!(
            "no reachable active RPC endpoint configured: {}",
            errors.join("; ")
        ));
    };
    parse_rpc_http_url(&best.rpc)?;
    Ok(best.rpc)
}

fn local_canonical_hash_at_height(
    db_path: Option<&str>,
    height: u64,
) -> Result<Option<String>, String> {
    let Some(path) = db_path else {
        return Ok(None);
    };
    let storage = RocksDbStorage::new(path)
        .map_err(|err| format!("failed to open local chain DB for sync gate: {err}"))?;
    storage
        .get_canonical_block_hash_at_height(height)
        .map_err(|err| format!("failed to read local canonical hash at height {height}: {err:?}"))
}

fn ensure_local_chain_matches_rpc_at_height(
    rpc: &str,
    db_path: Option<&str>,
    height: u64,
) -> Result<(), String> {
    if height == 0 {
        return Ok(());
    }
    let Some(local_hash) = local_canonical_hash_at_height(db_path, height)? else {
        return Ok(());
    };
    let remote_hash = fetch_rpc_block_response(rpc, height)?.block_hash;
    if local_hash != remote_hash {
        return Err(format!(
            "mining locked: local chain differs from network bootstrap at height {height}. local={local_hash}, bootstrap={remote_hash}. Run rpc-sync/reorg recovery before mining."
        ));
    }
    Ok(())
}

fn cmd_rpc_list(args: &[String]) -> Result<(), String> {
    let as_json = has_flag(args, "--json");
    let network = NetworkProfile::from_args(args)?;
    let active = active_rpc_endpoints_from_config(args, network)?;
    if as_json {
        println!(
            "{}",
            serde_json::json!({
                "chain_id": network.chain_id(),
                "active_rpc": active,
            })
        );
        return Ok(());
    }
    println!("Configured active RPC endpoints");
    println!("Chain id: {}", network.chain_id());
    if active.is_empty() {
        println!("No active RPC endpoints configured.");
        return Ok(());
    }
    for rpc in active {
        println!(
            "- {} | {} | {} | {} | logging={}",
            rpc.label, rpc.operator, rpc.region, rpc.url, rpc.logging_policy
        );
    }
    Ok(())
}

fn rpc_json_u64(value: &serde_json::Value, keys: &[&str]) -> Option<u64> {
    for key in keys {
        if let Some(v) = value.get(*key).and_then(|v| v.as_u64()) {
            return Some(v);
        }
    }
    None
}

fn cmd_rpc_height(args: &[String]) -> Result<(), String> {
    let as_json = has_flag(args, "--json");
    let network = NetworkProfile::from_args(args)?;
    let chain_id = network.chain_id();
    let rpc = resolve_rpc_url(args, network)?;
    let timeout_secs = parse_u64_arg(option_value(args, "--timeout-secs"), 3, "--timeout-secs")?;
    let body = rpc_http_get_json(&rpc, "/chain/height", timeout_secs)?;
    let height = rpc_json_u64(&body, &["height", "chain_height"])
        .ok_or_else(|| "RPC height response did not include height".to_string())?;
    if as_json {
        println!(
            "{}",
            serde_json::json!({
                "rpc": rpc,
                "chain_id": chain_id,
                "height": height,
                "raw": body,
            })
        );
    } else {
        println!("RPC height");
        println!("Endpoint: {rpc}");
        println!("Chain id: {chain_id}");
        println!("Height:   {height}");
    }
    Ok(())
}

fn cmd_rpc_sync(args: &[String]) -> Result<(), String> {
    let as_json = has_flag(args, "--json");
    let (network, db_path) = chain_db_path_for_args(args)?;
    let chain_id = network.chain_id();
    let active = active_rpc_endpoints_from_config(args, network)?;
    let rpc_endpoints = active.iter().map(|rpc| rpc.url.clone()).collect::<Vec<_>>();
    if rpc_endpoints.is_empty() {
        return Err("no active RPC endpoint configured for sync".to_string());
    }
    let limit = parse_u64_arg(option_value(args, "--limit"), 1024, "--limit")?.clamp(1, 100_000);
    {
        let storage = open_storage(&db_path)?;
        ensure_atomic_economics_db(&storage)?;
    }
    let mut peer_view = zoka::state::sync::PeerChainView::default();
    let start_height = current_known_height_for_network(Some(&db_path), network);
    let mut imported = 0u64;
    let mut events = Vec::new();
    let mut stalled_backfill_rounds = 0u32;

    while imported < limit {
        let before = current_known_height_for_network(Some(&db_path), network);
        let batch_limit = (limit - imported).min(64);
        let Some(event) = run_rpc_sync_batch(
            chain_id,
            Some(&db_path),
            &rpc_endpoints,
            &mut peer_view,
            None,
            batch_limit,
        )?
        else {
            break;
        };
        events.push(event);
        let after = current_known_height_for_network(Some(&db_path), network);
        if after <= before {
            if peer_view.has_active_branch_backfill() && stalled_backfill_rounds < 32 {
                stalled_backfill_rounds = stalled_backfill_rounds.saturating_add(1);
                continue;
            }
            break;
        }
        stalled_backfill_rounds = 0;
        imported = imported.saturating_add(after - before);
    }

    let end_height = current_known_height_for_network(Some(&db_path), network);
    if as_json {
        println!(
            "{}",
            serde_json::json!({
                "chain_id": chain_id,
                "db": db_path,
                "start_height": start_height,
                "end_height": end_height,
                "imported": imported,
                "rpc_endpoints": rpc_endpoints,
                "events": events,
            })
        );
        return Ok(());
    }

    println!("RPC sync");
    println!("Chain id:      {chain_id}");
    println!("DB:            {db_path}");
    println!("Start height:  {start_height}");
    println!("End height:    {end_height}");
    println!("Imported:      {imported}");
    for event in events.iter().rev().take(5).rev() {
        println!("- {event}");
    }
    Ok(())
}

fn cmd_rpc_mempool(args: &[String]) -> Result<(), String> {
    let as_json = has_flag(args, "--json");
    let network = NetworkProfile::from_args(args)?;
    let chain_id = network.chain_id();
    let rpc = resolve_rpc_url(args, network)?;
    let timeout_secs = parse_u64_arg(option_value(args, "--timeout-secs"), 3, "--timeout-secs")?;
    let body = rpc_http_get_json(&rpc, "/mempool/size", timeout_secs)?;
    let size = rpc_json_u64(&body, &["size", "pending", "mempool_size"])
        .ok_or_else(|| "RPC mempool response did not include size".to_string())?;
    if as_json {
        println!(
            "{}",
            serde_json::json!({
                "rpc": rpc,
                "chain_id": chain_id,
                "mempool_size": size,
                "raw": body,
            })
        );
    } else {
        println!("RPC mempool");
        println!("Endpoint: {rpc}");
        println!("Chain id: {chain_id}");
        println!("Size:     {size}");
    }
    Ok(())
}

fn cmd_rpc_status(args: &[String]) -> Result<(), String> {
    let as_json = has_flag(args, "--json");
    let network = NetworkProfile::from_args(args)?;
    let chain_id = network.chain_id();
    let rpc = resolve_rpc_url(args, network)?;
    let timeout_secs = parse_u64_arg(option_value(args, "--timeout-secs"), 3, "--timeout-secs")?;
    let health = rpc_http_get_json(&rpc, "/health", timeout_secs)?;
    let height_body = rpc_http_get_json(&rpc, "/chain/height", timeout_secs)?;
    let height = rpc_json_u64(&height_body, &["height", "chain_height"]);
    if as_json {
        println!(
            "{}",
            serde_json::json!({
                "rpc": rpc,
                "chain_id": chain_id,
                "reachable": true,
                "height": height,
                "health": health,
                "height_raw": height_body,
            })
        );
    } else {
        println!("RPC status");
        println!("Endpoint:  {rpc}");
        println!("Chain id:  {chain_id}");
        println!("Reachable: yes");
        if let Some(height) = height {
            println!("Height:    {height}");
        }
        println!(
            "Health:    {}",
            truncate_dashboard_text(&health.to_string(), 120)
        );
    }
    Ok(())
}

fn resolve_fee(args: &[String]) -> Result<u64, String> {
    if let Some(fee) = option_value(args, "--fee") {
        return parse_zka_amount_arg(Some(fee), 0, "--fee");
    }
    let priority = option_value(args, "--priority").unwrap_or_else(|| "normal".to_string());
    let password = option_value(args, "--password");
    let db_path = option_value(args, "--db");
    let snapshot = refresh_fee_market_snapshot(db_path.as_deref(), password.as_deref());
    let pending = snapshot.pending_private_txs;
    let pending_tier = match pending {
        0..=10 => 0u64,
        11..=25 => 1u64,
        26..=100 => 2u64,
        _ => 3u64,
    };
    let chain_tier = if snapshot.avg_recent_block_txs < 5.0 {
        0
    } else if snapshot.avg_recent_block_txs < 20.0 {
        1
    } else if snapshot.avg_recent_block_txs < 50.0 {
        2
    } else {
        3
    };
    let load_tier = pending_tier.saturating_add(chain_tier).min(6);
    let dynamic = match priority.as_str() {
        "low" => load_tier.saturating_sub(1),
        "normal" => load_tier,
        "high" => {
            if load_tier == 0 {
                0
            } else {
                load_tier.saturating_add(1)
            }
        }
        other => {
            return Err(format!(
                "invalid --priority '{}'; expected low|normal|high",
                other
            ));
        }
    };
    Ok(dynamic.saturating_mul(FEE_MARKET_STEP_ATOMS))
}

fn cmd_estimate_fee(args: &[String]) -> Result<(), String> {
    let amount = parse_zka_amount_arg(
        Some(required_option_value(args, "--amount")?),
        0,
        "--amount",
    )?;
    let fee = resolve_fee(args)?;
    println!("Fee estimate");
    println!("Amount: {}", format_zka(amount));
    println!(
        "Priority: {}",
        option_value(args, "--priority").unwrap_or_else(|| "normal".to_string())
    );
    println!("Estimated fee: {}", format_zka(fee));
    Ok(())
}

/// URL del nodo local para los comandos de equipo.
///
/// Deliberadamente el nodo de esta maquina y no un RPC remoto: crear un equipo
/// o unirse a uno compromete la direccion de cobro de quien ejecuta el comando.
/// Apuntar sin querer a un nodo ajeno haria que el equipo se armara con la
/// direccion del otro, y el error no se notaria hasta que las recompensas
/// llegaran a la persona equivocada. `--rpc` sigue disponible para quien corra
/// el nodo en otro puerto o en otra maquina suya.
fn team_rpc_url(args: &[String]) -> String {
    option_value(args, "--rpc").unwrap_or_else(|| "http://127.0.0.1:3000".to_string())
}

fn team_timeout(args: &[String]) -> Result<u64, String> {
    parse_u64_arg(option_value(args, "--timeout-secs"), 5, "--timeout-secs")
}

/// `zoka team-status [--json]` -- estado del equipo de mineria.
fn cmd_team_status(args: &[String]) -> Result<(), String> {
    let rpc = team_rpc_url(args);
    let estado = rpc_http_get_json(&rpc, "/team-mining/status", team_timeout(args)?)?;
    if has_flag(args, "--json") {
        println!("{estado}");
        return Ok(());
    }
    println!("{}", dashboard_title("Team mining", ANSI_CYAN));
    let cadena = |clave: &str| {
        estado
            .get(clave)
            .and_then(|v| v.as_str())
            .unwrap_or("-")
            .to_string()
    };
    println!("{}", dashboard_line("Join status", cadena("join_status")));
    println!("{}", dashboard_line("Team id", cadena("team_id")));
    println!("{}", dashboard_line("Invite code", cadena("invite_code")));
    println!("{}", dashboard_line("Coordinator", cadena("coordinator")));
    let miembros = estado
        .get("members")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    println!("{}", dashboard_line("Members", miembros));
    println!(
        "{}",
        dashboard_line(
            "Hashing",
            yes_no(
                estado
                    .get("hashing")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            )
        )
    );
    Ok(())
}

/// `zoka team-create [--json]` -- crea un equipo y devuelve el codigo a repartir.
fn cmd_team_create(args: &[String]) -> Result<(), String> {
    let rpc = team_rpc_url(args);
    let respuesta = rpc_http_post_json(
        &rpc,
        "/team-mining/team",
        &serde_json::json!({}),
        team_timeout(args)?,
    )?;
    if has_flag(args, "--json") {
        println!("{respuesta}");
        return Ok(());
    }
    let codigo = respuesta
        .get("invite_code")
        .and_then(|v| v.as_str())
        .unwrap_or("-");
    println!("Team created. Share this code with whoever should join:");
    println!("  {codigo}");
    Ok(())
}

/// `zoka team-join --code <codigo> [--json]` -- entra a un equipo existente.
fn cmd_team_join(args: &[String]) -> Result<(), String> {
    let codigo = option_value(args, "--code")
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .ok_or_else(|| "pass --code <invite-code>".to_string())?;
    let rpc = team_rpc_url(args);
    let respuesta = rpc_http_post_json(
        &rpc,
        "/team-mining/join",
        &serde_json::json!({ "invite_code": codigo }),
        team_timeout(args)?,
    )?;
    if has_flag(args, "--json") {
        println!("{respuesta}");
        return Ok(());
    }
    println!("Join requested. Run `zoka team-status` to see when it is accepted.");
    Ok(())
}

/// `zoka team-leave` -- sale del equipo. El coordinador no puede irse: o
/// traspasa la coordinacion o disuelve el equipo.
fn cmd_team_leave(args: &[String]) -> Result<(), String> {
    let rpc = team_rpc_url(args);
    let respuesta = rpc_http_post_json(
        &rpc,
        "/team-mining/leave",
        &serde_json::json!({}),
        team_timeout(args)?,
    )?;
    if has_flag(args, "--json") {
        println!("{respuesta}");
    } else {
        println!("Left the team.");
    }
    Ok(())
}

/// `zoka team-start [--threads <n>]` / `zoka team-stop` -- prende y apaga el
/// aporte de computo al equipo.
fn cmd_team_start(args: &[String]) -> Result<(), String> {
    let hilos = parse_u64_arg(option_value(args, "--threads"), 1, "--threads")?;
    let rpc = team_rpc_url(args);
    let respuesta = rpc_http_post_json(
        &rpc,
        "/team-mining/start",
        &serde_json::json!({ "threads": hilos.max(1) }),
        team_timeout(args)?,
    )?;
    if has_flag(args, "--json") {
        println!("{respuesta}");
    } else {
        println!("Team mining started with {} thread(s).", hilos.max(1));
    }
    Ok(())
}

fn cmd_team_stop(args: &[String]) -> Result<(), String> {
    let rpc = team_rpc_url(args);
    let respuesta = rpc_http_post_json(
        &rpc,
        "/team-mining/stop",
        &serde_json::json!({}),
        team_timeout(args)?,
    )?;
    if has_flag(args, "--json") {
        println!("{respuesta}");
    } else {
        println!("Team mining stopped.");
    }
    Ok(())
}

/// Bloques hasta que se puede gastar una recompensa de minería en esa red.
fn coinbase_maturity_for_network(network: NetworkProfile) -> u64 {
    #[cfg(feature = "quantic")]
    if let Some(quantic) = zoka::quantic::network_for_chain(network.chain_id()) {
        return u64::from(zoka::quantic::consensus_params(quantic).coinbase_maturity);
    }
    let _ = network;
    COINBASE_MATURITY_BLOCKS
}

fn cmd_mine_status(args: &[String]) -> Result<(), String> {
    let network = NetworkProfile::from_args(args)?;
    let state = load_user_mining_state_for_display_for_network(network)?;
    let as_json = has_flag(args, "--json");
    let locked_rewards_atoms = state
        .pending_rewards
        .iter()
        .map(|reward| reward.amount)
        .sum::<u64>();
    let next_unlock_height = state
        .pending_rewards
        .iter()
        .map(|reward| reward.unlock_height)
        .min();
    let next_unlock_in = next_unlock_height.map(|height| height.saturating_sub(state.chain_height));
    if as_json {
        println!(
            "{}",
            serde_json::json!({
                "running": state.is_mining,
                "blocks_mined": state.total_blocks,
                "total_rewards_atoms": state.total_rewards,
                "total_rewards": format_zka_amount(state.total_rewards),
                "chain_height": state.chain_height,
                "difficulty_bits": state.current_difficulty_bits,
                "hashrate_hps": state.estimated_hashrate_hps,
                "avg_solve_time_secs": state.avg_solve_time_secs,
                "last_solve_time_secs": state.last_solve_time_secs,
                "virtual_time_elapsed_secs": state.virtual_time_elapsed_secs,
                "total_supply_emitted_atoms": state.total_supply_emitted,
                "total_supply_emitted": format_zka_amount(state.total_supply_emitted),
                "mode": state.mode,
                "algorithm": state.algorithm,
                "available_cpu_threads": default_mining_threads(),
                "locked_reward_count": state.pending_rewards.len(),
                "locked_rewards_atoms": locked_rewards_atoms,
                "locked_rewards": format_zka_amount(locked_rewards_atoms),
                "next_reward_unlock_height": next_unlock_height,
                "next_reward_unlock_in_blocks": next_unlock_in,
                "coinbase_maturity_blocks": coinbase_maturity_for_network(network),
                "legacy_pow_sim_detected": legacy_pow_sim_state_detected(&state)
            })
        );
        return Ok(());
    }
    println!("Mining status");
    println!("Running: {}", state.is_mining);
    println!("Mode: {}", state.mode);
    println!("Algorithm: {}", state.algorithm);
    println!("Available CPU threads: {}", default_mining_threads());
    println!(
        "Legacy sim detected: {}",
        yes_no(legacy_pow_sim_state_detected(&state))
    );
    println!("Blocks mined: {}", state.total_blocks);
    println!("Total rewards: {}", format_zka(state.total_rewards));
    println!("Locked rewards: {}", format_zka(locked_rewards_atoms));
    println!("Locked reward outputs: {}", state.pending_rewards.len());
    if let Some(height) = next_unlock_height {
        println!(
            "Next unlock: height {} ({} block(s) from current mining height)",
            height,
            next_unlock_in.unwrap_or(0)
        );
    }
    println!("Chain height: {}", state.chain_height);
    println!("Difficulty bits: {}", state.current_difficulty_bits);
    println!(
        "Hashrate: {}",
        if state.is_mining {
            format_hashrate_hps(state.estimated_hashrate_hps as f64)
        } else {
            "0 H/s".to_string()
        }
    );
    println!("Avg solve time (s): {:.2}", state.avg_solve_time_secs);
    println!("Last solve time (s): {:.2}", state.last_solve_time_secs);
    println!(
        "Total supply emitted: {}",
        format_zka(state.total_supply_emitted)
    );
    Ok(())
}

fn cmd_mine_console(args: &[String]) -> Result<(), String> {
    let network = NetworkProfile::from_args(args)?;
    let wallet_storage = wallet_storage_for_network(network)?;
    let state = load_user_mining_state_for_display_for_network(network)?;
    let as_json = has_flag(args, "--json");
    let password = option_value(args, "--password");
    let db_path =
        option_value(args, "--db").or_else(|| Some(network.default_db_path().to_string()));
    let wallet_selector = match wallet_selector_value(args) {
        Some(selector) => Some(selector),
        None => {
            first_local_wallet_selector_from_storage(&wallet_storage, password.as_deref(), network)?
        }
    };
    let wallet_snapshot = match wallet_selector.as_deref() {
        Some(selector) => Some(load_wallet_runtime_snapshot_from_storage(
            &wallet_storage,
            selector,
            password.as_deref(),
            network,
        )?),
        None => None,
    };
    let fee_snapshot = refresh_fee_market_snapshot(db_path.as_deref(), password.as_deref());
    let known_height = current_known_height_for_network(db_path.as_deref(), network);

    if as_json {
        println!(
            "{}",
            serde_json::json!({
                "running": state.is_mining,
                "mode": state.mode,
                "algorithm": state.algorithm,
                "height": state.chain_height,
                "difficulty_bits": state.current_difficulty_bits,
                "hashrate_hps": state.estimated_hashrate_hps,
                "available_cpu_threads": default_mining_threads(),
                "avg_solve_time_secs": state.avg_solve_time_secs,
                "last_solve_time_secs": state.last_solve_time_secs,
                "virtual_time_elapsed_secs": state.virtual_time_elapsed_secs,
                "session_blocks": state.total_blocks,
                "session_rewards_atoms": state.total_rewards,
                "session_rewards": format_zka_amount(state.total_rewards),
                "next_reward_atoms": next_block_reward(state.chain_height, state.total_supply_emitted, None),
                "next_reward": format_zka_amount(next_block_reward(state.chain_height, state.total_supply_emitted, None)),
                "supply_emitted_atoms": state.total_supply_emitted,
                "supply_emitted": format_zka_amount(state.total_supply_emitted),
                "supply_remaining_atoms": EMISSION_MAX_SUPPLY.saturating_sub(state.total_supply_emitted),
                "supply_remaining": format_zka_amount(EMISSION_MAX_SUPPLY.saturating_sub(state.total_supply_emitted)),
                "halving_in_blocks": EMISSION_HALVING_INTERVAL - (state.chain_height % EMISSION_HALVING_INTERVAL),
                "locked_rewards": state.pending_rewards.iter().map(|reward| reward.amount).sum::<u64>(),
                "coinbase_maturity_blocks": coinbase_maturity_for_network(network),
                "legacy_pow_sim_detected": legacy_pow_sim_state_detected(&state),
                "known_height": known_height,
                "wallet": wallet_snapshot.as_ref().map(|wallet| {
                    serde_json::json!({
                        "name": wallet.wallet_name,
                        "address": wallet.address,
                        "balance_atoms": wallet.balance,
                        "balance": format_zka_amount(wallet.balance),
                        "locked_balance_atoms": wallet.locked_balance,
                        "locked_balance": format_zka_amount(wallet.locked_balance),
                        "total_balance_atoms": wallet.total_balance,
                        "total_balance": format_zka_amount(wallet.total_balance),
                        "tx_count": wallet.tx_count,
                        "view_only": wallet.view_only,
                        "restore_height": wallet.restore_height,
                    })
                }),
                "fee_market": {
                    "pending_private_txs": fee_snapshot.pending_private_txs,
                    "avg_recent_block_txs": fee_snapshot.avg_recent_block_txs,
                }
            })
        );
        return Ok(());
    }

    let console = render_mining_dashboard(
        wallet_snapshot.as_ref(),
        &state,
        &fee_snapshot,
        known_height,
        Duration::from_secs_f64(state.virtual_time_elapsed_secs.max(0.0)),
        state.total_blocks,
        state.total_rewards,
        None,
        None,
        None,
        "Snapshot only.",
    );
    set_terminal_title("ZOKA Mining 1.0")?;
    println!("{}", console);
    Ok(())
}

fn cmd_status_overview(args: &[String]) -> Result<(), String> {
    let network = NetworkProfile::from_args(args)?;
    let wallet_storage = wallet_storage_for_network(network)?;
    let db_path =
        option_value(args, "--db").unwrap_or_else(|| network.default_db_path().to_string());
    let password =
        option_value(args, "--password").or_else(|| option_value(args, "--wallet-password"));
    let as_json = has_flag(args, "--json");
    let db_exists = Path::new(&db_path).exists();
    let chain_height = if db_exists {
        current_known_height_for_network(Some(&db_path), network)
    } else {
        0
    };
    let fee_snapshot = if db_exists {
        refresh_fee_market_snapshot(Some(&db_path), password.as_deref())
    } else {
        FeeMarketSnapshot {
            pending_private_txs: 0,
            avg_recent_block_txs: 0.0,
            updated_at: now_secs(),
        }
    };
    let pending_hashes = if db_exists {
        local_pending_private_tx_hashes(&db_path, 5).unwrap_or_default()
    } else {
        Vec::new()
    };
    let mining_state = load_user_mining_state_for_display_for_network(network)?;
    let locked_rewards_atoms = mining_state
        .pending_rewards
        .iter()
        .map(|reward| reward.amount)
        .sum::<u64>();
    let next_unlock_height = mining_state
        .pending_rewards
        .iter()
        .map(|reward| reward.unlock_height)
        .min();
    let explicit_wallet_selector = wallet_selector_value(args);
    let wallet_selector = match explicit_wallet_selector.clone() {
        Some(selector) => Some(selector),
        None => {
            first_local_wallet_selector_from_storage(&wallet_storage, password.as_deref(), network)
                .ok()
                .flatten()
        }
    };
    let wallet_snapshot = match wallet_selector.as_deref() {
        Some(selector) => match load_wallet_runtime_snapshot_from_storage(
            &wallet_storage,
            selector,
            password.as_deref(),
            network,
        ) {
            Ok(snapshot) => Some(snapshot),
            Err(err) if explicit_wallet_selector.is_some() => return Err(err),
            Err(_) => None,
        },
        None => None,
    };
    let wallet_count =
        count_wallet_files_in_dir(wallet_storage.private_wallets_dir(), ".private.json");
    let network_config_path = option_value(args, "--network-config")
        .map(PathBuf::from)
        .unwrap_or_else(default_mainnet_network_config_path);
    let network_config = if network_config_path.exists() {
        load_mainnet_network_config(&network_config_path).ok()
    } else {
        None
    };
    let network_status = network_config
        .as_ref()
        .map(|config| config.status.clone())
        .unwrap_or_else(|| "missing".to_string());
    let active_seed_count = network_config
        .as_ref()
        .map(|config| {
            config
                .seed_nodes
                .iter()
                .filter(|seed| seed.status.eq_ignore_ascii_case("active"))
                .count()
        })
        .unwrap_or(0);
    let active_rpc_count = network_config
        .as_ref()
        .map(|config| config.active_rpc_count())
        .unwrap_or(0);

    if as_json {
        println!(
            "{}",
            serde_json::json!({
                "schema": "zoka-status-dashboard-v1",
                "db": db_path,
                "db_exists": db_exists,
                "chain_height": chain_height,
                "wallet_count": wallet_count,
                "wallet": wallet_snapshot.as_ref().map(|wallet| serde_json::json!({
                    "name": wallet.wallet_name,
                    "address": wallet.address,
                    "balance_atoms": wallet.balance,
                    "balance": format_zka_amount(wallet.balance),
                    "locked_balance_atoms": wallet.locked_balance,
                    "locked_balance": format_zka_amount(wallet.locked_balance),
                    "total_balance_atoms": wallet.total_balance,
                    "total_balance": format_zka_amount(wallet.total_balance),
                    "tx_count": wallet.tx_count,
                    "view_only": wallet.view_only,
                })),
                "transfers": {
                    "pending_private_txs": fee_snapshot.pending_private_txs,
                    "pending_private_tx_hashes": pending_hashes,
                    "avg_recent_block_txs": fee_snapshot.avg_recent_block_txs,
                },
                "mining": {
                    "running": mining_state.is_mining,
                    "process_id": mining_state.mining_process_id,
                    "algorithm": mining_state.algorithm,
                    "mode": mining_state.mode,
                    "hashrate_hps": mining_state.estimated_hashrate_hps,
                    "difficulty_bits": mining_state.current_difficulty_bits,
                    "blocks_mined": mining_state.total_blocks,
                    "total_rewards_atoms": mining_state.total_rewards,
                    "total_rewards": format_zka_amount(mining_state.total_rewards),
                    "locked_rewards_atoms": locked_rewards_atoms,
                    "locked_rewards": format_zka_amount(locked_rewards_atoms),
                    "next_reward_unlock_height": next_unlock_height,
                    "available_cpu_threads": default_mining_threads(),
                },
                "node": {
                    "local_db_ready": db_exists,
                    "local_height": chain_height,
                    "network_config": network_config_path.display().to_string(),
                    "network_status": network_status,
                    "active_seed_count": active_seed_count,
                    "active_rpc_count": active_rpc_count,
                },
                "hashrate_hps": mining_state.estimated_hashrate_hps,
            })
        );
        return Ok(());
    }

    println!("{}", dashboard_title("ZOKA Status", ANSI_YELLOW));
    println!("{}", dashboard_divider());
    println!(
        "{}  {}  {}  {}",
        dashboard_line("Wallets", wallet_count),
        dashboard_line("Local height", chain_height),
        dashboard_line("Mining", yes_no(mining_state.is_mining)),
        dashboard_line("Pending txs", fee_snapshot.pending_private_txs)
    );
    println!("{}", dashboard_divider());
    println!("{}", dashboard_section("Wallet", ANSI_MAGENTA));
    if let Some(wallet) = wallet_snapshot {
        println!("{}", dashboard_line("Local label", wallet.wallet_name));
        println!("{}", dashboard_line("Address", wallet.address));
        println!(
            "{}",
            dashboard_line("Available", format_zka(wallet.balance))
        );
        println!(
            "{}",
            dashboard_line("Locked", format_zka(wallet.locked_balance))
        );
        println!(
            "{}",
            dashboard_line("Total", format_zka(wallet.total_balance))
        );
        println!("{}", dashboard_line("Transactions", wallet.tx_count));
    } else {
        println!(
            "{}",
            dashboard_line(
                "Wallet",
                "No wallet snapshot available; pass --password if encrypted"
            )
        );
    }
    println!("{}", dashboard_section("Transfers", ANSI_BLUE));
    println!(
        "{}",
        dashboard_line("Pending private txs", fee_snapshot.pending_private_txs)
    );
    println!(
        "{}",
        dashboard_line(
            "Recent avg block txs",
            format!("{:.2}", fee_snapshot.avg_recent_block_txs)
        )
    );
    if !pending_hashes.is_empty() {
        println!("{}", dashboard_label("Recent pending hashes"));
        for hash in pending_hashes {
            println!("  - {}", hash);
        }
    }
    println!("{}", dashboard_section("Mining", ANSI_GREEN));
    println!(
        "{}",
        dashboard_line("Running", yes_no(mining_state.is_mining))
    );
    println!("{}", dashboard_line("Algorithm", mining_state.algorithm));
    println!(
        "{}",
        dashboard_line(
            "Hashrate",
            format_hashrate_hps(mining_state.estimated_hashrate_hps as f64)
        )
    );
    println!(
        "{}",
        dashboard_line(
            "Difficulty",
            format!("{} bits", mining_state.current_difficulty_bits)
        )
    );
    println!(
        "{}",
        dashboard_line("Locked rewards", format_zka(locked_rewards_atoms))
    );
    if let Some(height) = next_unlock_height {
        println!("{}", dashboard_line("Next unlock height", height));
    }
    println!("{}", dashboard_section("Node", ANSI_CYAN));
    println!("{}", dashboard_line("DB", db_path));
    println!("{}", dashboard_line("DB ready", yes_no(db_exists)));
    println!(
        "{}",
        dashboard_line("Network config", network_config_path.display())
    );
    println!("{}", dashboard_line("Network status", network_status));
    println!("{}", dashboard_line("Active seeds", active_seed_count));
    println!("{}", dashboard_line("Active RPCs", active_rpc_count));
    Ok(())
}

fn cmd_node_status(args: &[String]) -> Result<(), String> {
    let network = NetworkProfile::from_args(args)?;
    let wallet_storage = wallet_storage_for_network(network)?;
    let db_path =
        option_value(args, "--db").unwrap_or_else(|| network.default_db_path().to_string());
    let password =
        option_value(args, "--password").or_else(|| option_value(args, "--wallet-password"));
    let as_json = has_flag(args, "--json");
    let db_exists = Path::new(&db_path).exists();
    let chain_height = if db_exists {
        current_known_height_for_network(Some(&db_path), network)
    } else {
        0
    };
    let pending_private_txs = if db_exists {
        network_pending_private_txs(Some(&db_path))
    } else {
        0
    };
    let mining_state = load_user_mining_state_for_display_for_network(network)?;
    let wallet_snapshot = match wallet_selector_value(args) {
        Some(selector) => Some(load_wallet_runtime_snapshot_from_storage(
            &wallet_storage,
            &selector,
            password.as_deref(),
            network,
        )?),
        None => match first_local_wallet_selector_from_storage(
            &wallet_storage,
            password.as_deref(),
            network,
        ) {
            Ok(Some(selector)) => load_wallet_runtime_snapshot_from_storage(
                &wallet_storage,
                &selector,
                password.as_deref(),
                network,
            )
            .ok(),
            Ok(None) | Err(_) => None,
        },
    };

    if as_json {
        println!(
            "{}",
            serde_json::json!({
                "db": db_path,
                "db_exists": db_exists,
                "chain_height": chain_height,
                "pending_private_txs": pending_private_txs,
                "mining_running": mining_state.is_mining,
                "mining_process_id": mining_state.mining_process_id,
                "algorithm": mining_state.algorithm,
                "difficulty_bits": mining_state.current_difficulty_bits,
                "available_cpu_threads": default_mining_threads(),
                "locked_rewards_atoms": mining_state.pending_rewards.iter().map(|reward| reward.amount).sum::<u64>(),
                "locked_rewards": format_zka_amount(mining_state.pending_rewards.iter().map(|reward| reward.amount).sum::<u64>()),
                "wallet": wallet_snapshot.as_ref().map(|wallet| serde_json::json!({
                    "name": wallet.wallet_name,
                    "address": wallet.address,
                    "balance_atoms": wallet.balance,
                    "balance": format_zka_amount(wallet.balance),
                    "locked_balance_atoms": wallet.locked_balance,
                    "locked_balance": format_zka_amount(wallet.locked_balance),
                    "total_balance_atoms": wallet.total_balance,
                    "total_balance": format_zka_amount(wallet.total_balance),
                    "view_only": wallet.view_only,
                })),
            })
        );
        return Ok(());
    }

    println!("{}", dashboard_title("ZOKA Node Status", ANSI_CYAN));
    println!("{}", dashboard_divider());
    println!("{}", dashboard_section("Local Node", ANSI_BLUE));
    println!("{}", dashboard_line("DB", &db_path));
    println!("{}", dashboard_line("DB ready", yes_no(db_exists)));
    println!("{}", dashboard_line("Chain height", chain_height));
    println!(
        "{}",
        dashboard_line("Pending private txs", pending_private_txs)
    );
    println!("{}", dashboard_section("Mining", ANSI_GREEN));
    println!(
        "{}",
        dashboard_line("Auto/mining active", yes_no(mining_state.is_mining))
    );
    println!("{}", dashboard_line("Algorithm", &mining_state.algorithm));
    println!(
        "{}",
        dashboard_line(
            "Difficulty",
            format!("{} bits", mining_state.current_difficulty_bits)
        )
    );
    println!(
        "{}",
        dashboard_line("Available CPU threads", default_mining_threads())
    );
    println!(
        "{}",
        dashboard_line(
            "Locked rewards",
            format_zka(
                mining_state
                    .pending_rewards
                    .iter()
                    .map(|reward| reward.amount)
                    .sum::<u64>()
            )
        )
    );
    println!("{}", dashboard_section("Wallet", ANSI_MAGENTA));
    if let Some(wallet) = wallet_snapshot {
        println!("{}", dashboard_line("Local label", wallet.wallet_name));
        println!("{}", dashboard_line("Address", wallet.address));
        println!(
            "{}",
            dashboard_line("Available", format_zka(wallet.balance))
        );
        println!(
            "{}",
            dashboard_line("Locked", format_zka(wallet.locked_balance))
        );
        println!(
            "{}",
            dashboard_line("Total", format_zka(wallet.total_balance))
        );
    } else {
        println!(
            "{}",
            dashboard_line(
                "Wallet",
                "No wallet snapshot available; pass --password if encrypted"
            )
        );
    }
    Ok(())
}

fn running_as_zokahd() -> bool {
    let Some(exe_path) = std::env::args().next() else {
        return false;
    };
    let Some(stem) = Path::new(&exe_path).file_stem().and_then(|s| s.to_str()) else {
        return false;
    };
    stem.eq_ignore_ascii_case("zokahd")
}

/// Los comandos de Zenith: en una build de Quantic no se ejecutan (crearían
/// wallets o transacciones de Zenith dentro de una red de Quantic).
#[cfg(feature = "quantic")]
const ZENITH_ONLY_COMMANDS: &[&str] = &[
    "create", "restore", "generate-seed", "balance", "receive", "history", "send", "fee", "pending",
    "zokahd-guided", "sync", "backup", "restore-backup", "wipe", "status", "dashboard",
    "wallet-create", "wallet-generate-seed", "wallet-restore-from-seed", "wallet-restore-from-backup",
    "wallet-restore-from-keys", "wallet-import", "wallet-change-passphrase", "wallet-backup", "wallet-wipe",
    "wallet-list", "wallet-export-secrets", "wallet-balance", "wallet-history", "wallet-receive",
    "wallet-recovery", "sync-status", "chain-economics-migrate", "private-tx-import", "private-tx-publish",
    "private-tx-audit", "private-pending-diagnostics", "tx-pending", "estimate-fee", "tx-send", "tx-send-many",
    "team-status", "team-create", "team-join", "team-leave", "team-start", "team-stop", "mine", "mine-console",
    "genesis-verify", "mainnet-network-lint", "chain-republish-private-txs", "trusted-setup-init",
];

#[cfg(feature = "quantic")]
const QUANTIC_INSTEAD: &str = "Use `zoka quantic-wallet` for wallets, `zoka fullnode --network quantic-testnet` for the node (add --quantic-mine-to <address> to mine), or ZSilent 3.0.";

#[cfg(feature = "quantic")]
fn print_quantic_start() {
    println!("zoka {} — Zoka Quantic", env!("CARGO_PKG_VERSION"));
    println!("Networks: quantic-testnet (default) and quantic-mainnet. Zenith was retired on 2026-10-21.");
    println!("  zoka quantic-wallet new --network quantic-testnet --name <wallet> --password-stdin");
    println!("  zoka fullnode --network quantic-testnet [--quantic-mine-to <address>]");
    println!("  zoka help   (every command)");
}

async fn run() -> Result<(), String> {
    enable_ansi_console_support();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let is_hd = running_as_zokahd();
    if is_hd {
        if matches!(
            args.first().map(String::as_str),
            Some("--version" | "-V" | "version")
        ) {
            println!("zokahd {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        // El piloto automático de zokahd crea una wallet de Zenith y la mina:
        // en una build de Quantic no hay Zenith a la que llevarlo.
        #[cfg(feature = "quantic")]
        if args.is_empty() {
            print_quantic_start();
            return Ok(());
        }
        #[cfg(not(feature = "quantic"))]
        if args.is_empty() {
            return run_zokahd_guided_default().await;
        }
        let first = &args[0];
        if first == "--help" || first == "-h" || first == "help" {
            println!("zokahd (automatic fullnode)");
            println!(
                "Usage: zokahd [--network testnet] [--address <wallet-address>] [--password <pwd>|--password-env <var>|--password-stdin] [--seed <multiaddr> ...] [--network-config <file>] [--no-network-config] [--allow-clearnet-seeds] [--allow-isolated-start] [--seconds <n>] [--show-keys] [--no-mine] [--power low|normal|max] [--threads <n>] [--db <path>] [--genesis <file>] [--tor-proxy host:port] [--tor-only] [--external-addr <multiaddr> ...] [--inline]"
            );
            println!(
                "Runs fullnode + sync warmup + auto-mining by default until interrupted. --password is required when auto-creating a wallet."
            );
            println!(
                "Without arguments, starts the autopilot launcher: wallet, sync, then mining."
            );
            println!(
                "On Windows it opens in its own console window by default. Use --inline to keep it in the current terminal."
            );
            return Ok(());
        }
        if first.starts_with('-') {
            return cmd_fullnode_auto_boxed(&args).await;
        }
        if first == "fullnode" {
            return cmd_fullnode_auto_boxed(&args[1..]).await;
        }
    }

    #[cfg(feature = "quantic")]
    if args.is_empty() {
        print_quantic_start();
        return Ok(());
    }
    #[cfg(not(feature = "quantic"))]
    if args.is_empty() {
        return run_interactive_menu().await;
    }

    let command = &args[0];
    let tail = &args[1..];
    #[cfg(feature = "quantic")]
    if ZENITH_ONLY_COMMANDS.contains(&command.as_str()) {
        return Err(format!("`zoka {command}` belonged to Zenith. {ZENITH_RETIRED} {QUANTIC_INSTEAD}"));
    }
    match command.as_str() {
        "version" | "--version" | "-V" => {
            println!("zoka {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "help" | "--help" | "-h" => {
            #[cfg(feature = "quantic")]
            print_quantic_usage();
            #[cfg(not(feature = "quantic"))]
            print_usage();
            Ok(())
        }
        "create" => {
            let args = normalize_positional_alias_args(tail, &["--name"]);
            cmd_wallet_create_unified(&args)
        }
        "restore" => {
            let args = normalize_positional_alias_args(tail, &["--seed"]);
            cmd_wallet_restore_from_seed(&args)
        }
        "generate-seed" => cmd_wallet_generate_seed(tail),
        "wallets" | "list" => cmd_wallet_list_unified(tail),
        "balance" => {
            let args = normalize_positional_alias_args(tail, &["--address"]);
            cmd_wallet_balance_unified(&args)
        }
        "receive" => {
            let args = normalize_positional_alias_args(tail, &["--address"]);
            cmd_receive_address(&args)
        }
        "history" => {
            let args = normalize_positional_alias_args(tail, &["--address"]);
            cmd_wallet_history_unified(&args)
        }
        "send" => {
            let args = normalize_positional_alias_args(tail, &["--from", "--to", "--amount"]);
            cmd_tx_send_unified(&args)
        }
        "consolidate" | "private-consolidate" => {
            let args = normalize_positional_alias_args(tail, &["--from"]);
            cmd_private_consolidate(&args)
        }
        "fee" => {
            let args = normalize_positional_alias_args(tail, &["--amount"]);
            cmd_estimate_fee(&args)
        }
        "pending" => cmd_pending_transfers(tail),
        "node" => cmd_fullnode_auto_boxed(tail).await,
        "node-status" => cmd_node_status(tail),
        "status" => cmd_status_overview(tail),
        "zokahd-guided" => run_zokahd_guided_default().await,
        "sync" => {
            let args = normalize_positional_alias_args(tail, &["--address"]);
            cmd_sync_status_unified(&args)
        }
        "backup" => {
            let args = normalize_positional_alias_args(tail, &["--address", "--out"]);
            cmd_wallet_backup_unified(&args)
        }
        "restore-backup" => {
            let args = normalize_positional_alias_args(tail, &["--in"]);
            cmd_wallet_restore_from_backup_unified(&args)
        }
        "wipe" => {
            let args = normalize_positional_alias_args(tail, &["--address"]);
            cmd_wallet_wipe_unified(&args)
        }
        "wallet-create" => cmd_wallet_create_unified(tail),
        "wallet-generate-seed" => cmd_wallet_generate_seed(tail),
        "wallet-restore-from-seed" => cmd_wallet_restore_from_seed(tail),
        "wallet-restore-from-backup" => cmd_wallet_restore_from_backup_unified(tail),
        "wallet-restore-from-keys" => cmd_wallet_restore_from_keys_unified(tail),
        "wallet-import" => cmd_wallet_import_disabled(tail),
        "wallet-change-passphrase" => cmd_wallet_change_passphrase_unified(tail),
        "wallet-backup" => cmd_wallet_backup_unified(tail),
        "wallet-wipe" => cmd_wallet_wipe_unified(tail),
        "wallet-list" => cmd_wallet_list_unified(tail),
        "wallet-export-secrets" => cmd_wallet_export_secrets(tail),
        "wallet-balance" => cmd_wallet_balance_unified(tail),
        "height" => cmd_height(tail),
        "block-info" => cmd_block_info(tail),
        "chain-stats" => cmd_chain_stats(tail),
        "chain-economics-migrate" => cmd_chain_economics_migrate(tail),
        "sync-status" => cmd_sync_status_unified(tail),
        "wallet-history" => cmd_wallet_history_unified(tail),
        "wallet-receive" => cmd_receive_address(tail),
        "wallet-recovery" => cmd_wallet_export_secrets(tail),
        "private-tx-import" => cmd_private_tx_import(tail),
        "private-tx-publish" => cmd_private_tx_publish(tail),
        "private-tx-audit" => cmd_private_tx_audit(tail),
        "checkpoint-keygen" => cmd_checkpoint_keygen(tail),
        "checkpoint-sign" => cmd_checkpoint_sign(tail),
        #[cfg(feature = "quantic")]
        "quantic-wallet" => cmd_quantic_wallet(tail),
        #[cfg(feature = "quantic")]
        "quantic-genesis" => cmd_quantic_genesis(tail),
        "checkpoint-status" => cmd_checkpoint_status(tail),
        "private-pending-diagnostics" => cmd_private_pending_diagnostics(tail),
        "tx-pending" => cmd_pending_transfers(tail),
        "estimate-fee" => cmd_estimate_fee(tail),
        "fullnode" => cmd_fullnode_auto_boxed(tail).await,
        "fullnode-status" => cmd_node_status(tail),
        "tx-send" => cmd_tx_send_unified(tail),
        "tx-send-many" => {
            Err("tx-send-many is disabled in unified private wallet mode".to_string())
        }
        "team-status" => cmd_team_status(tail),
        "team-create" => cmd_team_create(tail),
        "team-join" => cmd_team_join(tail),
        "team-leave" => cmd_team_leave(tail),
        "team-start" => cmd_team_start(tail),
        "team-stop" => cmd_team_stop(tail),
        "mine-status" => cmd_mine_status(tail),
        "mine-console" => cmd_mine_console(tail),
        "dashboard" => cmd_status_overview(tail),
        "mine" => {
            // Alias for a connected, mining-enabled fullnode. Used to call
            // cmd_mine_easy, which mined into the local chain DB with no P2P
            // networking at all -- nothing it "mined" was ever broadcast to
            // real peers or credited on the real chain, so it looked like it
            // worked and then silently went nowhere on mainnet.
            let args = normalize_positional_alias_args(tail, &["--address"]);
            cmd_fullnode_auto_boxed(&args).await
        }
        "genesis-verify" => cmd_genesis_verify(tail),
        "mainnet-network-lint" => cmd_mainnet_network_lint(tail),
        "rpc-list" => cmd_rpc_list(tail),
        "rpc-status" => cmd_rpc_status(tail),
        "rpc-height" => cmd_rpc_height(tail),
        "rpc-sync" => cmd_rpc_sync(tail),
        "rpc-mempool" => cmd_rpc_mempool(tail),
        "release-keygen" => cmd_release_keygen(tail),
        "snapshot-info" => cmd_snapshot_info(tail),
        "snapshot-export" => cmd_snapshot_export(tail),
        "snapshot-import" => cmd_snapshot_import(tail),
        "snapshot-fetch-iroh" => cmd_snapshot_fetch_iroh(tail),
        "iroh-id" => cmd_iroh_id(tail),
        "chain-prune" => cmd_chain_prune(tail),
        "chain-republish-private-txs" => cmd_chain_republish_private_txs(tail),
        "trusted-setup-init" => cmd_trusted_setup_init(tail),
        other => Err(format!(
            "unknown command: {other}. Run `zoka` for the guided console or `zoka help` for commands."
        )),
    }
}

/// `zoka chain-prune --keep-last <N> [--db <path>] [--dry-run] [--json]`
///
/// Removes block bodies older than the retention window (`head - N`)
/// from RocksDB. Keeps the chain head metadata, the last N blocks, the
/// data needed by `mining::difficulty::expected_for_next_block`, and
/// silently floors `--keep-last` at `chain::PRUNE_MIN_RETAINED_BLOCKS`
/// so retarget never breaks. Pass `--dry-run` to print what would be
/// removed without touching disk.
fn cmd_chain_prune(args: &[String]) -> Result<(), String> {
    let keep_last_raw = required_option_value(args, "--keep-last")?;
    let keep_last: u64 = keep_last_raw.parse().map_err(|e| {
        format!("--keep-last must be a non-negative integer (got '{keep_last_raw}'): {e}")
    })?;
    let db_path_owned = option_value(args, "--db");
    let db_path = db_path_owned.as_deref().unwrap_or(".zoka/blockchain.db");
    let dry_run = has_flag(args, "--dry-run");
    let as_json = has_flag(args, "--json");

    let storage = RocksDbStorage::new(db_path)
        .map_err(|e| format!("failed to open chain db '{db_path}': {e}"))?;

    let report = zoka::state::chain::prune_below_head(&storage, keep_last, dry_run)
        .map_err(|e| format!("prune failed: {e}"))?;

    if as_json {
        let payload = serde_json::json!({
            "db": db_path,
            "head_height": report.head_height,
            "retained_from_height": report.retained_from_height,
            "blocks_pruned": report.blocks_pruned,
            "height_indices_pruned": report.height_indices_pruned,
            "metas_pruned": report.metas_pruned,
            "dry_run": report.dry_run,
            "min_retained_floor": zoka::state::chain::PRUNE_MIN_RETAINED_BLOCKS,
        });
        println!("{}", payload);
    } else {
        let mode = if report.dry_run { "DRY-RUN" } else { "applied" };
        println!("Chain prune ({mode})");
        println!("  DB:                       {db_path}");
        println!("  Head height:              {}", report.head_height);
        println!(
            "  Retained from height:     {}",
            report.retained_from_height
        );
        println!(
            "  Floor (PRUNE_MIN_RETAINED_BLOCKS): {}",
            zoka::state::chain::PRUNE_MIN_RETAINED_BLOCKS
        );
        println!("  Block bodies pruned:      {}", report.blocks_pruned);
        println!(
            "  Height indices pruned:    {}",
            report.height_indices_pruned
        );
        println!("  Block metadata pruned:    {}", report.metas_pruned);
        if report.dry_run {
            println!();
            println!("Dry-run only; storage NOT modified. Re-run without --dry-run to apply.");
        }
    }
    Ok(())
}

/// `zoka chain-republish-private-txs --rpc <url> [--db <path>] [--limit <n>] [--json]`
///
/// Bulk-uploads every private transaction body stored locally under
/// `priv_tx:<hash>` to the target RPC's `POST /private/tx` archive
/// endpoint. Used to backfill bootstrap/RPC nodes that never received the
/// bodies (e.g. txs whose original sender failed to POST or whose RPC
/// was offline at the time). Skips bodies the remote already has and
/// keeps going past per-tx errors so a single bad body never aborts the
/// whole sweep.
fn cmd_chain_republish_private_txs(args: &[String]) -> Result<(), String> {
    let rpc_url = required_option_value(args, "--rpc")?;
    let db_path_owned = option_value(args, "--db");
    let db_path = db_path_owned.as_deref().unwrap_or(".zoka/blockchain.db");
    let limit: usize = option_value(args, "--limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(usize::MAX);
    let as_json = has_flag(args, "--json");

    let storage = RocksDbStorage::new(db_path)
        .map_err(|e| format!("failed to open chain db '{db_path}': {e}"))?;

    let bodies = storage
        .iterate_private_transactions()
        .map_err(|e| format!("failed to iterate priv_tx storage: {e}"))?;

    let total = bodies.len();
    let mut published = 0usize;
    let mut decode_errors = 0usize;
    let mut publish_errors: Vec<(String, String)> = Vec::new();

    for (hash, body_bytes) in bodies.into_iter().take(limit) {
        let tx: PrivateTransaction = match serde_json::from_slice(&body_bytes) {
            Ok(t) => t,
            Err(err) => {
                decode_errors = decode_errors.saturating_add(1);
                publish_errors.push((hash, format!("decode failed: {err}")));
                continue;
            }
        };
        match publish_private_tx_body_to_rpc(&rpc_url, &tx) {
            Ok(()) => published = published.saturating_add(1),
            Err(err) => publish_errors.push((hash, err)),
        }
    }

    if as_json {
        println!(
            "{}",
            serde_json::json!({
                "db": db_path,
                "rpc": rpc_url,
                "total_bodies": total,
                "published": published,
                "decode_errors": decode_errors,
                "publish_errors": publish_errors.iter().map(|(h, e)| serde_json::json!({"hash": h, "error": e})).collect::<Vec<_>>(),
            })
        );
    } else {
        println!("Chain republish private txs");
        println!("  DB:               {db_path}");
        println!("  RPC:              {rpc_url}");
        println!("  Total bodies:     {total}");
        println!("  Published:        {published}");
        println!("  Decode errors:    {decode_errors}");
        println!("  Publish errors:   {}", publish_errors.len());
        if !publish_errors.is_empty() {
            println!();
            println!("First 10 errors:");
            for (hash, err) in publish_errors.iter().take(10) {
                println!("  {hash}: {err}");
            }
        }
    }
    Ok(())
}

fn cmd_mainnet_network_lint(args: &[String]) -> Result<(), String> {
    let path = args
        .iter()
        .find(|arg| !arg.starts_with("--"))
        .map(PathBuf::from)
        .unwrap_or_else(default_mainnet_network_config_path);
    let as_json = has_flag(args, "--json");
    let allow_placeholders = has_flag(args, "--allow-placeholders");
    let report = lint_mainnet_network_config(&path, allow_placeholders)?;

    if as_json {
        println!(
            "{}",
            serde_json::to_string(&report)
                .map_err(|e| format!("failed to encode network lint json: {e}"))?
        );
    } else {
        println!("Mainnet network lint");
        println!("Path:                         {}", report.path);
        println!("Status:                       {}", report.status);
        println!("Schema:                       {}", report.schema);
        println!("Chain id:                     {}", report.chain_id);
        println!(
            "Protocol freeze:              {}",
            report.protocol_freeze_id
        );
        println!("Seed nodes:                   {}", report.seed_count);
        println!("Active seed nodes:            {}", report.active_seed_count);
        println!(
            "Active anonymous candidates:  {}",
            report.active_seed_candidates_without_clearnet
        );
        println!(
            "Active candidates w/clearnet: {}",
            report.active_seed_candidates_with_clearnet
        );
        println!("RPC endpoints:                {}", report.public_rpc_count);
        println!("Active RPC endpoints:         {}", report.active_rpc_count);
        println!(
            "Snapshot signers:             {}",
            report.snapshot_signer_count
        );
        for warning in &report.warnings {
            println!("WARNING: {warning}");
        }
        for error in &report.errors {
            println!("ERROR: {error}");
        }
        if report.ok {
            println!("OK - network config is launch-ready.");
        }
    }

    if report.ok {
        Ok(())
    } else {
        Err(format!(
            "mainnet network config is not launch-ready ({} error(s))",
            report.errors.len()
        ))
    }
}

fn cmd_release_keygen(args: &[String]) -> Result<(), String> {
    let out_path = required_option_value(args, "--out")?;
    let label = option_value(args, "--label").unwrap_or_else(|| "release-key".to_string());
    let as_json = has_flag(args, "--json");
    let force = has_flag(args, "--force");
    let path = PathBuf::from(&out_path);

    if path.exists() && !force {
        return Err(format!(
            "refusing to overwrite '{}'; pass --force to replace it",
            path.display()
        ));
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|e| {
                format!(
                    "failed to create key output directory '{}': {e}",
                    parent.display()
                )
            })?;
        }
    }

    let mut secret = [0u8; 32];
    use rand::RngCore as _;
    rand::rngs::OsRng.fill_bytes(&mut secret);
    let signing_key = ed25519_dalek::SigningKey::from_bytes(&secret);
    let public = signing_key.verifying_key().to_bytes();
    let created_unix_secs = now_secs();
    let public_key_hex = hex::encode(public);
    let key_file = ReleaseKeypairFile {
        schema: "zoka-ed25519-keypair-v1",
        label: label.clone(),
        public_key_hex: public_key_hex.clone(),
        secret_key_hex: hex::encode(secret),
        created_unix_secs,
        warning: "SECRET KEY MATERIAL. Keep offline. Do not commit this file.",
    };
    let encoded = serde_json::to_string_pretty(&key_file)
        .map_err(|e| format!("failed to encode keypair json: {e}"))?;
    fs::write(&path, encoded)
        .map_err(|e| format!("failed to write keypair file '{}': {e}", path.display()))?;

    let report = ReleaseKeygenPublicReport {
        schema: "zoka-ed25519-keypair-v1",
        label,
        public_key_hex,
        secret_key_file: path.display().to_string(),
        created_unix_secs,
    };

    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report)
                .map_err(|e| format!("failed to encode keygen report json: {e}"))?
        );
    } else {
        println!("Release key generated");
        println!("Label:           {}", report.label);
        println!("Verifying key hex:  {}", report.public_key_hex);
        println!("Secret key file: {}", report.secret_key_file);
        println!("Do not commit the secret key file.");
    }

    Ok(())
}

fn cmd_snapshot_info(args: &[String]) -> Result<(), String> {
    let in_path = args
        .iter()
        .find(|arg| !arg.starts_with("--"))
        .cloned()
        .ok_or_else(|| {
            "missing argument: pass the snapshot path (e.g. zoka snapshot-info ./mainnet.snapshot.json)"
                .to_string()
        })?;
    let as_json = has_flag(args, "--json");
    let require_signature = has_flag(args, "--require-signature");
    let pubkey_hex =
        option_value(args, "--signer-key").or_else(|| option_value(args, "--public-key"));

    let bytes =
        fs::read(&in_path).map_err(|e| format!("failed to read snapshot '{in_path}': {e}"))?;
    let snapshot =
        zoka::snapshot::deserialize(&bytes).map_err(|e| format!("snapshot parse failed: {e}"))?;

    let pubkey_array;
    let pubkey_ref: Option<&[u8; 32]> = match pubkey_hex {
        Some(hex_key) => {
            let decoded = hex::decode(hex_key.trim())
                .map_err(|e| format!("snapshot signer key must be 32-byte hex: {e}"))?;
            if decoded.len() != 32 {
                return Err(format!(
                    "snapshot signer key must decode to 32 bytes (got {})",
                    decoded.len()
                ));
            }
            let mut key = [0u8; 32];
            key.copy_from_slice(&decoded);
            pubkey_array = key;
            Some(&pubkey_array)
        }
        None => None,
    };

    let opts = zoka::snapshot::VerifyOptions {
        expected_chain_id: MAINNET_CHAIN_ID,
        require_signature: require_signature || pubkey_ref.is_some(),
        expected_pubkey: pubkey_ref,
        expected_hybrid_pubkey: None,
        require_hybrid: false,
    };
    let digest = zoka::snapshot::verify(&snapshot, &opts)
        .map_err(|e| format!("snapshot verify failed: {e}"))?;

    if as_json {
        println!(
            "{}",
            serde_json::json!({
                "path": in_path,
                "digest_sha256_hex": digest,
                "schema": snapshot.header.schema,
                "chain_id": snapshot.header.chain_id,
                "head_height": snapshot.header.head_height,
                "head_hash": snapshot.header.head_hash,
                "block_count": snapshot.header.block_count,
                "cumulative_work": snapshot.header.cumulative_work.to_string(),
                "current_difficulty_bits": snapshot.header.current_difficulty_bits,
                "total_supply_emitted": snapshot.header.total_supply_emitted,
                "generated_unix_secs": snapshot.header.generated_unix_secs,
                "signed": snapshot.signature_hex.is_some(),
                "signer_pubkey_hex": snapshot.signer_pubkey_hex,
            })
        );
        return Ok(());
    }

    println!("Snapshot:           {in_path}");
    println!("Schema:             {}", snapshot.header.schema);
    println!("Chain id:           {}", snapshot.header.chain_id);
    println!("Head height:        {}", snapshot.header.head_height);
    println!("Head hash:          {}", snapshot.header.head_hash);
    println!("Block count:        {}", snapshot.header.block_count);
    println!("Cumulative work:    {}", snapshot.header.cumulative_work);
    println!(
        "Current difficulty: {}",
        snapshot.header.current_difficulty_bits
    );
    println!(
        "Total supply (ZKA): {}",
        format_zka_amount(snapshot.header.total_supply_emitted)
    );
    println!(
        "Generated unix:     {}",
        snapshot.header.generated_unix_secs
    );
    println!("Signed:             {}", snapshot.signature_hex.is_some());
    if let Some(pubkey) = snapshot.signer_pubkey_hex.as_deref() {
        println!("Signer pubkey:      {pubkey}");
    }
    println!("SHA-256 digest:     {digest}");
    Ok(())
}

/// `zoka snapshot-export <out-path> [--db <path>] [--sign-key-hex <hex>]`
///
/// Reads the canonical chain from RocksDB, packs every block + chain
/// metadata into a `zoka-snapshot-v1` JSON, optionally signs it with
/// ed25519, writes the result to disk, and prints the SHA-256 digest.
fn cmd_snapshot_export(args: &[String]) -> Result<(), String> {
    let out_path = args
        .iter()
        .find(|arg| !arg.starts_with("--"))
        .cloned()
        .ok_or_else(|| {
            "missing argument: pass the output path (e.g. zoka snapshot-export ./mainnet.snapshot.json)"
                .to_string()
        })?;
    let db_path_owned = option_value(args, "--db");
    let db_path = db_path_owned.as_deref().unwrap_or(".zoka/blockchain.db");
    let sign_key_hex = option_value(args, "--sign-key-hex");

    let storage = RocksDbStorage::new(db_path)
        .map_err(|e| format!("failed to open chain db '{db_path}': {e}"))?;

    let mut snapshot = zoka::snapshot::export_from_storage(&storage, MAINNET_CHAIN_ID, now_secs())
        .map_err(|e| format!("snapshot export failed: {e}"))?;

    if let Some(hex_key) = sign_key_hex {
        let bytes = hex::decode(hex_key.trim())
            .map_err(|e| format!("--sign-key-hex must be 32-byte hex: {e}"))?;
        if bytes.len() != 32 {
            return Err(format!(
                "--sign-key-hex must decode to 32 bytes (got {})",
                bytes.len()
            ));
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes);
        match option_value(args, "--mldsa-key-file") {
            // Firma híbrida (etapa 6): la clave Ed25519 de --sign-key-hex más
            // la semilla ML-DSA-65 de ese archivo.
            #[cfg(feature = "pq-experimental")]
            Some(mldsa_path) => {
                let seed = zoka::hybrid_signature::load_mldsa_seed(Path::new(&mldsa_path))?;
                let hybrid = zoka::hybrid_signature::HybridSigningKey::from_parts(
                    ed25519_dalek::SigningKey::from_bytes(&key),
                    seed.as_ref(),
                )
                .map_err(|e| format!("hybrid snapshot key: {e}"))?;
                zoka::snapshot::sign_hybrid(&mut snapshot, &hybrid)
                    .map_err(|e| format!("snapshot signing failed: {e}"))?;
            }
            #[cfg(not(feature = "pq-experimental"))]
            Some(_) => {
                return Err("--mldsa-key-file needs a build with --features pq-experimental".to_string())
            }
            None => zoka::snapshot::sign(&mut snapshot, &key)
                .map_err(|e| format!("snapshot signing failed: {e}"))?,
        }
    } else if option_value(args, "--mldsa-key-file").is_some() {
        return Err("--mldsa-key-file is the second half of --sign-key-hex; pass both".to_string());
    }

    let bytes = zoka::snapshot::serialize(&snapshot)
        .map_err(|e| format!("snapshot serialization failed: {e}"))?;
    let digest = zoka::snapshot::compute_digest(&snapshot)
        .map_err(|e| format!("snapshot digest failed: {e}"))?;

    fs::write(&out_path, &bytes)
        .map_err(|e| format!("failed to write snapshot to '{out_path}': {e}"))?;

    // Sidecar manifest is what makes this snapshot discoverable via
    // GET /snapshots and /snapshots/latest (both scan for *.snap.manifest).
    // Its sha256_hex covers the bytes actually written to disk, which is
    // not the same value as `digest` above (that one hashes the canonical
    // signed payload, excluding the signature, for verification purposes).
    let mut file_hasher = Sha256::new();
    file_hasher.update(&bytes);
    let file_sha256_hex = hex::encode(file_hasher.finalize());
    let manifest_path = zoka::snapshot::write_manifest_sidecar(
        &snapshot,
        std::path::Path::new(&out_path),
        bytes.len() as u64,
        file_sha256_hex,
        Vec::new(),
    )
    .map_err(|e| format!("failed to write manifest sidecar: {e}"))?;

    println!("Snapshot:           {out_path}");
    println!("Manifest sidecar:   {}", manifest_path.display());
    println!("Schema:             {}", zoka::snapshot::SNAPSHOT_SCHEMA_ID);
    println!("Chain id:           {}", snapshot.header.chain_id);
    println!("Head height:        {}", snapshot.header.head_height);
    println!("Head hash:          {}", snapshot.header.head_hash);
    println!("Block count:        {}", snapshot.header.block_count);
    println!("Cumulative work:    {}", snapshot.header.cumulative_work);
    println!(
        "Total supply (ZKA): {}",
        format_zka_amount(snapshot.header.total_supply_emitted)
    );
    println!("Signed:             {}", snapshot.signature_hex.is_some());
    println!("SHA-256 digest:     {digest}");
    Ok(())
}

/// `zoka snapshot-import <in-path> [--db <path>] [--require-signature] [--signer-key <hex>]`
///
/// Loads a snapshot from disk, verifies header + signature per options,
/// then replays every block through `state::chain::apply_block` so
/// every consensus rule (PoW, coinbase, fork-link, difficulty) is
/// re-checked. Idempotent: blocks the local chain already has are
/// skipped.
fn cmd_snapshot_import(args: &[String]) -> Result<(), String> {
    let in_path = args
        .iter()
        .find(|arg| !arg.starts_with("--"))
        .cloned()
        .ok_or_else(|| {
            "missing argument: pass the snapshot path (e.g. zoka snapshot-import ./mainnet.snapshot.json)"
                .to_string()
        })?;
    let db_path_owned = option_value(args, "--db");
    let db_path = db_path_owned.as_deref().unwrap_or(".zoka/blockchain.db");
    let require_signature = has_flag(args, "--require-signature");
    let pubkey_hex =
        option_value(args, "--signer-key").or_else(|| option_value(args, "--public-key"));

    let bytes =
        fs::read(&in_path).map_err(|e| format!("failed to read snapshot '{in_path}': {e}"))?;
    let snapshot =
        zoka::snapshot::deserialize(&bytes).map_err(|e| format!("snapshot parse failed: {e}"))?;

    let pubkey_array;
    let pubkey_ref: Option<&[u8; 32]> = match pubkey_hex {
        Some(hex_key) => {
            let decoded = hex::decode(hex_key.trim())
                .map_err(|e| format!("snapshot signer key must be 32-byte hex: {e}"))?;
            if decoded.len() != 32 {
                return Err(format!(
                    "snapshot signer key must decode to 32 bytes (got {})",
                    decoded.len()
                ));
            }
            let mut key = [0u8; 32];
            key.copy_from_slice(&decoded);
            pubkey_array = key;
            Some(&pubkey_array)
        }
        None => None,
    };

    let opts = zoka::snapshot::VerifyOptions {
        expected_chain_id: MAINNET_CHAIN_ID,
        require_signature: require_signature || pubkey_ref.is_some(),
        expected_pubkey: pubkey_ref,
        expected_hybrid_pubkey: None,
        require_hybrid: false,
    };
    let digest = zoka::snapshot::verify(&snapshot, &opts)
        .map_err(|e| format!("snapshot verify failed: {e}"))?;

    let storage = Arc::new(
        RocksDbStorage::new(db_path)
            .map_err(|e| format!("failed to open chain db '{db_path}': {e}"))?,
    );

    let applied = zoka::snapshot::import_into_storage(&storage, MAINNET_CHAIN_ID, &snapshot)
        .map_err(|e| format!("snapshot import failed: {e}"))?;

    println!("Snapshot:           {in_path}");
    println!("SHA-256 digest:     {digest}");
    println!("Head height:        {}", snapshot.header.head_height);
    println!("Blocks applied:     {applied}");
    println!(
        "Blocks skipped:     {}",
        snapshot.header.block_count.saturating_sub(applied)
    );
    println!(
        "Local chain height: {}",
        storage.get_chain_height().unwrap_or(0)
    );
    Ok(())
}

/// Export the current chain as serialized snapshot bytes plus its head height.
fn export_snapshot_bytes(
    storage: &RocksDbStorage,
    chain_id: &str,
) -> Result<(Vec<u8>, u64), String> {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let snapshot = zoka::snapshot::export_from_storage(storage, chain_id, timestamp)
        .map_err(|e| format!("export failed: {e}"))?;
    let height = snapshot.header.head_height;
    let bytes =
        zoka::snapshot::serialize(&snapshot).map_err(|e| format!("serialize failed: {e}"))?;
    Ok((bytes, height))
}

/// True if `snapshot` carries a valid signature from one of the manifest's
/// trusted `snapshot_signers`. A network-fetched snapshot must clear this
/// before import so a peer can't feed a fresh node an alternate chain; block
/// re-validation on import already blocks *invalid* chains, this blocks
/// *unauthorized valid* ones.
fn snapshot_signed_by_trusted(
    snapshot: &zoka::snapshot::ChainSnapshotV1,
    chain_id: &str,
    signers: &[String],
) -> bool {
    zoka::snapshot::verify_against_trusted_signers(snapshot, chain_id, signers).is_ok()
}

/// If this is a fresh node (empty chain) and the manifest advertises iroh
/// snapshot seeds, pull a signed snapshot over iroh and import it before IBD --
/// catching up in one transfer instead of crawling block-by-block. Fully
/// guarded: any failure logs and the node falls through to normal IBD. Inert
/// (no-op) until a seed's EndpointId is listed in the manifest and the
/// snapshot is signed by a trusted snapshot_signer.
async fn maybe_bootstrap_from_iroh(
    storage: Option<&Arc<RocksDbStorage>>,
    chain_id: &str,
    network: NetworkProfile,
    config_path: &Path,
    enabled: bool,
    require_signed_snapshot: bool,
) {
    if !enabled {
        return;
    }
    let Some(storage) = storage else {
        return;
    };
    if storage.get_chain_height().unwrap_or(0) > 0 {
        // Only bootstrap a genuinely empty node; anything with a chain uses
        // the local-snapshot path or normal IBD.
        return;
    }
    let config = match load_network_config_for(config_path, network) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(target: "zoka.iroh", error = %e, "iroh bootstrap skipped: network config load failed");
            return;
        }
    };
    let seed_ids = config.iroh_snapshot_seed_ids();
    if seed_ids.is_empty() {
        return;
    }
    // The iroh EndpointId in the trusted manifest is itself the authorization:
    // to serve from EndpointId X you must hold X's secret key, so a
    // manifest-listed id cryptographically authenticates the source (the same
    // trust the snapshot signature provides). We therefore don't also require
    // the snapshot's own signature on this path -- and the import re-validates
    // every block against consensus from genesis regardless. Opt in to extra
    // signature enforcement with --iroh-require-signed-snapshot.
    let require_signature = require_signed_snapshot;
    for id_str in &seed_ids {
        let endpoint_id: iroh::EndpointId = match id_str.parse() {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!(target: "zoka.iroh", id = %id_str, error = %e, "skipping invalid iroh seed id");
                continue;
            }
        };
        // Print outcomes to stdout (not tracing) so they show in the ZSilent
        // log panel, which only captures the child's stdout. Bound the fetch
        // with a timeout so an unreachable relay/hole-punch can't hang node
        // startup (this runs before node.listen) -- it falls back to IBD.
        println!(
            "iroh bootstrap: connecting to snapshot seed {endpoint_id} over iroh (n0 relay) ..."
        );
        let fetch = zoka::network::iroh_snapshot_transfer::fetch_snapshot(
            endpoint_id.into(),
            iroh_preset(),
        );
        let bytes = match tokio::time::timeout(Duration::from_secs(120), fetch).await {
            Ok(Ok(b)) => b,
            Ok(Err(e)) => {
                println!("iroh bootstrap: fetch from {endpoint_id} failed: {e}. Trying next seed / falling back to IBD.");
                continue;
            }
            Err(_) => {
                println!("iroh bootstrap: fetch from {endpoint_id} timed out after 120s (relay/hole-punch unreachable from this network). Falling back to IBD.");
                continue;
            }
        };
        println!(
            "iroh bootstrap: transferred {} bytes from {endpoint_id}; verifying and importing ...",
            bytes.len()
        );
        let snapshot = match zoka::snapshot::deserialize(&bytes) {
            Ok(s) => s,
            Err(e) => {
                println!(
                    "iroh bootstrap: fetched snapshot failed to parse: {e}. Falling back to IBD."
                );
                continue;
            }
        };
        if require_signature
            && !snapshot_signed_by_trusted(&snapshot, chain_id, &config.snapshot_signers)
        {
            println!("iroh bootstrap: snapshot rejected (not signed by a trusted snapshot_signer). Falling back to IBD.");
            continue;
        }
        match zoka::snapshot::import_into_storage(storage, chain_id, &snapshot) {
            Ok(applied) => {
                println!(
                    "iroh bootstrap complete: imported {applied} blocks up to height {} (IBD will take it from here).",
                    snapshot.header.head_height
                );
                return;
            }
            Err(e) => {
                println!("iroh bootstrap: snapshot import failed: {e}. Trying next seed / falling back to IBD.");
                continue;
            }
        }
    }
    println!("iroh bootstrap: no snapshot seed could be reached; using normal block-by-block IBD.");
}

/// Live chain view served over iroh (see [`ChainServe`]). Head height and
/// individual blocks are read straight from storage so followers always get
/// the current tip; `snapshot` is the periodically re-exported blob that backs
/// `SNP1` fresh-bootstrap pulls.
struct StorageChainServe {
    storage: Arc<RocksDbStorage>,
    snapshot: zoka::network::iroh_snapshot_transfer::SnapshotProvider,
}

impl zoka::network::iroh_snapshot_transfer::ChainServe for StorageChainServe {
    fn head_height(&self) -> u64 {
        self.storage.get_chain_height().unwrap_or(0)
    }

    fn snapshot_bytes(&self) -> Arc<Vec<u8>> {
        self.snapshot
            .read()
            .map(|g| g.clone())
            .unwrap_or_else(|_| Arc::new(Vec::new()))
    }

    fn block_bytes(&self, height: u64) -> Option<Vec<u8>> {
        let block = self.storage.get_block_by_height(height).ok().flatten()?;
        let encoded = serde_json::to_vec(&block).ok()?;
        // Ship a full BlockResponseMessage so the follower applies it through
        // the exact same validated path as a gossiped/RPC-synced block, and
        // include the private-tx bodies so a node catching up over iroh (e.g.
        // after a disconnect) can still recover private transfers it received
        // while offline -- not just the block header/commitments.
        let private_tx_bodies = collect_private_tx_bodies_from_storage(&self.storage, &block);
        let response = zoka::network::message::BlockResponseMessage {
            block_height: block.header.block_height,
            encrypted_block: encoded,
            block_hash: block.header.hash.clone(),
            difficulty_bits: None,
            private_tx_bodies,
            timestamp: block.header.timestamp,
        };
        serde_json::to_vec(&response).ok()
    }
}

/// Client side of the incremental iroh sync: probe each manifest iroh seed's
/// head height and, if it is ahead of us, pull the missing blocks over iroh and
/// apply them through the standard validated sync path. This is what keeps a
/// NAT'd node current when there is no live libp2p link to the miner — the same
/// n0-relay hole-punching that makes snapshot bootstrap work, reused for the
/// steady-state tail. Fully guarded and bounded by timeouts; any failure just
/// logs and the node keeps relying on libp2p/RPC. Returns a short event string
/// when it advanced the chain.
async fn maybe_iroh_head_sync(
    storage: &Arc<RocksDbStorage>,
    chain_id: &str,
    seed_ids: &[String],
    own_endpoint_id: Option<&str>,
) -> Option<String> {
    // How many blocks to pull per tick, so one catch-up can't hog the loop.
    const MAX_BLOCKS_PER_TICK: u32 = 64;
    let local_height = storage.get_chain_height().unwrap_or(0);

    for id_str in seed_ids {
        let id_str = id_str.trim();
        if id_str.is_empty() || own_endpoint_id == Some(id_str) {
            continue;
        }
        let endpoint_id: iroh::EndpointId = match id_str.parse() {
            Ok(id) => id,
            Err(_) => continue,
        };

        // Cheap staleness probe first.
        let head_fut = zoka::network::iroh_snapshot_transfer::fetch_head(
            endpoint_id.into(),
            iroh_preset(),
        );
        let seed_head = match tokio::time::timeout(Duration::from_secs(8), head_fut).await {
            Ok(Ok(h)) => h,
            _ => continue, // unreachable seed / timeout -> try next
        };
        if seed_head <= local_height {
            continue; // this seed has nothing newer
        }

        let want = ((seed_head - local_height).min(MAX_BLOCKS_PER_TICK as u64)) as u32;
        let blocks_fut = zoka::network::iroh_snapshot_transfer::fetch_block_range(
            endpoint_id.into(),
            iroh_preset(),
            local_height + 1,
            want,
        );
        let block_msgs = match tokio::time::timeout(Duration::from_secs(30), blocks_fut).await {
            Ok(Ok(b)) if !b.is_empty() => b,
            _ => continue,
        };

        // Apply in order; each apply advances the head by one so the next block
        // is exactly head+1 (avoids the GapAhead guard).
        let mut applied = 0u64;
        for raw in &block_msgs {
            let response: zoka::network::message::BlockResponseMessage =
                match serde_json::from_slice(raw) {
                    Ok(r) => r,
                    Err(_) => break,
                };
            match zoka::state::sync::try_apply_synced_block_payload(storage, chain_id, &response) {
                zoka::state::sync::SyncOutcome::Applied(_) => applied += 1,
                zoka::state::sync::SyncOutcome::AlreadyKnown { .. } => continue,
                // Anything else (bad payload, gap, sibling) -> stop this batch;
                // the next tick re-probes from the new local height.
                _ => break,
            }
        }
        if applied > 0 {
            let new_height = storage.get_chain_height().unwrap_or(local_height);
            return Some(format!(
                "iroh tail-sync: pulled {applied} block(s) from seed {endpoint_id}; now at height {new_height}."
            ));
        }
    }
    None
}

/// Bind an iroh endpoint that serves this node's chain as a snapshot so fresh
/// nodes can bootstrap over iroh, and spawn a background task that re-exports
/// periodically as the chain grows (via an updatable provider, no rebind).
/// Opt-in and fully guarded: on any error it logs and returns None, never
/// blocking node startup. The returned endpoint must be held for as long as
/// serving should continue.
async fn maybe_serve_iroh_snapshot(
    storage: Option<&Arc<RocksDbStorage>>,
    chain_id: &str,
    enabled: bool,
) -> Option<iroh::Endpoint> {
    if !enabled {
        return None;
    }
    let Some(storage) = storage else {
        tracing::warn!(target: "zoka.iroh", "iroh snapshot seed skipped: no chain DB configured");
        return None;
    };
    let key_path = zoka_home().join("iroh_key");
    let key = match zoka::network::iroh_snapshot_transfer::load_or_generate_secret_key(&key_path) {
        Ok(k) => k,
        Err(e) => {
            tracing::warn!(target: "zoka.iroh", error = %e, "iroh snapshot seed disabled: identity key load failed");
            return None;
        }
    };
    let (bytes, height) = match export_snapshot_bytes(storage, chain_id) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(target: "zoka.iroh", error = %e, "iroh snapshot seed disabled");
            return None;
        }
    };
    let provider = zoka::network::iroh_snapshot_transfer::provider_from_arc(Arc::new(bytes));
    // Serve the live chain (not just a static snapshot blob): the same endpoint
    // now answers head-height probes and incremental block-range pulls, so a
    // NAT'd follower can stay current over iroh without a live libp2p mesh. The
    // snapshot provider (refreshed below) still backs SNP1 fresh-bootstrap; head
    // and blocks come straight from live storage.
    let source: Arc<dyn zoka::network::iroh_snapshot_transfer::ChainServe> =
        Arc::new(StorageChainServe {
            storage: storage.clone(),
            snapshot: provider.clone(),
        });
    let endpoint = match zoka::network::iroh_snapshot_transfer::serve_chain(
        source,
        key,
        iroh_preset(),
    )
    .await
    {
        Ok(endpoint) => endpoint,
        Err(e) => {
            tracing::warn!(target: "zoka.iroh", error = %e, "iroh snapshot serve failed");
            return None;
        }
    };
    println!(
        "iroh snapshot seed live at height {height}; EndpointId = {} (advertise this in the manifest 'iroh' field)",
        endpoint.id()
    );

    // Refresh the served snapshot as the chain grows so late-joining nodes
    // don't have to IBD a long tail. Runs for the endpoint's lifetime.
    let storage_bg = storage.clone();
    let chain_id_bg = chain_id.to_string();
    tokio::spawn(async move {
        const REEXPORT_INTERVAL: Duration = Duration::from_secs(300);
        loop {
            tokio::time::sleep(REEXPORT_INTERVAL).await;
            match export_snapshot_bytes(&storage_bg, &chain_id_bg) {
                Ok((bytes, new_height)) => {
                    zoka::network::iroh_snapshot_transfer::update_provider(&provider, bytes);
                    tracing::debug!(target: "zoka.iroh", height = new_height, "iroh snapshot refreshed");
                }
                Err(e) => {
                    tracing::warn!(target: "zoka.iroh", error = %e, "iroh snapshot re-export failed; keeping previous");
                }
            }
        }
    });
    Some(endpoint)
}

/// `zoka iroh-id [--json]` -- print this node's stable iroh EndpointId (the
/// address to advertise in the manifest 'iroh' field). Loads or generates the
/// persistent identity key, so it matches what `--serve-iroh-snapshot` uses.
fn cmd_iroh_id(args: &[String]) -> Result<(), String> {
    let key_path = zoka_home().join("iroh_key");
    let key = zoka::network::iroh_snapshot_transfer::load_or_generate_secret_key(&key_path)
        .map_err(|e| format!("failed to load iroh key '{}': {e}", key_path.display()))?;
    let id = key.public();
    if has_flag(args, "--json") {
        println!("{{\"iroh_endpoint_id\":\"{id}\"}}");
    } else {
        println!("{id}");
    }
    Ok(())
}

/// `zoka snapshot-fetch-iroh --iroh-id <endpoint-id> [--db <path>]
/// [--require-signature] [--signer-key <hex>]`
///
/// Pull a full-chain snapshot from a seed's iroh endpoint (QUIC + NAT
/// hole-punching over the n0 relays -- no open port, no Tor) and import it,
/// verifying its signature exactly like `snapshot-import`. This is the fast
/// bootstrap path: a fresh node catches up in the time it takes to transfer +
/// replay one snapshot instead of crawling block-by-block IBD.
fn cmd_snapshot_fetch_iroh(args: &[String]) -> Result<(), String> {
    let iroh_id = option_value(args, "--iroh-id").ok_or_else(|| {
        "missing --iroh-id <endpoint-id> (the seed's iroh EndpointId from the network manifest)"
            .to_string()
    })?;
    let db_path_owned = option_value(args, "--db");
    let db_path = db_path_owned.as_deref().unwrap_or(".zoka/blockchain.db");
    let require_signature = has_flag(args, "--require-signature");
    let pubkey_hex =
        option_value(args, "--signer-key").or_else(|| option_value(args, "--public-key"));

    let endpoint_id: iroh::EndpointId = iroh_id
        .trim()
        .parse()
        .map_err(|e| format!("invalid iroh endpoint id '{iroh_id}': {e}"))?;

    println!("Pulling snapshot over iroh from {endpoint_id} ...");
    // Run the async fetch on a dedicated thread with its own runtime: the CLI
    // dispatch may already be inside a tokio runtime, and a nested block_on
    // panics ("Cannot start a runtime from within a runtime").
    let bytes = std::thread::spawn(move || -> Result<Vec<u8>, String> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("failed to start runtime for iroh fetch: {e}"))?;
        rt.block_on(async move {
            zoka::network::iroh_snapshot_transfer::fetch_snapshot(
                endpoint_id.into(),
                iroh_preset(),
            )
            .await
            .map_err(|e| format!("iroh snapshot fetch failed: {e}"))
        })
    })
    .join()
    .map_err(|_| "iroh fetch thread panicked".to_string())??;
    println!("Transferred {} bytes.", bytes.len());

    let snapshot =
        zoka::snapshot::deserialize(&bytes).map_err(|e| format!("snapshot parse failed: {e}"))?;

    let pubkey_array;
    let pubkey_ref: Option<&[u8; 32]> = match pubkey_hex {
        Some(hex_key) => {
            let decoded = hex::decode(hex_key.trim())
                .map_err(|e| format!("snapshot signer key must be 32-byte hex: {e}"))?;
            if decoded.len() != 32 {
                return Err(format!(
                    "snapshot signer key must decode to 32 bytes (got {})",
                    decoded.len()
                ));
            }
            let mut key = [0u8; 32];
            key.copy_from_slice(&decoded);
            pubkey_array = key;
            Some(&pubkey_array)
        }
        None => None,
    };

    let opts = zoka::snapshot::VerifyOptions {
        expected_chain_id: MAINNET_CHAIN_ID,
        require_signature: require_signature || pubkey_ref.is_some(),
        expected_pubkey: pubkey_ref,
        expected_hybrid_pubkey: None,
        require_hybrid: false,
    };
    let digest = zoka::snapshot::verify(&snapshot, &opts)
        .map_err(|e| format!("snapshot verify failed: {e}"))?;

    let storage = Arc::new(
        RocksDbStorage::new(db_path)
            .map_err(|e| format!("failed to open chain db '{db_path}': {e}"))?,
    );
    let applied = zoka::snapshot::import_into_storage(&storage, MAINNET_CHAIN_ID, &snapshot)
        .map_err(|e| format!("snapshot import failed: {e}"))?;

    println!("SHA-256 digest:     {digest}");
    println!("Head height:        {}", snapshot.header.head_height);
    println!("Blocks applied:     {applied}");
    println!(
        "Local chain height: {}",
        storage.get_chain_height().unwrap_or(0)
    );
    Ok(())
}

/// `zoka genesis-verify <path> [--json]`
///
/// Pure offline validator. Loads the genesis JSON, runs `validate_static`
/// + `compute_digest`, and prints a human report (or a machine-readable
/// JSON line with `--json`). Operators run this BEFORE booting any node
/// against a published genesis to confirm the digest matches the
/// announced value.
fn cmd_genesis_verify(args: &[String]) -> Result<(), String> {
    let path = args
        .iter()
        .find(|arg| !arg.starts_with("--"))
        .cloned()
        .ok_or_else(|| {
            "missing argument: pass the path to the genesis json (e.g. zoka genesis-verify docs/genesis.mainnet.v1.json)"
                .to_string()
        })?;
    let as_json = has_flag(args, "--json");

    let bytes =
        fs::read(&path).map_err(|e| format!("failed to read genesis file '{}': {e}", path))?;
    let genesis: zoka::genesis::MainnetGenesisV1 = serde_json::from_slice(&bytes).map_err(|e| {
        format!(
            "failed to parse genesis '{}' as zoka-genesis-v1: {e}\n  hint: every field is required and unknown fields are rejected by design",
            path
        )
    })?;

    let report = zoka::genesis::GenesisVerifyReport::from_genesis(&genesis)
        .map_err(|e| format!("genesis '{}' rejected: {e}", path))?;

    if as_json {
        let payload = serde_json::json!({
            "path": path,
            "digest_sha256_hex": report.digest_hex,
            "chain_id": report.chain_id,
            "pow_algorithm": report.pow_algorithm,
            "validator_count": report.validator_count,
            "active_voting_power": report.active_voting_power,
            "pre_mine_total_zka": report.pre_mine_total_zka,
            "initial_account_count": report.initial_account_count,
            "schema": zoka::genesis::GENESIS_SCHEMA_ID,
        });
        println!("{}", payload);
    } else {
        println!("Genesis file:        {}", path);
        println!("Schema:              {}", zoka::genesis::GENESIS_SCHEMA_ID);
        println!("Chain id:            {}", report.chain_id);
        println!("PoW algorithm:       {}", report.pow_algorithm);
        println!("SHA-256 digest:      {}", report.digest_hex);
        println!("Validator count:     {}", report.validator_count);
        println!("Active voting power: {}", report.active_voting_power);
        println!("Pre-mine total ZKA:  {}", report.pre_mine_total_zka);
        println!("Initial accounts:    {}", report.initial_account_count);
        println!();
        println!(
            "OK - genesis is structurally valid. Compare the digest against the published value before applying it."
        );
    }
    Ok(())
}

/// Pila del hilo donde corre el programa. La principal de Windows tiene 1 MB y
/// Halo 2 y ML-DSA (la clave híbrida de la autoridad) recursan hondo: un nodo
/// de Quantic con su clave de autoridad se caía al arrancar. Es reserva de
/// direcciones, no memoria usada.
const MAIN_STACK_BYTES: usize = 64 * 1024 * 1024;
/// La de los hilos de trabajo de tokio (2 MB por omisión).
const WORKER_STACK_BYTES: usize = 8 * 1024 * 1024;

fn main() {
    let program = std::thread::Builder::new()
        .name("zoka".to_string())
        .stack_size(MAIN_STACK_BYTES)
        .spawn(|| {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_stack_size(WORKER_STACK_BYTES)
                .build()
                .expect("failed to start the tokio runtime")
                .block_on(async_main())
        })
        .expect("failed to start the main thread");
    if program.join().is_err() {
        // El pánico ya se imprimió; mismo código que un pánico en main.
        std::process::exit(101);
    }
}

async fn async_main() {
    // The fluid streaming console is the default (see cmd_fullnode_auto), so
    // keep the tracing log stream (p2p gossip INFO, etc.) from drowning it: if
    // the operator didn't pick a level explicitly, default to warn. Only the
    // full-screen --dashboard opt-out (flag, env, or an on-disk
    // zoka.config.json explicitly setting `mining_console: false`) restores
    // the old default (no forced level). Checked here, before
    // tracing_subscriber initializes, because by the time cmd_fullnode_auto
    // would normally expand the config file into args it's too late to change
    // the subscriber's level. An explicit RUST_LOG always wins either way.
    if std::env::var_os("RUST_LOG").is_none() {
        let raw_args: Vec<String> = std::env::args().skip(1).collect();
        let dashboard_opt_out = raw_args.iter().any(|a| a == "--dashboard")
            || std::env::var("ZOKA_DASHBOARD").as_deref() == Ok("1")
            || find_node_config_path(&raw_args)
                .ok()
                .flatten()
                .and_then(|path| fs::read_to_string(path).ok())
                .and_then(|raw| serde_json::from_str::<NodeConfigFile>(&raw).ok())
                .and_then(|cfg| cfg.mining_console)
                == Some(false);
        if !dashboard_opt_out {
            std::env::set_var("RUST_LOG", "warn");
        }
    }
    let _ = tracing_subscriber::fmt::try_init();
    setup_zoka_home();
    if let Err(err) = run().await {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

/// Resolve the canonical Zoka data directory (shared by CLI and ZSilent) and
/// change the process working directory into it before any subcommand runs.
/// All existing relative paths like ".zoka/mainnet" then resolve against the
/// single shared home, regardless of where the binary was launched from.
fn setup_zoka_home() {
    let home = zoka_home();
    if fs::create_dir_all(&home).is_err() {
        return;
    }
    migrate_legacy_zoka_dirs(&home);
    let _ = std::env::set_current_dir(&home);
}

/// Canonical per-user Zoka home. Resolution order:
///   1. $ZOKA_HOME (explicit override)
///   2. Windows: %APPDATA%\Zoka
///   3. macOS:   ~/Library/Application Support/Zoka
///   4. Linux/other: $XDG_DATA_HOME/zoka or ~/.local/share/zoka
///   5. Fallback: current working directory (legacy behavior)
fn zoka_home() -> PathBuf {
    if let Ok(dir) = std::env::var("ZOKA_HOME") {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            if !appdata.trim().is_empty() {
                return PathBuf::from(appdata).join("Zoka");
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            if !home.trim().is_empty() {
                return PathBuf::from(home)
                    .join("Library")
                    .join("Application Support")
                    .join("Zoka");
            }
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
            if !xdg.trim().is_empty() {
                return PathBuf::from(xdg).join("zoka");
            }
        }
        if let Ok(home) = std::env::var("HOME") {
            if !home.trim().is_empty() {
                return PathBuf::from(home)
                    .join(".local")
                    .join("share")
                    .join("zoka");
            }
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// One-shot migration. Looks at known legacy locations and, if the new home is
/// still empty, copies the `.zoka/` tree over so existing CLI and ZSilent users
/// keep their wallets and chain state when the binary is upgraded.
fn migrate_legacy_zoka_dirs(home: &Path) {
    let target = home.join(".zoka");
    if zoka_dir_has_data(&target) {
        return;
    }
    // Build the candidate list with the historically-used user data
    // locations first, so a stray `.zoka/` left behind in a dev or build
    // directory cannot shadow the real wallet.
    let mut candidates: Vec<PathBuf> = Vec::new();
    #[cfg(target_os = "windows")]
    if let Ok(appdata) = std::env::var("APPDATA") {
        let legacy = PathBuf::from(appdata).join("ZSilent Core").join(".zoka");
        if legacy != target {
            candidates.push(legacy);
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        let legacy = cwd.join(".zoka");
        if legacy != target && !candidates.contains(&legacy) {
            candidates.push(legacy);
        }
    }

    // Score each candidate by total bytes of wallet files so we always pick
    // the "richest" legacy tree instead of the first one we find.
    let mut best: Option<(u64, PathBuf)> = None;
    for source in candidates {
        if !zoka_dir_has_data(&source) {
            continue;
        }
        let score = zoka_dir_wallet_bytes(&source);
        if score == 0 {
            continue;
        }
        match &best {
            Some((current, _)) if *current >= score => {}
            _ => best = Some((score, source)),
        }
    }
    let Some((_, source)) = best else { return };

    if copy_dir_recursive(&source, &target).is_ok() {
        eprintln!(
            "zoka: migrated legacy data from '{}' to shared home '{}'",
            source.display(),
            target.display()
        );
    }
}

/// Total bytes of every private wallet file under any network subdir. Used as
/// a heuristic to pick the most meaningful legacy tree during migration.
fn zoka_dir_wallet_bytes(root: &Path) -> u64 {
    let mut total: u64 = 0;
    for network in ["mainnet", "testnet"] {
        for sub in ["private_wallets", "wallets"] {
            let dir = root.join(network).join(sub);
            if let Ok(entries) = fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    if let Ok(meta) = entry.metadata() {
                        if meta.is_file() {
                            total = total.saturating_add(meta.len());
                        }
                    }
                }
            }
        }
    }
    total
}

/// A Zoka data tree counts as "having data" if at least one wallet file or a
/// blockchain.db exists under any network subdir. Empty placeholder folders
/// (e.g. created by an early wallet-storage init) don't count.
fn zoka_dir_has_data(root: &Path) -> bool {
    if !root.is_dir() {
        return false;
    }
    for network in ["mainnet", "testnet"] {
        let net = root.join(network);
        if !net.is_dir() {
            continue;
        }
        if let Ok(entries) = fs::read_dir(net.join("private_wallets")) {
            if entries.flatten().next().is_some() {
                return true;
            }
        }
        if let Ok(entries) = fs::read_dir(net.join("wallets")) {
            if entries.flatten().next().is_some() {
                return true;
            }
        }
        if net.join("blockchain.db").is_dir() {
            return true;
        }
    }
    false
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let dst_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&entry.path(), &dst_path)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), dst_path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {

    /// Ir atrasado no puede apagar la descarga en paralelo.
    ///
    /// El paso de prefetch se salteaba entero mientras hubiera un branch
    /// backfill activo. Pero backfill no significa "bifurcacion": arranca cada
    /// vez que llega un bloque por encima de la cabeza local, o sea siempre
    /// que el nodo va atrasado. Resultado medido: cero prefetch, un pedido por
    /// bloque a ~50s sobre Tor, ~1,2 bloques/min contra los ~2 que produce la
    /// red. El nodo se atrasaba mas rapido de lo que avanzaba y no alcanzaba
    /// la punta nunca; uno estuvo 16 horas sin aplicar un bloque.
    #[test]
    fn a_backward_walk_no_longer_disables_parallel_prefetch() {
        // Caso real: cabeza 15641, el paseo bajando por 15720.
        let ventana = super::ibd_prefetch_window(15641, Some(15720), 15884, 8);
        assert_eq!(
            ventana,
            Some((15712, 15719)),
            "hay que adelantar las ocho alturas que el paseo pedira despues"
        );
    }

    #[test]
    fn each_network_listens_on_its_own_port_by_default() {
        // La PC corre Zenith y Quantic a la vez: con el mismo puerto el segundo
        // nodo no arrancaba sin --listen. Zenith sigue en el de siempre.
        use super::NetworkProfile;
        let ports = [
            NetworkProfile::Zenith.default_p2p_port(),
            NetworkProfile::QuanticMainnet.default_p2p_port(),
            NetworkProfile::QuanticTestnet.default_p2p_port(),
            NetworkProfile::QuanticRegtest.default_p2p_port(),
        ];
        assert_eq!(ports[0], 28080);
        let distinct: std::collections::HashSet<_> = ports.iter().collect();
        assert_eq!(distinct.len(), ports.len(), "{ports:?}");
    }

    #[cfg(feature = "quantic")]
    #[test]
    fn quantic_mainnet_is_its_own_network_with_its_own_manifest() {
        use super::NetworkProfile;
        let all = [
            NetworkProfile::Zenith,
            NetworkProfile::QuanticMainnet,
            NetworkProfile::QuanticTestnet,
            NetworkProfile::QuanticRegtest,
        ];
        for name in ["quantic-mainnet", "quantic", "Quantic-Mainnet"] {
            assert_eq!(NetworkProfile::from_name(name).unwrap(), NetworkProfile::QuanticMainnet, "{name}");
        }
        let mainnet = NetworkProfile::QuanticMainnet;
        assert!(mainnet.is_quantic());
        assert_eq!(mainnet.chain_id(), zoka::economics::QUANTIC_MAINNET_CHAIN_ID);
        assert_eq!(NetworkProfile::from_chain_id(mainnet.chain_id()).unwrap(), mainnet);
        // Nada se comparte entre redes: protocolo, tópicos, carpetas y manifiestos.
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a.protocol_id(), b.protocol_id());
                assert_ne!(a.topic_prefix(), b.topic_prefix());
                assert_ne!(a.local_dir(), b.local_dir());
                assert_ne!(a.default_network_config_path(), b.default_network_config_path());
                assert_ne!(a.label(), b.label());
            }
        }
        // El manifiesto embebido es el de la mainnet de Quantic y se lee.
        let manifest = super::parse_network_config_for(super::embedded_network_config_for(mainnet), mainnet)
            .expect("the embedded Quantic mainnet manifest parses");
        assert_eq!(manifest.chain_id, zoka::economics::QUANTIC_MAINNET_CHAIN_ID);
        assert_eq!(manifest.protocol_freeze_id, mainnet.protocol_id());
        // Sin génesis fijo no hay bloque embebido que importar.
        assert!(zoka::quantic::embedded_genesis_block(zoka::quantic::protocol::Network::Mainnet).is_none());
    }

    // El manifiesto de la testnet trae su primera semilla, la PC del equipo:
    // la misma onion y la misma identidad de iroh que tenía en Zenith (ZSilent
    // usa una sola clave para cada una), en el puerto P2P de la testnet. Sin
    // eso, quien instale ZSilent 3.0 arranca sin ningún par.
    // La misma frase restaurada dos veces con contraseñas distintas (pasó el
    // 22/10 con la wallet del equipo): por dirección se elige la que abre.
    #[cfg(feature = "quantic")]
    #[test]
    fn a_repeated_address_picks_the_wallet_that_opens_with_the_password() {
        use zoka::quantic::protocol::Network;
        use zoka::quantic::wallet_file;
        let dir = tempfile::tempdir().expect("tempdir");
        let (phrase, address) =
            wallet_file::create(dir.path(), "equipo", Network::Regtest, "olvidada").expect("create");
        let again = wallet_file::restore(dir.path(), "zsilent-2", Network::Regtest, phrase.as_str(), "nueva")
            .expect("restore");
        assert_eq!(again, address);
        let both = vec!["equipo".to_string(), "zsilent-2".to_string()];
        let pick = |password: Option<&str>| {
            super::pick_wallet_for_address(
                dir.path(),
                Network::Regtest,
                both.clone(),
                password.map(|p| zeroize::Zeroizing::new(p.to_string())),
            )
        };
        assert_eq!(pick(Some("nueva")).as_deref(), Some("zsilent-2"));
        assert_eq!(pick(Some("olvidada")).as_deref(), Some("equipo"));
        assert_eq!(pick(Some("otra")).as_deref(), Some("equipo"));
        assert_eq!(pick(None).as_deref(), Some("equipo"));
    }

    // ZSilent decide si puede lanzar el nodo leyendo `zoka --help`: con un flag
    // de menos en la ayuda, la 3.0.0 no arrancaba el nodo con la onion.
    #[cfg(feature = "quantic")]
    #[test]
    fn the_quantic_help_names_the_flags_zsilent_checks_before_starting_the_node() {
        let help = super::QUANTIC_FULLNODE_USAGE.join("\n");
        for flag in ["--tor-proxy", "--tor-only", "--external-addr"] {
            assert!(help.contains(flag), "{flag}");
        }
    }

    #[cfg(feature = "quantic")]
    #[test]
    fn the_quantic_testnet_manifest_carries_its_first_seed() {
        use super::NetworkProfile;
        let testnet = NetworkProfile::QuanticTestnet;
        let manifest = super::parse_network_config_for(super::embedded_network_config_for(testnet), testnet)
            .expect("the embedded Quantic testnet manifest parses");
        assert_eq!(manifest.chain_id, zoka::economics::QUANTIC_TESTNET_CHAIN_ID);
        assert_eq!(manifest.protocol_freeze_id, testnet.protocol_id());
        let seeds = manifest.active_seed_candidates(false);
        assert_eq!(seeds.len(), 1, "{:?}", seeds.iter().map(|s| &s.address).collect::<Vec<_>>());
        assert_eq!(seeds[0].transport, "tor");
        let port = format!(":{}", testnet.default_p2p_port());
        assert!(seeds[0].address.starts_with("/onion3/") && seeds[0].address.ends_with(&port), "{}", seeds[0].address);
        let iroh = manifest.iroh_snapshot_seed_ids();
        assert_eq!(iroh.len(), 1);
        assert_eq!(iroh[0].len(), 64);
    }

    #[test]
    fn the_prefetch_window_stops_at_the_local_head() {
        // El paseo esta a cuatro bloques de engancharse: no inventar alturas
        // por debajo de la cabeza local, que ya estan aplicadas.
        assert_eq!(
            super::ibd_prefetch_window(15641, Some(15645), 15884, 8),
            Some((15642, 15644))
        );
        // Ya enganchado: no queda nada por debajo que adelantar.
        assert_eq!(
            super::ibd_prefetch_window(15641, Some(15642), 15884, 8),
            None
        );
    }

    #[test]
    fn without_a_backward_walk_the_window_still_runs_forward() {
        assert_eq!(
            super::ibd_prefetch_window(15641, None, 15884, 8),
            Some((15642, 15649))
        );
        // Al dia: nada que bajar.
        assert_eq!(super::ibd_prefetch_window(15884, None, 15884, 8), None);
        // Cerca de la punta, la ventana se recorta y no la pasa.
        assert_eq!(
            super::ibd_prefetch_window(15881, None, 15884, 8),
            Some((15882, 15884))
        );
    }

    fn sync_status_for_signing(
        status: &str,
        mode: &str,
        ready_to_mine: bool,
        connected_peers: u64,
    ) -> zoka::api::SyncStatusSnapshot {
        zoka::api::SyncStatusSnapshot {
            status: status.to_string(),
            mode: mode.to_string(),
            ready_to_mine,
            local_height: 25_300,
            target_height: 25_300,
            lag: 0,
            connected_peers,
            min_peers: 0,
            startup_ready: true,
            startup_phase: "ready".to_string(),
            rpc_fallback_active: false,
            last_sync_source: None,
            last_sync_event: None,
            last_sync_error: None,
            last_block_at: 0,
            last_updated: 0,
            gossip_rx_block_response: 0,
            gossip_publish_err: 0,
        }
    }

    #[test]
    fn the_authority_only_signs_when_the_network_confirms_its_tip() {
        let signs = |status, mode, ready, peers| {
            super::checkpoint_signing_allowed(&sync_status_for_signing(status, mode, ready, peers))
        };
        assert!(signs("caught_up", "caught_up", true, 3));
        // Con `--min-mine-peers 0` y sin pares el nodo figura al dia; firmar
        // ahi ataria la red a la rama que la PC mina sola.
        assert!(!signs("caught_up", "caught_up", true, 0));
        // Pares conectados que todavia no anunciaron su punta.
        assert!(!signs("caught_up", "caught_up", false, 3));
        // Bajando una rama competidora: todavia no se sabe cual gana.
        assert!(!signs("caught_up", "backfill", true, 3));
        assert!(!signs("syncing", "syncing", false, 3));
        assert!(!signs("isolated", "isolated", false, 0));
    }

    #[test]
    fn only_the_checkpoint_authority_mines_without_peers() {
        // ZSilent pide `--min-mine-peers 1`; la autoridad lo ignora.
        assert_eq!(super::authority_min_mine_peers(1, true), 0);
        assert_eq!(super::authority_min_mine_peers(1, false), 1);
        assert_eq!(super::authority_min_mine_peers(0, false), 0);
    }

    #[test]
    fn the_authority_does_not_sign_over_a_branch_its_latest_checkpoint_corrects() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[9u8; 32]);
        let fixed = zoka::checkpoint_authority::SignedCheckpoint::sign(
            &key,
            "zoka-test",
            300,
            &"a".repeat(64),
        );
        let local = |hash: &'static str| move |height: u64| (height == 300).then(|| hash.to_string());
        // Sin checkpoint todavia: nada que contradecir.
        assert!(super::chain_contains_checkpoint(None, local("")));
        // La rama local lo contiene: sigue firmando.
        let same = "a".repeat(64);
        assert!(super::chain_contains_checkpoint(Some(&fixed), |_| Some(same.clone())));
        // Uno firmado a mano sobre otra rama: no firma hasta reorganizarse.
        assert!(!super::chain_contains_checkpoint(Some(&fixed), local("bbbb")));
        // Uno por encima de la punta local: tampoco.
        assert!(!super::chain_contains_checkpoint(Some(&fixed), |_| None));
    }
    use super::*;

    fn temp_db_arg() -> (tempfile::TempDir, String) {
        let tmp = tempfile::TempDir::new().expect("temp db dir");
        let db = tmp.path().join("chain.db");
        (tmp, db.to_string_lossy().to_string())
    }

    fn offer(
        chain_id: &str,
        head_height: u64,
        cumulative_work: u128,
        iroh: Option<&str>,
    ) -> zoka::network::snapshot_request_response::SnapshotManifest {
        zoka::network::snapshot_request_response::SnapshotManifest {
            chain_id: chain_id.to_string(),
            schema: zoka::snapshot::SNAPSHOT_SCHEMA_ID.to_string(),
            head_height,
            head_hash: format!("hash-{head_height}"),
            cumulative_work,
            block_count: head_height,
            generated_unix_secs: 0,
            size_bytes: 0,
            sha256_hex: String::new(),
            download_urls: Vec::new(),
            iroh_endpoint_id: iroh.map(|s| s.to_string()),
            signature_hex: None,
            signer_pubkey_hex: None,
        }
    }

    #[test]
    fn pending_private_txs_are_offered_for_regossip_and_confirmed_ones_are_not() {
        let (_tmp, db) = temp_db_arg();
        let storage = open_storage(&db).expect("storage");

        // Dos pendientes con cuerpo, una pendiente sin cuerpo y una que no
        // esta pendiente. Solo las dos primeras se pueden reenviar.
        storage
            .save_private_transaction_bytes("aaa", b"{\"tx_hash\":\"aaa\"}")
            .expect("body aaa");
        storage
            .save_pending_private_transaction_hash("aaa")
            .expect("pending aaa");
        storage
            .save_private_transaction_bytes("bbb", b"{\"tx_hash\":\"bbb\"}")
            .expect("body bbb");
        storage
            .save_pending_private_transaction_hash("bbb")
            .expect("pending bbb");
        storage
            .save_pending_private_transaction_hash("ccc")
            .expect("pending sin cuerpo");
        storage
            .save_private_transaction_bytes("ddd", b"{\"tx_hash\":\"ddd\"}")
            .expect("body ddd");

        let picked = pending_private_tx_bodies_to_regossip(&storage, 16);
        let hashes: Vec<&str> = picked.iter().map(|(h, _)| h.as_str()).collect();
        assert!(hashes.contains(&"aaa"));
        assert!(hashes.contains(&"bbb"));
        assert!(
            !hashes.contains(&"ccc"),
            "sin cuerpo no hay nada que reenviar"
        );
        assert!(
            !hashes.contains(&"ddd"),
            "una que no esta pendiente no se reenvia"
        );

        // El lote acota el gasto por ronda: con una red lenta, reenviar todo
        // de golpe compite con el propio trafico de bloques.
        assert_eq!(pending_private_tx_bodies_to_regossip(&storage, 1).len(), 1);

        // Y al confirmarse deja de ofrecerse.
        storage
            .delete_pending_private_transaction_hash("aaa")
            .expect("confirmar aaa");
        let after = pending_private_tx_bodies_to_regossip(&storage, 16);
        assert!(!after.iter().any(|(h, _)| h == "aaa"));
    }

    #[test]
    fn the_mining_diagnostic_rotates_instead_of_growing_forever() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = dir.path().join("mining_diagnostic.log");

        // Por debajo del techo no se toca: rotar de más tira las líneas que
        // justamente se quieren leer.
        fs::write(&path, vec![b'x'; 16]).expect("small log");
        rotate_mining_log_if_oversized(&path);
        assert_eq!(fs::metadata(&path).expect("still there").len(), 16);
        assert!(!dir.path().join("mining_diagnostic.log.1").exists());

        fs::write(&path, vec![b'x'; MINING_LOG_MAX_BYTES as usize]).expect("oversized log");
        rotate_mining_log_if_oversized(&path);
        assert!(
            !path.exists(),
            "tras rotar, el activo arranca de cero en la proxima escritura"
        );
        assert_eq!(
            fs::metadata(dir.path().join("mining_diagnostic.log.1"))
                .expect("rotado")
                .len(),
            MINING_LOG_MAX_BYTES
        );

        // Una segunda rotación pisa la copia vieja en vez de acumular copias.
        fs::write(&path, vec![b'y'; MINING_LOG_MAX_BYTES as usize]).expect("oversized again");
        rotate_mining_log_if_oversized(&path);
        let rotated = fs::read(dir.path().join("mining_diagnostic.log.1")).expect("rotado");
        assert!(rotated.iter().all(|b| *b == b'y'));
        let logs: Vec<String> = fs::read_dir(dir.path())
            .expect("read dir")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(
            logs.len(),
            1,
            "solo la copia rotada deberia quedar: {logs:?}"
        );
    }

    #[test]
    fn atomic_write_replaces_an_existing_file_and_leaves_no_temp() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = dir.path().join("user_mining_state.json");

        write_file_atomically(&path, b"{\"total_rewards\":1}").expect("first write");
        assert_eq!(fs::read_to_string(&path).unwrap(), "{\"total_rewards\":1}");

        // Sobrescribir es el caso real y no el de estreno: en Windows el rename
        // tiene que reemplazar un destino que ya existe, no fallar.
        write_file_atomically(&path, b"{\"total_rewards\":2}").expect("overwrite");
        assert_eq!(fs::read_to_string(&path).unwrap(), "{\"total_rewards\":2}");

        let leftovers: Vec<String> = fs::read_dir(dir.path())
            .expect("read temp dir")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "quedaron temporales: {leftovers:?}");
    }

    #[test]
    fn a_zero_filled_state_file_is_rejected_rather_than_read_as_state() {
        // La firma exacta de la corrupcion por reinicio: el archivo conserva su
        // largo pero los datos nunca llegaron al disco. Si esto alguna vez
        // parsea, el nodo arranca con un estado inventado en vez de rehacerlo.
        let zeros = "\u{0}".repeat(64);
        assert!(serde_json::from_str::<UserMiningState>(&zeros).is_err());

        // Y lo que sí guardamos tiene que poder releerse.
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = dir.path().join("user_mining_state.json");
        let mut state = UserMiningState::default();
        state.total_rewards = 12;
        state.mode = "pow".to_string();
        let raw = serde_json::to_string_pretty(&state).expect("encode");
        write_file_atomically(&path, raw.as_bytes()).expect("write");

        let back: UserMiningState =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).expect("reload");
        assert_eq!(back.total_rewards, 12);
        assert_eq!(back.mode, "pow");
    }

    /// El objetivo de vecinos no depende de cuantas semillas haya.
    ///
    /// Antes salia de ahi y quedaba topado en 3, asi que con una sola semilla
    /// activa el nodo dejaba de marcar al conseguir UN par -- aunque tuviera
    /// decenas de direcciones descubiertas sin usar. A quien tiene el puerto
    /// abierto no le afecta, porque junta vecinos por conexiones entrantes; a
    /// quien esta detras de un router que no controla, salir a buscar es la
    /// unica via y era justo lo que se apagaba.
    /// Remarcar a un par ya conectado no suma un vecino: el marcado funciona
    /// al instante porque la conexion existe, la cuenta no se mueve, la espera
    /// vence y se anota como fallo. Con dos intentos por barrido, un solo par
    /// conectado se lleva media ronda para siempre y las direcciones a las que
    /// todavia NO estamos conectados no reciben ninguno.
    ///
    /// Es el estado en que un nodo se queda clavado en un vecino y no sube
    /// nunca, por mas que el objetivo sea seis y tenga direcciones sanas
    /// guardadas.
    #[test]
    fn an_already_connected_peer_never_consumes_a_dial_attempt() {
        let conectado = "/onion3/pwhjqtjv:28080".to_string();
        let libre_a = "/onion3/qh6t3c7u:28080".to_string();
        let libre_b = "/onion3/qckkck62:28080".to_string();

        let orden = ranked_dial_candidates(
            &[conectado.clone(), libre_a.clone(), libre_b.clone()],
            &[conectado.clone()],
            &[],
        );

        assert!(
            !orden.contains(&conectado),
            "no se remarca al que ya esta conectado: {orden:?}"
        );
        assert_eq!(
            orden.len(),
            2,
            "los dos intentos van a direcciones sin conectar: {orden:?}"
        );
    }

    /// El peerstore ya sabe cuales direcciones sirven: las ordena por score,
    /// ultima conexion exitosa y fallos. Ese orden se estaba tirando al barajar
    /// la lista entera, asi que una semilla muerta salia sorteada tan seguido
    /// como un par sano -- y con dos intentos por barrido, eso es media ronda
    /// regalada.
    #[test]
    fn a_dead_address_is_tried_after_the_healthy_ones() {
        let sana_a = "/onion3/aaa:28080".to_string();
        let sana_b = "/onion3/bbb:28080".to_string();
        let muerta = "/dns4/seed-muerta.example/tcp/28080".to_string();
        // Tal como lo devuelve dial_addrs: de mas sana a menos.
        let por_salud = vec![sana_a.clone(), sana_b.clone(), muerta.clone()];

        let orden = ranked_dial_candidates(
            &[muerta.clone(), sana_a.clone(), sana_b.clone()],
            &[],
            &por_salud,
        );

        assert_eq!(orden.len(), 3);
        assert_eq!(
            orden.last(),
            Some(&muerta),
            "la direccion con peor salud tiene que quedar ultima: {orden:?}"
        );
    }

    /// Una direccion recien agregada a mano no esta en el peerstore. No se la
    /// premia sobre un par probado, pero tampoco se la castiga como a uno que
    /// ya fallo: va al medio.
    #[test]
    fn an_unknown_address_outranks_a_failing_one() {
        let sana = "/onion3/aaa:28080".to_string();
        let muerta = "/onion3/zzz:28080".to_string();
        let nueva = "/onion3/nueva:28080".to_string();
        let por_salud = vec![
            sana.clone(),
            "/onion3/bbb:28080".to_string(),
            muerta.clone(),
        ];

        let orden = ranked_dial_candidates(
            &[muerta.clone(), nueva.clone(), sana.clone()],
            &[],
            &por_salud,
        );

        let pos = |a: &String| orden.iter().position(|x| x == a).unwrap();
        assert!(pos(&sana) < pos(&nueva), "la probada va antes: {orden:?}");
        assert!(
            pos(&nueva) < pos(&muerta),
            "la nueva va antes que la muerta: {orden:?}"
        );
    }

    /// Los comandos de equipo hablan con el nodo de ESTA maquina por defecto.
    ///
    /// Crear un equipo o unirse compromete la direccion de cobro de quien
    /// ejecuta el comando: apuntar sin querer a un nodo ajeno armaria el equipo
    /// con la direccion del otro, y el error no se notaria hasta que las
    /// recompensas llegaran a la persona equivocada.
    #[test]
    fn team_commands_talk_to_this_machine_unless_told_otherwise() {
        assert_eq!(team_rpc_url(&[]), "http://127.0.0.1:3000");
        assert_eq!(
            team_rpc_url(&["--rpc".to_string(), "http://127.0.0.1:4000".to_string()]),
            "http://127.0.0.1:4000"
        );
    }

    /// Unirse sin codigo tiene que fallar con un mensaje que diga que falta,
    /// no mandar una peticion vacia que el nodo rechace con algo generico.
    #[test]
    fn joining_without_a_code_says_what_is_missing() {
        let err = cmd_team_join(&[]).unwrap_err();
        assert!(err.contains("--code"), "mensaje poco util: {err}");
        let vacio = cmd_team_join(&["--code".to_string(), "   ".to_string()]).unwrap_err();
        assert!(
            vacio.contains("--code"),
            "un codigo en blanco es no tener codigo: {vacio}"
        );
    }

    /// Marcar la direccion propia falla siempre con "tried to dial local peer
    /// id", y con dos intentos por barrido se lleva media ronda sin ninguna
    /// posibilidad. Un nodo con onion publicada la tiene en su propio
    /// manifiesto, asi que el caso es la norma.
    #[test]
    fn dialing_never_wastes_an_attempt_on_our_own_address() {
        let propia = "/onion3/d5pk3nhesk5wlvz4f5unbhcozxgtj7u6mhagauyprjrxndzpwbaoeiid:28080";
        let ajena = "/onion3/qckkck62hvizbwuh2w2ci7xvvacfy2bbqxiosrhyhwjbbsuxkr25ffyd:28080";
        let orden = shuffled_peer_addrs_excluding(
            &[propia.to_string(), ajena.to_string()],
            &[propia.to_string()],
        );
        assert_eq!(orden, vec![ajena.to_string()]);
    }

    /// La misma onion puede venir del manifiesto y de la linea de comandos con
    /// distinto sufijo; sin normalizar, el filtro no la reconoceria.
    #[test]
    fn our_own_address_is_recognised_across_suffix_differences() {
        assert!(direcciones_equivalentes(
            "/onion3/abc:28080",
            "/onion3/abc:28080/p2p/12D3KooWfoo",
        ));
        assert!(direcciones_equivalentes(
            "/onion3/ABC:28080/",
            "/onion3/abc:28080"
        ));
        assert!(!direcciones_equivalentes(
            "/onion3/abc:28080",
            "/onion3/xyz:28080"
        ));
        assert!(!direcciones_equivalentes("", "/onion3/abc:28080"));
    }

    #[test]
    fn peer_target_does_not_shrink_to_the_number_of_seeds() {
        // Una sola semilla y muchas direcciones conocidas: se apunta a una
        // malla sana, no a uno.
        assert_eq!(seed_connection_target(1, 32), HEALTHY_PEER_TARGET);
        assert_eq!(seed_connection_target(3, 32), HEALTHY_PEER_TARGET);
    }

    /// Pero no se promete mas de lo que se puede marcar: apuntar a seis con
    /// dos direcciones en el pozo solo produciria reintentos contra las mismas.
    #[test]
    fn peer_target_never_exceeds_the_addresses_actually_known() {
        assert_eq!(seed_connection_target(1, 2), 2);
        assert_eq!(seed_connection_target(5, 1), 1);
        assert_eq!(seed_connection_target(1, 0), 0);
    }

    #[test]
    fn snapshot_offer_selection_prefers_most_cumulative_work_not_height() {
        // A peer can cheaply mine a long low-difficulty branch, so height is
        // not a safe ranking signal; cumulative work is what consensus uses.
        let chosen = choose_best_snapshot_offer(
            vec![
                offer("zoka", 90_000, 10, Some("tall-but-light")),
                offer("zoka", 20_000, 999, Some("short-but-heavy")),
            ],
            "zoka",
            0,
        )
        .expect("an offer should be chosen");
        assert_eq!(chosen.iroh_endpoint_id.as_deref(), Some("short-but-heavy"));
    }

    #[test]
    fn snapshot_offer_selection_skips_peers_without_an_iroh_id() {
        // Pre-1.8.1 peers advertise no endpoint, so there is nothing to pull
        // from even when they claim the heaviest chain.
        assert!(
            choose_best_snapshot_offer(vec![offer("zoka", 90_000, 999, None)], "zoka", 0).is_none()
        );
    }

    #[test]
    fn snapshot_offer_selection_ignores_other_chains() {
        assert!(choose_best_snapshot_offer(
            vec![offer("some-other-chain", 90_000, 999, Some("id"))],
            "zoka",
            0
        )
        .is_none());
    }

    #[test]
    fn snapshot_offer_selection_ignores_offers_within_normal_sync_range() {
        // A small gap is what ordinary block-by-block sync is for.
        let local = 10_000u64;
        assert!(choose_best_snapshot_offer(
            vec![offer(
                "zoka",
                local + PEER_SNAPSHOT_PULL_MIN_GAP,
                999,
                Some("id")
            )],
            "zoka",
            local
        )
        .is_none());
    }

    #[test]
    fn snapshot_offer_is_declined_when_incremental_sync_is_cheaper() {
        // Observed on mainnet: a node at 9300 with the network at 10377. The
        // gap clears the absolute floor, but a snapshot ships the entire chain
        // (~143 MB at ~14 KB/block) to deliver ~1 000 blocks that incremental
        // sync fetches in ~15 MB. Taking the snapshot here would be a ten-fold
        // waste, so the node must decline and keep syncing normally.
        assert!(choose_best_snapshot_offer(
            vec![offer("zoka", 10_377, 999, Some("id"))],
            "zoka",
            9_300
        )
        .is_none());
    }

    #[test]
    fn snapshot_offer_is_taken_when_most_of_the_chain_is_missing() {
        // The case this feature exists for: a fresh (or nearly fresh) node
        // that would otherwise crawl the whole chain block by block, and which
        // must be able to bootstrap from any peer rather than needing a seed.
        assert!(choose_best_snapshot_offer(
            vec![offer("zoka", 10_377, 999, Some("id"))],
            "zoka",
            0
        )
        .is_some());
        assert!(choose_best_snapshot_offer(
            vec![offer("zoka", 10_377, 999, Some("id"))],
            "zoka",
            4_000
        )
        .is_some());
    }

    #[test]
    fn seed_address_for_peer_appends_p2p_suffix_once() {
        let peer_id = "12D3KooWSeedPeer";
        assert_eq!(
            seed_address_for_peer("/onion3/exampleaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:28080", peer_id),
            "/onion3/exampleaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:28080/p2p/12D3KooWSeedPeer"
        );
        assert_eq!(
            seed_address_for_peer("/ip4/127.0.0.1/tcp/28080/p2p/12D3KooWSeedPeer", peer_id),
            "/ip4/127.0.0.1/tcp/28080/p2p/12D3KooWSeedPeer"
        );
    }

    #[test]
    fn seed_addresses_for_peer_deduplicates_external_and_listen_addrs() {
        let peer_id = "12D3KooWSeedPeer";
        let addrs = vec![
            "/ip4/127.0.0.1/tcp/28080".to_string(),
            "/ip4/127.0.0.1/tcp/28080/".to_string(),
            "/dns4/seed.example/tcp/28080/p2p/12D3KooWSeedPeer".to_string(),
        ];
        assert_eq!(
            seed_addresses_for_peer(&addrs, peer_id),
            vec![
                "/ip4/127.0.0.1/tcp/28080/p2p/12D3KooWSeedPeer".to_string(),
                "/dns4/seed.example/tcp/28080/p2p/12D3KooWSeedPeer".to_string(),
            ]
        );
    }

    fn save_test_coinbase_block(
        storage: &RocksDbStorage,
        height: u64,
        amount: u64,
        prev_hash: String,
    ) -> String {
        let coinbase = zoka::mining::block_validator::build_coinbase_tx(
            "zka1testminer".to_string(),
            amount,
            height,
            1_780_000_000 + height,
        );
        let block = zoka::types::Block::new(
            prev_hash,
            1_780_000_000 + height,
            0,
            height,
            vec![coinbase],
            zoka::types::PublicKey(Vec::new()),
            zoka::types::Signature(Vec::new()),
        );
        let hash = block.header.hash.clone();
        storage.save_block(&block).expect("save test block");
        storage
            .set_chain_height(height)
            .expect("set test chain height");
        storage
            .set_last_block_hash(&hash)
            .expect("set test last hash");
        hash
    }

    #[test]
    fn a_manual_checkpoint_is_only_signed_by_the_authority_and_well_formed() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let key_path = dir.path().join("checkpoint-authority.key");
        let public = zoka::checkpoint_authority::generate_signing_key(&key_path).expect("keygen");
        let authority: Option<zoka::checkpoint_authority::AuthorityKeys> =
            zoka::checkpoint_authority::parse_public_key(&public).map(Into::into);
        let chain = "zoka-test-manual-checkpoint";
        let hash = "AB".repeat(32);

        let checkpoints =
            sign_manual_checkpoint(&key_path, chain, authority.clone(), 120, &hash).expect("signs");
        assert_eq!(checkpoints.len(), 1, "una cadena sin clave ML-DSA: solo la versión 1");
        let checkpoint = &checkpoints[0];
        assert_eq!(checkpoint.block_hash, "ab".repeat(32), "hash normalizado");
        checkpoint
            .verify(authority.as_ref().unwrap())
            .expect("verifies with the authority key");

        // Una clave que no es la compilada firmaría algo que todos rechazan.
        let other = Some(ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]).verifying_key().into());
        assert!(sign_manual_checkpoint(&key_path, chain, other, 120, &hash).is_err());
        // Sin autoridad compilada, sin clave, con altura 0 o un hash que no es hex.
        assert!(sign_manual_checkpoint(&key_path, chain, None, 120, &hash).is_err());
        let missing = dir.path().join("no-existe.key");
        assert!(sign_manual_checkpoint(&missing, chain, authority.clone(), 120, &hash).is_err());
        assert!(sign_manual_checkpoint(&key_path, chain, authority.clone(), 0, &hash).is_err());
        assert!(sign_manual_checkpoint(&key_path, chain, authority, 120, "xyz").is_err());
    }

    #[cfg(feature = "pq-experimental")]
    #[test]
    fn with_the_ml_dsa_half_the_authority_signs_hybrid_checkpoints() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let key_path = dir.path().join(zoka::checkpoint_authority::AUTHORITY_KEY_FILE);
        let mldsa_path = dir.path().join(zoka::checkpoint_authority::AUTHORITY_MLDSA_KEY_FILE);
        let public = zoka::checkpoint_authority::generate_signing_key(&key_path).expect("keygen");
        let ed25519 = zoka::checkpoint_authority::parse_public_key(&public).unwrap();
        let chain = "zoka-test-hybrid-checkpoint";
        let hash = "cd".repeat(32);

        // La cadena exige la versión 2 y falta la mitad ML-DSA: no firma nada.
        let other_mldsa = zoka::pq::MlDsa65SecretKey::from_seed(&[3u8; 32]).unwrap().public_key();
        let wrong = zoka::checkpoint_authority::AuthorityKeys {
            ed25519,
            mldsa: Some(other_mldsa),
        };
        assert!(sign_manual_checkpoint(&key_path, chain, Some(wrong.clone()), 120, &hash).is_err());

        let mldsa_public = zoka::hybrid_signature::generate_mldsa_seed(&mldsa_path).expect("mldsa keygen");
        // Con otra clave ML-DSA compilada tampoco: los nodos la rechazarían.
        assert!(sign_manual_checkpoint(&key_path, chain, Some(wrong), 120, &hash).is_err());
        let hybrid = zoka::checkpoint_authority::AuthorityKeys {
            ed25519,
            mldsa: zoka::checkpoint_authority::parse_mldsa_public_key(&mldsa_public),
        };
        let checkpoints = sign_manual_checkpoint(&key_path, chain, Some(hybrid.clone()), 120, &hash)
            .expect("signs both halves");
        assert_eq!(checkpoints.len(), 1, "fuera de Zenith no hace falta la versión 1");
        assert_eq!(checkpoints[0].version, zoka::checkpoint_authority::SIGNED_CHECKPOINT_HYBRID_VERSION);
        checkpoints[0].verify(&hybrid).expect("hybrid verifies");

        // En Zenith publica también la versión 1, para los nodos viejos.
        let zenith = sign_manual_checkpoint(&key_path, MAINNET_CHAIN_ID, Some(hybrid.clone()), 120, &hash)
            .expect("signs for zenith");
        assert_eq!(zenith.len(), 2);
        assert_eq!(zenith[1].version, zoka::checkpoint_authority::SIGNED_CHECKPOINT_VERSION);
        zenith[1].verify(&ed25519.into()).expect("v1 for old nodes");
        assert_eq!(
            zenith[1].verify(&hybrid),
            Err(zoka::checkpoint_authority::CheckpointError::HybridRequired)
        );
    }

    #[test]
    fn writing_from_a_stale_copy_no_longer_erases_a_send_reservation() {
        let tmp = tempfile::TempDir::new().expect("temp wallet dir");
        let storage = WalletStorage::from_base_dir(tmp.path()).expect("wallet storage");
        let mut manager = PrivateWalletManager::new();
        manager.create_private_wallet("minero").expect("create");
        manager
            .deposit_to_private_wallet("minero", 5)
            .expect("first note");
        persist_all_private_wallets(&storage, &manager, None).expect("persist");

        // El minero lee al empezar la tanda...
        let stale = read_private_wallet_manager(&storage, None).expect("stale copy");
        // ...y en el medio un envío, desde otro proceso, reserva la nota y guarda.
        let mut sender = read_private_wallet_manager(&storage, None).expect("sender copy");
        let note = &mut sender.get_private_wallet_mut("minero").unwrap().private_notes[0];
        note.reserved = true;
        note.reserved_by_tx = Some("envio".to_string());
        persist_all_private_wallets(&storage, &sender, None).expect("send persists");

        // Al cerrar la tanda acredita una recompensa madura. Antes lo hacía
        // sobre `stale` y la guardaba: la nota del envío volvía a estar libre.
        let ((), _) = with_fresh_private_wallets(&storage, None, |fresh| {
            fresh
                .deposit_to_private_wallet("minero", 7)
                .map_err(|e| e.to_string())?;
            persist_all_private_wallets(&storage, fresh, None)
        })
        .expect("settle on the copy on disk");
        drop(stale);

        let on_disk = read_private_wallet_manager(&storage, None).expect("reload");
        let wallet = on_disk.get_private_wallet("minero").unwrap();
        assert_eq!(wallet.private_notes.len(), 2, "entra la recompensa");
        assert!(
            wallet
                .private_notes
                .iter()
                .any(|note| note.reserved_by_tx.as_deref() == Some("envio")),
            "y la reserva del envío sigue en pie"
        );
        assert_eq!(wallet.private_balance, 7, "solo la nota nueva queda disponible");
    }

    #[test]
    fn advance_private_wallet_restore_height_skips_already_scanned_blocks() {
        let tmp = tempfile::TempDir::new().expect("temp wallet dir");
        let storage = WalletStorage::from_base_dir(tmp.path()).expect("wallet storage");
        let mut manager = PrivateWalletManager::new();
        let wallet_name = "rescan-test";
        manager
            .create_private_wallet(wallet_name)
            .expect("create private wallet");

        // A brand-new wallet has restore_height 0; the first sync should not
        // advance past a chain it hasn't actually scanned.
        assert_eq!(
            manager
                .get_private_wallet(wallet_name)
                .unwrap()
                .restore_height,
            0
        );

        advance_private_wallet_restore_height(&storage, &mut manager, wallet_name, None, 100)
            .expect("advance after first scan");
        assert_eq!(
            manager
                .get_private_wallet(wallet_name)
                .unwrap()
                .restore_height,
            101
        );

        // A later sync that only covered up to height 150 should push the
        // checkpoint forward again — the next unlock only rescans 101..=150.
        advance_private_wallet_restore_height(&storage, &mut manager, wallet_name, None, 150)
            .expect("advance after second scan");
        assert_eq!(
            manager
                .get_private_wallet(wallet_name)
                .unwrap()
                .restore_height,
            151
        );

        // A stale/concurrent call reporting a lower height than the wallet's
        // current checkpoint must never move the checkpoint backwards.
        advance_private_wallet_restore_height(&storage, &mut manager, wallet_name, None, 120)
            .expect("stale advance is a no-op");
        assert_eq!(
            manager
                .get_private_wallet(wallet_name)
                .unwrap()
                .restore_height,
            151
        );

        // The checkpoint must survive a reload from disk, otherwise every
        // unlock would still rescan from genesis.
        let reloaded =
            load_private_wallet_manager_for_network(&storage, None, NetworkProfile::Zenith)
                .expect("reload manager from disk");
        assert_eq!(
            reloaded
                .get_private_wallet(wallet_name)
                .unwrap()
                .restore_height,
            151
        );
    }

    fn make_valid_zka1() -> String {
        let scan = [0x11u8; 32];
        let spend = [0x22u8; 32];
        let zpriv = format!("zpriv:{}:{}", hex::encode(scan), hex::encode(spend));
        user_private_address_from_zpriv(&zpriv).expect("encode zka1")
    }

    #[test]
    fn smart_fee_estimator_allows_zero_fee_on_quiet_network() {
        let (_tmp, db) = temp_db_arg();
        let args = vec![
            "--amount".to_string(),
            "1".to_string(),
            "--db".to_string(),
            db,
        ];

        assert_eq!(resolve_fee(&args).expect("fee estimate"), 0);
    }

    #[test]
    fn smart_fee_estimator_keeps_zero_fee_for_small_pending_load() {
        let (_tmp, db) = temp_db_arg();
        let storage = RocksDbStorage::new(&db).expect("temp rocksdb");
        storage
            .save_pending_private_transaction_hash("pending-a")
            .expect("save pending tx");
        drop(storage);
        let args = vec![
            "--amount".to_string(),
            "1".to_string(),
            "--db".to_string(),
            db,
        ];

        assert_eq!(resolve_fee(&args).expect("fee estimate"), 0);
    }

    #[test]
    fn smart_fee_estimator_raises_fee_when_real_pending_load_exists() {
        let (_tmp, db) = temp_db_arg();
        let storage = RocksDbStorage::new(&db).expect("temp rocksdb");
        for i in 0..11 {
            storage
                .save_pending_private_transaction_hash(&format!("pending-{i}"))
                .expect("save pending tx");
        }
        drop(storage);
        let args = vec![
            "--amount".to_string(),
            "1".to_string(),
            "--db".to_string(),
            db,
        ];

        assert_eq!(
            resolve_fee(&args).expect("fee estimate"),
            FEE_MARKET_STEP_ATOMS
        );
    }

    #[test]
    fn explicit_fee_accepts_fractional_zka_units() {
        let args = vec!["--fee".to_string(), "0.0001".to_string()];

        assert_eq!(
            resolve_fee(&args).expect("fractional fee"),
            FEE_MARKET_STEP_ATOMS
        );
    }

    #[test]
    fn mining_power_presets_map_to_cpu_threads() {
        let available = default_mining_threads();

        assert_eq!(mining_threads_for_power_preset("low").unwrap(), 1);
        assert_eq!(
            mining_threads_for_power_preset("normal").unwrap(),
            (available / 2).max(1)
        );
        assert_eq!(mining_threads_for_power_preset("max").unwrap(), available);
    }

    #[test]
    fn explicit_mining_threads_override_power_preset() {
        let args = vec![
            "--power".to_string(),
            "low".to_string(),
            "--threads".to_string(),
            "3".to_string(),
        ];

        assert_eq!(parse_thread_count_arg(&args).unwrap(), 3);
    }

    #[test]
    fn team_share_worker_solves_the_exact_coordinator_template() {
        let address = make_valid_zka1();
        let mut team = zoka::team_mining::TeamState::create(address.clone(), now_secs()).unwrap();
        let template = team
            .set_template(
                "zoka-dev-team".to_string(),
                "parent-hash".to_string(),
                1_000_000,
                now_secs(),
                1,
            )
            .unwrap();
        let difficulty = team.members.get(&address).unwrap().share_difficulty_bits;
        let solved = solve_team_share(
            template.clone(),
            address,
            difficulty,
            2,
            Arc::new(AtomicBool::new(false)),
        )
        .expect("team share");

        let validated = team.validate_share(&solved.share).expect("valid share");
        assert!(validated.actual_difficulty_bits >= difficulty);
        assert!(solved.hashes_attempted > 0);

        let solution = zoka::team_mining::NetworkSolution {
            share: solved.share,
            template: template.clone(),
        };
        assert_eq!(
            verify_team_solution_for_candidate(
                &solution,
                &template.chain_id,
                template.height,
                &template.prev_hash,
                template.network_difficulty_bits,
            )
            .unwrap(),
            hex::decode(&solution.share.pow_hash_hex).unwrap()
        );
        assert!(verify_team_solution_for_candidate(
            &solution,
            &template.chain_id,
            template.height,
            "different-parent",
            template.network_difficulty_bits,
        )
        .is_err());
    }

    #[test]
    fn team_recipient_can_scan_fee_claims_from_the_block() {
        let mut manager = PrivateWalletManager::new();
        manager.create_private_wallet("sender").unwrap();
        let receiver = manager.create_private_wallet("receiver").unwrap();
        let team_recipient = manager.create_private_wallet("team-recipient").unwrap();
        manager.deposit_to_private_wallet("sender", 40).unwrap();
        let tx_hash = manager
            .send_private_transaction_with_fee("sender", receiver, 10, 3)
            .unwrap();
        let claims = manager
            .create_local_private_fee_claims_for_block(
                std::slice::from_ref(&tx_hash),
                &team_recipient,
            )
            .unwrap();
        let mut block = zoka::types::Block::new(
            "parent".to_string(),
            100,
            0,
            1,
            Vec::new(),
            zoka::types::PublicKey(Vec::new()),
            zoka::types::Signature(Vec::new()),
        );
        block.private_fee_claims = claims;
        block.private_tx_bodies =
            vec![serde_json::to_vec(&manager.get_private_transaction(&tx_hash).unwrap()).unwrap()];

        assert_eq!(
            claim_private_fee_rewards_from_block(&mut manager, "receiver", &block).unwrap(),
            0
        );
        assert_eq!(
            claim_private_fee_rewards_from_block(&mut manager, "team-recipient", &block).unwrap(),
            1
        );
        assert_eq!(manager.get_private_balance("team-recipient"), Some(3));
        assert_eq!(
            claim_private_fee_rewards_from_block(&mut manager, "team-recipient", &block).unwrap(),
            0
        );
    }

    #[test]
    fn atomic_economics_marker_initializes_empty_db() {
        let (_tmp, db) = temp_db_arg();
        let storage = RocksDbStorage::new(&db).expect("temp rocksdb");

        ensure_atomic_economics_db(&storage).expect("empty db should be marked");
        let raw = storage
            .get_raw(CHAIN_ECONOMICS_VERSION_KEY)
            .expect("read marker")
            .expect("marker exists");
        assert_eq!(
            String::from_utf8(raw).unwrap(),
            economics::ECONOMICS_VERSION
        );
    }

    /// El orden que arregla al usuario nuevo: sellar mientras la base esta
    /// vacia, y recien despues llenarla de bloques.
    ///
    /// Es la unica secuencia que funciona. Al reves -- sincronizar primero y
    /// sellar despues -- cae en el rechazo del test siguiente, y eso es lo que
    /// le pasaba a cualquiera que instalara ZSilent: ni el IBD ni el import de
    /// snapshot escriben el marcador, y el genesis (que si lo escribia) no se
    /// aplica porque el instalador no trae el archivo.
    #[test]
    fn a_db_marked_while_empty_stays_valid_once_blocks_arrive() {
        let (_tmp, db) = temp_db_arg();
        let storage = RocksDbStorage::new(&db).expect("temp rocksdb");

        ensure_atomic_economics_db(&storage).expect("empty db should be marked");
        storage.set_chain_height(1).expect("set height");

        ensure_atomic_economics_db(&storage)
            .expect("una base sellada de vacia sigue valiendo cuando llegan bloques");
    }

    #[test]
    fn atomic_economics_marker_rejects_unmarked_nonempty_db() {
        let (_tmp, db) = temp_db_arg();
        let storage = RocksDbStorage::new(&db).expect("temp rocksdb");
        storage.set_chain_height(1).expect("set height");

        let err = ensure_atomic_economics_db(&storage).unwrap_err();
        assert!(err.contains("no atomic economics marker"));
    }

    // Desde el 21/10/2026 las dos redes son las de Quantic: "testnet" y
    // "mainnet" las nombran a ellas, y Zenith no se abre. Una build sin
    // `quantic` sigue siendo la de Zenith (para las pruebas de su código).
    #[cfg(feature = "quantic")]
    #[test]
    fn the_two_networks_are_quantic_and_zenith_no_longer_opens() {
        for name in ["testnet", "TESTNET", "quantic-testnet"] {
            assert_eq!(NetworkProfile::from_name(name).expect(name), NetworkProfile::QuanticTestnet);
        }
        for name in ["mainnet", " Mainnet ", "quantic-mainnet", "quantic"] {
            assert_eq!(NetworkProfile::from_name(name).expect(name), NetworkProfile::QuanticMainnet);
        }
        let err = NetworkProfile::from_name("zenith").unwrap_err();
        assert!(err.contains("retired"), "{err}");
        assert_eq!(NetworkProfile::QuanticTestnet.display_name(), "Zoka Quantic Testnet");
        assert_eq!(NetworkProfile::QuanticMainnet.display_name(), "Zoka Quantic Mainnet");
        if std::env::var_os("ZOKA_NETWORK").is_none() {
            assert_eq!(NetworkProfile::from_args(&[]).expect("default"), NetworkProfile::QuanticTestnet);
        }
    }

    // Zenith conserva sus rutas de siempre (sus datos quedan donde estaban).
    #[test]
    fn every_network_name_opens_zenith_with_its_historical_paths() {
        #[cfg(not(feature = "quantic"))]
        for name in ["testnet", "TESTNET", "zenith", "mainnet", " Mainnet "] {
            assert_eq!(
                NetworkProfile::from_name(name).expect(name),
                NetworkProfile::Zenith
            );
        }
        let zenith = NetworkProfile::Zenith;
        assert_eq!(zenith.chain_id(), "zoka-mainnet-zenith-v1");
        assert_eq!(zenith.local_dir(), ".zoka/mainnet");
        assert_eq!(zenith.default_db_path(), ".zoka/mainnet/blockchain.db");
        assert_eq!(
            zenith.default_genesis_path(),
            "docs/genesis.mainnet.v1.json"
        );
        assert_eq!(
            zenith.default_network_config_path(),
            "docs/mainnet-network.v1.json"
        );
        assert_eq!(zenith.network_config_schema(), "zoka-mainnet-network-v1");
        assert_eq!(zenith.mining_mode(), "pow-mainnet");
        assert_eq!(zenith.label(), "mainnet");
        assert_eq!(zenith.role(), "retired");
        assert_eq!(zenith.display_name(), "Zenith (retired)");
        #[cfg(not(feature = "quantic"))]
        if std::env::var_os("ZOKA_NETWORK").is_none() {
            assert_eq!(NetworkProfile::from_args(&[]).expect("default"), zenith);
        }
    }

    #[test]
    fn retired_testnet_chain_and_unknown_names_are_rejected() {
        let err = NetworkProfile::from_chain_id(TESTNET_CHAIN_ID).unwrap_err();
        assert!(err.contains("retired"), "{err}");
        assert!(NetworkProfile::from_name("devnet").is_err());
        assert_eq!(
            NetworkProfile::from_chain_id(MAINNET_CHAIN_ID).expect("zenith"),
            NetworkProfile::Zenith
        );
    }

    // Las bases existentes guardan como marca de genesis la etiqueta "mainnet"
    // o el chain id. Si la etiqueta interna cambiara, el nodo rechazaria su
    // propia base al actualizarse.
    #[test]
    fn databases_marked_with_the_former_label_still_open_as_zenith() {
        for marker in ["mainnet", MAINNET_CHAIN_ID] {
            let (_tmp, db) = temp_db_arg();
            let storage = RocksDbStorage::new(&db).expect("temp rocksdb");
            storage
                .put_raw("chain:genesis:network", marker.as_bytes().to_vec())
                .expect("set genesis network");
            validate_genesis_network_marker(&storage, NetworkProfile::Zenith)
                .unwrap_or_else(|err| panic!("marker {marker}: {err}"));
        }
        let (_tmp, db) = temp_db_arg();
        let storage = RocksDbStorage::new(&db).expect("temp rocksdb");
        storage
            .put_raw(
                "chain:genesis:network",
                TESTNET_CHAIN_ID.as_bytes().to_vec(),
            )
            .expect("set genesis network");
        assert!(validate_genesis_network_marker(&storage, NetworkProfile::Zenith).is_err());
    }

    #[test]
    fn atomic_economics_migration_marks_audited_atomic_chain() {
        let (_tmp, db) = temp_db_arg();
        let storage = RocksDbStorage::new(&db).expect("temp rocksdb");
        let reward = economics::block_reward_for_height(1, 0);
        save_test_coinbase_block(
            &storage,
            1,
            reward,
            genesis_pow_hash_for_chain(NetworkProfile::Zenith.chain_id()),
        );
        storage
            .set_total_supply_emitted(reward)
            .expect("set emitted supply");
        storage
            .put_raw(
                "chain:genesis:network",
                NetworkProfile::Zenith.chain_id().as_bytes().to_vec(),
            )
            .expect("set genesis network");

        let report = migrate_atomic_economics_marker(
            &storage,
            &db,
            NetworkProfile::Zenith,
            false,
            true,
            false,
        )
        .expect("migration should mark audited atomic chain");

        assert!(report.marker_written);
        assert_eq!(report.computed_emission_atoms, reward);
        assert_eq!(
            read_chain_economics_marker(&storage).unwrap().unwrap(),
            economics::ECONOMICS_VERSION
        );
    }

    #[test]
    fn atomic_economics_migration_rejects_whole_zka_coinbase() {
        let (_tmp, db) = temp_db_arg();
        let storage = RocksDbStorage::new(&db).expect("temp rocksdb");
        save_test_coinbase_block(
            &storage,
            1,
            12,
            genesis_pow_hash_for_chain(NetworkProfile::Zenith.chain_id()),
        );
        storage
            .set_total_supply_emitted(12)
            .expect("set emitted supply");

        let err = migrate_atomic_economics_marker(
            &storage,
            &db,
            NetworkProfile::Zenith,
            false,
            true,
            false,
        )
        .expect_err("whole-ZKA chain must not be marked atomic");

        assert!(err.contains("expected"));
        assert!(err.contains("refusing to mark"));
    }

    #[test]
    fn current_known_height_with_explicit_db_ignores_local_mining_state() {
        let (_tmp, db) = temp_db_arg();
        let storage = RocksDbStorage::new(&db).expect("temp rocksdb");
        storage.set_chain_height(7).expect("set height");
        drop(storage);

        assert_eq!(
            current_known_height_for_network(Some(&db), NetworkProfile::Zenith),
            7
        );
    }

    fn private_tx_with_unknown_proof_profile() -> PrivateTransaction {
        serde_json::from_value(serde_json::json!({
            "tx_hash": "unknown-proof-profile",
            "commitment": "commitment",
            "private_proof_version": 2,
            "private_proof_profile": "private_tx_v2_i1",
            "nullifier": "nullifier",
            "zk_proof": null,
            "timestamp": 1,
            "status": "Pending"
        }))
        .expect("deserialize private tx fixture")
    }

    fn valid_private_tx_fixture() -> PrivateTransaction {
        let mut manager = PrivateWalletManager::new();
        manager.create_private_wallet("alice").unwrap();
        let bob = manager.create_private_wallet("bob").unwrap();
        manager.deposit_to_private_wallet("alice", 10).unwrap();
        let tx_hash = manager
            .send_private_transaction("alice", bob, 1)
            .expect("create private tx");
        manager
            .get_private_transaction(&tx_hash)
            .expect("created tx")
            .clone()
    }

    #[test]
    fn pending_private_tx_selection_rejects_unknown_proof_profile() {
        let (_tmp, db) = temp_db_arg();
        let storage = RocksDbStorage::new(&db).expect("temp rocksdb");
        let tx = private_tx_with_unknown_proof_profile();
        let body = serde_json::to_vec(&tx).expect("serialize private tx fixture");
        storage
            .save_private_transaction_bytes(&tx.tx_hash, &body)
            .expect("save private tx body");
        storage
            .save_pending_private_transaction_hash(&tx.tx_hash)
            .expect("save pending hash");

        assert!(
            select_non_conflicting_pending_private_tx_hashes(&storage, vec![tx.tx_hash], 1, 1,)
                .is_empty()
        );
    }

    #[test]
    fn pending_private_tx_diagnostics_reports_skip_reason() {
        let (_tmp, db) = temp_db_arg();
        let storage = RocksDbStorage::new(&db).expect("temp rocksdb");
        let tx = private_tx_with_unknown_proof_profile();
        let body = serde_json::to_vec(&tx).expect("serialize private tx fixture");
        storage
            .save_private_transaction_bytes(&tx.tx_hash, &body)
            .expect("save private tx body");
        storage
            .save_pending_private_transaction_hash(&tx.tx_hash)
            .expect("save pending hash");

        let diagnostics = diagnose_pending_private_tx_selection(&storage, vec![tx.tx_hash], 1, 1);

        assert_eq!(diagnostics.total_candidates, 1);
        assert!(diagnostics.selected.is_empty());
        assert!(diagnostics.candidates[0]
            .reason
            .contains("private transaction validation failed"));
    }

    /// A double-spend attempt where an attacker (or a buggy client) resubmits
    /// the same spend under a fresh `tx_hash` — same `nullifier`, otherwise
    /// identical body. Block-level `validate_private_nullifiers` (see
    /// `state::chain`) is the ultimate consensus backstop, but a miner should
    /// never even select both into a candidate block in the first place: only
    /// the first-seen hash may be selected, the conflicting one must be
    /// skipped.
    #[test]
    fn pending_private_tx_selection_skips_conflicting_duplicate_nullifier() {
        let (_tmp, db) = temp_db_arg();
        let storage = RocksDbStorage::new(&db).expect("temp rocksdb");

        let tx_a = valid_private_tx_fixture();
        let mut tx_b = tx_a.clone();
        tx_b.tx_hash = format!("{}-dup", tx_a.tx_hash);
        assert_eq!(
            tx_a.nullifier, tx_b.nullifier,
            "fixture must share the spend tag"
        );

        for tx in [&tx_a, &tx_b] {
            let body = serde_json::to_vec(tx).expect("serialize private tx fixture");
            storage
                .save_private_transaction_bytes(&tx.tx_hash, &body)
                .expect("save private tx body");
            storage
                .save_pending_private_transaction_hash(&tx.tx_hash)
                .expect("save pending hash");
        }

        let selected = select_non_conflicting_pending_private_tx_hashes(
            &storage,
            vec![tx_a.tx_hash.clone(), tx_b.tx_hash.clone()],
            10,
            1,
        );

        assert_eq!(
            selected,
            vec![tx_a.tx_hash.clone()],
            "only the first-seen tx spending a shared nullifier may be selected"
        );
        assert!(
            !selected.contains(&tx_b.tx_hash),
            "conflicting duplicate-nullifier tx must be skipped from the mining candidate set"
        );
    }

    /// A pending private tx whose nullifier is already confirmed on-chain
    /// (e.g. it was mined via a different path, or is a replay of an old
    /// spend) must never be re-selected for mining — it would only be
    /// rejected downstream by `validate_private_nullifiers`, wasting PoW
    /// work on a block that can never be accepted.
    #[test]
    fn pending_private_tx_selection_skips_nullifier_already_spent_on_chain() {
        let (_tmp, db) = temp_db_arg();
        let storage = RocksDbStorage::new(&db).expect("temp rocksdb");

        let tx = valid_private_tx_fixture();
        let body = serde_json::to_vec(&tx).expect("serialize private tx fixture");
        storage
            .save_private_transaction_bytes(&tx.tx_hash, &body)
            .expect("save private tx body");
        storage
            .save_pending_private_transaction_hash(&tx.tx_hash)
            .expect("save pending hash");
        storage
            .save_spent_nullifier(&tx.nullifier, 1)
            .expect("mark nullifier spent on-chain");

        let diagnostics =
            diagnose_pending_private_tx_selection(&storage, vec![tx.tx_hash.clone()], 10, 2);

        assert!(
            diagnostics.selected.is_empty(),
            "an already-spent nullifier must never be re-selected for mining"
        );
        assert!(diagnostics.candidates[0]
            .reason
            .contains("nullifier already spent on-chain"));
    }

    #[test]
    fn mining_candidate_validation_uses_exact_v2_activation_height() {
        let tx = private_tx_with_unknown_proof_profile();
        let err = validate_private_tx_for_block_candidate_at(&tx, 9, Some(10), Some(10))
            .expect_err("v2 i1 must remain excluded before activation");
        assert!(err.contains("not active at height 9"));

        let err = validate_private_tx_for_block_candidate_at(&tx, 10, Some(10), Some(10))
            .expect_err("deliberately incomplete fixture remains invalid after activation");
        assert!(err.contains("hash does not match body"));
        assert!(!err.contains("not active"));
    }

    #[test]
    fn canonical_private_tx_body_collection_is_all_or_nothing() {
        let (_tmp, db) = temp_db_arg();
        let storage = RocksDbStorage::new(&db).expect("temp rocksdb");
        storage
            .save_private_transaction_bytes("private-tx-a", b"body-a")
            .expect("save first private body");
        let hashes = vec!["private-tx-a".to_string(), "private-tx-b".to_string()];

        assert_eq!(
            collect_canonical_private_tx_bodies_for_hashes(&storage, &hashes),
            None
        );

        storage
            .save_private_transaction_bytes("private-tx-b", b"body-b")
            .expect("save second private body");
        assert_eq!(
            collect_canonical_private_tx_bodies_for_hashes(&storage, &hashes),
            Some(vec![b"body-a".to_vec(), b"body-b".to_vec()])
        );
    }

    #[test]
    fn block_response_prefers_embedded_canonical_private_bodies() {
        let (_tmp, db) = temp_db_arg();
        let storage = RocksDbStorage::new(&db).expect("temp rocksdb");
        storage
            .save_private_transaction_bytes("private-tx-a", b"sidecar-body")
            .expect("save sidecar private body");
        let block = zoka::types::Block::new(
            "prev".to_string(),
            1,
            0,
            1,
            Vec::new(),
            zoka::types::PublicKey(Vec::new()),
            zoka::types::Signature(Vec::new()),
        )
        .with_private_tx_hashes(vec!["private-tx-a".to_string()])
        .with_private_tx_bodies(vec![b"canonical-body".to_vec()]);

        assert_eq!(
            collect_private_tx_bodies_from_storage(&storage, &block),
            vec![b"canonical-body".to_vec()]
        );
    }

    #[test]
    fn sidecar_import_rejects_unknown_proof_profile() {
        let (_tmp, db) = temp_db_arg();
        let storage = RocksDbStorage::new(&db).expect("temp rocksdb");
        let tx = private_tx_with_unknown_proof_profile();
        let sidecar = private_tx_sidecar_dir(&db);
        std::fs::create_dir_all(&sidecar).expect("create sidecar dir");
        std::fs::write(
            sidecar.join(format!("{}.privtx", tx.tx_hash)),
            serde_json::to_vec(&tx).expect("serialize private tx fixture"),
        )
        .expect("write sidecar fixture");

        assert_eq!(import_sidecar_private_txs(&db, &storage), 0);
        assert!(storage
            .get_private_transaction_bytes(&tx.tx_hash)
            .expect("read stored body")
            .is_none());
    }

    #[test]
    fn sidecar_import_removes_already_confirmed_private_tx() {
        let (_tmp, db) = temp_db_arg();
        let storage = RocksDbStorage::new(&db).expect("temp rocksdb");
        let tx = valid_private_tx_fixture();
        let sidecar = private_tx_sidecar_dir(&db);
        let sidecar_file = sidecar.join(format!("{}.privtx", tx.tx_hash));
        std::fs::create_dir_all(&sidecar).expect("create sidecar dir");
        std::fs::write(
            &sidecar_file,
            serde_json::to_vec(&tx).expect("serialize private tx fixture"),
        )
        .expect("write sidecar fixture");
        let block = zoka::types::Block::new(
            genesis_pow_hash_for_chain(NetworkProfile::Zenith.chain_id()),
            1_780_000_001,
            0,
            1,
            Vec::new(),
            zoka::types::PublicKey(Vec::new()),
            zoka::types::Signature(Vec::new()),
        )
        .with_private_tx_hashes(vec![tx.tx_hash.clone()]);
        storage.save_block(&block).expect("save confirmed block");
        storage.set_chain_height(1).expect("set chain height");

        let summary = import_sidecar_private_txs_with_summary(&db, &storage);

        assert_eq!(summary.imported, 0);
        assert_eq!(summary.removed_confirmed, 1);
        assert!(!sidecar_file.exists());
        assert!(storage
            .iterate_pending_private_transaction_hashes()
            .expect("read pending hashes")
            .is_empty());
    }

    #[test]
    fn prune_confirmed_pending_private_txs_removes_marker_and_sidecar() {
        let (_tmp, db) = temp_db_arg();
        let storage = RocksDbStorage::new(&db).expect("temp rocksdb");
        let tx = valid_private_tx_fixture();
        let body = serde_json::to_vec(&tx).expect("serialize private tx fixture");
        storage
            .save_private_transaction_bytes(&tx.tx_hash, &body)
            .expect("save private tx body");
        storage
            .save_pending_private_transaction_hash(&tx.tx_hash)
            .expect("save pending hash");
        let sidecar = private_tx_sidecar_dir(&db);
        let sidecar_file = sidecar.join(format!("{}.privtx", tx.tx_hash));
        std::fs::create_dir_all(&sidecar).expect("create sidecar dir");
        std::fs::write(&sidecar_file, &body).expect("write sidecar fixture");
        let block = zoka::types::Block::new(
            genesis_pow_hash_for_chain(NetworkProfile::Zenith.chain_id()),
            1_780_000_001,
            0,
            1,
            Vec::new(),
            zoka::types::PublicKey(Vec::new()),
            zoka::types::Signature(Vec::new()),
        )
        .with_private_tx_hashes(vec![tx.tx_hash.clone()]);
        storage.save_block(&block).expect("save confirmed block");
        storage.set_chain_height(1).expect("set chain height");

        assert_eq!(prune_confirmed_pending_private_txs(&storage, Some(&db)), 1);
        assert!(!sidecar_file.exists());
        assert!(storage
            .iterate_pending_private_transaction_hashes()
            .expect("read pending hashes")
            .is_empty());
    }

    #[test]
    fn sidecar_hash_listing_exposes_unimported_private_txs() {
        let (_tmp, db) = temp_db_arg();
        let sidecar = private_tx_sidecar_dir(&db);
        std::fs::create_dir_all(&sidecar).expect("create sidecar dir");
        std::fs::write(sidecar.join("b.privtx"), b"{}").expect("write sidecar b");
        std::fs::write(sidecar.join("a.privtx"), b"{}").expect("write sidecar a");

        assert_eq!(
            list_sidecar_private_tx_hashes(&db),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn committed_private_tx_body_archive_rejects_hash_mismatch() {
        let (_tmp, db) = temp_db_arg();
        let storage = RocksDbStorage::new(&db).expect("temp rocksdb");
        let mut manager = PrivateWalletManager::new();
        manager.create_private_wallet("alice").unwrap();
        let bob = manager.create_private_wallet("bob").unwrap();
        manager.deposit_to_private_wallet("alice", 10).unwrap();
        let tx_hash = manager
            .send_private_transaction("alice", bob, 1)
            .expect("create private tx");
        let tx = manager
            .get_private_transaction(&tx_hash)
            .expect("created tx");
        let value = serde_json::to_value(tx).expect("serialize private tx value");

        let err = archive_committed_private_tx_body_json(&storage, "deadbeef", &value)
            .expect_err("mismatched archive must fail");
        assert!(err.contains("hash mismatch"));
        assert!(storage
            .get_private_transaction_bytes("deadbeef")
            .expect("read mismatched slot")
            .is_none());
    }

    #[test]
    fn committed_private_tx_body_archive_stores_valid_body() {
        let (_tmp, db) = temp_db_arg();
        let storage = RocksDbStorage::new(&db).expect("temp rocksdb");
        let mut manager = PrivateWalletManager::new();
        manager.create_private_wallet("alice").unwrap();
        let bob = manager.create_private_wallet("bob").unwrap();
        manager.deposit_to_private_wallet("alice", 10).unwrap();
        let tx_hash = manager
            .send_private_transaction("alice", bob, 1)
            .expect("create private tx");
        let tx = manager
            .get_private_transaction(&tx_hash)
            .expect("created tx");
        let value = serde_json::to_value(tx).expect("serialize private tx value");

        archive_committed_private_tx_body_json(&storage, &tx_hash, &value)
            .expect("valid archive should store body");
        let stored = storage
            .get_private_transaction_bytes(&tx_hash)
            .expect("read archived body")
            .expect("body stored");
        let stored_tx: PrivateTransaction =
            serde_json::from_slice(&stored).expect("parse archived body");
        assert_eq!(stored_tx.tx_hash, tx_hash);
    }

    #[test]
    fn private_tx_hash_range_collection_deduplicates_canonical_hashes() {
        let (_tmp, db) = temp_db_arg();
        let storage = RocksDbStorage::new(&db).expect("temp rocksdb");
        let block_one = zoka::types::Block::new(
            "prev".to_string(),
            1,
            0,
            1,
            Vec::new(),
            zoka::types::PublicKey(Vec::new()),
            zoka::types::Signature(Vec::new()),
        )
        .with_private_tx_hashes(vec!["aaaa".to_string(), "bbbb".to_string()]);
        storage.save_block(&block_one).expect("save block one");
        let block_two = zoka::types::Block::new(
            block_one.header.hash.clone(),
            2,
            0,
            2,
            Vec::new(),
            zoka::types::PublicKey(Vec::new()),
            zoka::types::Signature(Vec::new()),
        )
        .with_private_tx_hashes(vec![
            "bbbb".to_string(),
            "invalid hash".to_string(),
            "cccc".to_string(),
        ]);
        storage.save_block(&block_two).expect("save block two");

        assert_eq!(
            collect_private_tx_hashes_in_height_range(&storage, 1, 2),
            vec!["aaaa".to_string(), "bbbb".to_string(), "cccc".to_string()]
        );
    }

    #[test]
    fn missing_private_tx_body_hashes_skips_present_and_invalid_entries() {
        let (_tmp, db) = temp_db_arg();
        let storage = RocksDbStorage::new(&db).expect("temp rocksdb");
        storage
            .save_private_transaction_bytes("aaaa", b"body-a")
            .expect("save existing body");
        let hashes = vec![
            "aaaa".to_string(),
            "bbbb".to_string(),
            "invalid hash".to_string(),
            "cccc".to_string(),
        ];

        assert_eq!(
            missing_private_tx_body_hashes(&storage, &hashes),
            vec!["bbbb".to_string(), "cccc".to_string()]
        );
    }

    #[test]
    fn private_body_scan_advance_policy_retries_sticky_initial_backfill() {
        assert!(
            should_advance_private_body_scan(false, true, 0, 0),
            "complete windows can advance even without RPC peers"
        );
        assert!(
            !should_advance_private_body_scan(false, true, 2, 0),
            "missing bodies should not be marked scanned when no RPC peers are configured"
        );
        assert!(
            !should_advance_private_body_scan(true, true, 2, 0),
            "initial backfill with unresolved missing bodies should be retried"
        );
        assert!(
            should_advance_private_body_scan(true, true, 2, 1),
            "a productive initial backfill can advance"
        );
        assert!(
            should_advance_private_body_scan(true, false, 2, 0),
            "steady-state scans advance so they do not pin the live loop forever"
        );
    }

    #[test]
    fn private_tx_audit_accepts_zero_fee_without_private_fee_commitment() {
        let mut manager = PrivateWalletManager::new();
        manager.create_private_wallet("alice").unwrap();
        let bob = manager.create_private_wallet("bob").unwrap();
        manager.deposit_to_private_wallet("alice", 10).unwrap();

        let tx_hash = manager
            .send_private_transaction("alice", bob, 1)
            .expect("zero fee private tx");
        let tx = manager
            .get_private_transaction(&tx_hash)
            .expect("created tx");
        let report = audit_private_transaction_privacy(&tx);

        assert!(report.audit_passed);
        assert!(report.zero_fee);
        assert!(!report.private_fee);
        assert!(!report.reveals_clear_fee);
    }

    #[test]
    fn private_tx_audit_rejects_clear_nonzero_fee_and_local_opening() {
        let mut manager = PrivateWalletManager::new();
        manager.create_private_wallet("alice").unwrap();
        let bob = manager.create_private_wallet("bob").unwrap();
        manager.deposit_to_private_wallet("alice", 10).unwrap();

        let tx_hash = manager
            .send_private_transaction("alice", bob, 1)
            .expect("zero fee private tx");
        let mut tx = manager
            .get_private_transaction(&tx_hash)
            .expect("created tx");
        tx.fee = 1;
        tx.fee_opening = Some(zoka::private_wallet::PrivateFeeOpening {
            fee: 1,
            fee_blind: vec![0u8; 32],
        });
        let report = audit_private_transaction_privacy(&tx);

        assert!(!report.audit_passed);
        assert!(report.reveals_clear_fee);
        assert!(report.reveals_fee_opening);
    }

    #[test]
    fn validate_address_accepts_canonical_zka1() {
        let addr = make_valid_zka1();
        let canonical = validate_user_private_address(&addr).expect("valid zka1 address");
        assert!(canonical.starts_with("zpriv:"));
    }

    #[test]
    fn network_operations_reject_local_wallet_names() {
        let err = require_private_network_address("alice", "--from").unwrap_err();
        assert!(err.contains("full zka1 wallet address"));
        assert!(err.contains("local wallet names are not accepted"));
    }

    #[test]
    fn wallet_selector_requires_address_flag_only() {
        let args = vec!["--name".to_string(), "alice".to_string()];
        let err = required_wallet_selector(&args).unwrap_err();
        assert!(err.contains("--address <wallet-address>"));
    }

    #[test]
    fn wallet_address_lookup_rejects_local_wallet_names() {
        let mut manager = PrivateWalletManager::new();
        manager
            .create_private_wallet("alice")
            .expect("create test wallet");
        let err = resolve_private_wallet_name_required(&manager, "alice").unwrap_err();
        assert!(err.contains("local wallet names are not accepted"));
    }

    #[test]
    fn network_operations_accept_full_wallet_addresses() {
        let addr = make_valid_zka1();
        let canonical =
            require_private_network_address(&addr, "--address").expect("valid network address");
        assert!(canonical.starts_with("zpriv:"));
    }

    // `--to` sigue aceptando zka1 como siempre; una zkq1 se rechaza con un
    // mensaje claro mientras la red no fije su activación (hoy, nunca).
    #[test]
    fn recipient_accepts_zka1_and_gates_zkq1_on_activation() {
        let addr = make_valid_zka1();
        assert_eq!(
            require_private_recipient_address(&addr, "--to").unwrap(),
            require_private_network_address(&addr, "--to").unwrap()
        );
        if economics::pq_hybrid_outputs_activation_height().is_none() {
            let err = require_private_recipient_address("zkq1abc", "--to").unwrap_err();
            assert!(
                err.contains("not active") && err.contains("zka1"),
                "got: {err}"
            );
        }
    }

    #[test]
    fn validate_address_rejects_empty_with_actionable_message() {
        let err = validate_user_private_address("").unwrap_err();
        assert!(
            err.contains("zka1"),
            "error must mention the expected prefix; got: {err}"
        );
    }

    #[test]
    fn validate_address_rejects_unknown_prefix_with_actionable_message() {
        let err = validate_user_private_address("bc1qxyz").unwrap_err();
        assert!(
            err.contains("zka1") && err.contains("wallet-list"),
            "error must guide the user toward the right prefix and wallet-list; got: {err}"
        );
    }

    #[test]
    fn validate_address_rejects_non_base58_with_actionable_message() {
        // '0' is not in the bs58 alphabet
        let err = validate_user_private_address("zka10000").unwrap_err();
        assert!(
            err.to_lowercase().contains("base58"),
            "error must explain Base58 decoding failure; got: {err}"
        );
    }

    #[test]
    fn validate_address_rejects_wrong_length_with_actionable_message() {
        // Valid Base58, but wrong decoded length.
        let err = validate_user_private_address("zka1abc").unwrap_err();
        assert!(
            err.to_lowercase().contains("length"),
            "error must mention length; got: {err}"
        );
    }

    #[test]
    fn validate_address_rejects_checksum_mismatch_with_actionable_message() {
        let mut addr = make_valid_zka1();
        // Flip the last character (checksum byte) to an arbitrary other Base58 char.
        let last = addr.pop().expect("non-empty");
        let replacement = if last == 'A' { 'B' } else { 'A' };
        addr.push(replacement);
        let err = validate_user_private_address(&addr).unwrap_err();
        assert!(
            err.to_lowercase().contains("checksum"),
            "error must mention checksum mismatch; got: {err}"
        );
    }

    #[test]
    fn validate_address_rejects_zpriv_with_short_scan_key() {
        let bad = format!("zpriv:{}:{}", hex::encode([0u8; 8]), hex::encode([0u8; 32]));
        let err = validate_user_private_address(&bad).unwrap_err();
        assert!(
            err.to_lowercase().contains("scan key"),
            "error must call out the scan key; got: {err}"
        );
    }

    #[test]
    fn zpriv_roundtrip_preserves_canonical_form() {
        let addr = make_valid_zka1();
        let canonical_a = zpriv_from_user_private_address(&addr).expect("decode zka1");
        let canonical_b = validate_user_private_address(&addr).expect("validate zka1");
        assert_eq!(canonical_a, canonical_b);
    }

    #[test]
    fn positional_alias_normalizer_maps_short_commands() {
        let send = normalize_positional_alias_args(
            &["alice".to_string(), "bob".to_string(), "10".to_string()],
            &["--from", "--to", "--amount"],
        );
        assert_eq!(
            send,
            vec!["--from", "alice", "--to", "bob", "--amount", "10"]
        );

        let balance = normalize_positional_alias_args(&["piola".to_string()], &["--address"]);
        assert_eq!(balance, vec!["--address", "piola"]);
    }

    #[test]
    fn rpc_http_url_parser_accepts_local_http_only() {
        let parsed = parse_rpc_http_url("http://node.local:28880/api").expect("http url");
        assert_eq!(parsed.host, "node.local");
        assert_eq!(parsed.port, 28880);
        assert_eq!(parsed.base_path, "/api");
        assert!(parse_rpc_http_url("https://rpc.example").is_err());
        assert!(parse_rpc_http_url("http://user:pass@node.local").is_err());
    }

    #[test]
    fn mainnet_network_config_accepts_prelaunch_template() {
        let raw = r#"{
          "schema": "zoka-mainnet-network-v1",
          "status": "prelaunch-template",
          "chain_id": "zoka-mainnet-zenith-v1",
          "protocol_freeze_id": "/zoka/1.9.0-mainnet-recovery",
          "transport_policy": {
            "anonymous_transport_preferred": true,
            "clearnet_requires_explicit_operator_choice": true,
            "supported": ["tor", "i2p", "tcp"]
          },
          "seed_nodes": [],
          "public_rpc": [],
          "snapshot_signers": [],
          "notes": []
        }"#;
        let config = parse_mainnet_network_config(raw).expect("valid config");
        assert_eq!(config.chain_id, MAINNET_CHAIN_ID);
        assert!(config.active_seed_candidates(false).is_empty());
    }

    #[test]
    fn embedded_mainnet_network_config_accepts_documentation_fields() {
        let config =
            parse_mainnet_network_config(EMBEDDED_MAINNET_NETWORK_CONFIG).expect("valid config");

        assert_eq!(config.chain_id, MAINNET_CHAIN_ID);
        assert!(!config.active_seed_candidates(true).is_empty());
    }

    #[test]
    fn mainnet_network_lint_accepts_launch_ready_active_config() {
        let raw = r#"{
          "schema": "zoka-mainnet-network-v1",
          "status": "launch-ready",
          "chain_id": "zoka-mainnet-zenith-v1",
          "protocol_freeze_id": "/zoka/1.9.0-mainnet-recovery",
          "transport_policy": {
            "anonymous_transport_preferred": true,
            "clearnet_requires_explicit_operator_choice": true,
            "supported": ["tor", "i2p", "tcp"]
          },
          "seed_nodes": [
            {
              "label": "seed-na",
              "operator": "operator-na",
              "region": "north-america",
              "multiaddr": "/dns4/seed-na.valid-zoka.net/tcp/28080/p2p/12D3KooWNaSeedPeerRealPubKeyAlphaBetaGamma",
              "onion": "/onion3/seednarealabcdefghiabcdefghiabcdefghiabcdefghiabcdefghiabcd:28080/p2p/12D3KooWNaSeedPeerRealPubKeyAlphaBetaGamma",
              "i2p": null,
              "role": "seed",
              "status": "active"
            },
            {
              "label": "seed-eu",
              "operator": "operator-eu",
              "region": "europe",
              "multiaddr": "/dns4/seed-eu.valid-zoka.net/tcp/28080/p2p/12D3KooWEuSeedPeerRealPubKeyAlphaBetaGamma",
              "onion": null,
              "i2p": "/dnsaddr/seed-eu-valid.b32.i2p/p2p/12D3KooWEuSeedPeerRealPubKeyAlphaBetaGamma",
              "role": "seed",
              "status": "active"
            },
            {
              "label": "seed-sa",
              "operator": "operator-sa",
              "region": "south-america",
              "multiaddr": "/dns4/seed-sa.valid-zoka.net/tcp/28080/p2p/12D3KooWSaSeedPeerRealPubKeyAlphaBetaGamma",
              "onion": "/onion3/seedsarealabcdefghiabcdefghiabcdefghiabcdefghiabcdefghiabcd:28080/p2p/12D3KooWSaSeedPeerRealPubKeyAlphaBetaGamma",
              "i2p": null,
              "role": "seed",
              "status": "active"
            }
          ],
          "public_rpc": [
            {
              "label": "rpc-eu",
              "operator": "operator-eu",
              "region": "europe",
              "url": "https://rpc-eu.valid-zoka.net",
              "onion": "http://rpceurealabcdefghiabcdefghiabcdefghiabcdefghiabcdefghiabcd.onion",
              "i2p": null,
              "status": "active",
              "logging_policy": "docs/infrastructure-logs-policy.md"
            }
          ],
          "snapshot_signers": [
            "0123456789abcdeffedcba98765432100123456789abcdeffedcba9876543210"
          ],
          "notes": []
        }"#;
        let tmp = tempfile::NamedTempFile::new().expect("temp config");
        fs::write(tmp.path(), raw).expect("write temp config");

        let report = lint_mainnet_network_config(tmp.path(), false).expect("lint report");
        assert!(report.ok, "unexpected errors: {:?}", report.errors);
        assert_eq!(report.active_seed_count, 3);
        assert_eq!(report.active_seed_candidates_without_clearnet, 3);
        assert_eq!(report.snapshot_signer_count, 1);
    }

    #[test]
    fn mainnet_network_config_rejects_wrong_chain_id() {
        let raw = r#"{
          "schema": "zoka-mainnet-network-v1",
          "status": "bad",
          "chain_id": "zoka-test-net",
          "protocol_freeze_id": "/zoka/1.9.0-mainnet-recovery",
          "transport_policy": {
            "anonymous_transport_preferred": true,
            "clearnet_requires_explicit_operator_choice": true,
            "supported": ["tor", "i2p", "tcp"]
          },
          "seed_nodes": [],
          "public_rpc": [],
          "snapshot_signers": [],
          "notes": []
        }"#;
        let err = parse_mainnet_network_config(raw).unwrap_err();
        assert!(err.contains("chain_id"));
    }

    #[test]
    fn seed_candidates_require_clearnet_opt_in_when_policy_requires_it() {
        let raw = r#"{
          "schema": "zoka-mainnet-network-v1",
          "status": "candidate",
          "chain_id": "zoka-mainnet-zenith-v1",
          "protocol_freeze_id": "/zoka/1.9.0-mainnet-recovery",
          "transport_policy": {
            "anonymous_transport_preferred": true,
            "clearnet_requires_explicit_operator_choice": true,
            "supported": ["tor", "i2p", "tcp"]
          },
          "seed_nodes": [{
            "label": "seed-a",
            "operator": "operator-a",
            "region": "us-east",
            "multiaddr": "/ip4/203.0.113.10/tcp/28080",
            "onion": "/onion3/exampleexampleexampleexampleexampleexampleexampleexampleexample:28080",
            "i2p": null,
            "role": "seed",
            "status": "active"
          }],
          "public_rpc": [],
          "snapshot_signers": [],
          "notes": []
        }"#;
        let config = parse_mainnet_network_config(raw).expect("valid config");
        let anonymous_only = config.active_seed_candidates(false);
        assert_eq!(anonymous_only.len(), 1);
        assert_eq!(anonymous_only[0].transport, "tor");

        let with_clearnet = config.active_seed_candidates(true);
        assert_eq!(with_clearnet.len(), 2);
        assert_eq!(with_clearnet[1].transport, "tcp");
    }
}

// ---------------------------------------------------------------------------
// trusted-setup-init
// ---------------------------------------------------------------------------

fn cmd_trusted_setup_init(args: &[String]) -> Result<(), String> {
    use zoka::zk::groth16_backend::generate_circuit_params_with_osrng;
    use zoka::zk::setup_io::{
        compute_directory_digest, write_circuit_params, write_digest, REQUIRED_CIRCUITS,
    };

    let out_dir = option_value(args, "--out").unwrap_or_else(|| "./trusted-setup".to_string());
    let force = has_flag(args, "--force");
    let as_json = has_flag(args, "--json");
    let out_path = std::path::Path::new(&out_dir);

    if out_path.exists() && !force {
        // Check if it already has params; refuse to overwrite silently.
        let has_files = std::fs::read_dir(out_path)
            .ok()
            .and_then(|mut d| d.next())
            .is_some();
        if has_files {
            return Err(format!(
                "output directory '{}' already exists and is non-empty. \
                 Use --force to overwrite.",
                out_dir
            ));
        }
    }

    std::fs::create_dir_all(out_path)
        .map_err(|e| format!("failed to create output directory '{}': {e}", out_dir))?;

    if !as_json {
        println!("{}", dashboard_divider());
        println!("{}", dashboard_section("Trusted Setup Ceremony", ANSI_CYAN));
        println!("{}", dashboard_line("Output", &out_dir));
        println!(
            "{}",
            dashboard_line(
                "Entropy source",
                "OsRng (kernel entropy — not reproducible, toxic waste never stored)"
            )
        );
        println!(
            "{}",
            dashboard_line("Circuits", &REQUIRED_CIRCUITS.join(", "))
        );
        println!("{}", dashboard_divider());
        println!();
        println!("Generating parameters for 4 circuits. Each circuit takes ~5-30s.");
        println!("The randomness comes from your OS entropy pool and is discarded");
        println!("as soon as the Parameters object is written to disk.");
        println!();
    }

    let total_start = std::time::Instant::now();
    let mut circuit_digests: Vec<(String, String, f64)> = Vec::new();

    for circuit in REQUIRED_CIRCUITS {
        if !as_json {
            print!(
                "  [{}/{}] Generating {}... ",
                circuit_digests.len() + 1,
                REQUIRED_CIRCUITS.len(),
                circuit
            );
            let _ = std::io::Write::flush(&mut std::io::stdout());
        }

        let t = std::time::Instant::now();
        let params = generate_circuit_params_with_osrng(circuit)?;
        let elapsed = t.elapsed().as_secs_f64();

        write_circuit_params(out_path, circuit, &params)
            .map_err(|e| format!("failed to write {circuit} params: {e}"))?;

        // Hash just this file for per-circuit reporting
        let file_name = format!("{circuit}.params");
        let file_path = out_path.join(&file_name);
        let file_bytes = std::fs::metadata(&file_path).map(|m| m.len()).unwrap_or(0);

        if !as_json {
            println!(
                "done ({:.1}s, {:.0} KB)",
                elapsed,
                file_bytes as f64 / 1024.0
            );
        }

        circuit_digests.push((circuit.to_string(), file_name, elapsed));
    }

    // Compute and write the bundle digest
    let digest = compute_directory_digest(out_path)
        .map_err(|e| format!("failed to compute setup digest: {e}"))?;
    write_digest(out_path, &digest).map_err(|e| format!("failed to write DIGEST file: {e}"))?;

    let digest_hex = hex::encode(digest);
    let total_elapsed = total_start.elapsed().as_secs_f64();

    if as_json {
        println!(
            "{}",
            serde_json::json!({
                "status": "ok",
                "out_dir": out_dir,
                "digest": digest_hex,
                "circuits": circuit_digests.iter().map(|(name, file, secs)| {
                    serde_json::json!({ "circuit": name, "file": file, "secs": secs })
                }).collect::<Vec<_>>(),
                "total_secs": total_elapsed,
                "env_var": format!("ZOKA_TRUSTED_SETUP_DIR={}", out_dir),
            })
        );
    } else {
        println!();
        println!("{}", dashboard_divider());
        println!("{}", dashboard_section("Ceremony Complete", ANSI_GREEN));
        println!(
            "{}",
            dashboard_line("Total time", &format!("{:.1}s", total_elapsed))
        );
        println!("{}", dashboard_line("Output dir", &out_dir));
        println!("{}", dashboard_line("Bundle digest", &digest_hex));
        println!("{}", dashboard_divider());
        println!();
        println!("To use this trusted setup, set the environment variable:");
        println!();
        println!("  Windows:  $env:ZOKA_TRUSTED_SETUP_DIR = \"{}\"", out_dir);
        println!("  Linux:    export ZOKA_TRUSTED_SETUP_DIR={}", out_dir);
        println!();
        println!("All nodes using this network must point to the same bundle");
        println!("(same digest: {}).", &digest_hex[..16]);
        println!();
        println!("To bind the binary to this specific CRS at compile time:");
        println!("  ZOKA_EXPECTED_SETUP_DIGEST_COMPILED={}", digest_hex);
    }

    Ok(())
}
