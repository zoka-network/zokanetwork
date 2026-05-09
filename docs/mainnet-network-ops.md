# ZOKA Mainnet Network Operations v1

This document freezes the operational format for public seed nodes, optional
public RPC endpoints, and signed snapshots. Real endpoints are intentionally
not filled until launch operators exist.

## Network Config

Canonical template: [`mainnet-network.v1.json`](mainnet-network.v1.json).
Launch material template:
[`mainnet-launch-material.template.json`](mainnet-launch-material.template.json).

Required before launch:

- `seed_nodes`: at least 3 entries across independent regions and operators.
- `public_rpc`: optional; if published, each endpoint must state transport and
  logging policy.
- `snapshot_signers`: at least one ed25519 public key if fast-sync snapshots
  are published.

`zokahd` and `zoka fullnode` load this file automatically from
`docs/mainnet-network.v1.json` when no manual `--seed` is supplied. The daemon
rejects a config whose `chain_id` or `protocol_freeze_id` does not match the
binary. Operators can override the path with `--network-config <file>` or turn
off auto-load with `--no-network-config`.

Manual bootstrap supports repeated `--seed <multiaddr>` values. A late or
recovering node should receive several independent seed addresses so it can
fall back if one bootstrap peer is busy, unreachable or partitioned.

## Seed Node Entry

Each seed node entry uses this shape:

```json
{
  "label": "seed-na-1",
  "operator": "zoka-core-or-independent-operator",
  "region": "north-america",
  "multiaddr": "/dns4/seed-na-1.zoka.example/tcp/28080/p2p/<peer-id>",
  "onion": "/onion3/<service-id>:28080/p2p/<peer-id>",
  "i2p": "/dnsaddr/<service>.b32.i2p/p2p/<peer-id>",
  "role": "seed",
  "status": "active"
}
```

Rules:

- Do not put all seed nodes on one provider, region, or legal operator.
- Publish onion/I2P endpoints whenever available.
- Rotate failed seed nodes by publishing a new signed network config.

## Public RPC Entry

Public RPC is optional. If offered, publish:

```json
{
  "label": "rpc-eu-1",
  "operator": "zoka-core-or-independent-operator",
  "region": "europe",
  "url": "https://rpc-eu-1.zoka.example",
  "onion": "http://<service-id>.onion",
  "i2p": "http://<service>.b32.i2p",
  "logging_policy": "docs/infrastructure-logs-policy.md",
  "status": "active"
}
```

RPC rules:

- No IP logging.
- No wallet seed, spend key, or scan key ever leaves the user machine.
- Remote RPC must be treated as untrusted by wallet code.

## Current Status

Fase 4 technical plumbing is implemented: anonymous transport config, explicit
clearnet fallback, IBD sync, pruning strategy, signed snapshot import/export,
and automatic loading of `docs/mainnet-network.v1.json` by `zokahd`/`fullnode`.

Remaining operational blockers:

- fill a copy of `docs/mainnet-launch-material.template.json` outside the repo
  with real operator material, then apply it with
  `scripts/apply_mainnet_launch_material.ps1 -InPlace`;
- promote staged operators/endpoints to `active` after final reachability checks;
- replace placeholder peer IDs/onion service IDs with launch values in `docs/mainnet-network.v1.json`;
- complete and publish contact ownership in `docs/mainnet-operators-roster.md`;
- finalize snapshot signer public key material if fast-sync is enabled.

Operational runbooks used by launch:

- `docs/runbook-key-rotation.md`
- `docs/runbook-pbft-halt-recovery.md`
- `docs/runbook-incidents.md`
- `docs/runbook-upgrade-hardfork.md`
