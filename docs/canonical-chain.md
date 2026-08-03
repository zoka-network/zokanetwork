# Canonical chain anchors

Every node enforces these checkpoints. A node whose block at one of these
heights does not match the hash below is on a different chain, and its blocks
are rejected — it is **not** part of the ZOKA network.

Such a branch is a *side chain*: typically a fork built by a node that was
mining in isolation, disconnected from the mesh, and drifted onto its own
history. The network follows the chain containing these anchors; everything
else is discarded.

## Block 3127

```text
height 3127  hash b5c20cfbdc9e2f56bb143334c03f7bcc9df60fbd7c2c7ed553b61ee2dd9740df
```

Enforced from 1.8.0 onward.

This checkpoint also anchors the serverless difficulty reset: from **block
3128** onward, difficulty is capped to a sane target. The hashrate collapse had
pushed it far beyond what the remaining miners could meet, which would have
stalled the chain. Both rules apply only on the chain carrying the 3127 anchor.

## Block 10887

```text
height 10887  hash d30c20100f6405424d32e5c204799bc10a1197ff5cdc1fe0b04c2c538b11086e
```

Added by the 1.9.0 recovery to resolve the fork observed around that height.

A node on a side branch stops mining and converges back to the canonical chain
by itself. **Databases are never deleted and wallet balances are preserved.**

Every node must run 1.9.0 or later to stay on the network: the recovery protocol
does not talk to older clients. From 2.0 the network manifest requires 2.0.0.

## Core properties

- User transfers are private by default.
- Wallet files are encrypted and stored per network.
- RandomX (`zoka-randomx-v1`) is the proof-of-work backend.
- ZKA uses 6 decimal places: `1 ZKA = 1,000,000 atoms`.
- Coinbase maturity is 60 blocks.
