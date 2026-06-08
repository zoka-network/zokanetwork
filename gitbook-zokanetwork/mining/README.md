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

