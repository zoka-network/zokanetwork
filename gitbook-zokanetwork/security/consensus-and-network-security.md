# Consensus and Network Security

## Double-Spend Prevention

Private notes produce nullifiers. A nullifier can be public enough to prevent
reuse while not revealing which specific output was spent.

## Proof-of-Work Security

RandomX proof-of-work makes block production costly and supports open mining
participation.

## Network Attacks

Operators should consider:

- **Sybil attacks**: many fake peers trying to influence peer view.
- **Eclipse attacks**: isolating a node from honest peers.
- **Spam**: flooding mempool or RPC surfaces.
- **DoS**: exhausting node, RPC, or explorer resources.

## Mitigations

Use multiple peers, monitor RPC health, keep nodes updated, avoid exposing
secrets, and separate public RPC from wallet custody.

