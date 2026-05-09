# RandomX PoW Decision

ZOKA public network binaries use RandomX as the canonical proof-of-work backend:

```text
zoka-randomx-v1
```

## Rationale

ZOKA targets ordinary users and independent operators. RandomX is designed to be
CPU-oriented and to reduce the advantage of specialized hardware compared with
simple hash functions.

The CPU fallback backend exists only for local development and fast CI tests. It
is not the public-network PoW backend.

## Build Requirement

Public testnet and mainnet candidate binaries should be built with:

```sh
cargo build --release --features randomx --bin zoka --bin zokahd
```

If a chain requires RandomX and the binary was built without it, the node should
fail clearly instead of silently mining or validating with the wrong backend.

## Consensus Rule

The expected PoW algorithm is selected by chain ID in `src/economics.rs`.
Changing the PoW algorithm for a live public network requires a coordinated
protocol upgrade.
