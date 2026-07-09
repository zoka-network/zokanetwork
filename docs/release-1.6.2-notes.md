# ZOKA / ZSilent 1.6.2 — patch release notes

Patch release after 1.6.1. Fully protocol-compatible (same freeze id
`/zoka/1.6.0-mainnet-zenith`), no reseed needed.

Repos:

- Node: `ZokaNetwork/ZokaNetwork` (tag `v1.6.2`)
- App: `ZokaNetwork/zsilent-core` (tag `v1.6.2`, embeds the node `v1.6.2` binaries)

## Fixes included so far

### 1. Node: gossip-storm memory leak in the periodic block rebroadcast
- **Commit:** `7314756` (node) — **already deployed live to BOOT + VPS2** on
  2026-07-06 as an emergency fix (see below), independent of this doc.
- **Problem:** Caught live: BOOT (bootstrap seed) was OOM-killed every
  ~13-19 minutes starting 2026-07-06 ~11:08 UTC (52 restarts before the
  fix). Root cause: `publish_recent_chain_blocks` (re-gossips up to
  `sync_rebroadcast_recent_blocks` full block payloads) shared the same 3s
  timer as the cheap height/announce broadcasts. BOOT/VPS2 run with
  `--sync-rebroadcast-recent-blocks 32`, so that's ~10.7 full block
  payloads/sec published forever, unconditionally — each as a brand-new
  gossipsub message, so peers can't dedupe it against the identical content
  they already have. `gossip_rx_block_response` measured at ~64/sec during
  the incident, consistent with this self-inflicted storm being amplified
  across the mesh. Regular miners/ZSilent/standalone users were never
  exposed: the flag defaults to `0` when not passed, and only the two seeds
  set it explicitly.
- **Fix:** `publish_recent_chain_blocks` / `post_recent_chain_blocks_to_rpcs`
  now run on their own 120s timer instead of the shared 3s one. Recent
  blocks only need to be nudged back onto the wire occasionally to help a
  peer that missed one during a brief disconnect — IBD/RPC sync remains the
  real catch-up path — so 120s is plenty, cutting the worst-case
  rebroadcast rate ~40x (from ~10.7 blocks/sec to ~0.27/sec) even with the
  flag restored to 32.
- **Emergency mitigation timeline:** set `--sync-rebroadcast-recent-blocks 0`
  on both seeds first (no rebuild needed, stopped the bleeding immediately)
  → shipped the code fix → redeployed the fixed binary to both seeds →
  restored the flag to `32` (safe now with the 120s timer) → verified RSS
  stopped growing under load.

### 2. Node: `mining_console` set via `zoka.config.json` didn't quiet the logs
- **Commit:** `3ba3b47` (node)
- **Problem:** Caught live testing the standalone config-file flow. Enabling
  `mining_console: true` in `zoka.config.json` (instead of passing
  `--mining-console` on the command line) left the console cluttered with
  raw `INFO`-level p2p/gossip noise interleaved with the clean
  warmup/status lines — the exact "desprolijo, choca" complaint that
  motivated `--mining-console` in the first place. Root cause: `main()`
  decides whether to default `RUST_LOG=warn` by checking the raw process
  argv *before* `tracing_subscriber` initializes, but `--mining-console`
  only gets added to the effective args later, inside
  `cmd_fullnode_auto`'s config-file expansion — by then the subscriber has
  already initialized at whatever level `RUST_LOG` was (or wasn't), so
  setting it afterward does nothing.
- **Fix:** `main()` now also resolves and parses the node config file
  (reusing `find_node_config_path`/`NodeConfigFile`) before initializing
  tracing, and treats `mining_console: true` there the same as the CLI
  flag. Verified live: a `--config` run with `mining_console: true` and no
  `--mining-console` flag now shows only warmup/status/heartbeat lines,
  zero p2p INFO spam.

### 3. Node: relative `network_config`/`db` paths in `zoka.config.json` resolved wrong
- **Commit:** `3b6644f` (node)
- **Problem:** Caught live: a standalone `zb/` setup with
  `"network_config": "mainnet-network.v1.json"` (a bare relative filename,
  matching the shipped example) failed with `mainnet network config file
  not found: mainnet-network.v1.json` even though the file sat right next
  to `zoka.exe` and `zoka.config.json`. Root cause: `main()` calls
  `setup_zoka_home()`, which `chdir`s the process into the platform data
  directory (e.g. `%APPDATA%\Zoka`) so the fullnode's own relative paths
  like `.zoka/mainnet` resolve correctly — and it does this *before*
  `cmd_fullnode_auto` ever reads `zoka.config.json`. A relative
  `network_config`/`db` value from the config then resolved against the
  wrong (data) directory instead of the folder the config file actually
  lives in.
- **Fix:** `expand_node_config_into_args` now resolves relative
  `network_config`/`db` values against the config file's own directory
  before pushing them as `--network-config`/`--db`, so they work
  regardless of the later `chdir`. Verified live: a config with a bare
  relative `network_config` now connects to seeds correctly.

### 4. Node: fluid mining console becomes the default, with colors and a block-found banner
- **Commit:** `4f67c98` (node), `9f9eb22` (zsilent-core)
- **Change:** The streaming console is now the fullnode's default output
  mode instead of the full-screen dashboard — no `--mining-console` flag or
  `zoka.config.json` field needed. A new `--dashboard` flag (or
  `ZOKA_DASHBOARD=1`, or `"mining_console": false` in the config) opts back
  into the old full-screen view.
- **Visual redesign** using the existing ANSI color infrastructure:
  peer count colored green/yellow/red by health, height bolded,
  `mining=on/off` in green/yellow, hashrate in magenta, session
  block/reward counters in green, trailing event text dimmed. A new
  `print_block_found_banner()` prints a prominent bold-green `*** Block
  found! height=X reward=Y ***` line the moment this session wins a block —
  separate from the routine status line so it doesn't blend into the
  scrollback. The `mining: progress …` line is dimmed on the terminal only
  (the plain-text `mining_diagnostic.log` file is untouched).
- **ZSilent compatibility:** ZSilent's own log panel parses raw node stdout
  and is tuned to the full-screen dashboard's line format
  (`DASHBOARD_NOISE` in `NodeController.kt`), so `zsilent-core` now passes
  `--dashboard` explicitly when launching the node, keeping its behavior
  unchanged regardless of the node's new default. Accumulating in
  `zsilent-core` for its own next release.
- Verified live end-to-end: greeting, connecting, and status lines all
  render with correct ANSI codes while actually syncing against mainnet.

### 5. Node: interactive menu's "Start mining" and `zoka mine` never actually joined the network
- **Problem:** Found while packaging the standalone miner: `zoka.exe` run
  with no args → `Mining` → `Start mining`, and the direct CLI alias
  `zoka mine`, both called a legacy helper (`cmd_mine_easy`) that mined
  straight into the local RocksDB chain state with **zero P2P networking**.
  `rpc_sync_endpoints` was only ever used to pull pending private txs, never
  to push or broadcast mined blocks, so anything "mined" this way was a
  private, disconnected fork — invisible to the real network and never
  actually credited. It looked like it worked (console output, no errors)
  and then silently went nowhere.
- **Fix:** Both entry points now redirect to `cmd_fullnode_auto` — the same
  connected, P2P-mining fullnode path already used by `zoka fullnode` /
  `--mining-console`, and already verified live multiple times this session.
  Removed `cmd_mine_easy` and the dead code it left behind
  (`active_public_rpc_urls`, `rpc_sync_endpoints_from_args`,
  `best_reachable_rpc_height_from_args`, `standalone_mining_sync_gate`, the
  unused `MiningBatchSummary.last_solve_time_secs` field). `cargo check`
  clean with zero warnings; `cargo test --lib` green.

### 6. Node: mining console didn't show which wallet was mining, and stale private peer addresses never expired
- **Problem:** The recurring console status line showed balance and hashrate
  but no wallet identifier, so once startup output scrolled past there was
  no way to tell which wallet was actually mining without restarting.
  Separately, `PeerStore::dial_addrs` could keep handing out
  datacenter-internal peer addresses (e.g. a seed's `10.x` VPC interface)
  written to disk by a build predating the insert-time filter added
  earlier — a node would retry an address it could never reach forever,
  producing constant `Failed to negotiate transport` / connection-timeout
  noise.
- **Fix:** `print_mining_console_status` now takes a `wallet_name` and
  prints it on every tick. `PeerStore::dial_addrs` now re-applies
  `discovered_addr_is_publicly_dialable` on read, so a `peer_store.json`
  written before the filter existed self-heals instead of needing a manual
  purge. Added a regression test for the stale-entry case.

### 7. Node: console now shows the full receiving address, and surfaces why the balance failed to load
- **Problem:** User feedback: wanted the actual `zka1...` receiving
  address on the status line (not the short local wallet label), and
  wanted to know *why* balance sometimes showed a bare `—` instead of a
  number — a wrong or missing wallet password silently produces this with
  no indication that's the cause.
- **Fix:** The status line now shows the full wallet address. When the
  wallet snapshot fails to load, the (truncated) error is shown inline in
  red instead of a silent dash — e.g. it now surfaces "could not be
  unlocked with the provided password" directly.

<!-- Add new entries above this line as we fix more. -->
