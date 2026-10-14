# Zoka 2.1.2 (Zenith testnet) — release notes

**This release moves the Zenith hard fork earlier.** 2.1.0 had scheduled it a
week ahead so that other nodes had time to update. When it was released, the
team's node was the only one online, and at the current difficulty that week
had turned into about eleven days. 2.1.2 activates the same features within
hours:

| Height | What activates | Was (2.1.0 / 2.1.1) |
|---|---|---|
| **25,570** | Canonical private bodies: every block carries the bodies of its private transactions and every node validates them. | 30,284 |
| **25,630** | Hybrid post-quantum addresses (`zkq1`, testing only): v2 outputs encrypted with X-Wing (X25519 + ML-KEM-768). | 31,004 |

**Do not use 2.1.0 or 2.1.1 anymore:** they keep the old heights and would end
up on a different chain. Minimum client version is now 2.1.2.

Everything else is the same as 2.1.1: signed checkpoints from the team's node,
the authority mines without peers, the wallet fix for inflated balances, Tor
0.4.9.14 in ZSilent, P2P Exchange removed. See the
[2.1.0 notes](https://github.com/zoka-network/zokanetwork/releases/tag/v2.1.0)
for the full list and known limitations.

Repositories and tags: node `zoka-network/zokanetwork` `v2.1.2`; desktop
`zoka-network/zsilent-core` `v2.1.2`; Android `zoka-network/zkapriv` `v2.1.2`.
