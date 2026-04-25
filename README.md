# 🔐 ZOKA - Private Blockchain

Rust-based private blockchain with zero-knowledge proofs, commitments, nullifiers, and PBFT consensus.

## Quick Start

```bash
cargo build --release
cargo test --all
cargo run --example private_scenario
```

## Current Status

- **Privacy:** 22% (Monero-equivalent)
- **Build:** ✅ Passing (0 errors)
- **Tests:** ✅ 177/177 passing

## Implementation Roadmap

See **[ROADMAP.md](ROADMAP.md)** for the complete plan to reach **95%+ Monero-level privacy** (15-17 hours).

5 Phases:
1. P2P Integration (4-6h) → 40%
2. Stealth Addresses (3-4h) → 70%
3. Range Proofs (2-3h) → 85%
4. Persistent Wallets (2-3h) → 90%
5. Balance Privacy (4-5h) → 95%

## Project Structure

```
src/
├── crypto/           # Signatures, hashing, KDF
├── transaction/      # Commitments, nullifiers, validation
├── zk/              # ZK circuits & proofs
├── consensus/       # PBFT protocol
├── network/         # P2P with libp2p
├── private_wallet/  # Private transaction management
├── state/           # State + Merkle tree
├── storage/         # RocksDB persistence
├── block/           # Block structure
├── mempool/         # Transaction pool
└── bin/            # CLI utilities


tests/             # Integration tests
```

## Build & Test

```bash
# Build
cargo build

# Test everything
cargo test --all

# Run example
cargo run --example private_scenario

# Interactive CLI
cargo run --bin user-cli-private
```

## Requirements

- Rust 1.70+
- libp2p 0.53+
- Bellman cryptography
- RocksDB

---

**See [ROADMAP.md](ROADMAP.md) to implement Monero-level privacy.**
