# ZOKA Genesis Schema Notes

ZOKA genesis files use the `zoka-genesis-v1` schema. The same schema shape is
used by the testnet and mainnet candidate artifacts.

## Required Fields

| Field | Purpose |
|---|---|
| `schema` | Must be `zoka-genesis-v1` |
| `chain_id` | Network identity, such as `zoka-testnet-v1` |
| `pow_algorithm` | Expected PoW backend |
| `genesis_timestamp` | Unix timestamp |
| `economics` | Supply, reward, halving, target time, maturity |
| `validators` | Optional validator set |
| `pre_mine` | Optional pre-mine entries |
| `initial_accounts` | Optional bookkeeping accounts |

## Validation

The CLI validates genesis files with:

```sh
zoka genesis-verify docs/genesis.testnet.v1.json
zoka genesis-verify docs/genesis.mainnet.v1.json
```

Validation checks schema, chain ID, PoW algorithm, economic drift, duplicate
entries, malformed validator keys, and deterministic digest output.

## Operator Rule

Always verify the genesis file from the same release tag as the binary before
starting a node. A node should not be pointed at a genesis file copied from a
different release line unless the operator intentionally knows why.
