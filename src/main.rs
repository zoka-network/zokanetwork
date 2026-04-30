use std::fmt::Write as _;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use curve25519_dalek::constants::RISTRETTO_BASEPOINT_POINT;
use curve25519_dalek::scalar::Scalar;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::time::timeout;
use zoka::economics;
use zoka::identity::HDWallet;
use zoka::network::p2p::{PrivacyTransportMode, PrivacyTransportStatus, PROTOCOL_FREEZE_ID};
use zoka::network::Node;
use zoka::private_wallet::derive_spending_key_from_recovery_seed;
use zoka::private_wallet::PrivateWallet;
use zoka::private_wallet::PrivateWalletManager;
use zoka::state::StateManager;
use zoka::storage::RocksDbStorage;
use zoka::wallet::storage::WalletStorage;

fn print_usage() {
    println!();
    println!("{}", dashboard_title("zoka CLI", ANSI_CYAN));
    println!("{}", dashboard_divider());
    println!(
        "  {}",
        paint(
            "Private-by-default wallet, transfer, mining and fullnode commands.",
            ANSI_DIM
        )
    );
    println!("{}", dashboard_divider());
    println!("{}", dashboard_section("Wallet", ANSI_MAGENTA));
    println!("  wallet-create [--name <label>] --password <pwd> [--export-secrets <path>]");
    println!("  wallet-restore-from-seed [--name <label>] --seed \"<bip39 words>\" --password <pwd> [--restore-height <n>]");
    println!("  wallet-restore-from-keys [--name <label>] --scan-key-hex <hex> [--spend-key-hex <hex> | --spend-public-key-hex <hex>] [--view-only] --password <pwd> [--restore-height <n>]");
    println!("  wallet-list [--password <pwd>] [--json]");
    println!("  wallet-balance --address <wallet-address> [--password <pwd>] [--json]");
    println!(
        "  wallet-history --address <wallet-address> [--password <pwd>] [--limit <n>] [--json]"
    );
    println!("  wallet-backup --address <wallet-address> --out <path> [--password <pwd>]");
    println!("  wallet-wipe --address <wallet-address> [--password <pwd>] [--force]");
    println!("  wallet-change-passphrase --address <wallet-address> [--old-password <pwd>] --new-password <pwd>");
    println!("  wallet-export-secrets --address <wallet-address> [--password <pwd>] [--out <path>] [--json]");
    println!("{}", dashboard_section("Transfers", ANSI_BLUE));
    println!("  tx-send --from <wallet-address|name> --to <wallet|address> --amount <n> [--fee <n>] [--priority low|normal|high] [--password <pwd>] [--db <path>] --yes");
    println!(
        "  estimate-fee --amount <n> [--priority low|normal|high] [--password <pwd>] [--db <path>]"
    );
    println!("{}", dashboard_section("Chain", ANSI_YELLOW));
    println!("  height [--db <path>] [--json]");
    println!("  chain-stats [--db <path>] [--json]");
    println!("  sync-status --address <wallet-address> [--password <pwd>] [--db <path>] [--json]");
    println!("{}", dashboard_section("Mining", ANSI_GREEN));
    println!("  mine --address <wallet-address> [--password <pwd>] [--minutes <n>] [--blocks <n>]  (runs until interrupted if no limit is set)");
    println!("  mine-status [--json]");
    println!("  mine-console [--json]");
    println!("{}", dashboard_section("Node", ANSI_CYAN));
    println!("  fullnode [--address <wallet-address>] [--password <pwd>] [--seed <multiaddr>] [--network-config <file>] [--no-network-config] [--allow-clearnet-seeds] [--seconds <n>] [--show-keys] [--no-mine] [--db <path>] [--genesis <file>]  (--password required when auto-creating a wallet)");
    println!("{}", dashboard_section("Genesis", ANSI_YELLOW));
    println!("  genesis-verify <path-to-genesis.json> [--json]  (offline validation; prints chain id, pow algorithm, validator count, sha-256 digest)");
    println!(
        "  mainnet-network-lint [path-to-mainnet-network.v1.json] [--json] [--allow-placeholders]"
    );
    println!("  release-keygen --out <path> [--label <label>] [--force] [--json]  (writes an ed25519 secret/public keypair file; prints only the public key)");
    println!("{}", dashboard_section("Snapshot", ANSI_BLUE));
    println!("  snapshot-info <snapshot-path> [--json] [--require-signature] [--public-key <hex>]");
    println!("  snapshot-export <out-path> [--db <path>] [--sign-key-hex <hex>]  (writes zoka-snapshot-v1 JSON; signs with ed25519 if a key is provided)");
    println!("  snapshot-import <in-path> [--db <path>] [--require-signature] [--public-key <hex>]  (verifies + replays every block via state::chain::apply_block)");
    println!("{}", dashboard_section("Chain", ANSI_GREEN));
    println!("  chain-prune --keep-last <N> [--db <path>] [--dry-run] [--json]  (drops block bodies older than head-N; floors --keep-last at PRUNE_MIN_RETAINED_BLOCKS so difficulty retarget keeps working)");
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
    snapshot_signers: Vec<String>,
    #[serde(default)]
    notes: Vec<String>,
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
}

#[derive(Debug, Clone, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct SeedCandidate {
    label: String,
    address: String,
    transport: String,
}

impl MainnetNetworkConfig {
    fn validate_for_mainnet(&self) -> Result<(), String> {
        if self.schema != "zoka-mainnet-network-v1" {
            return Err(format!(
                "network config schema '{}' is not supported",
                self.schema
            ));
        }
        if self.chain_id != MAINNET_CHAIN_ID {
            return Err(format!(
                "network config chain_id '{}' does not match binary chain_id '{}'",
                self.chain_id, MAINNET_CHAIN_ID
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

    if !config.status.eq_ignore_ascii_case("active")
        && !config.status.eq_ignore_ascii_case("launch-ready")
    {
        errors.push(format!(
            "network status is '{}'; expected active or launch-ready for release",
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
        errors.push(format!(
            "only {active_seed_count} active seed node(s); release policy expects at least 3"
        ));
    }
    let active_regions = config
        .seed_nodes
        .iter()
        .filter(|seed| seed.status.eq_ignore_ascii_case("active"))
        .map(|seed| seed.region.as_str())
        .collect::<std::collections::HashSet<_>>();
    if active_seed_count > 0 && active_regions.len() < 3 {
        errors.push(format!(
            "active seed nodes cover {} region(s); release policy expects multiple independent regions",
            active_regions.len()
        ));
    }

    if config.snapshot_signers.is_empty() {
        errors.push("no snapshot signer public keys configured".to_string());
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

fn default_mainnet_network_config_path() -> PathBuf {
    PathBuf::from("docs").join("mainnet-network.v1.json")
}

fn parse_mainnet_network_config(raw: &str) -> Result<MainnetNetworkConfig, String> {
    let config: MainnetNetworkConfig =
        serde_json::from_str(raw).map_err(|e| format!("invalid network config json: {e}"))?;
    config.validate_for_mainnet()?;
    Ok(config)
}

fn load_mainnet_network_config(path: &Path) -> Result<MainnetNetworkConfig, String> {
    let raw = fs::read_to_string(path)
        .map_err(|e| format!("failed to read network config '{}': {e}", path.display()))?;
    parse_mainnet_network_config(&raw)
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

fn count_wallet_files(dir_rel: &str, suffix: &str) -> usize {
    let Some(home) = home::home_dir() else {
        return 0;
    };
    let dir = home.join(dir_rel);
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

fn interactive_wallet_menu() -> Result<bool, String> {
    loop {
        print_interactive_menu_screen(
            "Wallet",
            ANSI_MAGENTA,
            Some("Create, recover, inspect and export private wallet material."),
            &[
                "1) Create wallet",
                "2) Restore wallet from seed",
                "3) Restore wallet from keys",
                "4) List wallets",
                "5) Wallet balance",
                "6) Wallet history",
                "7) Change wallet passphrase",
                "8) Export wallet secrets",
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
                let name = prompt_required("Wallet address (zka1...) or name: ")?;
                let password = prompt_optional("Password (optional): ")?;
                let mut args = vec!["--address".to_string(), name];
                push_opt_arg(&mut args, "--password", password);
                if let Err(err) = cmd_private_balance(&args) {
                    eprintln!("{}", err);
                }
            }
            "6" => {
                let name = prompt_required("Wallet address (zka1...) or name: ")?;
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
                let name = prompt_required("Wallet address (zka1...) or name: ")?;
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
            "8" => {
                let name = prompt_required("Wallet address (zka1...) or name: ")?;
                let password = prompt_optional("Password (optional): ")?;
                let out = prompt_optional("Export path (optional): ")?;
                let as_json = prompt_yes("Print as JSON? [y/N]: ")?;
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
            Some("Private sends with explicit confirmation and dynamic fee estimation."),
            &[
                "1) Send transaction",
                "2) Estimate fee",
                "0) Back",
                "q) Exit",
            ],
        );

        match prompt_line("> ")?.as_str() {
            "1" => {
                let from = prompt_required("From wallet (name or address): ")?;
                let to = prompt_required("To wallet or address: ")?;
                let amount = prompt_required("Amount: ")?;
                let priority = prompt_optional("Priority low|normal|high (default normal): ")?;
                let password = prompt_optional("Password (optional): ")?;
                if !prompt_yes("Confirm send? [y/N]: ")? {
                    println!("Cancelled.");
                    continue;
                }
                let mut args = vec![
                    "--from".to_string(),
                    from,
                    "--to".to_string(),
                    to,
                    "--amount".to_string(),
                    amount,
                    "--yes".to_string(),
                ];
                push_opt_arg(&mut args, "--priority", priority);
                push_opt_arg(&mut args, "--password", password);
                if let Err(err) = cmd_tx_send_unified(&args) {
                    eprintln!("{}", err);
                }
            }
            "2" => {
                let amount = prompt_required("Amount: ")?;
                let priority = prompt_optional("Priority low|normal|high (default normal): ")?;
                let mut args = vec!["--amount".to_string(), amount];
                push_opt_arg(&mut args, "--priority", priority);
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
            Some("CPU mining, live status and runtime console."),
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
                let wallet = prompt_required("Wallet address (zka1...): ")?;
                let minutes = prompt_optional("Session minutes (empty = keep running): ")?;
                let password = prompt_optional("Password (optional): ")?;
                let mut args = vec!["--address".to_string(), wallet];
                push_opt_arg(&mut args, "--minutes", minutes);
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
            Some("Automatic fullnode bootstrap and wallet inspection for the node identity."),
            &[
                "1) Start fullnode",
                "2) Show node wallet recovery info",
                "0) Back",
                "q) Exit",
            ],
        );

        match prompt_line("> ")?.as_str() {
            "1" => {
                println!("Leave fields empty to use defaults.");
                let wallet = prompt_optional("Wallet address (optional, zka1...): ")?;
                let password = prompt_optional("Wallet password (optional): ")?;
                let seed = prompt_optional("Seed peer multiaddr (optional): ")?;
                let seconds = prompt_optional("Run seconds (empty = keep running): ")?;
                let show_keys = prompt_yes("Show wallet keys on startup? [y/N]: ")?;
                let mut args = Vec::new();
                push_opt_arg(&mut args, "--seconds", seconds);
                push_opt_arg(&mut args, "--address", wallet);
                push_opt_arg(&mut args, "--password", password);
                push_opt_arg(&mut args, "--seed", seed);
                if show_keys {
                    args.push("--show-keys".to_string());
                }
                if let Err(err) = cmd_fullnode_auto(&args).await {
                    eprintln!("{}", err);
                }
            }
            "2" => {
                let wallet = prompt_optional("Wallet address (optional, zka1...): ")?;
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
            Some("Chain height, sync progress and wallet inspection."),
            &[
                "1) Chain height",
                "2) Wallet sync status",
                "3) Wallet list",
                "4) Wallet history",
                "0) Back",
                "q) Exit",
            ],
        );

        match prompt_line("> ")?.as_str() {
            "1" => {
                let db = prompt_optional("DB path (optional): ")?;
                let mut args = Vec::new();
                push_opt_arg(&mut args, "--db", db);
                if let Err(err) = cmd_height(&args) {
                    eprintln!("{}", err);
                }
            }
            "2" => {
                let wallet = prompt_required("Wallet address (zka1...) or name: ")?;
                let db = prompt_optional("DB path (optional): ")?;
                let password = prompt_optional("Password (optional): ")?;
                let mut args = vec!["--address".to_string(), wallet];
                push_opt_arg(&mut args, "--db", db);
                push_opt_arg(&mut args, "--password", password);
                if let Err(err) = cmd_sync_status_unified(&args) {
                    eprintln!("{}", err);
                }
            }
            "3" => {
                let password = prompt_optional("Password (optional): ")?;
                let mut args = Vec::new();
                push_opt_arg(&mut args, "--password", password);
                if let Err(err) = cmd_wallet_list_unified(&args) {
                    eprintln!("{}", err);
                }
            }
            "4" => {
                let wallet = prompt_required("Wallet address (zka1...) or name: ")?;
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
        print_interactive_menu_screen(
            "ZOKA 1.0",
            ANSI_CYAN,
            Some("Interactive mode. Privacy by default. English-only user surface."),
            &[
                "1) Wallets",
                "2) Transfers",
                "3) Mining",
                "4) Node",
                "5) Status",
                "h) Help",
                "0) Exit",
            ],
        );

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

fn parse_u64_arg(value: Option<String>, default: u64, flag_name: &str) -> Result<u64, String> {
    match value {
        Some(raw) => raw
            .parse::<u64>()
            .map_err(|_| format!("invalid numeric value for {flag_name}: {raw}")),
        None => Ok(default),
    }
}

fn required_option_value(args: &[String], flag: &str) -> Result<String, String> {
    option_value(args, flag)
        .ok_or_else(|| format!("missing required argument: {flag} (run with no args to see usage)"))
}

fn required_password_value(args: &[String]) -> Result<String, String> {
    let password = required_option_value(args, "--password")?;
    if password.is_empty() {
        return Err(
            "--password cannot be empty: choose a non-empty passphrase to protect this wallet"
                .to_string(),
        );
    }
    Ok(password)
}

fn wallet_selector_value(args: &[String]) -> Option<String> {
    option_value(args, "--address")
        .or_else(|| option_value(args, "--wallet"))
        .or_else(|| option_value(args, "--wallet-name"))
        .or_else(|| option_value(args, "--name"))
}

fn required_wallet_selector(args: &[String]) -> Result<String, String> {
    wallet_selector_value(args)
        .ok_or_else(|| "missing wallet selector: use --address <wallet-address|name>".to_string())
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
    if wallet_selector.starts_with("zka1") || wallet_selector.starts_with("zpriv:") {
        validate_user_private_address(wallet_selector)
            .map_err(|reason| format!("--address: {reason}"))?;
    }
    resolve_private_wallet_name(manager, wallet_selector).ok_or_else(|| {
        let known = manager.get_private_wallet_names();
        if known.is_empty() {
            format!(
                "no local wallets exist; run 'wallet-create --name {}' to create one (or restore an existing wallet with 'wallet-restore-from-seed')",
                wallet_selector
            )
        } else {
            let mut sorted = known;
            sorted.sort();
            let preview: Vec<String> = sorted.into_iter().take(5).collect();
            format!(
                "wallet '{}' not found; available wallets: {} (run 'wallet-list' for full list, or pass a zka1 address)",
                wallet_selector,
                preview.join(", ")
            )
        }
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
    wallet_selector: String,
    password: Option<String>,
    blocks: u64,
    mode: String,
    hashrate_hps: u64,
    base_difficulty: u32,
    target_block_time_secs: u64,
    /// RocksDB path for canonical chain persistence. When provided every
    /// mined block is persisted via `state::chain::apply_block` (single
    /// ingress path shared with peer-received blocks). Defaults to the
    /// fullnode's standard DB when None.
    chain_db_path: Option<String>,
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
        }
    }
}

const EMISSION_MAX_SUPPLY: u64 = economics::MAX_SUPPLY_ZKA;
const EMISSION_HALVING_INTERVAL: u64 = economics::HALVING_INTERVAL_BLOCKS;
const MAINNET_CHAIN_ID: &str = economics::MAINNET_CHAIN_ID;
const MAINNET_MINING_MODE: &str = "pow-mainnet";
const POW_TARGET_BLOCK_TIME_SECS: u64 = economics::TARGET_BLOCK_TIME_SECS;
const POW_INITIAL_DIFFICULTY_BITS: u32 = economics::POW_INITIAL_DIFFICULTY_BITS;
const POW_MIN_DIFFICULTY_BITS: u32 = economics::POW_MIN_DIFFICULTY_BITS;
const POW_MAX_DIFFICULTY_BITS: u32 = economics::POW_MAX_DIFFICULTY_BITS;
const COINBASE_MATURITY_BLOCKS: u64 = economics::COINBASE_MATURITY_BLOCKS;

/// Algorithm id chosen for the chain ZOKA currently runs against. With
/// `MAINNET_CHAIN_ID = zoka-mainnet-v1` this resolves to RandomX in
/// release builds compiled with `--features randomx`, and to the CPU
/// fallback otherwise — the `engine_for` call is the only place where
/// that choice is enforced at runtime.
fn active_pow_algorithm() -> &'static str {
    economics::pow_algorithm_for_chain(MAINNET_CHAIN_ID)
}

fn next_block_reward(height: u64, total_supply_emitted: u64, reward_override: Option<u64>) -> u64 {
    if let Some(reward) = reward_override {
        let remaining = EMISSION_MAX_SUPPLY.saturating_sub(total_supply_emitted);
        return reward.min(remaining);
    }
    economics::block_reward_for_height(height, total_supply_emitted)
}

fn genesis_pow_hash() -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"zoka-mainnet-genesis-pow");
    hasher.update([0x1f]);
    hasher.update(MAINNET_CHAIN_ID.as_bytes());
    hex::encode(hasher.finalize())
}

fn pow_engine_for_height(height: u64) -> Result<Box<dyn zoka::mining::pow::PowEngine>, String> {
    zoka::mining::pow::engine_for(MAINNET_CHAIN_ID, height).map_err(|e| match e {
        zoka::mining::pow::PowError::BackendNotAvailable(id) => format!(
            "PoW backend '{id}' is required by chain '{}' but is not compiled into this binary; rebuild with: cargo build --release --features randomx (see docs/pow-mainnet-decision.md)",
            MAINNET_CHAIN_ID
        ),
        other => format!("pow engine init failed: {other}"),
    })
}

fn build_pow_input(
    prev_hash: &str,
    wallet_address: &str,
    height: u64,
    timestamp: u64,
    nonce: u64,
) -> Vec<u8> {
    zoka::mining::pow::build_pow_input(
        MAINNET_CHAIN_ID,
        prev_hash,
        wallet_address,
        height,
        timestamp,
        nonce,
    )
}

fn benchmark_hashrate_hps(duration_ms: u64) -> u64 {
    let bench_for = Duration::from_millis(duration_ms.clamp(250, 2_000));
    let Ok(vm) = pow_engine_for_height(0) else {
        return 0;
    };
    let started = Instant::now();
    let mut nonce = 0u64;
    let mut hashes = 0u64;
    while started.elapsed() < bench_for {
        let payload = build_pow_input(&genesis_pow_hash(), "benchmark", 0, now_secs(), nonce);
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

fn hash_meets_difficulty(hash: &[u8], difficulty_bits: u32) -> bool {
    zoka::mining::pow::hash_meets_difficulty(hash, difficulty_bits)
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

fn user_mining_state_path() -> Result<PathBuf, String> {
    let home = home::home_dir().ok_or_else(|| "cannot determine home directory".to_string())?;
    let dir = home.join(".zoka");
    fs::create_dir_all(&dir).map_err(|e| format!("failed to create {}: {e}", dir.display()))?;
    Ok(dir.join("user_mining_state.json"))
}

fn load_user_mining_state() -> Result<UserMiningState, String> {
    let path = user_mining_state_path()?;
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
        state.mode = MAINNET_MINING_MODE.to_string();
    }
    if state.algorithm.is_empty() {
        state.algorithm = if state.mode == "pow-sim" {
            "sha256-sim".to_string()
        } else {
            active_pow_algorithm().to_string()
        };
    }
    if state.last_block_hash.is_empty() {
        state.last_block_hash = genesis_pow_hash();
    }
    Ok(state)
}

fn save_user_mining_state(state: &UserMiningState) -> Result<(), String> {
    let path = user_mining_state_path()?;
    let raw = serde_json::to_string_pretty(state)
        .map_err(|e| format!("failed to encode mining state json: {e}"))?;
    fs::write(&path, raw).map_err(|e| format!("failed to write {}: {e}", path.display()))
}

fn legacy_pow_sim_state_detected(state: &UserMiningState) -> bool {
    state.mode == "pow-sim" || state.algorithm == "sha256-sim"
}

fn legacy_pow_sim_reset_message() -> String {
    "legacy pow-sim state detected; reset .zoka/user_mining_state.json, .zoka/private_wallets and local chain DB before starting mainnet PoW".to_string()
}

fn fee_snapshot_db_path(db_path: Option<&str>) -> &str {
    db_path.unwrap_or(".zoka/blockchain.db")
}

fn network_pending_private_txs(db_path: Option<&str>) -> u64 {
    let storage = match RocksDbStorage::new(fee_snapshot_db_path(db_path)) {
        Ok(s) => s,
        Err(_) => return 0,
    };
    let Some(raw) = storage.get_raw("mempool:private:count").ok().flatten() else {
        return 0;
    };
    if let Ok(text) = std::str::from_utf8(&raw) {
        if let Ok(value) = text.trim().parse::<u64>() {
            return value;
        }
    }
    if raw.len() == 8 {
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&raw);
        return u64::from_le_bytes(bytes);
    }
    0
}

fn persist_fee_market_snapshot(db_path: Option<&str>, snapshot: &FeeMarketSnapshot) {
    if let Ok(storage) = RocksDbStorage::new(fee_snapshot_db_path(db_path)) {
        if let Ok(encoded) = serde_json::to_vec(snapshot) {
            let _ = storage.put_raw("fee:snapshot:v1", encoded);
        }
    }
}

fn recent_chain_activity_avg_txs(db_path: Option<&str>) -> f64 {
    let db = db_path.unwrap_or(".zoka/blockchain.db");
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
            tx_load = tx_load
                .saturating_add(block.transactions.len() as u64)
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

fn load_private_wallet_manager(
    storage: &WalletStorage,
    password: Option<&str>,
) -> Result<PrivateWalletManager, String> {
    let mut manager = PrivateWalletManager::new();
    let wallets = storage
        .load_all_private_wallets(password)
        .map_err(|e| format!("failed to load private wallets: {e}"))?;
    for wallet in wallets {
        let name = wallet.name.clone();
        let _ = manager.create_private_wallet(&name);
        if let Some(slot) = manager.get_private_wallet_mut(&name) {
            *slot = wallet;
            slot.normalize_key_material();
        }
    }
    if let Ok(mut mining_state) = load_user_mining_state() {
        let _ = settle_mature_mining_rewards(storage, &mut manager, password, &mut mining_state);
    }
    Ok(manager)
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
        save_user_mining_state(state)?;
    }
    Ok(matured_total)
}

fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|a| a == flag)
}

fn current_known_height(db_path: Option<&str>) -> u64 {
    let mut height = 0u64;
    let db = db_path.unwrap_or(".zoka/blockchain.db");
    if let Ok(storage) = RocksDbStorage::new(db) {
        if let Ok(h) = storage.get_chain_height() {
            height = height.max(h);
        }
    }
    if let Ok(state) = load_user_mining_state() {
        height = height.max(state.total_blocks);
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

fn first_local_wallet_selector(password: Option<&str>) -> Result<Option<String>, String> {
    let storage = WalletStorage::new().map_err(|e| format!("wallet storage init failed: {e}"))?;
    let manager = load_private_wallet_manager(&storage, password)?;
    let mut names = manager.get_private_wallet_names();
    names.sort();
    Ok(names.into_iter().next())
}

fn load_wallet_runtime_snapshot(
    wallet_selector: &str,
    password: Option<&str>,
) -> Result<WalletRuntimeSnapshot, String> {
    let storage = WalletStorage::new().map_err(|e| format!("wallet storage init failed: {e}"))?;
    let manager = load_private_wallet_manager(&storage, password)?;
    let mining_state = load_user_mining_state().unwrap_or_default();
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
            dashboard_line("Available", format!("{} ZKA", wallet.balance))
        )
        .ok();
        writeln!(
            &mut output,
            "{}",
            dashboard_line("Locked", format!("{} ZKA", wallet.locked_balance))
        )
        .ok();
        writeln!(
            &mut output,
            "{}",
            dashboard_line("Total", format!("{} ZKA", wallet.total_balance))
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
        dashboard_line("Session mined blocks", session_blocks)
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line("Session rewards", format!("{} ZKA", session_rewards))
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
        dashboard_line(
            "Total mined rewards",
            format!("{} ZKA", state.total_rewards)
        )
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
            format!("{} H/s", state.estimated_hashrate_hps)
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
        dashboard_line("Next block reward", format!("{} ZKA", next_reward))
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line(
            "Emitted supply",
            format!(
                "{} / {} ZKA",
                state.total_supply_emitted, EMISSION_MAX_SUPPLY
            )
        )
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line("Remaining supply", format!("{} ZKA", remaining_supply))
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
        dashboard_line("Available", format!("{} ZKA", wallet.balance))
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line("Locked", format!("{} ZKA", wallet.locked_balance))
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line("Total", format!("{} ZKA", wallet.total_balance))
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
        dashboard_line("Next block reward", format!("{} ZKA", next_reward))
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line(
            "Emitted supply",
            format!(
                "{} / {} ZKA",
                state.total_supply_emitted, EMISSION_MAX_SUPPLY
            )
        )
    )
    .ok();
    writeln!(
        &mut output,
        "{}",
        dashboard_line("Remaining supply", format!("{} ZKA", remaining_supply))
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
        dashboard_line("Session rewards", format!("{} ZKA", session_rewards))
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
            format!("{} H/s", state.estimated_hashrate_hps)
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
    wallet_selector: &str,
    password: Option<&str>,
    db_path: Option<&str>,
    session_elapsed: Duration,
    phase: &str,
    warmup_remaining: Option<Duration>,
    announced: &str,
    seed_peer: Option<&str>,
    auto_mine: bool,
    mine_interval_secs: u64,
    mine_blocks: u64,
    next_mine_in: Option<Duration>,
    session_mined_blocks: u64,
    session_mined_rewards: u64,
    last_event: &str,
) -> Result<(), String> {
    let wallet_snapshot = load_wallet_runtime_snapshot(wallet_selector, password)?;
    let state = load_user_mining_state()?;
    let fee_snapshot = refresh_fee_market_snapshot(db_path, password);
    let known_height = current_known_height(db_path);
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

fn run_mining_batch(config: &MiningEngineConfig) -> Result<MiningBatchSummary, String> {
    if config.blocks == 0 {
        return Err("--blocks must be >= 1".to_string());
    }
    if config.mode != MAINNET_MINING_MODE {
        return Err(format!(
            "invalid mining mode for mainnet user mode; expected {}",
            MAINNET_MINING_MODE
        ));
    }

    let mut state = load_user_mining_state()?;
    if !state.is_mining {
        state.is_mining = true;
    }

    let storage = WalletStorage::new().map_err(|e| format!("wallet storage init failed: {e}"))?;
    let mut private_manager = load_private_wallet_manager(&storage, config.password.as_deref())?;
    let wallet_name =
        resolve_private_wallet_name_required(&private_manager, &config.wallet_selector)?;
    let wallet_address = private_manager
        .get_private_wallet(&wallet_name)
        .map(|wallet| user_private_address_display(&wallet.commitment_key))
        .ok_or_else(|| format!("private wallet not found: {wallet_name}"))?;
    let wallet_commitment = private_manager
        .get_private_wallet(&wallet_name)
        .map(|wallet| wallet.commitment_key.clone())
        .ok_or_else(|| format!("private wallet not found: {wallet_name}"))?;

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
    let chain_storage: Option<Arc<RocksDbStorage>> = match config.chain_db_path.as_deref() {
        Some(path) => Some(open_storage(path)?),
        None => None,
    };
    let (chain_tip_height, chain_tip_hash, chain_tip_supply) = if let Some(s) = &chain_storage {
        match zoka::state::chain::read_chain_head(s)
            .map_err(|e| format!("failed to read chain head: {e}"))?
        {
            Some(head) => (head.height, head.hash, head.total_supply_emitted),
            None => (0, genesis_pow_hash(), 0),
        }
    } else {
        (
            state.chain_height,
            if state.last_block_hash.is_empty() {
                genesis_pow_hash()
            } else {
                state.last_block_hash.clone()
            },
            state.total_supply_emitted,
        )
    };
    state.chain_height = chain_tip_height;
    state.last_block_hash = chain_tip_hash.clone();
    state.total_supply_emitted = chain_tip_supply;

    let pow_epoch_blocks = economics::POW_KEY_EPOCH_BLOCKS;
    let mut next_height = chain_tip_height.saturating_add(1);
    let mut current_prev_hash = chain_tip_hash;
    let mut current_epoch = next_height / pow_epoch_blocks;
    let mut vm = pow_engine_for_height(next_height)?;
    for _ in 0..config.blocks {
        let height = next_height;
        let reward_for_block = next_block_reward(height, state.total_supply_emitted, None);
        if reward_for_block == 0 {
            break;
        }

        let epoch = height / pow_epoch_blocks;
        if epoch != current_epoch {
            vm = pow_engine_for_height(height)?;
            current_epoch = epoch;
        }
        // Source-of-truth for the next block's difficulty is the chain
        // DB, not the local state file. Every node — miner or verifier —
        // calls the same helper, so the miner cannot pick a number out
        // of thin air. Falls back to the local-state value when no
        // chain DB is configured (legacy CLI-only mining).
        let difficulty_bits = match chain_storage.as_ref() {
            Some(s) => zoka::mining::difficulty::expected_for_next_block(s)?,
            None => last_difficulty.clamp(POW_MIN_DIFFICULTY_BITS, POW_MAX_DIFFICULTY_BITS),
        };
        let started = Instant::now();
        let timestamp = now_secs();
        let prev_hash = current_prev_hash.clone();
        let mut nonce = 0u64;
        let (pow_hash_bytes, found_nonce) = loop {
            let input = build_pow_input(&prev_hash, &wallet_address, height, timestamp, nonce);
            let hash = vm
                .calculate_hash(&input)
                .map_err(|e| format!("pow hashing failed: {e}"))?;
            total_hashes = total_hashes.saturating_add(1);
            if hash_meets_difficulty(&hash, difficulty_bits) {
                break (hash, nonce);
            }
            nonce = nonce.wrapping_add(1);
        };
        let solve_secs = started.elapsed().as_secs_f64();

        // Construct the canonical Block this miner just produced and run
        // it through `validate_pow_block`. Any node receiving this block
        // would call the same validator; failing here means the miner
        // produced something the network would reject — which must never
        // happen, so we treat it as a hard error.
        let coinbase_tx = zoka::mining::block_validator::build_coinbase_tx(
            wallet_address.clone(),
            reward_for_block,
            height,
            timestamp,
        );
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
            private_tx_hashes: Vec::new(),
            private_fee_claims: Vec::new(),
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
            private_fee_claims: Vec::new(),
            private_coinbase: None,
        };

        zoka::mining::block_validator::validate_pow_block(
            &block,
            &zoka::mining::block_validator::PowBlockContext {
                chain_id: MAINNET_CHAIN_ID,
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
        // produced something the network would reject — escalate as a
        // hard error.
        if let Some(s) = chain_storage.as_ref() {
            zoka::state::chain::apply_block(s, MAINNET_CHAIN_ID, &block)
                .map_err(|e| format!("chain::apply_block rejected freshly mined block: {e}"))?;
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
        last_difficulty = match chain_storage.as_ref() {
            Some(s) => zoka::mining::difficulty::expected_for_next_block(s)?,
            None => retarget_difficulty(difficulty_bits, solve_secs, config.target_block_time_secs),
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
    state.algorithm = active_pow_algorithm().to_string();
    state.estimated_hashrate_hps = if total_solve_secs > 0.0 {
        (total_hashes as f64 / total_solve_secs) as u64
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
        &mut state,
    )?;
    save_user_mining_state(&state)?;

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
    message: &zoka::network::message::ZokaMessage,
    chain_db_path: Option<&str>,
    peer_view: &mut zoka::state::sync::PeerChainView,
) -> Option<String> {
    let response = match message {
        zoka::network::message::ZokaMessage::BlockResponse(r) => r,
        _ => return None,
    };

    let height = response.block_height;
    peer_view.observe(height);

    let path = match chain_db_path {
        Some(p) => p,
        None => {
            return Some(format!(
                "Peer block @ height {height} ignored: no chain DB configured"
            ))
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

    Some(sync_outcome_to_event(
        zoka::state::sync::try_apply_synced_block_payload(&storage, MAINNET_CHAIN_ID, response),
    ))
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
        } => format!(
            "Ignored peer block @ height {candidate}; local head is already {local_head}."
        ),
        zoka::state::sync::SyncOutcome::BadPayload(err) => {
            format!("Discarded peer block: payload not a valid Block ({err})")
        }
        zoka::state::sync::SyncOutcome::AnnouncedHashMismatch { announced, header } => format!(
            "Discarded peer block: announced hash {announced} != header hash {header}"
        ),
        zoka::state::sync::SyncOutcome::Rejected(err) => {
            format!("Rejected peer block: {err}")
        }
    }
}

async fn run_sync_step(
    node: &mut Node,
    chain_db_path: Option<&str>,
    peer_view: &zoka::state::sync::PeerChainView,
) -> Result<Option<String>, String> {
    let Some(path) = chain_db_path else {
        return Ok(None);
    };
    let storage =
        RocksDbStorage::new(path).map_err(|err| format!("IBD storage open failed ({err})"))?;
    let Some(next_height) = zoka::state::sync::next_required_height(&storage, peer_view)? else {
        return Ok(None);
    };

    let response = node
        .request_block_with_retry(next_height, None, 3, Duration::from_secs(6))
        .await
        .map_err(|err| format!("IBD request for block {next_height} failed: {err}"))?;
    let outcome =
        zoka::state::sync::try_apply_synced_block_payload(&storage, MAINNET_CHAIN_ID, &response);
    Ok(Some(format!(
        "IBD requested block {next_height}: {}",
        sync_outcome_to_event(outcome)
    )))
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
        match manager.get_account(&account.address).await {
            Some(existing) => {
                if existing.balance != account.balance {
                    return Err(format!(
                        "genesis/account mismatch for '{}': db balance={}, genesis balance={}",
                        account.address, existing.balance, account.balance
                    ));
                }
            }
            None => {
                manager
                    .create_account(account.address.clone(), account.balance)
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
        match manager.get_account(&account.address).await {
            Some(existing) => {
                if existing.balance != account.balance_zka {
                    return Err(format!(
                        "genesis initial account mismatch for '{}': db balance={}, genesis balance={}",
                        account.address, existing.balance, account.balance_zka
                    ));
                }
            }
            None => {
                manager
                    .create_account(account.address.clone(), account.balance_zka)
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
        match manager.get_account(&entry.recipient_address).await {
            Some(existing) => {
                if existing.balance != entry.amount_zka {
                    return Err(format!(
                        "genesis pre-mine mismatch for '{}': db balance={}, genesis amount={}",
                        entry.recipient_address, existing.balance, entry.amount_zka
                    ));
                }
            }
            None => {
                manager
                    .create_account(entry.recipient_address.clone(), entry.amount_zka)
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
        .set_total_supply_emitted(genesis.pre_mine.iter().map(|entry| entry.amount_zka).sum())
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

async fn connect_first_seed_candidate(
    node: &mut Node,
    candidates: &[SeedCandidate],
) -> Result<Option<SeedCandidate>, String> {
    let mut failures = Vec::new();
    for candidate in candidates {
        match connect_and_warmup(node, &candidate.address).await {
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
) -> Result<(PrivateWalletManager, String, bool), String> {
    let mut manager = load_private_wallet_manager(storage, password)?;
    if let Some(selector) = wallet_selector {
        if let Some(existing_name) = resolve_private_wallet_name(&manager, selector) {
            return Ok((manager, existing_name, false));
        }

        if selector.starts_with("zka1") || selector.starts_with("zpriv:") {
            validate_user_private_address(selector)
                .map_err(|reason| format!("--address: {reason}"))?;
            return Err(format!(
                "wallet address '{}' not found locally; import it with 'wallet-restore-from-seed' or 'wallet-restore-from-keys' first",
                selector
            ));
        }
        if !create_if_missing {
            return Err(format!(
                "wallet '{}' not found locally; run 'wallet-list' to see available wallets, 'wallet-create --name {}' to create it, or pass --create-if-missing to auto-create",
                selector, selector
            ));
        }
        let Some(password) = password else {
            return Err(
                "wallet auto-create requires --password so the new wallet is encrypted".to_string(),
            );
        };

        manager
            .create_private_wallet(selector)
            .map_err(|e| format!("private wallet create failed: {e}"))?;
        persist_private_wallet(storage, &manager, selector, Some(password))?;
        return Ok((manager, selector.to_string(), true));
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
    let wallet_selector = wallet_selector_value(args);
    let password =
        option_value(args, "--password").or_else(|| option_value(args, "--wallet-password"));
    let create_if_missing = has_flag(args, "--create-if-missing");
    let storage = WalletStorage::new().map_err(|e| format!("wallet storage init failed: {e}"))?;
    let (manager, wallet_name, created) = select_or_create_private_node_wallet(
        &storage,
        password.as_deref(),
        wallet_selector.as_deref(),
        create_if_missing,
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
    let wallet_selector = wallet_selector_value(args);
    let password =
        option_value(args, "--password").or_else(|| option_value(args, "--wallet-password"));
    let manual_seed_peer = option_value(args, "--seed").or_else(|| option_value(args, "--dial"));
    let network_config_arg = option_value(args, "--network-config");
    let use_network_config = !has_flag(args, "--no-network-config");
    let allow_clearnet_seeds = has_flag(args, "--allow-clearnet-seeds");
    let db_path = option_value(args, "--db");
    let genesis_path = option_value(args, "--genesis");
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
    let show_keys = has_flag(args, "--show-keys");
    let verbose = has_flag(args, "--verbose");
    let mining_target_block_time_secs = POW_TARGET_BLOCK_TIME_SECS;
    let mining_hashrate_hps = benchmark_hashrate_hps(350);
    let mining_base_difficulty = POW_INITIAL_DIFFICULTY_BITS;
    let current_mining_state = load_user_mining_state()?;
    if auto_mine && legacy_pow_sim_state_detected(&current_mining_state) {
        return Err(legacy_pow_sim_reset_message());
    }

    let storage = WalletStorage::new().map_err(|e| format!("wallet storage init failed: {e}"))?;
    let (manager, wallet_name, created) = select_or_create_private_node_wallet(
        &storage,
        password.as_deref(),
        wallet_selector.as_deref(),
        true,
    )?;
    let wallet = manager
        .get_private_wallet(&wallet_name)
        .ok_or_else(|| format!("wallet not found after selection: {wallet_name}"))?;
    let export = private_wallet_secrets_export(wallet);
    let wallet_address = export.user_address.clone();

    if created {
        println!("Node wallet created automatically.");
    } else {
        println!("Node wallet loaded.");
    }
    println!("Wallet address: {}", wallet_address);
    if show_keys {
        if let Some(mnemonic) = export.mnemonic.as_deref() {
            println!("Recovery phrase: {}", mnemonic);
        } else {
            println!("Recovery phrase: unavailable");
        }
        println!("Scan private key (hex): {}", export.scan_private_key_hex);
        println!("Spend private key (hex): {}", export.spend_private_key_hex);
    }
    let mut startup_notes = vec![if created {
        format!("Node wallet created automatically for {}.", wallet_address)
    } else {
        format!("Node wallet loaded for {}.", wallet_address)
    }];

    if let Some(db) = db_path.as_ref() {
        if let Some(genesis_file) = genesis_path.as_ref() {
            let digest = apply_genesis_file_to_db(genesis_file, db, false).await?;
            startup_notes.push(format!("Genesis ready on {} (digest={}).", db, digest));
        }
        let manager = open_state_manager(db).await?;
        let state = manager.get_state().await;
        startup_notes.push(format!(
            "State ready from {}: height={}, state_root={}, accounts={}",
            db,
            state.current_height,
            state.current_state_root,
            state.accounts.len()
        ));
    } else if genesis_path.is_some() {
        return Err("--genesis requires --db in fullnode".to_string());
    }

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
    let announced = announced.to_string();
    startup_notes.push(format!("Fullnode listening on {}.", announced));

    let mut seed_peer_display = None::<String>;
    if let Some(seed) = manual_seed_peer.as_deref() {
        connect_and_warmup(&mut node, &seed).await?;
        startup_notes.push(format!("Connected to seed peer {}.", seed));
        seed_peer_display = Some(seed.to_string());
    } else if use_network_config {
        let config_path = network_config_arg
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(default_mainnet_network_config_path);
        if config_path.exists() {
            let network_config = load_mainnet_network_config(&config_path)?;
            let seed_candidates = network_config.active_seed_candidates(allow_clearnet_seeds);
            startup_notes.push(format!(
                "Network config loaded from {}: status={}, active_seeds={}, active_rpc={}, snapshot_signers={}, notes={}.",
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
            match connect_first_seed_candidate(&mut node, &seed_candidates).await? {
                Some(seed) => {
                    startup_notes.push(format!(
                        "Connected to configured seed {} via {} ({}).",
                        seed.label, seed.transport, seed.address
                    ));
                    seed_peer_display = Some(format!(
                        "{} via {} ({})",
                        seed.label, seed.transport, seed.address
                    ));
                }
                None => {
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
            }
        } else if network_config_arg.is_some() {
            return Err(format!(
                "network config file not found: {}",
                config_path.display()
            ));
        } else {
            startup_notes.push(format!(
                "Default network config not found at {}; running standalone until peers connect.",
                config_path.display()
            ));
        }
    } else {
        startup_notes
            .push("No seed peer configured; running standalone until peers connect.".to_string());
    }

    let session_started = Instant::now();
    let mut session_mined_blocks = 0u64;
    let mut session_mined_rewards = 0u64;
    let mut last_event = startup_notes.join(" ");
    let mut peer_chain_view = zoka::state::sync::PeerChainView::default();

    let warmup_deadline = tokio::time::Instant::now() + Duration::from_secs(sync_warmup_secs);
    while tokio::time::Instant::now() < warmup_deadline {
        let now = tokio::time::Instant::now();
        let remaining = if warmup_deadline > now {
            warmup_deadline - now
        } else {
            Duration::default()
        };
        draw_fullnode_runtime_screen(
            &node,
            &wallet_address,
            password.as_deref(),
            db_path.as_deref(),
            session_started.elapsed(),
            "warmup",
            Some(remaining),
            &announced,
            seed_peer_display.as_deref(),
            auto_mine,
            mine_interval_secs,
            mine_blocks,
            None,
            session_mined_blocks,
            session_mined_rewards,
            &last_event,
        )?;
        match timeout(Duration::from_millis(500), node.next_message()).await {
            Ok(Ok(message)) => {
                last_event = if let Some(verdict) =
                    handle_inbound_block_message(&message, db_path.as_deref(), &mut peer_chain_view)
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
                if let Some(sync_event) =
                    run_sync_step(&mut node, db_path.as_deref(), &peer_chain_view).await?
                {
                    last_event = sync_event;
                }
            }
            Ok(Err(err)) => last_event = format!("receive error: {err}"),
            Err(_) => {
                node.pump_for(Duration::from_millis(100))
                    .await
                    .map_err(|e| format!("pump error: {e}"))?;
                if let Some(sync_event) =
                    run_sync_step(&mut node, db_path.as_deref(), &peer_chain_view).await?
                {
                    last_event = sync_event;
                }
            }
        }
    }
    last_event = if auto_mine {
        if mine_interval_secs == 0 {
            format!(
                "Sync warmup complete. Auto-mining enabled: continuous {} block batch with {}.",
                mine_blocks,
                active_pow_algorithm()
            )
        } else {
            format!(
                "Sync warmup complete. Auto-mining enabled: {} block(s) every {}s with {}.",
                mine_blocks,
                mine_interval_secs,
                active_pow_algorithm()
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
    loop {
        if let Some(deadline) = maybe_deadline {
            if tokio::time::Instant::now() >= deadline {
                last_event = "Fullnode session time limit reached.".to_string();
                break;
            }
        }

        if let Some(sync_event) =
            run_sync_step(&mut node, db_path.as_deref(), &peer_chain_view).await?
        {
            last_event = sync_event;
            continue;
        }

        if auto_mine && tokio::time::Instant::now() >= next_mine_at {
            let batch = run_mining_batch(&MiningEngineConfig {
                wallet_selector: wallet_address.clone(),
                password: password.clone(),
                blocks: mine_blocks,
                mode: MAINNET_MINING_MODE.to_string(),
                hashrate_hps: mining_hashrate_hps,
                base_difficulty: mining_base_difficulty,
                target_block_time_secs: mining_target_block_time_secs,
                chain_db_path: db_path.clone(),
            })
            .map_err(|e| format!("auto-mining failed: {e}"))?;
            session_mined_blocks = session_mined_blocks.saturating_add(batch.blocks_mined);
            session_mined_rewards = session_mined_rewards.saturating_add(batch.reward_credited);
            // Broadcast each freshly-produced block over gossipsub so
            // peers can apply it via state::chain::apply_block. Failures
            // here are non-fatal — the block is already persisted
            // locally; we only lose this single propagation attempt.
            let mut publish_failures = 0u32;
            for block in &batch.produced_blocks {
                let payload = match serde_json::to_vec(block) {
                    Ok(bytes) => bytes,
                    Err(_) => {
                        publish_failures = publish_failures.saturating_add(1);
                        continue;
                    }
                };
                if let Err(_err) = node.publish_block_response(
                    block.header.block_height,
                    block.header.hash.clone(),
                    payload,
                ) {
                    publish_failures = publish_failures.saturating_add(1);
                }
            }
            last_event = if publish_failures == 0 {
                format!(
                    "Auto-mined {} block(s), broadcast to peers, scheduled {} ZKA for maturity, next difficulty {} bits.",
                    batch.blocks_mined, batch.reward_credited, batch.last_difficulty_bits
                )
            } else {
                format!(
                    "Auto-mined {} block(s) ({} broadcast failure(s)), scheduled {} ZKA for maturity, next difficulty {} bits.",
                    batch.blocks_mined,
                    publish_failures,
                    batch.reward_credited,
                    batch.last_difficulty_bits,
                )
            };
            next_mine_at = tokio::time::Instant::now() + Duration::from_secs(mine_interval_secs);
        }

        let now = tokio::time::Instant::now();
        let next_mine_in = if auto_mine {
            Some(if next_mine_at > now {
                next_mine_at - now
            } else {
                Duration::default()
            })
        } else {
            None
        };
        draw_fullnode_runtime_screen(
            &node,
            &wallet_address,
            password.as_deref(),
            db_path.as_deref(),
            session_started.elapsed(),
            "running",
            None,
            &announced,
            seed_peer_display.as_deref(),
            auto_mine,
            mine_interval_secs,
            mine_blocks,
            next_mine_in,
            session_mined_blocks,
            session_mined_rewards,
            &last_event,
        )?;

        match timeout(Duration::from_millis(800), node.next_message()).await {
            Ok(Ok(message)) => {
                last_event = match handle_inbound_block_message(
                    &message,
                    db_path.as_deref(),
                    &mut peer_chain_view,
                ) {
                    Some(verdict) => verdict,
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
            Ok(Err(err)) => last_event = format!("receive error: {err}"),
            Err(_) => {
                node.pump_for(Duration::from_millis(100))
                    .await
                    .map_err(|e| format!("pump error: {e}"))?;
            }
        }
    }

    let next_mine_in = if auto_mine {
        Some(Duration::default())
    } else {
        None
    };
    draw_fullnode_runtime_screen(
        &node,
        &wallet_address,
        password.as_deref(),
        db_path.as_deref(),
        session_started.elapsed(),
        "stopped",
        None,
        &announced,
        seed_peer_display.as_deref(),
        auto_mine,
        mine_interval_secs,
        mine_blocks,
        next_mine_in,
        session_mined_blocks,
        session_mined_rewards,
        &last_event,
    )?;
    Ok(())
}

fn cmd_wallet_restore_from_keys(args: &[String]) -> Result<(), String> {
    let password = required_password_value(args)?;
    let storage = WalletStorage::new().map_err(|e| format!("wallet storage init failed: {e}"))?;
    let wallet_name = option_value(args, "--name")
        .unwrap_or_else(|| generate_available_private_wallet_name(&storage, "wallet"));
    let scan_key_hex = required_option_value(args, "--scan-key-hex")?;
    let spend_key_hex = option_value(args, "--spend-key-hex");
    let spend_public_key_hex = option_value(args, "--spend-public-key-hex");
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
                    "using --spend-public-key-hex requires --view-only (no spend secret present)"
                        .to_string(),
                );
                }
                let bytes = hex::decode(&spend_pub)
                    .map_err(|_| "invalid --spend-public-key-hex (hex expected)".to_string())?;
                if bytes.len() != 32 {
                    return Err(
                    "invalid --spend-public-key-hex length (expected 32 bytes compressed point)"
                        .to_string(),
                );
                }
                (Vec::new(), spend_pub, true)
            }
            (None, None) => {
                return Err(
                    "missing spend key material: provide --spend-key-hex or --spend-public-key-hex"
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
    println!("View-only: {}", wallet.view_only);
    println!("Restore height: {}", restore_height);
    Ok(())
}

fn cmd_private_wallet_create(args: &[String]) -> Result<(), String> {
    let password = required_password_value(args)?;
    let export_path = option_value(args, "--export-secrets");
    let storage = WalletStorage::new().map_err(|e| format!("wallet storage init failed: {e}"))?;
    let wallet_name = option_value(args, "--name")
        .unwrap_or_else(|| generate_available_private_wallet_name(&storage, "wallet"));
    if private_wallet_exists_on_disk(&storage, &wallet_name) {
        return Err(format!("private wallet already exists: {}", wallet_name));
    }
    let mut manager = load_private_wallet_manager(&storage, Some(&password))?;
    let commitment = manager
        .create_private_wallet(&wallet_name)
        .map_err(|e| format!("private wallet create failed: {e}"))?;
    persist_private_wallet(&storage, &manager, &wallet_name, Some(&password))?;
    let wallet = manager
        .get_private_wallet(&wallet_name)
        .ok_or_else(|| format!("private wallet not found after create: {}", wallet_name))?;
    let secrets = private_wallet_secrets_export(wallet);
    println!("Private wallet created");
    println!("Name: {}", wallet_name);
    println!("Address: {}", user_private_address_display(&commitment));
    if let Some(mnemonic) = secrets.mnemonic.as_deref() {
        println!("Recovery phrase: {}", mnemonic);
    } else {
        println!("Recovery phrase: unavailable");
    }
    println!("Scan private key (hex): {}", secrets.scan_private_key_hex);
    println!("Spend private key (hex): {}", secrets.spend_private_key_hex);
    if let Some(path) = export_path {
        write_secrets_export(&path, &secrets)?;
        println!("Secrets exported to {}", path);
    } else {
        println!("Tip: use --export-secrets <path> to save recovery material.");
    }
    Ok(())
}

fn cmd_wallet_export_secrets(args: &[String]) -> Result<(), String> {
    let wallet_selector = required_wallet_selector(args)?;
    let password = option_value(args, "--password");
    let out = option_value(args, "--out");
    let as_json = has_flag(args, "--json");

    let storage = WalletStorage::new().map_err(|e| format!("wallet storage init failed: {e}"))?;
    let mut manager = load_private_wallet_manager(&storage, password.as_deref())?;
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
    let password = option_value(args, "--password");
    let as_json = has_flag(args, "--json");
    let storage = WalletStorage::new().map_err(|e| format!("wallet storage init failed: {e}"))?;
    let manager = load_private_wallet_manager(&storage, password.as_deref())?;
    let mining_state = load_user_mining_state().unwrap_or_default();
    let wallets = manager.list_private_wallets();
    if as_json {
        let rows: Vec<_> = wallets
            .into_iter()
            .map(|(name, commitment, balance, tx_count)| {
                let address = user_private_address_display(&commitment);
                let locked_balance = locked_rewards_for_address(&mining_state, &address);
                serde_json::json!({
                    "name": name,
                    "address": address,
                    "balance": balance,
                    "locked_balance": locked_balance,
                    "total_balance": balance.saturating_add(locked_balance),
                    "tx_count": tx_count,
                })
            })
            .collect();
        println!("{}", serde_json::json!({ "private_wallets": rows }));
        return Ok(());
    }
    if wallets.is_empty() {
        if password.is_some() {
            let files = count_wallet_files(".zoka/private_wallets", ".private.json");
            if files > 0 {
                println!("No private wallets could be loaded with the provided password.");
                println!(
                    "Hint: detected {} wallet file(s) on disk. Check password or list without --password.",
                    files
                );
                return Ok(());
            }
        }
        println!("No private wallets found");
        return Ok(());
    }
    println!("Wallets:");
    for (name, commitment, balance, tx_count) in wallets {
        let address = user_private_address_display(&commitment);
        let locked_balance = locked_rewards_for_address(&mining_state, &address);
        println!(
            "- {} | {} | available={} | locked={} | total={} | tx_count={}",
            name,
            address,
            balance,
            locked_balance,
            balance.saturating_add(locked_balance),
            tx_count
        );
    }
    Ok(())
}

fn cmd_private_balance(args: &[String]) -> Result<(), String> {
    let wallet_selector = required_wallet_selector(args)?;
    let password = option_value(args, "--password");
    let as_json = has_flag(args, "--json");
    let storage = WalletStorage::new().map_err(|e| format!("wallet storage init failed: {e}"))?;
    let manager = load_private_wallet_manager(&storage, password.as_deref())?;
    let wallet_name = resolve_private_wallet_name_required(&manager, &wallet_selector)?;
    let wallet = manager
        .get_private_wallet(&wallet_name)
        .ok_or_else(|| format!("private wallet not found: {wallet_name}"))?;
    let address = user_private_address_display(&wallet.commitment_key);
    let mining_state = load_user_mining_state().unwrap_or_default();
    let locked_balance = locked_rewards_for_address(&mining_state, &address);
    if as_json {
        println!(
            "{}",
            serde_json::json!({
                "wallet": wallet_name,
                "address": address,
                "balance": wallet.private_balance,
                "locked_balance": locked_balance,
                "total_balance": wallet.private_balance.saturating_add(locked_balance),
                "view_only": wallet.view_only,
            })
        );
        return Ok(());
    }
    println!("Wallet: {}", wallet_name);
    println!("Address: {}", address);
    println!("Available balance: {}", wallet.private_balance);
    println!("Locked balance: {}", locked_balance);
    println!(
        "Total balance: {}",
        wallet.private_balance.saturating_add(locked_balance)
    );
    println!("View-only: {}", wallet.view_only);
    Ok(())
}

fn cmd_private_history(args: &[String]) -> Result<(), String> {
    let wallet_selector = required_wallet_selector(args)?;
    let password = option_value(args, "--password");
    let as_json = has_flag(args, "--json");
    let limit = parse_u64_arg(option_value(args, "--limit"), 20, "--limit")? as usize;
    let storage = WalletStorage::new().map_err(|e| format!("wallet storage init failed: {e}"))?;
    let manager = load_private_wallet_manager(&storage, password.as_deref())?;
    let wallet_name = resolve_private_wallet_name_required(&manager, &wallet_selector)?;
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
        println!(
            "{}",
            serde_json::json!({
                "wallet": wallet_name,
                "count": txs.len(),
                "transactions": txs
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
            tx.fee,
            tx.outputs.len()
        );
    }
    Ok(())
}

fn cmd_private_send(args: &[String]) -> Result<(), String> {
    let from_raw = required_option_value(args, "--from")?;
    let to_raw = required_option_value(args, "--to")?;
    let amount = parse_u64_arg(
        Some(required_option_value(args, "--amount")?),
        0,
        "--amount",
    )?;
    let fee = parse_u64_arg(option_value(args, "--fee"), 0, "--fee")?;
    let password = option_value(args, "--password");
    let confirm = has_flag(args, "--yes");
    if !confirm {
        return Err(
            "refusing to broadcast private transfer without explicit confirmation; add --yes"
                .to_string(),
        );
    }

    let storage = WalletStorage::new().map_err(|e| format!("wallet storage init failed: {e}"))?;
    let mut manager = load_private_wallet_manager(&storage, password.as_deref())?;
    let from_wallet = if manager.get_private_wallet(&from_raw).is_some() {
        from_raw.clone()
    } else if from_raw.starts_with("zka1") || from_raw.starts_with("zpriv:") {
        let sender_commitment = validate_user_private_address(&from_raw)
            .map_err(|reason| format!("--from: {reason}"))?;
        manager
            .get_private_wallet_names()
            .into_iter()
            .find(|name| {
                manager
                    .get_private_wallet(name)
                    .map(|wallet| wallet.commitment_key == sender_commitment)
                    .unwrap_or(false)
            })
            .ok_or_else(|| {
                format!(
                    "--from: address '{}' is valid but no local wallet matches it; run 'wallet-list' to see available wallets",
                    from_raw
                )
            })?
    } else {
        return Err(format!(
            "--from: '{}' is not a known wallet name or zka1 address; run 'wallet-list' to see your wallets",
            from_raw
        ));
    };
    let sender = manager
        .get_private_wallet(&from_wallet)
        .ok_or_else(|| format!("private wallet not found: {from_wallet}"))?;
    let sender_display = user_private_address_display(&sender.commitment_key);
    if sender.view_only || sender.spending_key.is_empty() {
        return Err("cannot send from view-only private wallet".to_string());
    }

    let looks_like_address = to_raw.starts_with("zka1") || to_raw.starts_with("zpriv:");
    let (to_commitment, recipient_wallet_name) = if let Some(target_wallet) =
        manager.get_private_wallet(&to_raw)
    {
        (target_wallet.commitment_key.clone(), Some(to_raw.clone()))
    } else if let Ok(target_wallet) = storage.load_private_wallet(&to_raw, None) {
        if !looks_like_address {
            return Err(format!(
                    "recipient wallet '{}' exists but could not be unlocked with provided password; local delivery requires both wallets loaded in the same session",
                    to_raw
                ));
        }
        (target_wallet.commitment_key.clone(), None)
    } else if looks_like_address {
        let decoded =
            validate_user_private_address(&to_raw).map_err(|reason| format!("--to: {reason}"))?;
        let matched_local = manager.get_private_wallet_names().into_iter().find(|name| {
            manager
                .get_private_wallet(name)
                .map(|wallet| wallet.commitment_key == decoded)
                .unwrap_or(false)
        });
        (decoded, matched_local)
    } else {
        return Err(format!(
            "--to: '{}' is not a known wallet name or zka1 address; run 'wallet-list' to see your wallets, or paste a recipient zka1 address",
            to_raw
        ));
    };

    let tx_hash = if fee > 0 {
        manager
            .send_private_transaction_with_fee(&from_wallet, to_commitment.clone(), amount, fee)
            .map_err(|e| format!("private send failed: {e}"))?
    } else {
        manager
            .send_private_transaction(&from_wallet, to_commitment.clone(), amount)
            .map_err(|e| format!("private send failed: {e}"))?
    };

    let tx = manager
        .get_private_transaction(&tx_hash)
        .ok_or_else(|| "private tx not found after creation".to_string())?;
    manager
        .validate_inbound_private_transaction(&tx)
        .map_err(|e| format!("private inbound validation failed: {e}"))?;
    manager
        .confirm_private_transaction(&tx_hash)
        .map_err(|e| format!("private confirm failed: {e}"))?;

    if let Some(recipient_name) = recipient_wallet_name.as_deref() {
        let decrypted = manager
            .decrypt_private_amount_for_wallet(recipient_name, &tx)
            .map_err(|e| format!("recipient decryption check failed: {e}"))?;
        if decrypted != amount {
            return Err(format!(
                "privacy integrity check failed: recipient decrypted amount {} but expected {}",
                decrypted, amount
            ));
        }
    }

    for candidate in manager.get_private_wallet_names() {
        if candidate == from_wallet {
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
    persist_all_private_wallets(&storage, &manager, password.as_deref())?;

    println!("Private transfer confirmed");
    println!("Hash: {}", tx_hash);
    println!("From: {}", sender_display);
    println!("To: {}", user_private_address_display(&to_commitment));
    println!("Amount: [HIDDEN]");
    println!("Fee: [HIDDEN]");
    Ok(())
}

fn cmd_wallet_create_unified(args: &[String]) -> Result<(), String> {
    cmd_private_wallet_create(args)
}

fn cmd_wallet_restore_from_seed(args: &[String]) -> Result<(), String> {
    let password = required_password_value(args)?;
    let storage = WalletStorage::new().map_err(|e| format!("wallet storage init failed: {e}"))?;
    let wallet_name = option_value(args, "--name")
        .unwrap_or_else(|| generate_available_private_wallet_name(&storage, "wallet"));
    let seed_phrase = required_option_value(args, "--seed")
        .or_else(|_| required_option_value(args, "--mnemonic"))?;
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
    let old_password =
        option_value(args, "--old-password").or_else(|| option_value(args, "--password"));
    let new_password = required_option_value(args, "--new-password")?;
    if new_password.is_empty() {
        return Err("--new-password cannot be empty".to_string());
    }

    let storage = WalletStorage::new().map_err(|e| format!("wallet storage init failed: {e}"))?;
    let mut manager = load_private_wallet_manager(&storage, old_password.as_deref())?;
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
    let wallet_selector = required_wallet_selector(args)?;
    let password = option_value(args, "--password");
    let db_path = option_value(args, "--db");
    let as_json = has_flag(args, "--json");
    let storage = WalletStorage::new().map_err(|e| format!("wallet storage init failed: {e}"))?;
    let manager = load_private_wallet_manager(&storage, password.as_deref())?;
    let wallet_name = resolve_private_wallet_name_required(&manager, &wallet_selector)?;
    let wallet = manager
        .get_private_wallet(&wallet_name)
        .ok_or_else(|| format!("wallet not found: {wallet_name}"))?;
    let chain_height = current_known_height(db_path.as_deref());
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
    let password = option_value(args, "--password");
    let storage = WalletStorage::new().map_err(|e| format!("wallet storage init failed: {e}"))?;
    let manager = load_private_wallet_manager(&storage, password.as_deref())?;
    let wallet_name = resolve_private_wallet_name_required(&manager, &wallet_selector)?;
    let home = home::home_dir().ok_or_else(|| "cannot determine home directory".to_string())?;
    let src = home
        .join(".zoka")
        .join("private_wallets")
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

fn cmd_wallet_wipe_unified(args: &[String]) -> Result<(), String> {
    let wallet_selector = required_wallet_selector(args)?;
    let password = option_value(args, "--password");
    let force = args.iter().any(|a| a == "--force");
    if !force {
        return Err(
            "wallet-wipe permanently deletes the wallet file from disk; pass --force to confirm (back it up first with 'wallet-backup --address <addr> --out <path>')"
                .to_string(),
        );
    }
    let storage = WalletStorage::new().map_err(|e| format!("wallet storage init failed: {e}"))?;
    let manager = load_private_wallet_manager(&storage, password.as_deref())?;
    let wallet_name = resolve_private_wallet_name_required(&manager, &wallet_selector)?;
    storage
        .delete_wallet(&wallet_name, true)
        .map_err(|e| format!("failed to wipe wallet file: {e}"))?;
    println!("Wallet file wiped: {}", wallet_name);
    Ok(())
}

fn cmd_tx_send_unified(args: &[String]) -> Result<(), String> {
    let from = required_option_value(args, "--from")?;
    let to = required_option_value(args, "--to")?;
    let amount = required_option_value(args, "--amount")?;
    let password = option_value(args, "--password");
    let confirm = has_flag(args, "--yes");
    if !confirm {
        return Err("refusing to broadcast without explicit confirmation; add --yes".to_string());
    }

    let fee = if let Some(raw) = option_value(args, "--fee") {
        raw
    } else {
        resolve_fee(args)?.to_string()
    };

    let mut forwarded = vec![
        "--from".to_string(),
        from,
        "--to".to_string(),
        to,
        "--amount".to_string(),
        amount,
        "--fee".to_string(),
        fee,
        "--yes".to_string(),
    ];
    push_opt_arg(&mut forwarded, "--password", password);
    cmd_private_send(&forwarded)
}

fn cmd_height(args: &[String]) -> Result<(), String> {
    let db_path = option_value(args, "--db");
    let as_json = has_flag(args, "--json");
    let h = current_known_height(db_path.as_deref());
    if as_json {
        println!("{}", serde_json::json!({ "height": h }));
        return Ok(());
    }
    println!("Height: {}", h);
    Ok(())
}

fn cmd_chain_stats(args: &[String]) -> Result<(), String> {
    let db_path_owned = option_value(args, "--db");
    let db_path = db_path_owned.as_deref().unwrap_or(".zoka/blockchain.db");
    let as_json = has_flag(args, "--json");
    let storage = RocksDbStorage::new(db_path)
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
                "chain_id": MAINNET_CHAIN_ID,
                "height": height,
                "head_hash": head_hash,
                "state_root": state_root,
                "private_commitment_root": private_commitment_root,
                "total_supply_emitted": total_supply_emitted,
                "max_supply": EMISSION_MAX_SUPPLY,
                "remaining_supply": remaining_supply,
                "cumulative_work": cumulative_work.to_string(),
                "current_difficulty_bits": current_difficulty_bits,
                "effective_difficulty_bits": effective_difficulty_bits,
                "next_height": next_height,
                "next_reward": next_reward,
                "halving_in_blocks": halving_in_blocks,
                "pow_algorithm": active_pow_algorithm(),
                "coinbase_maturity_blocks": COINBASE_MATURITY_BLOCKS,
            })
        );
        return Ok(());
    }

    println!("Chain stats");
    println!("DB:                       {db_path}");
    println!("Chain id:                 {MAINNET_CHAIN_ID}");
    println!("Height:                   {height}");
    println!("Head hash:                {head_hash}");
    println!("State root:               {state_root}");
    println!("Private commitment root:  {private_commitment_root}");
    println!("Total supply emitted:     {total_supply_emitted} ZKA");
    println!("Remaining supply:         {remaining_supply} ZKA");
    println!("Cumulative work:          {cumulative_work}");
    println!("Current difficulty bits:  {current_difficulty_bits}");
    println!("Effective difficulty:     {effective_difficulty_bits} bits");
    println!("PoW algorithm:            {}", active_pow_algorithm());
    println!("Next height:              {next_height}");
    println!("Next reward:              {next_reward} ZKA");
    println!("Halving in:               {halving_in_blocks} block(s)");
    println!("Coinbase maturity:        {COINBASE_MATURITY_BLOCKS} block(s)");
    Ok(())
}

fn resolve_fee(args: &[String]) -> Result<u64, String> {
    if let Some(fee) = option_value(args, "--fee") {
        return fee
            .parse::<u64>()
            .map_err(|_| format!("invalid numeric value for --fee: {fee}"));
    }
    let priority = option_value(args, "--priority").unwrap_or_else(|| "normal".to_string());
    let password = option_value(args, "--password");
    let db_path = option_value(args, "--db");
    let snapshot = refresh_fee_market_snapshot(db_path.as_deref(), password.as_deref());
    let pending = snapshot.pending_private_txs;
    let pending_tier = match pending {
        0..=2 => 0u64,
        3..=10 => 1u64,
        11..=25 => 2u64,
        _ => 3u64,
    };
    let chain_tier = if snapshot.avg_recent_block_txs < 1.0 {
        0
    } else if snapshot.avg_recent_block_txs < 5.0 {
        1
    } else if snapshot.avg_recent_block_txs < 20.0 {
        2
    } else {
        3
    };
    let load_tier = pending_tier.saturating_add(chain_tier).min(6);
    let dynamic = match priority.as_str() {
        "low" => 1u64.saturating_add(load_tier),
        "normal" => 2u64.saturating_add(load_tier.saturating_mul(2)),
        "high" => 4u64.saturating_add(load_tier.saturating_mul(3)),
        other => {
            return Err(format!(
                "invalid --priority '{}'; expected low|normal|high",
                other
            ))
        }
    };
    Ok(dynamic.max(1))
}

fn cmd_estimate_fee(args: &[String]) -> Result<(), String> {
    let amount = parse_u64_arg(
        Some(required_option_value(args, "--amount")?),
        0,
        "--amount",
    )?;
    let fee = resolve_fee(args)?;
    println!("Fee estimate");
    println!("Amount: {}", amount);
    println!(
        "Priority: {}",
        option_value(args, "--priority").unwrap_or_else(|| "normal".to_string())
    );
    println!("Estimated fee: {}", fee);
    Ok(())
}

fn cmd_mine_status(args: &[String]) -> Result<(), String> {
    let state = load_user_mining_state()?;
    let as_json = has_flag(args, "--json");
    if as_json {
        println!(
            "{}",
            serde_json::json!({
                "running": state.is_mining,
                "blocks_mined": state.total_blocks,
                "total_rewards": state.total_rewards,
                "chain_height": state.chain_height,
                "difficulty_bits": state.current_difficulty_bits,
                "hashrate_hps": state.estimated_hashrate_hps,
                "avg_solve_time_secs": state.avg_solve_time_secs,
                "last_solve_time_secs": state.last_solve_time_secs,
                "virtual_time_elapsed_secs": state.virtual_time_elapsed_secs,
                "total_supply_emitted": state.total_supply_emitted,
                "mode": state.mode,
                "algorithm": state.algorithm,
                "locked_rewards": state.pending_rewards.iter().map(|reward| reward.amount).sum::<u64>(),
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
    println!(
        "Legacy sim detected: {}",
        yes_no(legacy_pow_sim_state_detected(&state))
    );
    println!("Blocks mined: {}", state.total_blocks);
    println!("Total rewards: {}", state.total_rewards);
    println!(
        "Locked rewards: {}",
        state
            .pending_rewards
            .iter()
            .map(|reward| reward.amount)
            .sum::<u64>()
    );
    println!("Chain height: {}", state.chain_height);
    println!("Difficulty bits: {}", state.current_difficulty_bits);
    println!("Hashrate (H/s): {}", state.estimated_hashrate_hps);
    println!("Avg solve time (s): {:.2}", state.avg_solve_time_secs);
    println!("Last solve time (s): {:.2}", state.last_solve_time_secs);
    println!("Total supply emitted: {}", state.total_supply_emitted);
    Ok(())
}

fn cmd_mine_console(args: &[String]) -> Result<(), String> {
    let state = load_user_mining_state()?;
    let as_json = has_flag(args, "--json");
    let password = option_value(args, "--password");
    let db_path = option_value(args, "--db");
    let wallet_selector = match wallet_selector_value(args) {
        Some(selector) => Some(selector),
        None => first_local_wallet_selector(password.as_deref())?,
    };
    let wallet_snapshot = match wallet_selector.as_deref() {
        Some(selector) => Some(load_wallet_runtime_snapshot(selector, password.as_deref())?),
        None => None,
    };
    let fee_snapshot = refresh_fee_market_snapshot(db_path.as_deref(), password.as_deref());
    let known_height = current_known_height(db_path.as_deref());

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
                "avg_solve_time_secs": state.avg_solve_time_secs,
                "last_solve_time_secs": state.last_solve_time_secs,
                "virtual_time_elapsed_secs": state.virtual_time_elapsed_secs,
                "session_blocks": state.total_blocks,
                "session_rewards": state.total_rewards,
                "next_reward": next_block_reward(state.chain_height, state.total_supply_emitted, None),
                "supply_emitted": state.total_supply_emitted,
                "supply_remaining": EMISSION_MAX_SUPPLY.saturating_sub(state.total_supply_emitted),
                "halving_in_blocks": EMISSION_HALVING_INTERVAL - (state.chain_height % EMISSION_HALVING_INTERVAL),
                "locked_rewards": state.pending_rewards.iter().map(|reward| reward.amount).sum::<u64>(),
                "coinbase_maturity_blocks": COINBASE_MATURITY_BLOCKS,
                "legacy_pow_sim_detected": legacy_pow_sim_state_detected(&state),
                "known_height": known_height,
                "wallet": wallet_snapshot.as_ref().map(|wallet| {
                    serde_json::json!({
                        "name": wallet.wallet_name,
                        "address": wallet.address,
                        "balance": wallet.balance,
                        "locked_balance": wallet.locked_balance,
                        "total_balance": wallet.total_balance,
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
        "Snapshot only.",
    );
    set_terminal_title("ZOKA Mining 1.0")?;
    println!("{}", console);
    Ok(())
}

fn cmd_mine_easy(args: &[String]) -> Result<(), String> {
    let wallet_selector = wallet_selector_value(args)
        .ok_or_else(|| "missing required argument: --address <wallet-address|name>".to_string())?;
    let password = option_value(args, "--password");
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

    let hashrate_hps = benchmark_hashrate_hps(500);
    let base_difficulty = POW_INITIAL_DIFFICULTY_BITS;
    let deadline = if minutes == 0 {
        None
    } else {
        Some(Instant::now() + Duration::from_secs(minutes.saturating_mul(60)))
    };

    let wallet = load_wallet_runtime_snapshot(&wallet_selector, password.as_deref())?;
    let mut state = load_user_mining_state()?;
    if legacy_pow_sim_state_detected(&state) {
        return Err(legacy_pow_sim_reset_message());
    }
    state.is_mining = true;
    state.mode = MAINNET_MINING_MODE.to_string();
    state.algorithm = active_pow_algorithm().to_string();
    state.estimated_hashrate_hps = hashrate_hps;
    state.current_difficulty_bits = state
        .current_difficulty_bits
        .max(base_difficulty)
        .clamp(POW_MIN_DIFFICULTY_BITS, POW_MAX_DIFFICULTY_BITS);

    let session_started = Instant::now();
    let mut session_blocks = 0u64;
    let mut session_rewards = 0u64;
    let mut status_line = format!(
        "Ready. Mining into {} with {} at {} bits.",
        wallet.address,
        active_pow_algorithm(),
        base_difficulty
    );

    let render = |current_state: &UserMiningState,
                  status_line: &str,
                  session_blocks: u64,
                  session_rewards: u64|
     -> Result<(), String> {
        let wallet_snapshot = load_wallet_runtime_snapshot(&wallet_selector, password.as_deref())?;
        let fee_snapshot = refresh_fee_market_snapshot(None, password.as_deref());
        let known_height = current_known_height(None);
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

        let batch = run_mining_batch(&MiningEngineConfig {
            wallet_selector: wallet_selector.clone(),
            password: password.clone(),
            blocks: 1,
            mode: MAINNET_MINING_MODE.to_string(),
            hashrate_hps,
            base_difficulty,
            target_block_time_secs,
            chain_db_path: None,
        })?;
        session_blocks = session_blocks.saturating_add(batch.blocks_mined);
        session_rewards = session_rewards.saturating_add(batch.reward_credited);
        status_line = format!(
            "Last block solved in {:.2}s at {} bits. Reward {} ZKA scheduled for maturity.",
            batch.last_solve_time_secs, batch.last_difficulty_bits, batch.reward_credited
        );
        state = load_user_mining_state()?;
        render(&state, &status_line, session_blocks, session_rewards)?;
    }

    state = load_user_mining_state()?;
    state.is_mining = false;
    save_user_mining_state(&state)?;
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
        if args.is_empty() {
            return cmd_fullnode_auto(&[]).await;
        }
        let first = &args[0];
        if first == "--help" || first == "-h" || first == "help" {
            println!("zokahd (automatic fullnode)");
            println!("Usage: zokahd [--address <wallet-address>] [--password <pwd>] [--seed <multiaddr>] [--network-config <file>] [--no-network-config] [--allow-clearnet-seeds] [--seconds <n>] [--show-keys] [--no-mine] [--db <path>] [--genesis <file>] [--inline]");
            println!("Runs fullnode + sync warmup + auto-mining by default until interrupted. --password is required when auto-creating a wallet.");
            println!("On Windows it opens in its own console window by default. Use --inline to keep it in the current terminal.");
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
        "help" | "--help" | "-h" => {
            print_usage();
            Ok(())
        }
        "wallet-create" => cmd_wallet_create_unified(tail),
        "wallet-restore-from-seed" => cmd_wallet_restore_from_seed(tail),
        "wallet-restore-from-keys" => cmd_wallet_restore_from_keys_unified(tail),
        "wallet-import" => cmd_wallet_import_disabled(tail),
        "wallet-change-passphrase" => cmd_wallet_change_passphrase_unified(tail),
        "wallet-backup" => cmd_wallet_backup_unified(tail),
        "wallet-wipe" => cmd_wallet_wipe_unified(tail),
        "wallet-list" => cmd_wallet_list_unified(tail),
        "wallet-export-secrets" => cmd_wallet_export_secrets(tail),
        "wallet-balance" => cmd_wallet_balance_unified(tail),
        "height" => cmd_height(tail),
        "chain-stats" => cmd_chain_stats(tail),
        "sync-status" => cmd_sync_status_unified(tail),
        "wallet-history" => cmd_wallet_history_unified(tail),
        "wallet-recovery" => cmd_wallet_export_secrets(tail),
        "estimate-fee" => cmd_estimate_fee(tail),
        "fullnode" => cmd_fullnode_auto(tail).await,
        "tx-send" => cmd_tx_send_unified(tail),
        "tx-send-many" => {
            Err("tx-send-many is disabled in unified private wallet mode".to_string())
        }
        "mine-status" => cmd_mine_status(tail),
        "mine-console" => cmd_mine_console(tail),
        "mine" => cmd_mine_easy(tail),
        "genesis-verify" => cmd_genesis_verify(tail),
        "mainnet-network-lint" => cmd_mainnet_network_lint(tail),
        "release-keygen" => cmd_release_keygen(tail),
        "snapshot-info" => cmd_snapshot_info(tail),
        "snapshot-export" => cmd_snapshot_export(tail),
        "snapshot-import" => cmd_snapshot_import(tail),
        "chain-prune" => cmd_chain_prune(tail),
        other => Err(format!("unknown command: {other}")),
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
        println!("Public RPC endpoints:         {}", report.public_rpc_count);
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
        println!("Public key hex:  {}", report.public_key_hex);
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
    let pubkey_hex = option_value(args, "--public-key");

    let bytes =
        fs::read(&in_path).map_err(|e| format!("failed to read snapshot '{in_path}': {e}"))?;
    let snapshot =
        zoka::snapshot::deserialize(&bytes).map_err(|e| format!("snapshot parse failed: {e}"))?;

    let pubkey_array;
    let pubkey_ref: Option<&[u8; 32]> = match pubkey_hex {
        Some(hex_key) => {
            let decoded = hex::decode(hex_key.trim())
                .map_err(|e| format!("--public-key must be 32-byte hex: {e}"))?;
            if decoded.len() != 32 {
                return Err(format!(
                    "--public-key must decode to 32 bytes (got {})",
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
        snapshot.header.total_supply_emitted
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

    println!("Snapshot:           {out_path}");
    println!("Schema:             {}", zoka::snapshot::SNAPSHOT_SCHEMA_ID);
    println!("Chain id:           {}", snapshot.header.chain_id);
    println!("Head height:        {}", snapshot.header.head_height);
    println!("Head hash:          {}", snapshot.header.head_hash);
    println!("Block count:        {}", snapshot.header.block_count);
    println!("Cumulative work:    {}", snapshot.header.cumulative_work);
    println!(
        "Total supply (ZKA): {}",
        snapshot.header.total_supply_emitted
    );
    println!("Signed:             {}", snapshot.signature_hex.is_some());
    println!("SHA-256 digest:     {digest}");
    Ok(())
}

/// `zoka snapshot-import <in-path> [--db <path>] [--require-signature] [--public-key <hex>]`
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
    let pubkey_hex = option_value(args, "--public-key");

    let bytes =
        fs::read(&in_path).map_err(|e| format!("failed to read snapshot '{in_path}': {e}"))?;
    let snapshot =
        zoka::snapshot::deserialize(&bytes).map_err(|e| format!("snapshot parse failed: {e}"))?;

    let pubkey_array;
    let pubkey_ref: Option<&[u8; 32]> = match pubkey_hex {
        Some(hex_key) => {
            let decoded = hex::decode(hex_key.trim())
                .map_err(|e| format!("--public-key must be 32-byte hex: {e}"))?;
            if decoded.len() != 32 {
                return Err(format!(
                    "--public-key must decode to 32 bytes (got {})",
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
            "OK — genesis is structurally valid. Compare the digest against the published value before applying it."
        );
    }
    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("{err}");
        if err.starts_with("unknown command:") {
            print_usage();
        }
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_valid_zka1() -> String {
        let scan = [0x11u8; 32];
        let spend = [0x22u8; 32];
        let zpriv = format!("zpriv:{}:{}", hex::encode(scan), hex::encode(spend));
        user_private_address_from_zpriv(&zpriv).expect("encode zka1")
    }

    #[test]
    fn validate_address_accepts_canonical_zka1() {
        let addr = make_valid_zka1();
        let canonical = validate_user_private_address(&addr).expect("valid zka1 address");
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
    fn mainnet_network_config_accepts_prelaunch_template() {
        let raw = r#"{
          "schema": "zoka-mainnet-network-v1",
          "status": "prelaunch-template",
          "chain_id": "zoka-mainnet-v1",
          "protocol_freeze_id": "/zoka/1.0.0",
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
    fn committed_mainnet_network_config_matches_loader_schema() {
        let raw = include_str!("../docs/mainnet-network.v1.json");
        let config = parse_mainnet_network_config(raw).expect("committed network config");
        assert_eq!(config.chain_id, MAINNET_CHAIN_ID);
    }

    #[test]
    fn mainnet_network_lint_accepts_committed_launch_ready_config() {
        let tmp = tempfile::NamedTempFile::new().expect("temp config");
        fs::write(tmp.path(), include_str!("../docs/mainnet-network.v1.json"))
            .expect("write temp config");

        let report = lint_mainnet_network_config(tmp.path(), false).expect("lint report");
        assert!(report.ok, "unexpected errors: {:?}", report.errors);
        assert_eq!(report.active_seed_count, 3);
        assert_eq!(report.snapshot_signer_count, 1);
    }

    #[test]
    fn mainnet_network_lint_accepts_launch_ready_active_config() {
        let raw = r#"{
          "schema": "zoka-mainnet-network-v1",
          "status": "launch-ready",
          "chain_id": "zoka-mainnet-v1",
          "protocol_freeze_id": "/zoka/1.0.0",
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
          "protocol_freeze_id": "/zoka/1.0.0",
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
          "chain_id": "zoka-mainnet-v1",
          "protocol_freeze_id": "/zoka/1.0.0",
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
