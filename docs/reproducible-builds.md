# Reproducible Build Notes

The goal is for operators and auditors to rebuild release binaries from the
published source tag and compare them with official release artifacts.

## Source Tag

```sh
git clone https://github.com/zokachain/zoka-network.git
cd zoka-network
git fetch --tags
git checkout testnet-v0.1.0
```

## Build

```sh
cargo build --release --features randomx --bin zoka --bin zokahd
```

## Package

Windows packages are ZIP archives. Linux packages are tarballs. Each release
package should include:

- `zoka` or `zoka.exe`
- `zokahd` or `zokahd.exe`
- network artifacts for the target release
- `RELEASE-MANIFEST.txt`
- checksums in `SHA256SUMS.txt`

## Known Caveats

Exact byte-for-byte reproducibility can vary by platform, compiler version,
native RandomX toolchain, and archive metadata. When hashes differ, compare:

- source tag
- `Cargo.lock`
- Rust version
- RandomX native toolchain
- target triple
- package script version

Release notes should document any known non-reproducible artifact.
