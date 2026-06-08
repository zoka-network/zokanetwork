# Zero-Knowledge Proofs

Zero-knowledge proofs let one party prove that a statement is valid without
revealing the private data behind the statement.

In ZOKA, proof infrastructure supports private transfer validation. The network
can validate the required rules while the public explorer does not learn the
private sender, receiver, or amount.

## Why ZOKA Uses ZK

- Preserve private wallet data.
- Keep transactions verifiable.
- Prevent invalid spends.
- Support double-spend prevention through nullifiers.

## Important Boundary

Zero-knowledge proofs protect transaction data. They do not hide every network
metadata signal. Timing, IP-level behavior, public RPC usage, and address reuse
can still matter operationally.

