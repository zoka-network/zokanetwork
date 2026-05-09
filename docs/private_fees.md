# Private Fees v1

## Current State

- `PrivateTransaction` carries `fee_commitment` and `fee_range_proof` for non-zero private fees.
- `tx-send` accepts an explicit fee or computes an estimated fee by priority/load.
- `Block.private_fee_claims` commits encrypted fee-opening envelopes in the block Merkle root.
- `validate_pow_block` rejects malformed fee claims, duplicate fee claims, duplicate private tx hashes and claims for private txs not included in the same block.
- `PrivateWalletManager::claim_private_fees_from_block_claims` lets the producer decrypt block claims and verify each opening against the tx `fee_commitment` before crediting its wallet.

## Model

Every private tx fee remains hidden from observers:

- The public block commits only the private tx hash and encrypted fee claim envelope.
- The fee amount is not added to the visible coinbase amount.
- The producer can decrypt the envelope only with its wallet key.
- Wallet-side claim verification recomputes `fee_commitment = fee * G + r_fee * H` before crediting funds.

## Block Rules

For each `PrivateFeeClaim` in `Block.private_fee_claims`:

- `tx_hash` must be non-empty.
- `ephemeral_pubkey` and `ciphertext` must be non-empty.
- `tx_hash` must appear in `Block.private_tx_hashes`.
- only one claim is allowed per private tx hash.

These rules make fee-claim metadata consensus-visible without revealing the fee amount.

## Related Reward Path

- Block subsidies use `Block.private_coinbase` rather than only local wallet accounting.
- The subsidy amount remains public in `block.transactions[0]` so emission and max supply stay auditable.
- The spendable miner reward is a private output committed by the block Merkle root.
- The miner wallet claims it after coinbase maturity, verifying amount commitment and range proof before crediting funds.
