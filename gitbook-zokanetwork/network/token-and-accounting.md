# Token and Accounting

The native asset of ZOKA Network is **ZKA**.

## Decimal Precision

ZKA uses atom accounting:

```text
1 ZKA = 1,000,000 atoms
```

Wallet and CLI amounts accept up to 6 decimal places. The smallest unit is:

```text
0.000001 ZKA = 1 atom
```

## Coinbase Maturity

Coinbase rewards mature after 60 blocks before they can be spent.

## Privacy Boundary

Public consensus data necessarily includes block headers, proof-of-work,
coinbase structure, commitments, nullifiers, roots, and transaction hashes. A
private transfer's sender, receiver, and amount are not exposed as transparent
account data.

