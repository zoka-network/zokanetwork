# ZOKA Mainnet Release Cutover Checklist

## Build and provenance

- [ ] Release commit frozen (`v1.0.0-rcN` tag or equivalent).
- [ ] Windows build completed (`--release --features randomx`).
- [ ] Linux build completed (`--release --features randomx`).
- [ ] `scripts/mainnet_readiness_check.ps1 -BuildRelease` completed with
  `Failures: 0`.
- [ ] `scripts/package_release.ps1` executed on frozen commit.
- [ ] `SHA256SUMS.txt` generated.
- [ ] GPG signatures generated and verified.

## Verification

- [ ] Third-party reproducible build check passed.
- [ ] `docs/release-verification.md` steps validated end-to-end.
- [ ] Artifacts and manifest published.

## Runtime smoke

- [ ] `zokahd --help`
- [ ] `zoka wallet-create --help`
- [ ] `zoka wallet-restore-from-seed --help`
- [ ] `zoka wallet-balance --help`
- [ ] `zoka tx-send --help`
- [ ] `zoka mine --help`

## Local multi-node rehearsal

- [x] 2-4 local nodes boot with separate DBs, ports and wallets.
- [x] Nodes discover/connect to each other.
- [x] A mined block propagates from one node to local peers in 2-, 3- and
  4-node rehearsals.
- [x] 3+ node mining propagation hardened with seed reconnect and
  `--min-mine-peers`.
- [x] A fresh node syncs from height 0 to an observed peer head.
- [x] Restarted nodes preserve chain head and reconnect.
- [x] Staged late-join sync passes: node 1 reaches height 5, node 2 joins and
  reaches height 3, node 3 joins, and all three reach at least height 5
  (`staged-sync-20260501-135058`).
- [x] Mainnet fullnode bootstrap hardened with repeated `--seed`, multi-seed
  fallback, recent block rebroadcast and IBD retry cadence.
- [ ] Wallet flow over local network passes: create, backup, restore, balance,
  mine/receive coinbase, maturity, private transfer.
- [ ] Evidence archived: commands, node IDs, heights, head hashes and logs.

## CLI/UX gate

- [ ] Exhaustive command matrix for `zoka` and `zokahd` passes.
- [ ] Error cases are actionable: wrong password, missing wallet, invalid
  address, invalid path, no peers, empty DB.
- [ ] `--json` outputs remain stable and machine-readable.
- [ ] Human console output is readable: help, tables, errors, node/mining
  dashboard and wallet status.

## AI assistant gate

- [ ] AI assistant is optional and disabled unless explicitly invoked.
- [ ] Offline/local mode is the default.
- [ ] Assistant maps natural language to audited internal CLI/core actions.
- [ ] Assistant never reads, prints, sends or stores seeds, spend keys,
  passphrases or private material.
- [ ] Sensitive actions require explicit confirmation.
- [ ] Prompt-injection and secret-exfiltration tests pass.
