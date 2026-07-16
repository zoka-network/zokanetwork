# ZOKA Network

ZOKA is a Rust privacy blockchain with private wallet flows, RandomX proof of
work, zero-knowledge proof infrastructure, and a full-node daemon for the public
testnet.

This repository is intentionally small and user-facing. It contains the source
needed to build and run the ZOKA testnet software, plus the network files needed
to verify and join the testnet.

## Quick Start

Download binaries from GitHub Releases:

```text
https://github.com/ZokaNetwork/ZokaNetwork/releases
```

Build from source:

```sh
cargo build --release --features randomx --bin zoka --bin zokahd
```

Run a testnet node:

```sh
zokahd --network testnet --password "<wallet-password>"
```

Create a testnet wallet:

```sh
zoka create my-wallet --network testnet --password "<wallet-password>"
```

List local wallets:

```sh
zoka wallets --network testnet
```

## Documentation

| Topic | Document |
|---|---|
| Join the testnet | [docs/testnet.md](docs/testnet.md) |
| Run a VPS node | [docs/node-operator-guide.md](docs/node-operator-guide.md) |
| Deploy on Fly.io | [docs/fly-bootstrap.md](docs/fly-bootstrap.md) |
| CLI commands | [docs/user-cli-reference.md](docs/user-cli-reference.md) |
| Verify releases | [docs/release-verification.md](docs/release-verification.md) |
| Protocol overview | [docs/protocol-overview.md](docs/protocol-overview.md) |
| Troubleshooting | [docs/troubleshooting.md](docs/troubleshooting.md) |

## Testnet Files

| File | Purpose |
|---|---|
| `docs/genesis.testnet.v1.json` | Testnet genesis |
| `docs/testnet-network.v1.json` | Testnet seed/RPC configuration |
| `docs/economics.testnet.v1.json` | Testnet economics |

## Core Properties

- User transfers are private by default.
- Wallet files are encrypted and stored per network.
- Testnet data lives under `.zoka/testnet/`.
- RandomX (`zoka-randomx-v1`) is the public testnet PoW backend.
- ZKA uses 6 decimal places: `1 ZKA = 1,000,000 atoms`.
- Coinbase maturity is 60 blocks.

## Canonical Chain

The canonical ZOKA mainnet is anchored at **block 3127**:

```text
height 3127  hash b5c20cfbdc9e2f56bb143334c03f7bcc9df60fbd7c2c7ed553b61ee2dd9740df
```

From 1.8.0 on, every node enforces this checkpoint. A node whose block at
height 3127 does **not** match this hash is on a different chain and its blocks
are rejected — it is **not** part of ZOKA network. Any such branch is a *side
chain*: typically a fork built by a node that was mining in isolation
(disconnected from the mesh) and drifted onto its own history. The network
follows the chain that contains block 3127 above; everything else is discarded.

This checkpoint also anchors the serverless difficulty reset: from **block
3128** onward, difficulty is capped to a sane target (after the hashrate
collapse had pushed it far too high for the remaining miners), so the chain
keeps producing blocks at a healthy pace. Both rules apply only on the chain
that carries the 3127 anchor.

## Repository Layout

```text
Cargo.toml          Rust package definition
Cargo.lock          Locked dependency graph
src/                ZOKA node, CLI, wallet, mining, state, network, and ZK code
docs/               Public testnet documentation and network artifacts
```

## Security Notice

ZOKA testnet coins have no economic value. Do not publish wallet recovery
phrases, spend keys, scan keys, or VPS secrets. Always verify release checksums
before running binaries on a server.
