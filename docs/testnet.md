# ZOKA Testnet

ZOKA Testnet v0.1.0 is the first public testnet release line for the ZOKA
network software. It is intended for wallet testing, node operation, mining
rehearsal, private transfer testing, fee testing, and sync validation.

Testnet coins have no economic value.

## Network Identity

| Field | Value |
|---|---|
| Chain ID | `zoka-testnet-v1` |
| Release tag | `testnet-v0.1.0` |
| Protocol freeze ID | `/zoka/1.0.0` |
| PoW algorithm | `zoka-randomx-v1` |
| Token | `ZKA` |
| Decimals | `6` |
| Coinbase maturity | `60` blocks |
| Default DB | `.zoka/testnet/blockchain.db` |
| Default wallets | `.zoka/testnet/private_wallets/` |

## Files

| File | Purpose |
|---|---|
| `docs/genesis.testnet.v1.json` | Testnet genesis |
| `docs/testnet-network.v1.json` | Public seed/RPC configuration |
| `docs/economics.testnet.v1.json` | Testnet economics |

The first public VPS seed node is added by updating
`docs/testnet-network.v1.json` with its multiaddr after deployment.

## Build From Source

```sh
git clone https://github.com/zoka-network/zokanetwork.git
cd zoka-network
git checkout testnet-v0.1.0
cargo build --release --features randomx --bin zoka --bin zokahd
```

## Run A Testnet Node

```sh
target/release/zokahd --network testnet --password "<wallet-password>"
```

On Windows:

```powershell
target\release\zokahd.exe --network testnet --password "<wallet-password>" --inline
```

Run without mining:

```sh
zokahd --network testnet --no-mine --password "<wallet-password>"
```

Start from a specific seed:

```sh
zokahd --network testnet --seed <multiaddr> --password "<wallet-password>"
```

## Wallet Flow

Create a wallet:

```sh
zoka create testnet-wallet --network testnet --password "<wallet-password>"
```

List wallets:

```sh
zoka wallets --network testnet
```

Show balance:

```sh
zoka balance testnet-wallet --network testnet --password "<wallet-password>"
```

Send a private transfer:

```sh
zoka send <sender-zka1-address> <zka1-recipient-address> 0.000001 --network testnet --password "<wallet-password>" --fee 0.0001 --yes
```

## Mining Notes

- Public testnet binaries should be built with `--features randomx`.
- Coinbase rewards mature after 60 blocks.
- Private fees are claimed by the block producer through private fee claim data.
- The unsafe rehearsal difficulty flag is for local testing only and must not be
  used for the public testnet.

## Operator Expectations

- Keep the node clock synchronized.
- Keep wallet recovery material offline.
- Back up encrypted wallet files and seed phrases separately.
- Publish seed multiaddrs only after validating that the node restarts cleanly.
- Report crashes, consensus errors, wallet failures, and sync stalls with logs
  that do not include wallet secrets.
