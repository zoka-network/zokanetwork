# ZOKA Documentation

This directory contains the public documentation and network artifacts for the
ZOKA testnet and future mainnet release line.

## Start Here

| Document | Purpose |
|---|---|
| [testnet.md](testnet.md) | Public testnet guide |
| [node-operator-guide.md](node-operator-guide.md) | VPS and node operations |
| [user-cli-reference.md](user-cli-reference.md) | User-facing CLI commands |
| [release-verification.md](release-verification.md) | How to verify release binaries |
| [protocol-overview.md](protocol-overview.md) | Architecture and protocol summary |
| [troubleshooting.md](troubleshooting.md) | Common failures and fixes |
| [runbook-incidents.md](runbook-incidents.md) | Incident response |

## Network Artifacts

| File | Purpose |
|---|---|
| `genesis.testnet.v1.json` | Testnet genesis |
| `testnet-network.v1.json` | Testnet seed/RPC/snapshot configuration |
| `economics.testnet.v1.json` | Testnet economic constants |
| `genesis.mainnet.v1.json` | Mainnet genesis candidate |
| `mainnet-network.v1.json` | Mainnet network configuration candidate |
| `economics.mainnet.v2-atoms.json` | Mainnet economic constants |

## Technical References

| Document | Purpose |
|---|---|
| [pow-mainnet-decision.md](pow-mainnet-decision.md) | RandomX PoW decision |
| [genesis.mainnet.v1.spec.md](genesis.mainnet.v1.spec.md) | Genesis schema notes |
| [reproducible-builds.md](reproducible-builds.md) | Build reproducibility notes |
| [infrastructure-logs-policy.md](infrastructure-logs-policy.md) | Official infrastructure logging policy |
| [tor_i2p_transport.md](tor_i2p_transport.md) | Transport privacy policy |
| [genesis-dry-run-evidence.md](genesis-dry-run-evidence.md) | Mainnet candidate genesis digest evidence |

## Operational Runbooks

| Document | Purpose |
|---|---|
| [runbook-incidents.md](runbook-incidents.md) | General incident response |
| [runbook-key-rotation.md](runbook-key-rotation.md) | Operational key rotation |
| [runbook-pbft-halt-recovery.md](runbook-pbft-halt-recovery.md) | Future PBFT halt recovery |
| [runbook-upgrade-hardfork.md](runbook-upgrade-hardfork.md) | Coordinated upgrade and hard fork planning |

## Principle

Public docs should help a user, miner, operator, auditor, or contributor run the
network. Internal notes, stale roadmaps, and local rehearsal logs should stay out
of the public documentation set.
