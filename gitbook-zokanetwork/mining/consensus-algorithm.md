# Consensus Algorithm

ZOKA's public network uses RandomX proof-of-work.

The canonical algorithm ID in the codebase is:

```text
zoka-randomx-v1
```

Nodes validate block structure, proof-of-work, reward amount, private
transaction commitments, nullifiers, and other consensus rules before accepting
blocks.

## What Consensus Checks

Consensus validation is not only a hash check. A node must also protect the
ledger rules around:

- Chain ID and network configuration.
- Block reward and emission cap.
- Proof-of-work algorithm identity.
- Cumulative work and fork choice.
- Private transaction hash commitments.
- Nullifier uniqueness for double-spend prevention.
- Private fee claim structure where applicable.

Development or CI fallback mining modes are not the public-network consensus
backend. Public network builds should use the RandomX feature path.
