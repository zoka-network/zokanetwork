# ZOKA Agent Instructions

Use this file as the compact working context for coding agents in this repo.
Keep token use low: read only the files needed for the current task, summarize
findings before broad exploration, and avoid re-reading docs already cited here.

## Project

ZOKA is a Rust privacy blockchain with zero-knowledge proof infrastructure,
private wallet flows, RandomX PoW, chain state, networking, snapshots, release
tooling, and mainnet launch docs.

Core references:

- `README.md`: quick project map and canonical commands.
- `docs/project-state.md`: current built state.
- `docs/agent-vs-human-roadmap.md`: what an agent can still build vs external
  human/operator work.
- `PENDIENTE.md`: latest practical launch blockers.
- `docs/mainnet_launch_roadmap_v6.md`: canonical launch roadmap.

## Coding Posture

- Prefer existing Rust patterns and module boundaries.
- Keep public user flows private-by-default. Do not reintroduce normal public
  transfer paths unless the task explicitly asks for consensus-public surfaces.
- Treat consensus, economics, genesis, snapshots, PoW, and serialization changes
  as high risk. Add regression tests for drift.
- For ZK/SNARK work, keep circuit, prover, verifier, setup IO, and public input
  schema changes explicit and tested.
- Do not edit mainnet artifacts casually:
  - `docs/genesis.mainnet.v1.json`
  - `docs/mainnet-network.v1.json`
  - `docs/economics.mainnet.v1.json`

## Important Modules

- `src/zk/`: Bellman/Groth16, circuits, verifier policy, setup IO, range proofs.
- `src/crypto/`: hashing, Pedersen/Bulletproof-related helpers, MPC ceremony.
- `src/private_wallet/`: private wallet, CLSAG, claims and decryption.
- `src/transaction/`: commitments, nullifiers, transaction types.
- `src/block/`, `src/state/`, `src/mining/`: block validation, chain DB, PoW.
- `src/network/`, `src/consensus/`: libp2p and PBFT-related transport/logic.
- `src/cli/`, `src/main.rs`: user-facing command surface.

## Commands

Fast checks:

```powershell
cargo test --lib
cargo test --tests
cargo test --bin zoka
cargo check --bins
```

Mainnet/RandomX checks:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/check_randomx_toolchain.ps1 -Strict
cargo build --release --features randomx --bin zoka --bin zokahd
```

Use focused tests first when possible:

```powershell
cargo test <test_name>
cargo test --test <integration_test_name>
```

## Token Budget Rules

- Start with `rg --files` and `rg <symbol>` instead of opening whole folders.
- Read the smallest relevant file slices.
- Prefer local docs over internet searches.
- When making a change, state the touched files and tests run.
- If blocked by external actors, say so and point to the exact doc/runbook.

## Safety Notes

- The worktree may contain local RocksDB/test artifacts such as `.zoka/`,
  `.test_*`, `target/`, `target-codex/`, and similar generated directories.
  Do not clean or revert them unless explicitly asked.
- Mainnet constants are frozen in `src/economics.rs`: chain id
  `zoka-mainnet-v1`, PoW `zoka-randomx-v1`, max supply 23M ZKA, halving
  1,051,200 blocks, target block time 120s, coinbase maturity 60.
- Trusted setup env vars matter for release/security:
  `ZOKA_TRUSTED_SETUP_DIR`, `ZOKA_EXPECTED_SETUP_DIGEST`,
  `ZOKA_ALLOW_INSECURE_SEEDED_SETUP`. Seeded setup is dev-only.
- Circuit changes can invalidate ceremony/trusted setup. `private_tx` supports
  up to 96 inputs, with 8 membership proofs inside the circuit.
- Avoid `unwrap`/`expect` outside tests. Prefer existing error types such as
  `thiserror` errors and `BlockError`.
