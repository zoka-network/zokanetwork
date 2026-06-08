# Privacy Model

ZOKA's privacy model separates public verification from private wallet state.

The public chain provides enough data for consensus, mining, and auditability.
The wallet keeps the user-specific view: balance, history, spend authority, and
recovery material.

This means the chain can remain publicly verifiable without becoming a public
account ledger for every user. Nodes can reject invalid blocks and double
spends, while wallets decide which encrypted outputs belong to the local user.

## Summary

| Hidden from public explorer | Public for consensus |
|---|---|
| Sender | Block height |
| Receiver | Block hash |
| Amount | Timestamp |
| Wallet balance | Proof-of-work metadata |
| Address history | Commitments and nullifiers |
| Recovery phrase, scan key, spend key | Public transaction or private tx hashes |

## Design Goal

The goal is not to hide the existence of the blockchain. Blocks, mining,
supply, and network health are public. The privacy model focuses on preventing
public infrastructure from turning private payments into searchable sender,
receiver, amount, and balance records.
