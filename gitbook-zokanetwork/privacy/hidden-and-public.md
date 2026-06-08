# What Is Hidden and Public

## Hidden

For private transactions, the public explorer does not reveal:

- Sender.
- Receiver.
- Amount.
- Wallet balance.
- Full address history.
- Seed phrase.
- Scan key.
- Spend key.

## Public

The chain must still expose consensus data:

- Block height.
- Block hash.
- Timestamp.
- Proof-of-work metadata.
- Network status.
- Supply and emission metrics where exposed by node APIs.
- Commitments, nullifiers, roots, and transaction hashes needed for
  verification.

## Practical Rule

Use wallets for private history. Use the explorer for public chain state.

