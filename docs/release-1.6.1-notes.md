# ZOKA / ZSilent 1.6.1 — accumulating fixes

Running notes for the next patch release. Nothing here is built or published
yet — commits accumulate on the working branches until we cut 1.6.1.

Repos:
- Node: `ZokaNetwork/ZokaNetwork` (branch `codex/mainnet-hotfix-economics-private-tx`, ahead of `main`@v1.6.0)
- App: `ZokaNetwork/zsilent-core` (branch `main`)

## Fixes included so far

### 1. Node: `--seed` augments the manifest seeds instead of replacing them
- **Commit:** `65c0ed5` (node)
- **Problem:** A node launched with a single `--seed` plus `--network-config`
  only ever dialed that one seed. The manual-seed branch loaded the manifest
  for its public RPCs but dropped the manifest's `seed_nodes`, so
  `reconnect_seed_peers` stayed a one-element list and `target_seed_peers`
  clamped to 1. The node connected to that seed and stopped — capping it at a
  single P2P peer even though other seeds were reachable. This is exactly what
  the ZSilent wallet node hit: it passes only the first manifest seed as
  `--seed`, so it saw BOOT but never dialed VPS2 (1 peer, 0–1 mesh).
- **Fix:** Merge the manifest's active seeds into the reconnect pool in the
  manual-seed branch too (deduped), so `--seed` augments the network config.
  The node now dials every seed and forms a real multi-peer mesh.

### 2. ZSilent: Node section shows the local node's own peer count
- **Commit:** `a67188d` (zsilent-core)
- **Problem:** The Node section's "Connected peers" read
  `ui.stats.connectedPeers`, which is the **bootstrap seed's** peer count (from
  the remote RPC), not this node's. A node with 1 peer showed the seed's ~20 —
  hiding that the local node was barely connected.
- **Fix:** Added `localConnectedPeers` (from the local node's `/stats`) to the
  UI state and populated it in `pollRpc`. The Local node panel now shows YOUR
  node's peer count; the Network state panel shows both "Your node peers" and
  "Network peers" so the two figures are clearly distinct.

### 3. Node: filter private/loopback addresses out of peer discovery
- **Commit:** _(this commit)_ (node)
- **Problem:** A peer listening on `0.0.0.0` announces every local interface
  through Identify, including datacenter-internal (`10/8`, `172.16/12`,
  `192.168/16`), loopback and link-local addresses. External nodes recorded
  those, added them to the reconnect/Kademlia pool, and repeatedly tried to
  dial them — unreachable from anywhere but the peer's own LAN. This is why
  nodes kept dialing the seeds' `10.x` droplet-internal addresses and wasting
  connection attempts, which also hurt mesh formation network-wide.
- **Fix:** `record_observed_peer` now drops announced addresses that aren't
  publicly dialable (private/loopback/link-local/unspecified, IPv4 and IPv6),
  so they never enter the dial pool. Non-IP transports (dns, onion, i2p) pass
  through. Opt out for LAN/dev with `ZOKA_ALLOW_PRIVATE_PEER_ADDRS=1`.
  Regression test: `discovered_addr_filter_rejects_private_and_loopback`.

### 4. Node: tolerate a corrupt/empty mining-state file instead of crashing
- **Commit:** _(this commit)_ (node)
- **Problem:** `load_user_mining_state_for_network` propagated a hard error
  ("invalid mining state json … expected value at line 1 column 1") when
  `.zoka/<net>/user_mining_state.json` was empty or truncated — which happens
  if the process is Ctrl+C'd mid-save. The node then refused to start. Hit by
  a standalone CLI miner on the second run.
- **Fix:** The mining-state file is a local runtime cache (stats/progress), not
  consensus data, so a corrupt one now resets to default (with a warning) and
  is rebuilt on the next save, instead of stopping the node.

### 5. Node: `--mining-console` fluid streaming console for standalone miners
- **Commit:** _(this commit)_ (node)
- **Problem:** The fullnode runtime renders a full-screen dashboard that clears
  and redraws the terminal on every tick (`\x1B[2J\x1B[H`). That fights the
  streaming `mining: progress …` lines: the console flows fine until a block
  arrives or the dashboard redraws, at which point the screen wipes and "goes
  crazy" before settling back. Fine for the ZSilent-embedded node, jarring for
  someone running the CLI binary directly in a terminal.
- **Fix:** New `--mining-console` flag (or `ZOKA_MINING_CONSOLE=1`) switches the
  fullnode to a fluid, append-only console: no screen clear. The `mining:
  progress …` lines stream as before, and a compact one-line `status …` /
  `warmup …` / `stopped …` summary (height, peers, mining on/off, hashrate,
  session blocks/rewards, last event) is appended at most every 6s — or
  immediately when the last event changes, so block arrivals surface promptly
  without wiping the scrollback. The full-screen dashboard remains the default;
  this is opt-in for standalone/headless mining.

### 6. Node: editable `zoka.config.json` + trusted-setup auto-discovery (no wrapper script)
- **Commit:** _(this commit)_ (node)
- **Problem:** Running a standalone node meant either a long command line or a
  Windows-only `.bat`. Linux/macOS miners had no easy path, and every parameter
  tweak meant editing the launcher. Separately, a release binary panics on boot
  unless `ZOKA_TRUSTED_SETUP_DIR` is set, so the binary couldn't be run directly
  next to its `trusted-setup/` folder without the wrapper exporting that var.
- **Fix:** `zoka fullnode` now reads an optional editable `zoka.config.json`
  (à la `bitcoin.conf`), resolved from `--config <path>`, `$ZOKA_CONFIG`, or
  `zoka.config.json` next to the binary / in the CWD. Fields map onto existing
  flags (`network`, `address`, `password`/`password_env`, `threads`, `power`,
  `mine`, `mining_console`, `allow_isolated_start`, `network_config`, `db`,
  `seeds`); the command line always overrides the file, and `seeds` are unioned
  with the manifest. Password handling is flexible: plaintext if you want it,
  `password_env` for safety, or prompt when left empty. Additionally, when
  `ZOKA_TRUSTED_SETUP_DIR` is unset the node auto-discovers a `trusted-setup/`
  folder next to the executable (digest still verified, so a tampered bundle is
  rejected). Net effect: drop the binary + `trusted-setup/` + a `zoka.config.json`
  in a folder and just run `zoka fullnode` — cross-platform, no `.bat`.
  Docs: `docs/node-config.md`, example `docs/zoka.config.example.json`.

### 7. Node: prompt_required no longer spins forever on non-interactive stdin
- **Commit:** _(this commit)_ (node)
- **Problem:** `prompt_required` looped on empty input to re-ask, but couldn't
  tell "user pressed Enter on an empty line" from "stdin is closed/EOF"
  (`read_line` returns 0 bytes). In a headless run (systemd, piped, no TTY) it
  spun forever printing `Value is required.` — burning a CPU core and flooding
  logs. The new config-file flow made this far more likely: a standalone/Linux
  miner with an empty `password` would hit the wallet-password prompt with no
  TTY. (Caught live: a 30s headless run produced ~2 million lines.)
- **Fix:** `prompt_line` now distinguishes EOF (returns `None`) from an empty
  line. `prompt_required` fails fast with a clear message on EOF instead of
  looping; interactive behaviour (re-ask on empty Enter) is unchanged.

### 8. Node: mining-console polish (single ZKA unit, 6s throttle, quiet logs)
- **Commit:** _(this commit)_ (node)
- **Problem:** Shaking out the fluid console on a live run surfaced three warts:
  (a) the status line printed `0 ZKA ZKA` (double unit — `format_zka` already
  appends the unit); (b) it emitted a status line on every `last_event` change,
  which during IBD means one line per synced block (spam, not "fluid"); and
  (c) the node's default INFO tracing (p2p gossip, block responses) interleaved
  with and drowned the console.
- **Fix:** (a) use `format_zka_amount` inside the `(… ZKA)` wrapper; (b) throttle
  the status line to ~6s (a newly mined block still bypasses the throttle so it
  shows promptly) instead of keying on `last_event`; (c) in mining-console mode,
  default `RUST_LOG` to `warn` when unset so the streaming lines stay readable
  (an explicit `RUST_LOG` still wins; the shipped launcher also sets it).

### 9. Node: client IP was spoofable (rate-limit/allow-list bypass + blind audit log)
- **Commits:** _(this commit)_ (node)
- **Problem:** Caught live on 2026-07-05 while investigating an active
  `POST /sync/block` hammering pattern (~1 attempt/2s, ~98% rejected on both
  seeds, right after the protocol freeze was enforced -- consistent with a
  frozen-out peer trying to route around it via the ungated RPC path). The
  audit log showed **every single request as `127.0.0.1`**, on both seeds,
  even for real remote traffic: `auth_and_rate_limit` derived `client_ip`
  purely from the `X-Forwarded-For`/`X-Real-IP` request headers, which is
  ordinary attacker-controlled input on a node with no reverse proxy in
  front of it. Anyone could set an arbitrary (or absent) value on every
  request to mint a "new" identity each time, making the per-IP rate
  limiter and the `ip_whitelist`/`ip_blacklist` access-control lists fully
  bypassable, and erasing any real trail from the audit log.
- **Fix:** `client_ip` now comes from the real TCP peer address
  (`ConnectInfo<SocketAddr>`, wired in via
  `into_make_service_with_connect_info`), and the forwarded-for/real-ip
  headers are only honored when that peer is in a new, empty-by-default
  `ZOKA_TRUSTED_PROXY_IPS` allowlist (for operators who do run behind a
  real reverse proxy). Extracted into a small pure `resolve_client_ip`
  helper with dedicated tests, including a regression test for the exact
  bug caught live (rotating the forwarded header no longer mints distinct
  identities for the same untrusted peer).
- **Also fixed in the same handler:** `POST /sync/block`'s rejection
  responses (`GapAhead` "gap: local=X candidate=Y" and the generic
  catch-all) echoed the exact local chain height and internal `SyncOutcome`
  debug detail back to the caller -- free calibration feedback for whoever
  is probing that endpoint. Both now return a generic message to the
  client; full detail still goes to the server-side `tracing::warn!` log.
- **Not fixed / out of scope for this pass:** the underlying probing
  traffic itself was not blocked (it was already being safely rejected at
  the consensus level -- no forged block was ever applied) and, since the
  audit log was blind until this fix, we cannot retroactively attribute it
  to a real IP. Worth watching after this ships: does the same pattern
  keep going, now attributable to a real address?

<!-- Add new entries above this line as we fix more. -->
