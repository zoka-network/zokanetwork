# Privacy Model

ZOKA's user-facing transfer path is private by default.

## Wallet-Side Privacy

Wallets scan public chain data and decrypt only outputs that belong to the
wallet. This lets a wallet recover balance and history without a public server
learning which outputs belong to the user.

## Public Consensus Data

Some data must remain public for consensus and verification:

- Block headers.
- Proof-of-work.
- Coinbase structure.
- Commitments.
- Nullifiers.
- Roots.
- Transaction hashes.

## Hidden Transfer Data

Private transfers do not expose the sender, receiver, or amount as transparent
account activity. Nullifiers prevent double spends without revealing which
output was spent.

