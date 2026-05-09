# ZOKA Agent Notes

This file is for coding agents working in this repository. Keep public-facing
documentation in English and keep it focused on the actual network software.

## Project

ZOKA is a Rust privacy blockchain with private wallet flows, zero-knowledge
proof infrastructure, RandomX PoW, chain state, networking, snapshots, release
tooling, and public testnet documentation.

Core references:

- `README.md`: project entry point.
- `docs/testnet.md`: public testnet guide.
- `docs/node-operator-guide.md`: VPS and node operations.
- `docs/user-cli-reference.md`: user command surface.
- `docs/protocol-overview.md`: protocol and module overview.
- `ROADMAP.md`: public roadmap.

## Coding Posture

- Prefer existing Rust patterns and module boundaries.
- Keep user flows private by default.
- Do not reintroduce transparent user transfer paths unless the task explicitly
  asks for consensus-public surfaces.
- Treat consensus, economics, genesis, snapshots, PoW, and serialization changes
  as high risk. Add regression tests for drift.
- For ZK/SNARK work, keep circuit, prover, verifier, setup IO, and public input
  schema changes explicit and tested.
- Avoid `unwrap` and `expect` outside tests. Prefer existing error types.

## Important Modules

- `src/zk/`: Bellman/Groth16, circuits, verifier policy, setup IO, range proofs.
- `src/crypto/`: hashing, Pedersen/Bulletproof helpers, MPC ceremony tooling.
- `src/private_wallet/`: private wallet, CLSAG, claims, decryption.
- `src/transaction/`: commitments, nullifiers, transaction types.
- `src/block/`, `src/state/`, `src/mining/`: block validation, chain DB, PoW.
- `src/network/`, `src/consensus/`: libp2p and PBFT-related logic.
- `src/main.rs`, `src/bin/zokahd.rs`: CLI and daemon entry points.

## Commands

```powershell
cargo check --bins
cargo test --bin zoka
cargo test --lib wallet::storage::tests
cargo test --tests
cargo build --release --features randomx --bin zoka --bin zokahd
```

Use focused tests first when possible:

```powershell
cargo test <test_name>
cargo test --test <integration_test_name>
```

## Safety Notes

- Generated directories such as `.zoka/`, `.test_*`, `target/`, and
  `target-codex/` are local artifacts. Do not commit them.
- Do not edit frozen mainnet JSON artifacts casually:
  - `docs/genesis.mainnet.v1.json`
  - `docs/mainnet-network.v1.json`
  - `docs/economics.mainnet.v1.json`
- Trusted setup environment variables matter for release/security:
  `ZOKA_TRUSTED_SETUP_DIR`, `ZOKA_EXPECTED_SETUP_DIGEST`,
  `ZOKA_ALLOW_INSECURE_SEEDED_SETUP`.
- `ZOKA_ALLOW_UNSAFE_REHEARSAL=1` and
  `--unsafe-rehearsal-difficulty-bits` are local rehearsal tools only.
