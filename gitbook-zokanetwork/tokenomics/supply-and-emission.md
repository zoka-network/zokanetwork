# Supply and Emission

## Maximum Supply

The maximum supply is:

```text
23,000,000 ZKA
```

## Initial Block Reward

The initial block subsidy is:

```text
12 ZKA per block
```

## Halving Schedule

The subsidy halves every:

```text
1,051,200 blocks
```

With a 120-second target block time, this is roughly a 4-year interval.

The reward schedule is integer-based:

| Era | Reward |
|---|---|
| 0 | 12 ZKA |
| 1 | 6 ZKA |
| 2 | 3 ZKA |
| 3 | 1 ZKA |
| 4+ | 0 ZKA |

## Cap Enforcement

Block reward calculation clips the reward to the remaining supply cap. Once the
maximum supply has been emitted, the subsidy becomes zero and miner incentives
depend on fees.

## Release Discipline

Emission constants are consensus-critical. Any release that changes supply,
reward, halving, block time, or maturity behavior needs explicit release notes,
test coverage, and operator coordination. Documentation should follow the
active release branch rather than old screenshots or informal messages.
