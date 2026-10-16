# zoka-quantic

Shielded protocol of Zoka Quantic: the Ironwood circuit (corrected Orchard,
Halo 2, no trusted setup) through the `orchard` crate 0.16, **unmodified**, with
Orchard's elliptic-curve note encryption replaced by X-Wing (X25519 +
ML-KEM-768, from `zoka-pq`). Design and rationale (Spanish):
[docs/ETAPA5-diseno-quantic.md](../../docs/ETAPA5-diseno-quantic.md).

Not used by Zenith, the node or the apps yet. Not independently reviewed.

| Module | What it does |
|---|---|
| `keys` | Wallet keys from the phrase entropy: ZIP 32 Orchard key + X-Wing key |
| `address` | `zq1…` addresses: Orchard receiver + X-Wing key, F4Jumble, Base58, checksum |
| `note_encryption` | X-Wing note envelope, compact and outgoing decryption, public outputs |
| `tx` | Wire format, txid tree (ZIP 244 style) and authorizing digest |
| `builder` | Transfers and coinbases through the `orchard` PCZT roles |
| `verify` | Batch verification of proofs and signatures |
| `state` | Reference consensus: emission, coinbase maturity, anchors, nullifiers, premine |
| `wallet` | Reference wallet: scanning, witnesses, note selection |

```powershell
cargo test --manifest-path crates/zoka-quantic/Cargo.toml
cargo clippy --manifest-path crates/zoka-quantic/Cargo.toml --all-targets -- -D warnings
```

Dependencies are compiled with optimizations even in debug builds
(`[profile.dev.package."*"]`): Halo 2 proofs take minutes otherwise. The
end-to-end tests build real proofs and take about three minutes.
