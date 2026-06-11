# Coin Information

## Name and Ticker

```text
Name: Zoka
Ticker: ZKA
```

## Decimals

ZKA uses 6 decimal places:

```text
1 ZKA = 1,000,000 atoms
0.000001 ZKA = 1 atom
```

Applications should reject amounts with more than 6 decimal places. Internally,
amounts are represented as integer atoms, which avoids floating-point rounding
inside wallet, fee, and reward logic.

## Chain IDs

The current code separates public network data by chain ID:

```text
Mainnet: zoka-mainnet-v1
Testnet: zoka-testnet-v1
```

Wallet files, chain databases, and network configuration should not be mixed
between networks. A wallet interface may show the same ticker, but the active
network context determines which chain data and RPC endpoints are used.
