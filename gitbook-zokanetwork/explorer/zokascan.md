# ZokaScan Explorer

ZokaScan is the public block explorer for ZOKA Network:

[https://www.zokascan.cc](https://www.zokascan.cc)

## Search Modes

The explorer can route searches for:

- Block height.
- Block hash.
- Public transaction hash.
- Private transaction hash when anchored in blocks.
- Commitments, nullifiers, roots, and privacy-safe private records.
- `zka1...` receive addresses, without exposing public address history.
- Scan-key based mining reward checks performed client-side.

## Privacy Boundary

ZokaScan does **not** reveal:

- Wallet balances.
- Address histories.
- Senders.
- Receivers.
- Transfer amounts.
- Recovery phrases.
- Scan keys.
- Spend keys.

For scan-key based mining reward checks, the browser derives the relevant
private address locally and compares public coinbase commitments. The scan key
is not sent to the server.

## What It Shows

- Chain height and public status metrics.
- Recent blocks.
- Block timestamps, hashes, difficulty, nonce, and transaction counts.
- Public transaction hashes.
- Private transaction hashes anchored in blocks.
- Privacy-safe record pointers.

