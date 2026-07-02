use std::collections::VecDeque;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Read, Write};
use std::net::TcpStream;
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
    println!("{}", dashboard_divider());
    println!("{}", dashboard_section("Everyday commands", ANSI_GREEN));
    println!(
        "  zoka create <label> --network mainnet|testnet --password <pwd>|--password-env <var>|--password-stdin [--seed-words 12|24]"
    );
    println!(
        "  zoka restore \"<recovery phrase>\" --network mainnet|testnet --password <pwd>|--password-env <var>|--password-stdin [--name <label>]"
    );
    println!("  zoka wallets [--network mainnet|testnet] [--password <pwd>]");
    println!("  zoka balance <wallet-address> [--network mainnet|testnet] [--password <pwd>]");
    println!("  zoka receive <wallet-address> [--network mainnet|testnet] [--password <pwd>]");
    println!(
        "  zoka send <from-zka1> <to-zka1> <amount> [--network mainnet|testnet] [--password <pwd>] --yes"
    );
    println!("  zoka pending [--network mainnet|testnet] [--db <path>]");
    println!("  zoka backup <wallet-address> <path> [--password <pwd>]");
    println!(
        "  zoka restore-backup <path> [--name <label>] --password <pwd>|--password-env <var>|--password-stdin"
    );
    println!("  zoka wallet-generate-seed [--seed-words 12|24] [--json]");
    println!(
        "  zoka mine <wallet-zka1> [--network mainnet|testnet] [--password <pwd>] [--db <path>] [--rpc <http-url>] [--network-config <file>] [--minutes <n>] [--blocks <n>] [--power low|normal|max] [--allow-unsynced-mining]"
    );
    println!(
        "  zoka node [--network mainnet|testnet] [--password <pwd>|--password-env <var>|--password-stdin] [--seed <multiaddr> ...] [--power low|normal|max] [--tor-proxy host:port] [--tor-only]"
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
    println!("{}", dashboard_section("Chain and operators", ANSI_YELLOW));
    println!("  height [--network mainnet|testnet] [--db <path>] [--json]");
    println!(
        "  block-info --height <n> [--network mainnet|testnet] [--db <path>] [--json]  (miner address and reward for a specific block height)"
    );
    println!("  chain-stats [--network mainnet|testnet] [--db <path>] [--json]");
    println!(
        "  chain-economics-migrate [--network mainnet|testnet] [--db <path>] [--dry-run|--yes] [--repair-supply] [--json]"
    );
    println!(
        "  private-pending-diagnostics [--network mainnet|testnet] [--db <path>] [--height <n>] [--limit <n>] [--json]"
    );
    println!(
        "  rpc-list [--network mainnet|testnet] [--network-config <file>] [--json]  (lists configured read-only public RPC endpoints)"
    );
    println!(
        "  rpc-status [--rpc <http-url>] [--network-config <file>] [--json]  (read-only RPC health + height)"
    );
    println!(
        "  rpc-height [--rpc <http-url>] [--network-config <file>] [--json]  (read-only RPC chain height)"
    );
    println!(
        "  rpc-sync [--network mainnet|testnet] [--network-config <file>] [--db <path>] [--limit <n>] [--json]  (bootstrap local chain from configured RPC)"
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
        "  chain-prune --keep-last <N> [--db <path>] [--dry-run] [--json]  (drops block bodies older than head-N; floors --keep-last at PRUNE_MIN_RETAINED_BLOCKS so difficulty retarget keeps working)"
    );
}

fn prompt_line(prompt: &str) -> Result<String, String> {
    print!("{}", paint(prompt, ANSI_CYAN));
    io::stdout()
        .flush()
        .map_err(|e| format!("stdout flush failed: {e}"))?;
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .map_err(|e| format!("stdin read failed: {e}"))?;
    Ok(line.trim().to_string())
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
        let value = prompt_line(prompt)?;
        if !value.is_empty() {
            return Ok(value);
        }
        println!("Value is required.");
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
    #[serde(default)]
    snapshot_signers: Vec<String>,
    #[serde(default)]
    notes: Vec<String>,
    /// Minimum desktop client version the network expects; enforced by
    /// ZSilent, not by this binary. Accepted (not just ignored) so the
    /// manifest's deny_unknown_fields doesn't reject the whole file.
    #[serde(default)]
    minimum_client_version: String,
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
                network.label()
            ));
        }
        if self.chain_id != network.chain_id() {
            return Err(format!(
                "network config chain_id '{}' does not match binary chain_id '{}'",
                self.chain_id,
                network.chain_id()
            ));
        }
        if self.protocol_freeze_id != PROTOCOL_FREEZE_ID {
            return Err(format!(
                "network config protocol '{}' does not match binary protocol '{}'",
                self.protocol_freeze_id, PROTOCOL_FREEZE_ID
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
    let network_specific = match network {
        NetworkProfile::Mainnet => "ZOKA_MAINNET_NETWORK_CONFIG",
        NetworkProfile::Testnet => "ZOKA_TESTNET_NETWORK_CONFIG",
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
            network.label()
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
            network.label(),
            path.display()
        )
    })?;
    api_bind_addr_from_local_rpc_url(&rpc)
}

fn active_public_rpc_urls(config: &MainnetNetworkConfig) -> Vec<String> {
    config
        .public_rpc
        .iter()
        .filter(|rpc| rpc.status.eq_ignore_ascii_case("active"))
        .map(|rpc| rpc.url.trim_end_matches('/').to_string())
        .collect()
}

fn rpc_sync_endpoints_from_args(
    args: &[String],
    network: NetworkProfile,
) -> Result<Vec<String>, String> {
    let manual = option_values(args, "--rpc");
    if !manual.is_empty() {
        for rpc in &manual {
            parse_rpc_http_url(rpc)?;
        }
        return Ok(manual
            .into_iter()
            .map(|rpc| rpc.trim_end_matches('/').to_string())
            .collect());
    }
    if has_flag(args, "--no-network-config") {
        return Ok(Vec::new());
    }

    let config_path = network_config_path_from_args(args, network);
    let config = if config_path.exists() {
        load_network_config_for(&config_path, network)?
    } else {
        parse_network_config_for(embedded_network_config_for(network), network)?
    };
    let endpoints = active_public_rpc_urls(&config);
    for rpc in &endpoints {
        parse_rpc_http_url(rpc)?;
    }
    Ok(endpoints)
}

#[cfg(test)]
fn parse_mainnet_network_config(raw: &str) -> Result<MainnetNetworkConfig, String> {
    parse_network_config_for(raw, NetworkProfile::Mainnet)
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
    load_network_config_for(path, NetworkProfile::Mainnet)
}

/// Network manifests are baked into the binary so a fresh `zoka.exe` works in
/// any working directory, even when shipped without the `docs/` folder. The
/// loader still prefers a real file on disk if it exists (for operators
/// pointing at a custom config) and falls back to the embedded copy otherwise.
const EMBEDDED_MAINNET_NETWORK_CONFIG: &str = include_str!("../docs/mainnet-network.v1.json");
const EMBEDDED_TESTNET_NETWORK_CONFIG: &str = include_str!("../docs/testnet-network.v1.json");

fn embedded_network_config_for(network: NetworkProfile) -> &'static str {
    match network {
        NetworkProfile::Mainnet => EMBEDDED_MAINNET_NETWORK_CONFIG,
        NetworkProfile::Testnet => EMBEDDED_TESTNET_NETWORK_CONFIG,
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
                let to_commitment = match require_private_network_address(&to, "--to") {
                    Ok(value) => value,
                    Err(err) => {
                        eprintln!("{}", err);
                        continue;
                    }
                };
                let storage = match wallet_storage_for_network(NetworkProfile::Mainnet) {
                    Ok(value) => value,
                    Err(err) => {
                        eprintln!("{}", err);
                        continue;
                    }
                };
                let manager = match load_private_wallet_manager_for_network(
                    &storage,
                    password.as_deref(),
                    NetworkProfile::Mainnet,
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

fn interactive_mining_menu() -> Result<bool, String> {
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
                let wallet = prompt_wallet_selector_with_single_default()?;
                let threads = prompt_mining_cpu_threads()?;
                let minutes = prompt_optional("Session minutes (empty = keep running): ")?;
                let blocks = prompt_optional("Block limit (empty = no block limit): ")?;
                let password = prompt_optional("Password (optional): ")?;
                let mut args = vec!["--address".to_string(), wallet];
                args.push("--threads".to_string());
                args.push(threads.to_string());
                push_opt_arg(&mut args, "--minutes", minutes);
                push_opt_arg(&mut args, "--blocks", blocks);
                push_opt_arg(&mut args, "--password", password);
                if let Err(err) = cmd_mine_easy(&args) {
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
                if let Err(err) = cmd_fullnode_auto(&args).await {
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
                if let Err(err) = cmd_fullnode_auto(&args).await {
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
                if interactive_mining_menu()? {
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

fn find_private_wallet_name_by_commitment(
    manager: &PrivateWalletManager,
    commitment: &str,
) -> Option<String> {
    manager.get_private_wallet_names().into_iter().find(|name| {
        manager
            .get_private_wallet(name)
            .map(|wallet| wallet.commitment_key == commitment)
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
}

#[derive(Debug, Clone)]
struct MiningBatchSummary {
    blocks_mined: u64,
    reward_credited: u64,
    last_difficulty_bits: u32,
    last_solve_time_secs: f64,
    /// Blocks produced in this batch, in mining order. Callers that own a
    /// libp2p node use these to publish a `ZokaMessage::BlockResponse` so
    /// peers can apply the same block via `state::chain::apply_block`.
    /// Standalone `mine` callers without a node simply ignore this.
    produced_blocks: Vec<zoka::types::Block>,
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
const TESTNET_MINING_MODE: &str = "pow-testnet";
const POW_TARGET_BLOCK_TIME_SECS: u64 = economics::TARGET_BLOCK_TIME_SECS;
const POW_INITIAL_DIFFICULTY_BITS: u32 = economics::POW_INITIAL_DIFFICULTY_BITS;
const POW_MIN_DIFFICULTY_BITS: u32 = economics::POW_MIN_DIFFICULTY_BITS;
const POW_MAX_DIFFICULTY_BITS: u32 = economics::POW_MAX_DIFFICULTY_BITS;
const COINBASE_MATURITY_BLOCKS: u64 = economics::COINBASE_MATURITY_BLOCKS;
const LOCAL_PRIVATE_TXS_PER_BLOCK_LIMIT: usize = economics::PRIVATE_TX_HASHES_PER_BLOCK_LIMIT;
const MINING_STALE_AFTER_SECS: u64 = 30;
const DEFAULT_CHAIN_DB_PATH: &str = ".zoka/mainnet/blockchain.db";
const DEFAULT_TESTNET_CHAIN_DB_PATH: &str = ".zoka/testnet/blockchain.db";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NetworkProfile {
    Mainnet,
    Testnet,
}

impl NetworkProfile {
    fn from_args(args: &[String]) -> Result<Self, String> {
        let raw = option_value(args, "--network")
            .or_else(|| std::env::var("ZOKA_NETWORK").ok())
            .unwrap_or_else(|| "mainnet".to_string());
        match raw.to_ascii_lowercase().as_str() {
            "mainnet" => Ok(Self::Mainnet),
            "testnet" => Ok(Self::Testnet),
            other => Err(format!(
                "invalid --network '{other}'; expected mainnet|testnet"
            )),
        }
    }

    fn from_chain_id(chain_id: &str) -> Result<Self, String> {
        match chain_id {
            MAINNET_CHAIN_ID => Ok(Self::Mainnet),
            TESTNET_CHAIN_ID => Ok(Self::Testnet),
            other => Err(format!(
                "unknown chain id '{other}' for local wallet storage profile"
            )),
        }
    }

    fn chain_id(self) -> &'static str {
        match self {
            Self::Mainnet => MAINNET_CHAIN_ID,
            Self::Testnet => TESTNET_CHAIN_ID,
        }
    }

    fn default_db_path(self) -> &'static str {
        match self {
            Self::Mainnet => DEFAULT_CHAIN_DB_PATH,
            Self::Testnet => DEFAULT_TESTNET_CHAIN_DB_PATH,
        }
    }

    fn local_dir(self) -> &'static str {
        match self {
            Self::Mainnet => ".zoka/mainnet",
            Self::Testnet => ".zoka/testnet",
        }
    }

    fn default_genesis_path(self) -> &'static str {
        match self {
            Self::Mainnet => "docs/genesis.mainnet.v1.json",
            Self::Testnet => "docs/genesis.testnet.v1.json",
        }
    }

    fn default_network_config_path(self) -> &'static str {
        match self {
            Self::Mainnet => "docs/mainnet-network.v1.json",
            Self::Testnet => "docs/testnet-network.v1.json",
        }
    }

    fn network_config_schema(self) -> &'static str {
        match self {
            Self::Mainnet => "zoka-mainnet-network-v1",
            Self::Testnet => "zoka-testnet-network-v1",
        }
    }

    fn mining_mode(self) -> &'static str {
        match self {
            Self::Mainnet => MAINNET_MINING_MODE,
            Self::Testnet => TESTNET_MINING_MODE,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Mainnet => "mainnet",
            Self::Testnet => "testnet",
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

fn mining_log(message: &str) {
    eprintln!("mining: {message}");
    let path = PathBuf::from(NetworkProfile::Mainnet.local_dir()).join("mining_diagnostic.log");
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        use std::io::Write as _;
        let _ = writeln!(f, "{} mining: {}", now_secs(), message);
    }
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
    let mut state: UserMiningState = serde_json::from_str(&raw)
        .map_err(|e| format!("invalid mining state json {}: {e}", path.display()))?;
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
    load_user_mining_state_for_display_for_network(NetworkProfile::Mainnet)
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
    let raw = serde_json::to_string_pretty(state)
        .map_err(|e| format!("failed to encode mining state json: {e}"))?;
    fs::write(&path, raw).map_err(|e| format!("failed to write {}: {e}", path.display()))
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
    let sidecar = private_tx_sidecar_dir(db_path);
    if !sidecar.exists() {
        return 0;
    }
    let entries = match std::fs::read_dir(&sidecar) {
        Ok(e) => e,
        Err(_) => return 0,
    };
    let mut imported = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("privtx") {
            continue;
        }
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let tx_hash = match path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
        {
            Some(h) => h,
            None => continue,
        };
        let tx = match serde_json::from_slice::<PrivateTransaction>(&bytes) {
            Ok(tx) if tx.tx_hash == tx_hash => tx,
            _ => continue,
        };
        if validate_private_tx_for_pending_pool(&tx).is_err() {
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
            imported += 1;
        }
    }
    imported
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
    if let Ok(mut mining_state) = load_user_mining_state_for_network(network) {
        let _ = settle_mature_mining_rewards(
            storage,
            &mut manager,
            password,
            network,
            &mut mining_state,
        );
    }
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

fn persist_private_wallet(
    storage: &WalletStorage,
    manager: &PrivateWalletManager,
    wallet_name: &str,
    password: Option<&str>,
) -> Result<(), String> {
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
    for block in blocks {
        if block.header.block_height < restore_height {
            continue;
        }
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

    // Advance the wallet's scan checkpoint past every block this call covered
    // so the next unlock only scans new blocks instead of re-reading the
    // entire chain from height 1 every time. Coinbases are still
    // de-duplicated by output_id/block_hash above, so this is purely a
    // performance optimization, not a correctness dependency.
    advance_private_wallet_restore_height(storage, manager, wallet_name, password, chain_height)?;

    Ok(settled)
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
    let mut from = restore_height;
    while from <= chain_height {
        let to = from.saturating_add(255).min(chain_height);
        let responses = fetch_rpc_block_range(rpc, from, to)?;
        for response in responses {
            let block: zoka::types::Block = serde_json::from_slice(&response.encrypted_block)
                .map_err(|e| format!("RPC private coinbase block decode failed: {e}"))?;
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

    advance_private_wallet_restore_height(storage, manager, wallet_name, password, chain_height)?;

    Ok(settled)
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
            network.label()
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
        if let Some(block) = chain_storage
            .get_block_by_height(height)
            .map_err(|e| format!("failed to read block at height {height}: {e}"))?
        {
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
    let wallet_snapshot = match load_wallet_runtime_snapshot_from_storage(
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
    if config.mode != MAINNET_MINING_MODE && config.mode != TESTNET_MINING_MODE {
        return Err(format!(
            "invalid mining mode; expected {} or {}",
            MAINNET_MINING_MODE, TESTNET_MINING_MODE
        ));
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
        let difficulty_bits = match config.unsafe_rehearsal_difficulty_bits {
            Some(bits) => bits,
            None => match config.chain_db_path.as_deref() {
                Some(path) => {
                    let storage = open_storage(path)?;
                    zoka::mining::difficulty::expected_for_next_block(&storage)?
                }
                None => last_difficulty.clamp(POW_MIN_DIFFICULTY_BITS, POW_MAX_DIFFICULTY_BITS),
            },
        };
        let timestamp = now_secs();
        let prev_hash = current_prev_hash.clone();
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
        mining_log(&format!(
            "starting PoW solve for height={} difficulty_bits={} threads={}",
            height, difficulty_bits, config.threads
        ));
        let solve_started = Instant::now();
        let (pow_hash_bytes, found_nonce, block_hashes, solve_secs) = solve_pow_nonce(
            &config.chain_id,
            height,
            difficulty_bits,
            &prev_hash,
            &wallet_address,
            timestamp,
            config.threads,
            config.hashrate_hps,
            |snapshot| {
                if let Some(callback) = progress.as_deref_mut() {
                    callback(snapshot)?;
                }
                Ok(())
            },
        )?;
        mining_log(&format!(
            "solved height={} in {:.2}s ({} hashes, ~{:.0} H/s)",
            height,
            solve_secs,
            block_hashes,
            block_hashes as f64 / solve_secs.max(0.001)
        ));
        let _ = solve_started;
        total_hashes = total_hashes.saturating_add(block_hashes);

        // Construct the canonical Block this miner just produced and run
        // it through `validate_pow_block`. Any node receiving this block
        // would call the same validator; failing here means the miner
        // produced something the network would reject, which must never
        // happen, so we treat it as a hard error.
        let coinbase_tx = zoka::mining::block_validator::build_coinbase_tx(
            wallet_address.clone(),
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
        let private_fee_claims_for_block = private_manager
            .create_local_private_fee_claims_for_block(
                &private_tx_hashes_for_block,
                &wallet_commitment,
            )
            .map_err(|e| format!("failed to build private fee claims: {e}"))?;
        let private_coinbase = private_manager
            .create_private_coinbase_output(
                &wallet_name,
                reward_for_block,
                height,
                &wallet_address,
                height.saturating_add(COINBASE_MATURITY_BLOCKS),
            )
            .map_err(|e| format!("failed to build private coinbase output: {e}"))?;
        if private_coinbase.recipient_commitment != wallet_commitment {
            return Err("private coinbase recipient mismatch".to_string());
        }
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
                zoka::api::global_network_runtime().record_private_tx_included_in_block(
                    private_tx_hashes_for_block.len() as u64,
                );
            }
            let mut wallet_state_changed = private_manager
                .mark_private_transactions_confirmed_locally(&private_tx_hashes_for_block)
                > 0
                || !private_fee_claims_for_block.is_empty();
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
            if !private_fee_claims_for_block.is_empty() {
                wallet_state_changed = true;
            }
            if wallet_state_changed {
                // Re-load the on-disk wallet manager so that any concurrent
                // updates (e.g. another local wallet sending while this batch
                // was running) are not silently overwritten.  Then replay the
                // confirmed-status updates, incoming-output credits, and fee
                // claims before flushing back to disk.
                let mut fresh_manager = load_private_wallet_manager_for_network(
                    &storage,
                    config.password.as_deref(),
                    network,
                )
                .unwrap_or(private_manager);
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
                if !private_fee_claims_for_block.is_empty() {
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

        mined_reward = mined_reward
            .checked_add(reward_for_block)
            .ok_or_else(|| "reward overflow".to_string())?;
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
        state.pending_rewards.push(PendingMiningReward {
            wallet_address: wallet_address.clone(),
            amount: reward_for_block,
            source_height: height,
            unlock_height: height.saturating_add(COINBASE_MATURITY_BLOCKS),
            block_hash: header_hash.clone(),
            private_coinbase: Some(private_coinbase),
        });
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
    let _ = settle_mature_mining_rewards(
        &storage,
        &mut private_manager,
        config.password.as_deref(),
        network,
        &mut state,
    )?;
    save_user_mining_state_for_network(network, &state)?;

    Ok(MiningBatchSummary {
        blocks_mined: mined_blocks_effective,
        reward_credited: mined_reward,
        last_difficulty_bits: last_difficulty,
        last_solve_time_secs: last_solve_secs,
        produced_blocks,
    })
}

/// If `message` is a `BlockResponse`, deserialize the encoded block and
/// run it through `state::chain::apply_block`. Returns a human-readable
/// verdict for the dashboard. Non-block messages return `None` so the
/// caller falls back to the generic "received" log line.
fn handle_inbound_block_message(
    node: &mut Node,
    message: &zoka::network::message::ZokaMessage,
    chain_id: &str,
    chain_db_path: Option<&str>,
    peer_view: &mut zoka::state::sync::PeerChainView,
    unsafe_rehearsal_difficulty_bits: Option<u32>,
) -> Option<String> {
    let response = match message {
        zoka::network::message::ZokaMessage::BlockResponse(r) => r,
        _ => return None,
    };

    let height = response.block_height;

    let path = match chain_db_path {
        Some(p) => p,
        None => {
            return Some(format!(
                "Peer block @ height {height} ignored: no chain DB configured"
            ));
        }
    };

    let storage = match RocksDbStorage::new(path) {
        Ok(s) => s,
        Err(err) => {
            return Some(format!(
                "Peer block @ height {height} dropped: storage open failed ({err})"
            ));
        }
    };
    let local_height = storage.get_chain_height().unwrap_or(0);
    peer_view.observe(height, local_height);

    let outcome = match unsafe_rehearsal_difficulty_bits {
        Some(bits) => zoka::state::sync::try_apply_synced_block_payload_with_expected_difficulty(
            &storage, chain_id, response, bits,
        ),
        None => zoka::state::sync::try_apply_synced_block_payload(&storage, chain_id, response),
    };
    peer_view.record_sync_outcome(&outcome);
    peer_view.remember_block_response(&outcome, response);
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
    chain_db_path: Option<&str>,
    peer_view: &mut zoka::state::sync::PeerChainView,
) -> Option<String> {
    let height = announce.block_height;
    let hash = announce.block_hash.trim();
    if height == 0 || hash.is_empty() {
        return Some("Ignored malformed block announce: missing height/hash.".to_string());
    }

    let announced_work = announce.cumulative_work.parse::<u128>().ok();
    let local_head = chain_db_path
        .and_then(|path| RocksDbStorage::new(path).ok())
        .and_then(|storage| zoka::state::chain::read_chain_head(&storage).ok().flatten());
    let local_height = local_head.as_ref().map(|head| head.height).unwrap_or(0);
    peer_view.observe(height, local_height);

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

    match node.publish_block_request(height, Some(hash.to_string())) {
        Ok(()) => Some(format!(
            "Block announce @ height {height} received; requested body {hash}."
        )),
        Err(err) => Some(format!(
            "Block announce @ height {height} received but request publish failed: {err}"
        )),
    }
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
    chain_db_path: Option<&str>,
) -> Result<Option<String>, String> {
    let mut served = 0u64;
    let mut failed = 0u64;
    let mut last_height = 0u64;
    while let Some(pending) = node.take_pending_block_body_request() {
        let block_height = pending.request.block_height;
        last_height = block_height;
        let response = match chain_db_path {
            Some(path) => match RocksDbStorage::new(path) {
                Ok(storage) => block_body_response_from_storage(&storage, &pending.request),
                Err(err) => zoka::network::block_request_response::BlockBodyResponse::Rejected {
                    block_height,
                    block_hash: pending.request.block_hash.clone(),
                    reason: format!("storage open failed: {err}"),
                },
            },
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
    chain_db_path: Option<&str>,
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
            match chain_db_path {
                Some(path) => match RocksDbStorage::new(path) {
                    Ok(storage) => private_tx_body_response_from_storage(&storage, &pending.request),
                    Err(_) => {
                        zoka::network::private_tx_request_response::PrivateTxBodyResponse::RateLimited {
                            tx_hash,
                            retry_after_secs: 2,
                        }
                    }
                },
                None => zoka::network::private_tx_request_response::PrivateTxBodyResponse::NotFound {
                    tx_hash,
                    reason: "no chain DB configured".to_string(),
                },
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

    let max = (request.max as u64).min(512).min(tip.saturating_sub(start) + 1);
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

    let mut difficulty_bits = storage.get_current_difficulty_bits().unwrap_or(
        zoka::economics::POW_INITIAL_DIFFICULTY_BITS,
    );

    for h in start..=(start + max - 1) {
        if let Some(stop) = &request.stop_hash {
            if !stop.is_empty() {
                if headers.last().map(|s| &s.header.hash == stop).unwrap_or(false) {
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
    chain_db_path: Option<&str>,
) -> Result<Option<String>, String> {
    let mut served = 0u32;
    let mut failed = 0u32;
    while let Some(pending) = node.take_pending_header_request() {
        let response = match chain_db_path {
            Some(path) => match RocksDbStorage::new(path) {
                Ok(storage) => headers_response_from_storage(&storage, &pending.request),
                Err(err) => zoka::network::header_request_response::HeadersResponse::NotFound {
                    reason: format!("storage open failed: {err}"),
                },
            },
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

fn serve_pending_snapshot_manifest_requests(
    node: &mut zoka::network::p2p::Node,
) -> Result<Option<String>, String> {
    use zoka::network::snapshot_request_response::{SnapshotManifest, SnapshotManifestsResponse};

    let mut served = 0u32;
    let mut failed = 0u32;
    while let Some(pending) = node.take_pending_snapshot_manifest_request() {
        let dir = zoka::snapshot::default_snapshot_dir();
        let file_manifests = zoka::snapshot::scan_snapshot_manifests(&dir);
        let chain_id = &pending.request.chain_id;
        let max_height = pending.request.max_height;

        let network_manifests: Vec<SnapshotManifest> = file_manifests
            .into_iter()
            .filter(|m| {
                m.chain_id == *chain_id
                    && max_height.map_or(true, |h| m.head_height <= h)
            })
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
                signature_hex: m.signature_hex,
                signer_pubkey_hex: m.signer_pubkey_hex,
            })
            .collect();

        let response = if network_manifests.is_empty() {
            SnapshotManifestsResponse::NotAvailable {
                reason: "no snapshots available for the requested chain/height".to_string(),
            }
        } else {
            SnapshotManifestsResponse::Manifests { manifests: network_manifests }
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

async fn handle_fullnode_network_message(
    node: &mut Node,
    message: &zoka::network::message::ZokaMessage,
    chain_id: &str,
    chain_db_path: Option<&str>,
    peer_view: &mut zoka::state::sync::PeerChainView,
    unsafe_rehearsal_difficulty_bits: Option<u32>,
) -> Option<String> {
    match message {
        zoka::network::message::ZokaMessage::BlockResponse(_) => handle_inbound_block_message(
            node,
            message,
            chain_id,
            chain_db_path,
            peer_view,
            unsafe_rehearsal_difficulty_bits,
        ),
        zoka::network::message::ZokaMessage::BlockAnnounce(announce) => {
            handle_block_announce(node, announce, chain_db_path, peer_view)
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
            let local_height = chain_db_path
                .and_then(|path| RocksDbStorage::new(path).ok())
                .and_then(|storage| storage.get_chain_height().ok())
                .unwrap_or(0);
            peer_view.observe(sync.last_height, local_height);
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

fn publish_local_sync_state(
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
                "Skipped local sync-state publish: storage open failed ({err})"
            )));
        }
    };
    let head = match zoka::state::chain::read_chain_head(&storage) {
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

fn shuffled_peer_addrs(seed_addrs: &[String]) -> Vec<String> {
    let mut shuffled = seed_addrs.to_vec();
    shuffled.shuffle(&mut rand::thread_rng());
    shuffled
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
    let next_request = match zoka::state::sync::next_required_block(&storage, peer_view)? {
        Some(request) => request,
        None if node.connected_peer_count() > 0 && peer_view.perceived_max_height == 0 => {
            let local_height = storage
                .get_chain_height()
                .map_err(|err| format!("read chain height failed: {err}"))?;
            // Speculatively probe for the next block when peers are connected but
            // no gossip height announcement has arrived yet (e.g. RPC is down).
            // If the peer doesn't have it we'll get a "pending" error and retry.
            zoka::state::sync::SyncRequest {
                height: local_height.saturating_add(1).max(1),
                expected_hash: None,
            }
        }
        None => return Ok(None),
    };
    let next_height = next_request.height;

    let (response, response_source) = if let Some(cached) =
        peer_view.take_pending_block_response(&next_request)
    {
        (cached, "cached")
    } else {
        match node
            .request_block_with_retry(
                next_height,
                next_request.expected_hash.clone(),
                3,
                Duration::from_secs(8),
            )
            .await
        {
            Ok(response) => (response, "peer"),
            Err(err) => {
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
    peer_view.record_sync_outcome(&outcome);
    peer_view.remember_block_response(&outcome, &response);
    if matches!(
        &outcome,
        zoka::state::sync::SyncOutcome::BadPayload(_)
            | zoka::state::sync::SyncOutcome::AnnouncedHashMismatch { .. }
            | zoka::state::sync::SyncOutcome::Rejected(_)
    ) {
        tracing::warn!(
            target: "zoka.sync",
            height = next_height,
            source = response_source,
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

        peer_view.observe(remote_height, local_height);
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
        network.label(),
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
    node.connect_to_peer(dial_addr)
        .await
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

async fn maintain_seed_connections(
    node: &mut Node,
    seed_addrs: &[String],
    target_peers: usize,
) -> Option<String> {
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
    let shuffled_seed_addrs = shuffled_peer_addrs(seed_addrs);
    for seed in &shuffled_seed_addrs {
        attempted += 1;
        let before = node.connected_peer_count();
        match node.connect_to_peer(seed).await {
            Ok(()) => {
                let next_floor = before.saturating_add(1).min(target_peers).max(1);
                match node
                    .wait_for_min_connected_peers(next_floor, Duration::from_secs(4))
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
            Err(err) => {
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
    println!("Network: {}", network.label());
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

async fn cmd_fullnode_auto(args: &[String]) -> Result<(), String> {
    let network = NetworkProfile::from_args(args)?;
    let chain_id = network.chain_id().to_string();
    zoka::api::global_startup_status().reset(
        "starting",
        Some(format!("starting {} fullnode", network.label())),
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
    if let Some(path) = ready_file.as_deref() {
        write_fullnode_ready_file(
            path,
            "starting",
            serde_json::json!({
                "network": network.label(),
                "chain_id": chain_id.as_str(),
                "db": db_path,
            }),
        )?;
    }
    let listen_addr =
        option_value(args, "--listen").unwrap_or_else(|| "/ip4/0.0.0.0/tcp/28080".to_string());
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
    let auto_mine = !has_flag(args, "--no-mine");
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
    let mining_threads = parse_thread_count_arg(args)?;
    let mining_target_block_time_secs = POW_TARGET_BLOCK_TIME_SECS;
    let mut mining_hashrate_hps = 0;
    let mining_base_difficulty = POW_INITIAL_DIFFICULTY_BITS;
    let current_mining_state = load_user_mining_state_for_network(network)?;
    if auto_mine && legacy_pow_sim_state_detected(&current_mining_state) {
        return Err(legacy_pow_sim_reset_message());
    }

    let storage = wallet_storage_for_network(network)?;
    let (manager, wallet_name, created) = select_or_create_private_node_wallet(
        &storage,
        password.as_deref(),
        wallet_selector.as_deref(),
        true,
        network,
    )?;
    let wallet = manager
        .get_private_wallet(&wallet_name)
        .ok_or_else(|| format!("wallet not found after selection: {wallet_name}"))?;
    let export = private_wallet_secrets_export(wallet);
    let wallet_address = export.user_address.clone();
    if let Some(path) = ready_file.as_deref() {
        write_fullnode_ready_file(
            path,
            "wallet_loaded",
            serde_json::json!({
                "network": network.label(),
                "chain_id": chain_id.as_str(),
                "db": db_path,
                "wallet": wallet_name,
                "wallet_address": wallet_address,
            }),
        )?;
    }

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
    let mut startup_notes = vec![
        format!("Network profile: {} ({}).", network.label(), chain_id),
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

    // Receiver half of the API → fullnode-main-loop channel; populated
    // only when the HTTP API is spun up (i.e. `--db` path present).
    let mut private_tx_recv_outer: Option<
        tokio::sync::mpsc::UnboundedReceiver<zoka::private_wallet::PrivateTransaction>,
    > = None;
    let mut chain_storage_outer: Option<Arc<RocksDbStorage>> = None;

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
            let manifest = zoka::snapshot::scan_snapshot_manifests(
                &zoka::snapshot::default_snapshot_dir(),
            )
            .into_iter()
            .find(|m| {
                m.chain_id == chain_id
                    && m.head_height >= local_height.saturating_add(MIN_SNAPSHOT_LEAD_BLOCKS)
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
                                        let mut verified = false;
                                        let mut last_verify_err = String::new();
                                        for signer_hex in &trusted_signers {
                                            let Ok(decoded) = hex::decode(signer_hex.trim())
                                            else {
                                                continue;
                                            };
                                            if decoded.len() != 32 {
                                                continue;
                                            }
                                            let mut pubkey = [0u8; 32];
                                            pubkey.copy_from_slice(&decoded);
                                            let opts = zoka::snapshot::VerifyOptions {
                                                expected_chain_id: chain_id.as_str(),
                                                require_signature: true,
                                                expected_pubkey: Some(&pubkey),
                                            };
                                            match zoka::snapshot::verify(&snapshot, &opts) {
                                                Ok(_) => {
                                                    verified = true;
                                                    break;
                                                }
                                                Err(e) => last_verify_err = e.to_string(),
                                            }
                                        }
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
            async move {
                if let Err(e) = zoka::api::start_api_on_bind_with_broadcaster(
                    manager,
                    api_mempool,
                    api_bind_addr,
                    api_broadcaster,
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
    let mut node = Node::new(0).await.map_err(|e| {
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
    node.listen(&listen_addr).await.map_err(|e| {
        zoka::api::global_startup_status().mark_failed(Some(format!("listen error: {e}")));
        format!("listen error: {e}")
    })?;
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
        write_fullnode_ready_file(
            path,
            "ready",
            serde_json::json!({
                "network": network.label(),
                "chain_id": chain_id.as_str(),
                "peer_id": peer_id,
                "listen_address": announced,
                "seed_address": format!("{}/p2p/{}", announced, peer_id),
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
                rpc_sync_endpoints = network_config
                    .public_rpc
                    .iter()
                    .filter(|rpc| rpc.status.eq_ignore_ascii_case("active"))
                    .map(|rpc| rpc.url.clone())
                    .collect();
                startup_notes.push(format!(
                    "{} network config loaded from {} for public RPCs: status={}, active_rpc={}.",
                    network.label(),
                    config_path.display(),
                    if network_config.status.is_empty() {
                        "unknown"
                    } else {
                        network_config.status.as_str()
                    },
                    network_config.active_rpc_count(),
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
                network.label(),
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
                network.label(),
                config_path.display()
            ));
        } else {
            startup_notes.push(format!(
                "Default {} network config not found at {}; running standalone until peers connect.",
                network.label(),
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

    let session_started = Instant::now();
    let mut session_mined_blocks = 0u64;
    let mut session_mined_rewards = 0u64;
    let mut last_event = startup_notes.join(" ");
    let mut peer_chain_view = zoka::state::sync::PeerChainView::default();
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
    let mut warmup_seed_reconnect_failures = 0u32;
    while tokio::time::Instant::now() < warmup_deadline {
        let mut now = tokio::time::Instant::now();
        if now >= next_warmup_seed_reconnect_at {
            let target_seed_peers = reconnect_seed_peers.len().min(3).max(1);
            if let Some(reconnect_event) =
                maintain_seed_connections(&mut node, &reconnect_seed_peers, target_seed_peers).await
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
        match timeout(Duration::from_millis(500), node.next_message()).await {
            Ok(Ok(message)) => {
                last_event = if let Some(verdict) = handle_fullnode_network_message(
                    &mut node,
                    &message,
                    &chain_id,
                    db_path.as_deref(),
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
                    if let Some(sync_event) = run_rpc_sync_batch(
                        &chain_id,
                        db_path.as_deref(),
                        &rpc_sync_endpoints,
                        &mut peer_chain_view,
                        unsafe_rehearsal_difficulty_bits,
                        32,
                    )? {
                        next_warmup_ibd_at =
                            tokio::time::Instant::now() + ibd_retry_delay(&sync_event);
                        remember_sync_event(
                            "rpc",
                            &sync_event,
                            &mut last_sync_source,
                            &mut last_sync_event,
                            &mut last_sync_error,
                            &mut rpc_fallback_active,
                        );
                        last_event = sync_event;
                    } else if let Some(sync_event) = run_sync_step(
                        &mut node,
                        &chain_id,
                        db_path.as_deref(),
                        &mut peer_chain_view,
                        unsafe_rehearsal_difficulty_bits,
                    )
                    .await?
                    {
                        next_warmup_ibd_at =
                            tokio::time::Instant::now() + ibd_retry_delay(&sync_event);
                        remember_sync_event(
                            "p2p",
                            &sync_event,
                            &mut last_sync_source,
                            &mut last_sync_event,
                            &mut last_sync_error,
                            &mut rpc_fallback_active,
                        );
                        last_event = sync_event;
                    } else {
                        next_warmup_ibd_at = tokio::time::Instant::now() + Duration::from_secs(1);
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
                    if let Some(sync_event) = run_rpc_sync_batch(
                        &chain_id,
                        db_path.as_deref(),
                        &rpc_sync_endpoints,
                        &mut peer_chain_view,
                        unsafe_rehearsal_difficulty_bits,
                        32,
                    )? {
                        next_warmup_ibd_at =
                            tokio::time::Instant::now() + ibd_retry_delay(&sync_event);
                        remember_sync_event(
                            "rpc",
                            &sync_event,
                            &mut last_sync_source,
                            &mut last_sync_event,
                            &mut last_sync_error,
                            &mut rpc_fallback_active,
                        );
                        last_event = sync_event;
                    } else if let Some(sync_event) = run_sync_step(
                        &mut node,
                        &chain_id,
                        db_path.as_deref(),
                        &mut peer_chain_view,
                        unsafe_rehearsal_difficulty_bits,
                    )
                    .await?
                    {
                        next_warmup_ibd_at =
                            tokio::time::Instant::now() + ibd_retry_delay(&sync_event);
                        remember_sync_event(
                            "p2p",
                            &sync_event,
                            &mut last_sync_source,
                            &mut last_sync_event,
                            &mut last_sync_error,
                            &mut rpc_fallback_active,
                        );
                        last_event = sync_event;
                    } else {
                        next_warmup_ibd_at = tokio::time::Instant::now() + Duration::from_secs(1);
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
    let mut next_seed_reconnect_at = tokio::time::Instant::now();
    let mut seed_reconnect_failures = 0u32;
    let mut next_ibd_at = tokio::time::Instant::now();
    let mut next_miner_heartbeat_at = tokio::time::Instant::now();
    let mut next_peer_store_save_at = tokio::time::Instant::now() + Duration::from_secs(15);
    // Periodic RPC pull of pending private txs from public peers. This is the
    // NAT-friendly counterpart to libp2p gossip: a miner that can only open
    // outbound HTTP pulls pending private txs into its own pool so it can mine
    // them, even when inbound P2P gossip never reaches it.
    let mut next_private_pull_at = tokio::time::Instant::now();
    let mut next_private_body_pull_at = tokio::time::Instant::now();
    let mut last_private_body_scan_height = 0u64;
    let mut heartbeat_last_status_log = HashMap::<String, bool>::new();
    let mut heartbeat_recent_failures: VecDeque<(String, String)> = VecDeque::with_capacity(8);
    let mut last_runtime_mining_enabled = auto_mine;
    let mut mining_task: Option<tokio::task::JoinHandle<Result<MiningBatchSummary, String>>> = None;
    loop {
        if let Some(deadline) = maybe_deadline {
            if tokio::time::Instant::now() >= deadline {
                last_event = "Fullnode session time limit reached.".to_string();
                break;
            }
        }

        let mining_active = mining_task.is_some();
        let mining_control = zoka::api::global_local_mining_control().snapshot();
        let runtime_mining_enabled = auto_mine || mining_control.enabled;
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
        if !runtime_mining_enabled && !mining_active && runtime_mining_state.is_mining {
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
        zoka::api::global_network_runtime().set_gossip_stats(node.gossip_stats());
        zoka::api::global_network_runtime()
            .set_block_request_response_stats(node.block_request_response_stats());
        zoka::api::global_network_runtime()
            .set_local_miner(
                node.peer_id().to_string(),
                runtime_mining_enabled || mining_active,
                runtime_chain_height,
                if runtime_mining_enabled || mining_active {
                    runtime_mining_state.estimated_hashrate_hps
                } else {
                    0
                },
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
            serve_pending_block_body_requests(&mut node, db_path.as_deref())?
        {
            last_event = direct_event;
        }
        if let Some(private_tx_event) =
            serve_pending_private_tx_body_requests(&mut node, db_path.as_deref())?
        {
            last_event = private_tx_event;
        }
        if let Some(header_event) =
            serve_pending_header_requests(&mut node, db_path.as_deref())?
        {
            last_event = header_event;
        }
        if let Some(snapshot_event) = serve_pending_snapshot_manifest_requests(&mut node)? {
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
            if let Some(sync_event) = run_rpc_sync_batch(
                &chain_id,
                db_path.as_deref(),
                &rpc_sync_endpoints,
                &mut peer_chain_view,
                unsafe_rehearsal_difficulty_bits,
                64,
            )? {
                next_ibd_at = tokio::time::Instant::now() + ibd_retry_delay(&sync_event);
                remember_sync_event(
                    "rpc",
                    &sync_event,
                    &mut last_sync_source,
                    &mut last_sync_event,
                    &mut last_sync_error,
                    &mut rpc_fallback_active,
                );
                last_event = sync_event;
            } else if let Some(sync_event) = run_sync_step(
                &mut node,
                &chain_id,
                db_path.as_deref(),
                &mut peer_chain_view,
                unsafe_rehearsal_difficulty_bits,
            )
            .await?
            {
                next_ibd_at = tokio::time::Instant::now() + ibd_retry_delay(&sync_event);
                remember_sync_event(
                    "p2p",
                    &sync_event,
                    &mut last_sync_source,
                    &mut last_sync_event,
                    &mut last_sync_error,
                    &mut rpc_fallback_active,
                );
                last_event = sync_event;
            } else {
                next_ibd_at = tokio::time::Instant::now() + Duration::from_secs(1);
            }
        }

        if tokio::time::Instant::now() >= next_seed_reconnect_at {
            let target_seed_peers = reconnect_seed_peers.len().min(3).max(1);
            if let Some(reconnect_event) =
                maintain_seed_connections(&mut node, &reconnect_seed_peers, target_seed_peers).await
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
            if let Some(sync_event) = publish_local_sync_state(&mut node, db_path.as_deref())? {
                last_event = sync_event;
            }
            if let Some(announce_event) =
                publish_chain_head_block_announce(&mut node, db_path.as_deref())?
            {
                last_event = announce_event;
            }
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
            next_sync_state_at = tokio::time::Instant::now() + Duration::from_secs(3);
        }

        // Pull pending private txs from public RPC peers so a NAT'd miner can
        // include txs that were published to a droplet but never gossiped to
        // it. Bodies are dropped into the sidecar queue, which the mining loop
        // already drains into the local pending pool — this avoids fighting the
        // miner for the RocksDB write lock. Runs on every node with RPC
        // endpoints; harmless on the droplets (they re-stage what they have).
        if tokio::time::Instant::now() >= next_private_pull_at {
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
        if tokio::time::Instant::now() >= next_private_body_pull_at {
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
            last_event = if publish_failures == 0 {
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
            if rpc_submit_failures > 0 {
                last_event = format!(
                    "{last_event} RPC autopush had {rpc_submit_failures} endpoint failure(s); announce/request sync will keep peers aware of the new head."
                );
            }
            if let Some(sync_event) = publish_local_sync_state(&mut node, db_path.as_deref())? {
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
                last_event = format!(
                    "Auto-mining peer gate satisfied by RPC checkpoint at height {local_height} via {rpc}."
                );
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
        if let Err(err) = draw_fullnode_runtime_screen(
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

        match timeout(Duration::from_millis(800), node.next_message()).await {
            Ok(Ok(message)) => {
                last_event = match handle_fullnode_network_message(
                    &mut node,
                    &message,
                    &chain_id,
                    db_path.as_deref(),
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
    Ok(())
}

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
    let storage = wallet_storage_for_network(NetworkProfile::Mainnet)?;
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
    cmd_fullnode_auto(&args).await
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
    println!("Wallets ({})", network.label());
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
    if as_json {
        println!(
            "{}",
            serde_json::json!({
                "wallet": wallet_name,
                "address": address,
                "view_only": wallet.view_only,
            })
        );
        return Ok(());
    }
    println!("Receive ZKA");
    println!("Wallet: {}", wallet_name);
    println!("Address: {}", address);
    println!("Share this address to receive private ZKA.");
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
            network.label()
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
    let mut manager =
        load_private_wallet_manager_for_network(&storage, password.as_deref(), network)?;
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

    let to_commitment = require_private_network_address(&to_raw, "--to")?;
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
    if as_json {
        println!(
            "{}",
            serde_json::json!({
                "db": db_path,
                "count": hashes.len(),
                "pending_private_tx_hashes": hashes,
            })
        );
        return Ok(());
    }
    println!("Pending private transfers");
    println!("Node DB: {}", db_path);
    if hashes.is_empty() {
        println!("No pending private transfers.");
        println!(
            "When you send from the console, the transfer is queued here for your local node to mine."
        );
        return Ok(());
    }
    for hash in hashes {
        println!("- {}", hash);
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
    let to_commitment = require_private_network_address(&to, "--to")?;
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
    let mut stream = TcpStream::connect((url.host.as_str(), url.port))
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

fn best_reachable_rpc_height_from_args(
    args: &[String],
    network: NetworkProfile,
) -> Result<RpcHeightSample, String> {
    if let Some(rpc) = option_value(args, "--rpc") {
        parse_rpc_http_url(&rpc)?;
        let height = rpc_chain_height(&rpc, 3)?;
        return Ok(RpcHeightSample { rpc, height });
    }

    let active = active_rpc_endpoints_from_config(args, network)?;
    let endpoints = active.iter().map(|rpc| rpc.url.clone()).collect::<Vec<_>>();
    if endpoints.is_empty() {
        return Err("no active RPC endpoint configured; pass --rpc http://host:port".to_string());
    }
    let (samples, errors) = sample_rpc_heights(&endpoints, 3);
    samples.into_iter().next().ok_or_else(|| {
        if errors.is_empty() {
            "no reachable active RPC endpoint configured".to_string()
        } else {
            format!(
                "no reachable active RPC endpoint configured: {}",
                errors.join("; ")
            )
        }
    })
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

fn standalone_mining_sync_gate(
    args: &[String],
    network: NetworkProfile,
    db_path: Option<&str>,
) -> Result<Option<String>, String> {
    if has_flag(args, "--allow-unsynced-mining") {
        return Ok(Some(
            "Sync gate bypassed by --allow-unsynced-mining; this is only for isolated development."
                .to_string(),
        ));
    }
    if network != NetworkProfile::Mainnet {
        return Ok(None);
    }

    let best_rpc = best_reachable_rpc_height_from_args(args, network)?;
    let rpc = best_rpc.rpc;
    let network_height = best_rpc.height;
    let local_height = current_known_height_for_network(db_path, network);
    ensure_local_chain_matches_rpc_at_height(&rpc, db_path, local_height.min(network_height))?;
    if network_height > local_height {
        return Err(format!(
            "mining locked until sync completes: local height {local_height}, network height {network_height} via {rpc}. Start `zokahd --network {}` first and wait until local height catches up, or pass --allow-unsynced-mining only for isolated development.",
            network.label()
        ));
    }
    if local_height > network_height {
        return Err(format!(
            "mining locked because local height {local_height} is ahead of network bootstrap height {network_height} via {rpc}. Wait for the bootstrap to catch up or run a connected fullnode session; pass --allow-unsynced-mining only for isolated development."
        ));
    }

    Ok(Some(format!(
        "Sync gate OK: local height {local_height}, network height {network_height} via {rpc}."
    )))
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
                "coinbase_maturity_blocks": COINBASE_MATURITY_BLOCKS,
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
                "coinbase_maturity_blocks": COINBASE_MATURITY_BLOCKS,
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

fn cmd_mine_easy(args: &[String]) -> Result<(), String> {
    let network = NetworkProfile::from_args(args)?;
    let chain_id = network.chain_id().to_string();
    let wallet_selector_raw = wallet_selector_value(args)
        .ok_or_else(|| "missing required argument: --address <wallet-address>".to_string())?;
    let wallet_commitment = require_private_network_address(&wallet_selector_raw, "--address")?;
    let wallet_selector = user_private_address_display(&wallet_commitment);
    let password = password_value(args)?;
    let db_path =
        option_value(args, "--db").or_else(|| Some(network.default_db_path().to_string()));
    let rpc_sync_endpoints = rpc_sync_endpoints_from_args(args, network)?;
    let sync_gate_note = standalone_mining_sync_gate(args, network, db_path.as_deref())?;
    if let Some(path) = db_path.as_deref() {
        let storage = open_storage(path)?;
        ensure_atomic_economics_db(&storage)?;
    }
    let mining_threads = parse_thread_count_arg(args)?;
    let minutes_raw = option_value(args, "--minutes");
    let minutes = parse_u64_arg(minutes_raw.clone(), 0, "--minutes")?;
    let target_block_time_secs = POW_TARGET_BLOCK_TIME_SECS;
    let blocks_raw = option_value(args, "--blocks");
    let target_blocks = match blocks_raw {
        Some(raw) => Some(parse_u64_arg(Some(raw), 1, "--blocks")?),
        None if minutes > 0 => Some(((minutes.saturating_mul(60)) / target_block_time_secs).max(1)),
        None => None,
    };
    if matches!(target_blocks, Some(0)) {
        return Err("--blocks must be >= 1".to_string());
    }
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

    let base_difficulty = POW_INITIAL_DIFFICULTY_BITS;
    let deadline = if minutes == 0 {
        None
    } else {
        Some(Instant::now() + Duration::from_secs(minutes.saturating_mul(60)))
    };

    let wallet_storage = wallet_storage_for_network(network)?;
    let wallet = load_wallet_runtime_snapshot_from_storage(
        &wallet_storage,
        &wallet_selector,
        password.as_deref(),
        network,
    )?;
    let mut state = load_user_mining_state_for_network(network)?;
    if legacy_pow_sim_state_detected(&state) {
        return Err(legacy_pow_sim_reset_message());
    }
    state.is_mining = true;
    state.mining_process_id = Some(std::process::id());
    state.mode = network.mining_mode().to_string();
    state.algorithm = active_pow_algorithm_for_chain(&chain_id).to_string();
    state.estimated_hashrate_hps = 0;
    state.current_difficulty_bits = state
        .current_difficulty_bits
        .max(base_difficulty)
        .clamp(POW_MIN_DIFFICULTY_BITS, POW_MAX_DIFFICULTY_BITS);
    if let Some(bits) = unsafe_rehearsal_difficulty_bits {
        state.current_difficulty_bits = bits;
    }
    state.last_updated = now_secs();
    save_user_mining_state_for_network(network, &state)?;
    let hashrate_hps = benchmark_hashrate_hps_for_chain(&chain_id, 500);
    state.estimated_hashrate_hps = hashrate_hps;
    state.last_updated = now_secs();
    save_user_mining_state_for_network(network, &state)?;

    let session_started = Instant::now();
    let mut session_blocks = 0u64;
    let mut session_rewards = 0u64;
    let ready_line = format!(
        "Ready. Mining {} into {} with {} at {} bits using {}.",
        network.label(),
        wallet.address,
        active_pow_algorithm_for_chain(&chain_id),
        unsafe_rehearsal_difficulty_bits.unwrap_or(base_difficulty),
        mining_power_label_for_threads(mining_threads)
    );
    let mut status_line = sync_gate_note
        .as_deref()
        .map(|note| format!("{note} {ready_line}"))
        .unwrap_or(ready_line);

    let render = |current_state: &UserMiningState,
                  status_line: &str,
                  session_blocks: u64,
                  session_rewards: u64|
     -> Result<(), String> {
        let wallet_snapshot = load_wallet_runtime_snapshot_from_storage(
            &wallet_storage,
            &wallet_selector,
            password.as_deref(),
            network,
        )?;
        let fee_snapshot = refresh_fee_market_snapshot(db_path.as_deref(), password.as_deref());
        let known_height = current_known_height_for_network(db_path.as_deref(), network);
        let screen = render_mining_dashboard(
            Some(&wallet_snapshot),
            current_state,
            &fee_snapshot,
            known_height,
            session_started.elapsed(),
            session_blocks,
            session_rewards,
            target_blocks,
            deadline,
            Some(mining_threads),
            status_line,
        );
        set_terminal_title("ZOKA Mining 1.0")?;
        clear_runtime_screen()?;
        print!("{}", screen);
        io::stdout()
            .flush()
            .map_err(|e| format!("stdout flush failed: {e}"))?;
        Ok(())
    };

    render(&state, &status_line, session_blocks, session_rewards)?;
    let mut live_state = state.clone();

    loop {
        if let Some(limit) = target_blocks {
            if session_blocks >= limit {
                status_line = format!("Session completed. Target {} block(s) reached.", limit);
                break;
            }
        }
        if let Some(limit) = deadline {
            if Instant::now() >= limit {
                status_line = "Session completed. Time limit reached.".to_string();
                break;
            }
        }

        let mining_config = MiningEngineConfig {
            chain_id: chain_id.clone(),
            wallet_selector: wallet_selector.clone(),
            password: password.clone(),
            blocks: 1,
            mode: network.mining_mode().to_string(),
            hashrate_hps,
            threads: mining_threads,
            base_difficulty,
            unsafe_rehearsal_difficulty_bits,
            target_block_time_secs,
            chain_db_path: db_path.clone(),
            rpc_sync_endpoints: rpc_sync_endpoints.clone(),
        };
        let batch = {
            let mut last_live_render: Option<Instant> = None;
            let mut last_progress_log = Instant::now() - Duration::from_secs(10);
            let mut progress_callback = |progress: &MiningProgressSnapshot| -> Result<(), String> {
                let should_render = last_live_render
                    .map(|last| last.elapsed() >= Duration::from_millis(800))
                    .unwrap_or(true);
                if !should_render {
                    return Ok(());
                }
                if last_progress_log.elapsed() >= Duration::from_secs(3) {
                    mining_progress_log(progress);
                    last_progress_log = Instant::now();
                }

                live_state.is_mining = true;
                live_state.mining_process_id = Some(std::process::id());
                live_state.current_difficulty_bits = progress.difficulty_bits;
                live_state.estimated_hashrate_hps = if progress.estimated_hashrate_hps > 0.0 {
                    progress.estimated_hashrate_hps.ceil() as u64
                } else {
                    0
                };
                live_state.last_updated = now_secs();
                save_user_mining_state_for_network(network, &live_state)?;

                let live_status = format!(
                    "Mining block {} | tried {} hash(es) | {} | nonce {} | elapsed {}.",
                    progress.height,
                    progress.hashes_attempted,
                    format_hashrate_hps(progress.estimated_hashrate_hps),
                    progress.nonce,
                    format_runtime_duration(Duration::from_secs_f64(
                        progress.elapsed_secs.max(0.0)
                    ))
                );
                render(&live_state, &live_status, session_blocks, session_rewards)?;
                last_live_render = Some(Instant::now());
                Ok(())
            };
            run_mining_batch_with_progress(&mining_config, Some(&mut progress_callback))?
        };
        session_blocks = session_blocks.saturating_add(batch.blocks_mined);
        session_rewards = session_rewards.saturating_add(batch.reward_credited);
        status_line = format!(
            "Last block solved in {:.2}s at {} bits. Reward {} scheduled for maturity.",
            batch.last_solve_time_secs,
            batch.last_difficulty_bits,
            format_zka(batch.reward_credited)
        );
        state = load_user_mining_state_for_network(network)?;
        live_state = state.clone();
        render(&state, &status_line, session_blocks, session_rewards)?;
    }

    state = load_user_mining_state_for_network(network)?;
    state.is_mining = false;
    state.mining_process_id = None;
    save_user_mining_state_for_network(network, &state)?;
    render(&state, &status_line, session_blocks, session_rewards)?;
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
        if args.is_empty() {
            return run_zokahd_guided_default().await;
        }
        let first = &args[0];
        if first == "--help" || first == "-h" || first == "help" {
            println!("zokahd (automatic fullnode)");
            println!(
                "Usage: zokahd [--network mainnet|testnet] [--address <wallet-address>] [--password <pwd>|--password-env <var>|--password-stdin] [--seed <multiaddr> ...] [--network-config <file>] [--no-network-config] [--allow-clearnet-seeds] [--allow-isolated-start] [--seconds <n>] [--show-keys] [--no-mine] [--power low|normal|max] [--threads <n>] [--db <path>] [--genesis <file>] [--tor-proxy host:port] [--tor-only] [--inline]"
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
            return cmd_fullnode_auto(&args).await;
        }
        if first == "fullnode" {
            return cmd_fullnode_auto(&args[1..]).await;
        }
    }

    if args.is_empty() {
        return run_interactive_menu().await;
    }

    let command = &args[0];
    let tail = &args[1..];
    match command.as_str() {
        "version" | "--version" | "-V" => {
            println!("zoka {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "help" | "--help" | "-h" => {
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
        "node" => cmd_fullnode_auto(tail).await,
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
        "private-pending-diagnostics" => cmd_private_pending_diagnostics(tail),
        "tx-pending" => cmd_pending_transfers(tail),
        "estimate-fee" => cmd_estimate_fee(tail),
        "fullnode" => cmd_fullnode_auto(tail).await,
        "fullnode-status" => cmd_node_status(tail),
        "tx-send" => cmd_tx_send_unified(tail),
        "tx-send-many" => {
            Err("tx-send-many is disabled in unified private wallet mode".to_string())
        }
        "mine-status" => cmd_mine_status(tail),
        "mine-console" => cmd_mine_console(tail),
        "dashboard" => cmd_status_overview(tail),
        "mine" => {
            let args = normalize_positional_alias_args(tail, &["--address"]);
            cmd_mine_easy(&args)
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
        zoka::snapshot::sign(&mut snapshot, &key)
            .map_err(|e| format!("snapshot signing failed: {e}"))?;
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

#[tokio::main]
async fn main() {
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
    use super::*;

    fn temp_db_arg() -> (tempfile::TempDir, String) {
        let tmp = tempfile::TempDir::new().expect("temp db dir");
        let db = tmp.path().join("chain.db");
        (tmp, db.to_string_lossy().to_string())
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
            load_private_wallet_manager_for_network(&storage, None, NetworkProfile::Mainnet)
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

    #[test]
    fn atomic_economics_marker_rejects_unmarked_nonempty_db() {
        let (_tmp, db) = temp_db_arg();
        let storage = RocksDbStorage::new(&db).expect("temp rocksdb");
        storage.set_chain_height(1).expect("set height");

        let err = ensure_atomic_economics_db(&storage).unwrap_err();
        assert!(err.contains("no atomic economics marker"));
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
            genesis_pow_hash_for_chain(NetworkProfile::Mainnet.chain_id()),
        );
        storage
            .set_total_supply_emitted(reward)
            .expect("set emitted supply");
        storage
            .put_raw(
                "chain:genesis:network",
                NetworkProfile::Mainnet.chain_id().as_bytes().to_vec(),
            )
            .expect("set genesis network");

        let report = migrate_atomic_economics_marker(
            &storage,
            &db,
            NetworkProfile::Mainnet,
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
            genesis_pow_hash_for_chain(NetworkProfile::Mainnet.chain_id()),
        );
        storage
            .set_total_supply_emitted(12)
            .expect("set emitted supply");

        let err = migrate_atomic_economics_marker(
            &storage,
            &db,
            NetworkProfile::Mainnet,
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
            current_known_height_for_network(Some(&db), NetworkProfile::Mainnet),
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
          "protocol_freeze_id": "/zoka/1.0.2-mainnet-zenith",
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
          "protocol_freeze_id": "/zoka/1.0.2-mainnet-zenith",
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
          "protocol_freeze_id": "/zoka/1.0.2-mainnet-zenith",
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
          "protocol_freeze_id": "/zoka/1.0.2-mainnet-zenith",
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
