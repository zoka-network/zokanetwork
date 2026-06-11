# Mining

ZOKA uses proof-of-work mining. Public network binaries use RandomX through the
`randomx` feature.

Mining is open participation: any node operator can run a miner and route
rewards directly to a `zka1...` wallet address.

## Current Mainnet Config

The mainnet network configuration marks mining as:

```text
algorithm: randomx
open_participation: true
reward_routing: direct-to-miner
```

## Operator Priorities

Before mining, an operator should confirm:

- The binary was built with the public RandomX backend.
- The local chain is synced with the public network.
- The reward address is a full `zka1...` wallet address.
- The node has peer connectivity.
- Wallet recovery material is backed up before rewards are earned.

ZSilent Core is the preferred desktop interface for users who want node,
wallet, miner, logs, and metrics in one application.
