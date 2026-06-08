# FAQ

## Why Zoka?

ZOKA is built for users who want public blockchain verification without public
wallet histories and balances.

## How is Zoka different from Monero?

Both projects care about privacy, but ZOKA is its own Rust implementation with
its own wallet model, proof-of-work integration, ZK infrastructure, and app
ecosystem. Avoid assuming compatibility with Monero tooling.

## How is Zoka different from Zcash?

ZOKA uses its own chain, wallet apps, mining configuration, and protocol
implementation. It should be evaluated as an independent network rather than a
Zcash clone.

## Are balances private?

Balances are wallet-side. The public explorer does not expose private wallet
balances.

## Can anyone trace transactions?

The explorer can show public consensus data and anchored hashes. It does not
show private sender, receiver, or amount for private transfers. Network metadata
and user behavior can still matter.

## How are fees handled?

Private transaction fees are represented in the protocol through private fee
commitment and claim handling. Wallets expose user-facing ZKA amounts and
convert them to atoms.

## Is mining profitable?

Profitability depends on hardware, electricity, network difficulty, reward,
fees, and market price. The docs do not promise profitability.

## What can be seen on the explorer?

Blocks, hashes, timestamps, public chain metrics, private transaction hashes,
commitments, nullifiers, roots, and privacy-safe records. Not balances, senders,
receivers, or private amounts.

