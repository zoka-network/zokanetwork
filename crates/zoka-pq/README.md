# Experimental Quantic primitives

This crate is research infrastructure for the next ZOKA protocol. It does not
activate post-quantum privacy on Zenith, define addresses, migrate wallets or
change consensus. It builds with its own manifest and is not yet a dependency
of the root node.

Integration gate: the node lockfile pins iroh-base 1.0.0, which requires
curve25519-dalek 5.0.0-rc.0; X-Wing requires 5.0.0 through x25519-dalek 3.
Cargo cannot currently resolve both in that dependency graph. Upgrading the
network dependencies and validating compatibility is separate required work;
the isolated crate does not replace the node's working dependency graph.

Pinned RustCrypto implementations: ML-KEM 0.3.2 (ML-KEM-768), ML-DSA 0.1.1
(ML-DSA-65), and X-Wing 0.1.1. They have not been independently audited for this
project. The crate requires Rust 1.85 or later. Its own Cargo.lock is committed
for standalone device builds; root builds use the root lockfile.

## API boundaries

- Keys and ciphertexts require exact lengths; ML-KEM public keys additionally
  undergo canonical modulus checks. ML-DSA signatures are decoded and verified.
- Secret keys, exported `SecretSeed` and shared secrets have redacted Debug
  formatting and zeroization enabled. Explicit copies exposed by the caller
  remain the caller's responsibility; this is not a guarantee against all memory
  copies, swap or process compromise.
- ML-KEM ciphertext corruption uses implicit rejection: decapsulation yields a
  different secret, not an authentication error. Applications need authenticated
  encryption and a context-bound key derivation function.
- X-Wing follows the crate's draft-06 combiner and official published vectors.
  The experimental profile rejects non-contributory **ciphertexts** using the
  upstream `DecapsulationKeyRejectNonContrib` variant. Public-key parsing retains
  RFC 7748 decoding and does not prove key possession or reject every low-order
  public key. This policy is explicit and is not a finalized network protocol.
- Signing is pure deterministic ML-DSA with an explicit 0–255 byte context, not
  HashML-DSA. Snapshot hybrid signing/verification is not implemented here.
- Generation returns OS RNG failures; encapsulation follows the upstream API
  and can panic if the OS randomness source fails. Do not treat these wrappers
  as a hardened remotely callable service.

## Validation

```powershell
cargo test --manifest-path crates/zoka-pq/Cargo.toml --locked
cargo clippy --manifest-path crates/zoka-pq/Cargo.toml --locked --all-targets -- -D warnings
cargo run --manifest-path crates/zoka-pq/Cargo.toml --locked --release --bin pq-bench -- --iterations 100
./scripts/check-pq-platforms.ps1 -RunAndroidBench -Iterations 100 -AndroidRuns 3
```

Fixtures in `tests/vectors` are selected NIST ACVP and official X-Wing vectors.
Each JSON records its pinned source, source digest and selection. They are public
test data, not live secrets. See `scripts/` for fixture extraction. These tests
cover known answers, corruption, wrong keys, size checks, context binding and
redacted secret formatting. They are not exhaustive cryptanalysis.

The benchmark emits JSON with per-operation mean, median and p95 in microseconds.
ML-KEM reuses an expanded key; X-Wing includes expansion per decapsulation.
Expansion and signing are measured separately. Five warmup calls precede each
measurement. It runs only synthetic data and reads no wallet or chain files.
For platform results and limits see `../../docs/RESULTADOS-postcuantica-etapa1.md`.

## Protocol work still required

Hybrid address/payload versioning, domain-separated derivation, mobile scanning,
activation by height, a spend circuit that hides inputs while proving ownership
and unique nullifiers, a new ceremony, and external review are separate work.
The current Groth16 circuit cannot safely replace CLSAG merely by removing fields.
See `../../docs/REVISION-postcuantica-etapa1.md`.
