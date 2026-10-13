# Zoka 2.1.0 (Zenith testnet) — release notes

Zenith, the network running today, is now the **Zoka testnet**. Its ZKA are test
coins with no value and will not carry over to Zoka Quantic, the future main
network.

**This release schedules a hard fork on Zenith at block 30,284.** From that
height, nodes older than 2.1.0 follow a different chain. Update before then.

Repositories and tags:

- Node: `zoka-network/zokanetwork`, tag `v2.1.0`
- Desktop: `zoka-network/zsilent-core`, tag `v2.1.0` (embeds node 2.1.0 and Tor 0.4.9.14)
- Android: `zoka-network/zkapriv`, tag `v2.1.0` (ZKAPriv now follows the network version)

## Activation heights

| Height | What activates |
|---|---|
| 30,284 | **Canonical private bodies.** Every block carries the bodies of its private transactions and every node validates them. Before this height, blocks only committed hashes and nullifiers. |
| 31,004 | **Hybrid post-quantum addresses (`zkq1`, testing only).** v2 outputs encrypted with X-Wing (X25519 + ML-KEM-768) are mined only from this height. Wallets already show the `zkq1` address with its activation block; until then, keep using `zka1`. |

At the difficulty floor a single miner produces roughly two blocks per minute,
so the activation date depends on who is mining.

## Signed checkpoints from the team

The team's node signs a checkpoint every 30 blocks ("at height H the block is
X"), and 2.1.0 nodes refuse any branch that contradicts the latest one. This
prevents forks and long reorganizations. It is **explicit trust in the team**:
the key decides which valid chain is canonical, but it cannot make an invalid
block valid, create coins outside the emission schedule or spend anyone's funds.
Tested end to end on a local network: a node with a heavier conflicting branch
stops mining and switches to the signed branch. Zoka Quantic will use the same
mechanism with its own key.

## Fixes and changes

- **Wallet: inflated balance and sends that silently failed.** Several code
  paths saved a stale copy of the wallet and erased the reservation of a send
  made in between (the most frequent: the end of each mining batch). The notes
  looked spendable again and the next send reused them. Wallets are now always
  re-read under the lock before saving.
- **Tor 0.4.9.14** in ZSilent (high-severity security release affecting onion
  services). Signature and SHA-256 verified.
- **The P2P Exchange was removed** from the node, ZSilent and the docs.
- `fullnode --sync-only` starts again (it failed with an empty wallet address).
- Minimum client version is now 2.1.0 everywhere (node, network manifest,
  ZSilent).
- The miner fee claim travels in a hybrid envelope once v2 is active.

## Not changed

- The P2P protocol id stays `/zoka/1.9.0-mainnet-recovery`: 2.0.x nodes keep
  connecting until the activation height, so the network does not split on the
  day of the release.

## Known limitations

- **On Zenith the spend graph is public:** private transactions publish the
  commitment and one-time key of the notes they spend, so a spend can be linked
  to the output it consumes. Amounts and recipients stay hidden. Zoka Quantic
  replaces this with the Ironwood construction; see
  `docs/ETAPA5-gasto-sin-key-images.md`.
- `zkq1` protects the encryption of v2 outputs against a future quantum
  computer; it does not hide which note a transaction spends.
- The post-quantum code is experimental and has not been externally reviewed.

## Verification

SHA-256 of every artifact is listed in each release and in `SHA256SUMS.txt`.
Trusted setup digest: `219fdb5d5e47e115ca899d13a6de284df03c86eaaef05eb4ff4adb395cfb0d8e`
(release `trusted-setup-mainnet-v1`).
