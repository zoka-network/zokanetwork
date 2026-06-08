# Privacy Model

ZOKA's privacy model separates public verification from private wallet state.

The public chain provides enough data for consensus, mining, and auditability.
The wallet keeps the user-specific view: balance, history, spend authority, and
recovery material.

## Summary

| Hidden from public explorer | Public for consensus |
|---|---|
| Sender | Block height |
| Receiver | Block hash |
| Amount | Timestamp |
| Wallet balance | Proof-of-work metadata |
| Address history | Commitments and nullifiers |
| Recovery phrase, scan key, spend key | Public transaction or private tx hashes |

