# Zoka 2.1.1 (Zenith testnet) — release notes

Small update over 2.1.0. Same activation heights (30,284 and 31,004), same P2P
protocol, same minimum client (2.1.0): 2.1.0 nodes and 2.1.1 nodes follow the
same chain.

## Change

- **The checkpoint authority mines without peers.** Nodes wait for at least one
  connected peer before mining, so that an isolated node does not mine a branch
  of its own. The team's node, which signs the checkpoints every other node
  follows, now skips that wait: its chain is the reference, and without this a
  network where only the team's node is online (Zenith on October 13, 2026: no
  other seed answered) never advances. It still signs checkpoints only when it
  has peers, so blocks it mines alone are not pinned until the network sees them.
  Every other node keeps requiring peers.

Repositories and tags: node `zoka-network/zokanetwork` `v2.1.1`; desktop
`zoka-network/zsilent-core` `v2.1.1`. ZKAPriv stays at 2.1.0 (its engine did not
change).
