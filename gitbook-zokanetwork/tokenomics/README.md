# Tokenomics

ZOKA's native asset is **ZKA**. The current economic constants are defined in
`src/economics.rs` in the core repository.

| Field | Value |
|---|---|
| Name | Zoka |
| Ticker | ZKA |
| Decimals | 6 |
| Atomic unit | 1 ZKA = 1,000,000 atoms |
| Maximum supply | 23,000,000 ZKA |
| Initial block reward | 12 ZKA |
| Halving interval | 1,051,200 blocks |
| Target block time | 120 seconds |

## Economic Boundaries

The protocol tracks user-facing ZKA amounts and internal atomic units
separately. Wallets and CLI commands convert amounts at the boundary so wallet
balances, fees, and block rewards do not rely on floating-point math.

Block subsidy is clipped by the remaining hard cap. A block cannot emit more
subsidy than the remaining unissued supply.

Long-term miner incentives shift from block subsidy toward transaction fees as
halvings reduce the reward. Fee handling is privacy-aware: private transaction
fee claims are represented through protocol structures rather than a simple
public transparent transfer.
