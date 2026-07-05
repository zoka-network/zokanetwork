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

<!-- Add new entries above this line as we fix more. -->
