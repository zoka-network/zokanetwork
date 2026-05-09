# ZOKA Protocol Overview

ZOKA is a privacy-oriented blockchain implementation written in Rust. The
software combines private wallet flows, zero-knowledge proof infrastructure,
RandomX PoW, encrypted wallet storage, local chain state, and libp2p networking.

## Network

| Network | Chain ID | Purpose |
|---|---|---|
| Testnet | `zoka-testnet-v1` | Public testing and operator rehearsal |

Wallets and databases are separated by network. Testnet data is stored under
`.zoka/testnet/`.

## Token Accounting

ZKA uses atom accounting:

```text
1 ZKA = 1,000,000 atoms
```

The published economics artifacts define max supply, reward schedule, decimal
precision, fee step, and coinbase maturity.

## Proof Of Work

Public network binaries use RandomX through the `randomx` feature:

```sh
cargo build --release --features randomx --bin zoka --bin zokahd
```

The development CPU fallback exists for CI and local isolated testing only. It
is not the public-network PoW backend.

## Wallet Privacy

The user-facing wallet path is private by default:

- Local wallet files are encrypted.
- Wallets are separated by network.
- The CLI does not expose a normal transparent user-transfer workflow.
- Private transfers use commitments, nullifiers, private notes, and encrypted
  data needed by the recipient wallet.

Some consensus data is necessarily public, such as block headers, PoW, coinbase
structure, nullifiers, and commitments.

## Blocks And State

The chain state layer validates blocks, applies private transactions, tracks
chain metadata, handles fork choice through cumulative work, and supports sync
from peers.

Important modules:

| Module | Responsibility |
|---|---|
| `src/mining/` | PoW engine, difficulty, block validation |
| `src/state/` | Chain DB, fork choice, sync |
| `src/network/` | libp2p node, seed connection, peer messages |
| `src/private_wallet/` | Wallet notes, claims, decryption |
| `src/zk/` | Circuits, setup IO, prover, verifier |
| `src/wallet/` | Encrypted wallet file storage |

## Trusted Setup

The Groth16 setup path is explicit. Release operators must treat setup files as
release-critical material. Development-only seeded setup is guarded by
`ZOKA_ALLOW_INSECURE_SEEDED_SETUP` and must not be used for production-value
networks.
