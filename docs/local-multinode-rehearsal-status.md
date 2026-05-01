# ZOKA Local Multi-Node Rehearsal Status

Status: step 1 passed on 2026-05-01.

Scope validated:

- 2-node local rehearsal: passed.
- 3-node local rehearsal: passed.
- Each node used an isolated home directory, wallet storage and RocksDB path.
- Each node used a distinct local TCP listen port.
- Seed node listened successfully on `/ip4/127.0.0.1/tcp/29101`.
- Additional nodes connected to the seed with `--seed`.
- All nodes shut down cleanly by `--seconds` time limit.
- Mining was disabled for this first rehearsal with `-NoMine`; block
  propagation, IBD from an advanced peer, restart persistence and wallet
  transfer flow remain for the next rehearsal steps.

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
```

Latest evidence directory:

```text
.zoka\rehearsal\local-multinode-20260501-113353
```

Latest summary:

```text
node exit_code listening connected startup_ok clean_time_limit height
1    0         True      True      True       True             0
2    0         True      True      True       True             0
3    0         True      True      True       True             0
```

