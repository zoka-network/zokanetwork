# Consensus and Network Security

## Double-Spend Prevention

Private notes produce nullifiers. A nullifier can be public enough to prevent
reuse while not revealing which specific output was spent.

## Proof-of-Work Security

RandomX proof-of-work makes block production costly and supports open mining
participation.

Nodes also gate mining against local sync state so an operator does not
accidentally mine from a stale local chain view. That protects operators from
wasting work and helps keep new blocks aligned with the public network.

## Private Transaction Validation

Private transaction validation must reject reused nullifiers, invalid proofs,
invalid range/accounting constraints, and malformed private transaction data.
This protects the private ledger without turning private transfers into public
sender/receiver records.

## Network Attacks

Operators should consider:

- **Sybil attacks**: many fake peers trying to influence peer view.
- **Eclipse attacks**: isolating a node from honest peers.
- **Spam**: flooding mempool or RPC surfaces.
- **DoS**: exhausting node, RPC, or explorer resources.

## Mitigations

Use multiple peers, monitor RPC health, keep nodes updated, avoid exposing
secrets, and separate public RPC from wallet custody.

## RPC Exposure

Public RPC endpoints should be treated as infrastructure, not wallets. They can
serve blocks, chain height, mempool status, transaction records, and private
transaction bodies needed for wallet recovery. They should not receive recovery
phrases, scan keys, spend keys, or wallet passwords.
