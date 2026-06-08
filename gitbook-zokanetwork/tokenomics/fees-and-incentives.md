# Fees and Incentives

## Miner Rewards

Miners earn block rewards and can claim private transaction fees through the
protocol's private fee claim path.

## Transaction Fees

Private transaction fees are handled in atomic units. Wallets expose
user-facing ZKA amounts and convert them into atoms.

The codebase includes private fee commitments and fee claim handling so fees do
not need to become a simple transparent amount field in private transaction
records.

Wallets may estimate or accept an explicit fee depending on the flow. The
important rule is that fees are part of the private transaction accounting and
must be validated with the rest of the transaction. A nonzero fee should not
become a shortcut that leaks sender, receiver, or transfer amount through a
public explorer view.

## Fee Claims

Blocks can include private fee claim data for private transactions. This lets a
block producer claim fees while keeping private transaction records aligned with
the privacy model.

## Long-Term Incentives

The emission cap creates scarcity. As subsidy decreases through halvings,
transaction fees become increasingly important for miner incentives and network
security.

Because the network is still young, fee policy should be documented from the
active implementation and reviewed before presenting fixed public minimums or
market assumptions.
