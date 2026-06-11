# Mainnet Zenith

**Mainnet Zenith** is the current public ZOKA Network deployment.

The network is designed for self-custodial private payments. Wallets submit and
recover private activity through public chain data, while the explorer remains a
privacy-safe view of consensus metadata.

## Live Components

| Component | Status |
|---|---|
| Core chain and node software | Live |
| Public bootstrap and RPC fallback | Live |
| Permissionless mining | Live |
| ZokaScan public explorer | Live |
| ZSilent Core desktop wallet | Live |
| ZKAPriv Android wallet | Live |

## Network Identity

Use the public name **ZOKA Network: Mainnet Zenith** in user-facing docs,
release notes, explorer labels, and wallet network selectors.

## Public RPC Role

Public RPC endpoints are read-only chain access points for wallets, explorers,
and sync bootstrap. They are not custody services and should not receive wallet
secrets. Wallet-side private transaction construction keeps seed phrases, scan
keys, and spend keys on the user's device.

## What Users Should Know

- A `zka1...` address is a user-facing receive address.
- Wallet history and spendable balance are recovered inside ZKAPriv or ZSilent
  Core, not from the public explorer.
- The explorer can prove that private transaction hashes and block metadata are
  anchored, but it cannot reveal private participants or amounts.

