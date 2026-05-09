# ZOKA — Private Blockchain

Rust-based privacy chain with zero-knowledge proofs, RandomX PoW, BIP39 wallets,
and PBFT-ready consensus. Mainnet v1 in pre-launch.

## Where to start

| Audience | Read this first |
|---|---|
| You want the full picture of what's built | [`docs/project-state.md`](docs/project-state.md) |
| You want to know what's still pending | [`docs/agent-vs-human-roadmap.md`](docs/agent-vs-human-roadmap.md) |
| You want the canonical mainnet roadmap | [`docs/mainnet_launch_roadmap_v6.md`](docs/mainnet_launch_roadmap_v6.md) |
| You want to use the CLI | [`docs/user-cli-reference.en.md`](docs/user-cli-reference.en.md) |
| You want to verify a release binary | [`docs/release-verification.md`](docs/release-verification.md) |
| You want every doc indexed | [`docs/README.md`](docs/README.md) |

## Build

```sh
# Dev / testnet (works on any host with Rust)
cargo build --release

# Mainnet binary (requires MSVC + cmake on Windows, gcc + cmake on Linux)
powershell -ExecutionPolicy Bypass -File scripts/check_randomx_toolchain.ps1 -Strict
powershell -ExecutionPolicy Bypass -File scripts/build_mainnet_release.ps1
```

See [`docs/reproducible-builds.md`](docs/reproducible-builds.md) for the exact
canonical build command and the toolchain pin used for releases.

## Test

```sh
cargo test --lib       # ~600 unit tests
cargo test --tests     # integration tests in tests/
```

Test layout is documented in [`tests/README.md`](tests/README.md).

## Frozen mainnet decisions

- **PoW**: RandomX (`zoka-randomx-v1`). See [`docs/pow-mainnet-decision.md`](docs/pow-mainnet-decision.md).
- **Token**: ZKA, 6 decimals (`1 ZKA = 1,000,000 atoms`), max supply
  23,000,000, halving every 1,051,200 blocks, target block time 120s,
  coinbase maturity 60 blocks. See `src/economics.rs` and
  [`docs/economics.mainnet.v2-atoms.json`](docs/economics.mainnet.v2-atoms.json).
- **Genesis schema**: `zoka-genesis-v1`. See
  [`docs/genesis.mainnet.v1.spec.md`](docs/genesis.mainnet.v1.spec.md).
- **Snapshot schema**: `zoka-snapshot-v1`. See
  [`docs/pruning-snapshots-strategy.md`](docs/pruning-snapshots-strategy.md).

## Project structure

```
src/
├── main.rs                  # zoka CLI
├── lib.rs
├── economics.rs             # frozen economic constants
├── genesis.rs               # zoka-genesis-v1
├── snapshot.rs              # zoka-snapshot-v1
├── mining/                  # PoW engine, difficulty, block validator
├── state/                   # chain DB, fork choice, IBD sync
├── storage/                 # RocksDB wrapper
├── network/                 # libp2p (Tor/I2P aware), gossipsub, dandelion
├── consensus/               # PBFT (separate from PoW chain DB today)
├── private_wallet/          # PrivateWallet, claims, decryption
├── wallet/                  # encrypted wallet storage
├── transaction/             # CLSAG, nullifiers
├── crypto/                  # Pedersen, Bulletproofs, MPC ceremony
├── identity/                # HDWallet, BIP39
├── api/                     # HTTP RPC (axum)
└── bin/                     # auxiliary binaries (zokahd, ceremony, ...)

docs/                        # canonical docs (see docs/README.md)
scripts/                     # release tooling and ceremony scripts
tests/                       # integration tests (see tests/README.md)
fuzz/                        # cargo-fuzz targets
```

## Status snapshot

Updated periodically from [`docs/project-state.md`](docs/project-state.md):

- All 8 mainnet roadmap phases have a code-side answer:
  - Phases 1-4: code closed.
  - Phase 5 (audit) and Phase 6 (testnet) require external actors.
  - Phase 7 (release): tooling closed; remaining steps need MSVC toolchain
    + GPG keyholders.
  - Phase 8 (operations): runbooks closed; execution needs operators.
- ~600 unit tests + integration tests pass.
- Canonical mainnet artifacts ([`docs/genesis.mainnet.v1.json`](docs/genesis.mainnet.v1.json),
  [`docs/mainnet-network.v1.json`](docs/mainnet-network.v1.json)) have
  regression gates in the test suite that fail loudly on drift.

For the precise list of what an agent can still build vs what requires
humans, see [`docs/agent-vs-human-roadmap.md`](docs/agent-vs-human-roadmap.md).

## Legacy

The original `ROADMAP.md` (5-phase Monero privacy plan) is superseded by
`docs/mainnet_launch_roadmap_v6.md`. Likewise the older
`docs/mainnet_operational_roadmap_v5.md` is kept for reference only.
