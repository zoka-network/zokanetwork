# ZOKA Network

ZOKA is a Rust privacy blockchain built around private wallet flows,
zero-knowledge proof infrastructure, RandomX proof of work, per-network wallet
storage, and a full-node daemon for public testnet operation.

This repository is the public source tree for the ZOKA network software. The
current release line is **ZOKA Testnet v0.1.0**.

## Quick Start

Download release binaries from GitHub Releases when available:

```text
https://github.com/zokachain/zoka-network/releases
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
| Public testnet | [docs/testnet.md](docs/testnet.md) |
| Node and VPS operation | [docs/node-operator-guide.md](docs/node-operator-guide.md) |
| CLI reference | [docs/user-cli-reference.md](docs/user-cli-reference.md) |
| Release verification | [docs/release-verification.md](docs/release-verification.md) |
| Protocol overview | [docs/protocol-overview.md](docs/protocol-overview.md) |
| Troubleshooting | [docs/troubleshooting.md](docs/troubleshooting.md) |
| Documentation index | [docs/README.md](docs/README.md) |

## Network Artifacts

The published configuration artifacts live in `docs/`:

| Artifact | Purpose |
|---|---|
| `docs/genesis.testnet.v1.json` | Testnet genesis file |
| `docs/testnet-network.v1.json` | Testnet seed/RPC/snapshot configuration |
| `docs/economics.testnet.v1.json` | Testnet economic constants |
| `docs/genesis.mainnet.v1.json` | Mainnet genesis candidate |
| `docs/mainnet-network.v1.json` | Mainnet network configuration candidate |
| `docs/economics.mainnet.v2-atoms.json` | Mainnet economic constants |

## Core Properties

- Private wallet flows are the user default.
- User transfers are private; no transparent user-transfer UX is exposed.
- Wallet files are stored per network under `.zoka/<network>/private_wallets/`.
- Testnet and mainnet use separate chain databases and wallet directories.
- RandomX (`zoka-randomx-v1`) is the canonical PoW backend for public network
  binaries.
- ZKA uses 6 decimal places: `1 ZKA = 1,000,000 atoms`.
- Coinbase maturity is 60 blocks.

## Build And Test

```sh
cargo check --bins
cargo test --bin zoka
cargo test --lib wallet::storage::tests
cargo test --tests
cargo build --release --features randomx --bin zoka --bin zokahd
```

The full test suite includes networking and integration tests. Some long-running
soak scenarios are ignored by default.

## Repository Layout

```text
src/main.rs          zoka CLI
src/bin/zokahd.rs    full-node daemon wrapper
src/mining/          PoW engine, difficulty, block validation
src/state/           chain database, fork choice, sync
src/network/         libp2p transport and peer handling
src/private_wallet/  private wallet, claims, note handling
src/wallet/          encrypted wallet storage
src/zk/              proof circuits, prover, verifier, setup IO
docs/                public network docs and release artifacts
scripts/             build, release, ceremony, and operator tooling
tests/               integration tests
fuzz/                fuzz targets
```

## Security Notice

ZOKA testnet is for public testing, miner/operator rehearsal, and protocol
validation. Testnet coins have no economic value. Do not store production
secrets in the repository, do not publish wallet recovery phrases, and always
verify release checksums before running binaries on a VPS.
