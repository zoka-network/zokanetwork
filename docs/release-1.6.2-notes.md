# ZOKA / ZSilent 1.6.2 — accumulating fixes

Running notes for the next patch release after 1.6.1. Nothing here is built
or published as a standalone release yet — the node-side fix below is
already deployed live on both seeds (an active-incident exception to the
usual "commit now, build/publish later" rule), but no new GitHub
release/ZSilent installer has been cut. Commits accumulate on the working
branches until we cut 1.6.2.

Repos:
- Node: `ZokaNetwork/ZokaNetwork` (branch `codex/mainnet-hotfix-economics-private-tx`, ahead of `v1.6.1` tag)
- App: `ZokaNetwork/zsilent-core` (branch `main`, at `v1.6.1`)

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

<!-- Add new entries above this line as we fix more. -->
