# Public Surfaces Required by Consensus

This note documents what remains intentionally public in mainnet and why.

## Coinbase transaction is public

- The first transaction in a mined block is a public coinbase transaction.
- This is required for third-party auditability of emission and supply.
- Privacy-sensitive spendability still uses `private_coinbase` outputs with maturity rules.

## Supply accounting is public

- Emitted supply and cap enforcement (`23,000,000 ZKA`) are consensus-visible.
- Nodes must be able to independently verify reward schedule, halving, and cap.

## Why this does not violate privacy-first

- User transfer amounts and recipients in private flows are still protected by private transaction mechanisms.
- Public consensus surfaces are limited to what is necessary for objective chain auditability.
