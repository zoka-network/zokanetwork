# ZOKA Network

ZOKA Network is a privacy-first blockchain where value moves through encrypted
wallet flows by default. The live network is **ZOKA Network: Mainnet Zenith**.

The ecosystem currently includes:

- **Zoka Core**: Rust full node, CLI wallet, private transaction handling,
  RandomX proof-of-work, RPC, and chain state.
- **ZSilent Core**: desktop wallet and node control center for Windows and
  Linux.
- **ZKAPriv**: Android self-custodial wallet for private send, receive,
  restore, balance scanning, and recent activity.
- **ZokaScan**: public block explorer at [zokascan.cc](https://www.zokascan.cc)
  that surfaces public chain data without exposing wallet histories, balances,
  senders, or receivers.
- **zokaweb**: public website for downloads, roadmap, and repository links.

## What Makes ZOKA Different

ZOKA is designed around wallet-side privacy. A public explorer can show blocks,
hashes, timestamps, public chain metrics, commitments, nullifiers, and anchored
private transaction hashes. It cannot show a private user's balance, sender,
receiver, amount, seed phrase, scan key, or spend key.

Wallets reconstruct their own state by scanning chain data and decrypting only
the outputs that belong to their keys.

## Current Public Surfaces

| Surface | URL |
|---|---|
| Website | [zoka.network](https://zoka.network) |
| Explorer | [zokascan.cc](https://www.zokascan.cc) |
| Core repository | [ZokaNetwork/ZokaNetwork](https://github.com/ZokaNetwork/ZokaNetwork) |
| Android wallet | [ZokaNetwork/zkapriv](https://github.com/ZokaNetwork/zkapriv) |
| Desktop wallet | [ZokaNetwork/zsilent-core](https://github.com/ZokaNetwork/zsilent-core) |
| Explorer repository | [ZokaNetwork/zokaexplorer](https://github.com/ZokaNetwork/zokaexplorer) |

## Security Baseline

Never publish wallet recovery phrases, spend keys, scan keys, signing keys,
keystore files, RPC credentials, deployment secrets, or APK signing material.
Public docs should explain privacy guarantees without encouraging users to paste
secrets into websites, chats, or issue trackers.

