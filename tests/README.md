# ZOKA Test Layout

ZOKA follows standard Rust test layout:

- Unit tests live next to the modules they validate.
- Integration tests live in this `tests/` directory.
- CLI tests for `zoka` live in `src/main.rs` under `#[cfg(test)]`.

Keeping module tests next to private functions is intentional. Moving every test
to `tests/` would make many private invariants harder to validate.

## Common Commands

```sh
cargo check --bins
cargo test --bin zoka
cargo test --lib wallet::storage::tests
cargo test --tests
```

Run one integration target:

```sh
cargo test --test network_integration_test
cargo test --test pbft_transport_integration_test
```

Run with RandomX enabled:

```sh
cargo test --features randomx
cargo build --release --features randomx --bin zoka --bin zokahd
```

## Generated Test Artifacts

Some tests create local RocksDB or wallet fixtures under ignored directories
such as `.zoka/` or `.test_*`. These are runtime artifacts and should not be
committed.

## Fuzz Targets

The `fuzz/` directory contains cargo-fuzz targets for proof verification,
private transaction deserialization, range proof verification, and directory
digest verification.
