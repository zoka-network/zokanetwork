# Experimental Quantic primitives

This crate is research infrastructure for the next ZOKA protocol. It does not
activate post-quantum privacy on Zenith, define addresses, migrate wallets or
change consensus.

Node integration (2026-10-11): the root node depends on this crate as an
optional dependency behind the `pq-experimental` feature, off by default, and
re-exports it as `zoka::pq`. No consensus, wallet or address path uses it, and
the mobile engine (`default-features = false`) does not compile it. The former
gate was resolved by moving the node from iroh-base 1.0.2 to 1.0.3, which only
widens its version range so curve25519-dalek resolves to the stable 5.0.0 that
X-Wing needs through x25519-dalek 3. Between 5.0.0-rc.0 and 5.0.0 the only
changes are trait implementations behind the `group`/`lizard` features and the
AVX-512 cfg flag, none of them enabled in the node. The zeroize requirement is
`^1.8.2`; the node resolves 1.9.0 because iroh-base requires it.

Pinned RustCrypto implementations: ML-KEM 0.3.2 (ML-KEM-768), ML-DSA 0.1.1
(ML-DSA-65), and X-Wing 0.1.1. They have not been independently audited for this
project. The crate requires Rust 1.85 or later. Its own Cargo.lock is committed
for standalone device builds; root builds use the root lockfile. Both lockfiles
resolve every package of this crate's dependency tree to the same version, so
the standalone tests exercise exactly what the node links. Keep them aligned
when either lockfile changes (compare `cargo tree -e normal` of both).

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
# Inside the node build (root of the repository):
cargo test --features pq-experimental --test pq_experimental
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
