# ZOKA Local Multi-Node Rehearsal Status

Status: local steps 1-3 passed through wallet-over-local-network rehearsal on
2026-05-02. Remaining pre-mainnet work moves to CLI audit, console polish,
longer soak runs and public testnet/operator work.

Scope validated:

- 2-node local no-mining rehearsal: passed.
- 3-node local no-mining rehearsal: passed.
- 2-node local mining rehearsal: passed with unsafe local difficulty override.
- 3-node local mining rehearsal: passed after seed reconnect + minimum mining
  peer gate.
- 4-node local mining rehearsal: passed after the same hardening.
- Staged natural sync rehearsal: passed. Node 1 mined alone to height 5,
  node 2 joined and synced to height 3, then node 3 joined through multi-seed
  fallback; all three reached at least height 5.
- Staged competition + settle rehearsal: passed. After all three nodes synced
  to height 5, they mined concurrently past height 8, then settled to the same
  canonical head at height 9 / hash
  `4111557c4cc0efd371487b27f236a38352d3200ebacee84ebc47f2efd3e83374`.
- Churn rehearsal: passed after sync/fork-choice hardening. The test covered
  late joins, node offline/return with old DB, fresh node from DB empty,
  original seed loss, original seed return and settle. Final result: node-1,
  node-2, node-3 and node-4 reached height 5 with one canonical head
  `88c9ddf7eabec43883514009ca0737d90cdc5df36c2709011e8eb3601c6867ab`.
- Wallet network rehearsal: passed. A real miner wallet mined to coinbase
  maturity on local RandomX rehearsal difficulty, a verifier node synced to the
  same head, `tx-send` moved 1 ZKA from miner to receiver after maturity, and
  the private tx hash
  `a3f2c833aa851b8c730cb3c78e7d4e3f` was committed in block 65. Both nodes
  ended at height 65/head
  `f2072ed2982854c22b49afb251177642afe68d66b1f215eb046debb2e1f24070`.
- Late-join sync rehearsal: passed. A node with empty DB observed peer height
  and advanced to height 1.
- Restart rehearsal: passed. Restarted nodes preserved chain head and
  reconnected; the lagging node advanced to height 2.
- Each node used an isolated home directory, wallet storage and RocksDB path.
- Each node used a distinct local TCP listen port.
- Seed node listened successfully on `/ip4/127.0.0.1/tcp/29101`.
- Additional nodes connected to the seed with `--seed`.
- All nodes shut down cleanly by `--seconds` time limit.
- Mining rehearsal used `--unsafe-rehearsal-difficulty-bits 1` through the
  script-only `ZOKA_ALLOW_UNSAFE_REHEARSAL=1` guard. This is only for local
  propagation testing; mainnet difficulty constants remain unchanged.
- Earlier 3-node mining rehearsals exposed an isolated peer. That is now
  mitigated by automatic seed reconnect and `--min-mine-peers`, which blocks
  auto-mining until the expected peer count is connected.
- Staged sync exposed IBD starvation under late joins. `fullnode`/`zokahd` now
  support repeated `--seed` values, use seed fallback for late nodes, rebroadcast
  recent blocks, and run IBD with a retry cadence so the node can keep serving
  peer traffic while catching up.

Commands run:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\local_multinode_rehearsal.ps1 `
  -NodeCount 2 `
  -RunSeconds 12 `
  -NoMine

powershell -ExecutionPolicy Bypass -File scripts\local_multinode_rehearsal.ps1 `
  -NodeCount 3 `
  -RunSeconds 14 `
  -NoMine

powershell -ExecutionPolicy Bypass -File scripts\local_multinode_rehearsal.ps1 `
  -NodeCount 2 `
  -RunSeconds 45 `
  -StopGraceSeconds 40 `
  -SyncWarmupSeconds 8 `
  -MineIntervalSeconds 60 `
  -UnsafeRehearsalDifficultyBits 1

powershell -ExecutionPolicy Bypass -File scripts\local_multinode_rehearsal.ps1 `
  -NodeCount 3 `
  -RunSeconds 65 `
  -StopGraceSeconds 50 `
  -SyncWarmupSeconds 12 `
  -MineIntervalSeconds 90 `
  -UnsafeRehearsalDifficultyBits 1

powershell -ExecutionPolicy Bypass -File scripts\local_multinode_rehearsal.ps1 `
  -NodeCount 4 `
  -RunSeconds 75 `
  -StopGraceSeconds 60 `
  -SyncWarmupSeconds 16 `
  -MineIntervalSeconds 100 `
  -UnsafeRehearsalDifficultyBits 1

powershell -ExecutionPolicy Bypass -File scripts\staged_sync_rehearsal.ps1 `
  -NodeSeconds 900 `
  -WaitTimeoutSeconds 650 `
  -SeedMineIntervalSeconds 20 `
  -UnsafeRehearsalDifficultyBits 1 `
  -SyncRebroadcastRecentBlocks 8

powershell -ExecutionPolicy Bypass -File scripts\staged_sync_rehearsal.ps1 `
  -SeedMineIntervalSeconds 2 `
  -CompetitionMineIntervalSeconds 2 `
  -UnsafeRehearsalDifficultyBits 1 `
  -WaitTimeoutSeconds 300 `
  -NodeSeconds 360

powershell -ExecutionPolicy Bypass -File scripts\churn_multinode_rehearsal.ps1 `
  -MineIntervalSeconds 2 `
  -UnsafeRehearsalDifficultyBits 1 `
  -OfflineAdvanceHeight 7 `
  -ReturnCatchupHeight 7 `
  -LateJoinHeight 4 `
  -SeedDownHeight 8 `
  -FinalMinHeight 8 `
  -WaitTimeoutSeconds 420 `
  -PeerLossRecoverySeconds 45 `
  -NodeSeconds 1200 `
  -SettleSeconds 180 `
  -SettleAttempts 3
```

Latest evidence directory:

```text
.zoka\rehearsal\local-multinode-20260501-123747
```

Latest staged-sync evidence directory:

```text
.zoka\rehearsal\staged-sync-20260501-223605
```

Latest churn evidence directory:

```text
.zoka\rehearsal\churn-20260502-111857
```

Latest summary:

```text
node exit_code listening connected startup_ok clean_time_limit height
1    0         True      True      True       True             1
2    0         True      True      True       True             1
3    0         True      True      True       True             1
4    0         True      True      True       True             1
```

Important observed log lines:

```text
node-1: Auto-mined 1 block(s), broadcast to peers, scheduled 12 ZKA for maturity, next difficulty 1 bits.
node-2: Accepted peer block @ height 1 (chain head now @ 1 / 4 cumulative work)
```

Additional evidence:

```text
.zoka\rehearsal\late-sync-20260501-120928
.zoka\rehearsal\restart-20260501-121200
.zoka\rehearsal\local-multinode-20260501-123604
.zoka\rehearsal\local-multinode-20260501-123747
```

3-node hardened mining summary:

```text
.zoka\rehearsal\local-multinode-20260501-123604
node 1: height 1, clean shutdown true
node 2: height 1, clean shutdown true
node 3: height 1, clean shutdown true
```

4-node hardened mining summary:

```text
.zoka\rehearsal\local-multinode-20260501-123747
node 1: height 1, clean shutdown true
node 2: height 1, clean shutdown true
node 3: height 1, clean shutdown true
node 4: height 1, clean shutdown true
```

Late-sync summary:

```text
seed height: 2
late height: 1
late clean shutdown: true
```

Restart summary:

```text
before restart: seed=2, late=1
after restart:  seed-restart=2, late-restart=2
reconnected:    true
```

Staged natural sync summary:

```text
node1 reached height 5 before node2 launch
node2 reached height 3 before node3 launch
final milestones: node1=7, node2=5, node3=5
final persisted heights: node1=17, node2=5, node3=5
```

Staged competition + settle summary:

```text
node1 reached height 5 before node2 launch
node2 reached height 3 before node3 launch
all nodes reached sync floor height 5
competition target reached: node1=8, node2=8, node3=8
settled final height: node1=9, node2=9, node3=9
settled head hash: 4111557c4cc0efd371487b27f236a38352d3200ebacee84ebc47f2efd3e83374
canonical rewards: node1 won heights 1-5, node2 won heights 6-9
summary: .zoka\rehearsal\staged-sync-20260501-223605\summary.json
```

Latest churn/human-style rehearsal summary:

```text
scenario: node-2 late join, node-3 multi-seed join, node-2 offline/return,
          node-4 fresh DB, original seed loss, original seed return, settle
passed:   churn survival, fresh node sync, seed loss/return, final convergence
final:    node1 height 5 head 88c9ddf7eabec43883514009ca0737d90cdc5df36c2709011e8eb3601c6867ab
          node2 height 5 head 88c9ddf7eabec43883514009ca0737d90cdc5df36c2709011e8eb3601c6867ab
          node3 height 5 head 88c9ddf7eabec43883514009ca0737d90cdc5df36c2709011e8eb3601c6867ab
          node4 height 5 head 88c9ddf7eabec43883514009ca0737d90cdc5df36c2709011e8eb3601c6867ab
summary:  .zoka\rehearsal\churn-20260502-184407\summary.json
```

Wallet-over-local-network summary:

```text
miner reached spend-ready height 64, receiver started at 0
transfer: 1 ZKA private tx a3f2c833aa851b8c730cb3c78e7d4e3f
block:    height 65 private_tx_count=1
final:    node1 height 65 head f2072ed2982854c22b49afb251177642afe68d66b1f215eb046debb2e1f24070
          node2 height 65 head f2072ed2982854c22b49afb251177642afe68d66b1f215eb046debb2e1f24070
summary:  .zoka\rehearsal\wallet-network-20260502-211129\manual-summary.json
```
