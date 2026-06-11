# RPC API

The node exposes public read endpoints and selected write endpoints.

## Common Read Endpoints

```text
GET /health
GET /chain/height
GET /mempool/size
GET /blocks/:height
GET /blocks/:height/raw
GET /blocks/hash/:hash
GET /blocks/latest
GET /blocks/:height/transactions
GET /blocks/:height/private_tx_hashes
GET /blocks/range/:from/:to
GET /blocks/range/:from/:to/raw
GET /blocks/range/:from/:to/private_tx_hashes
GET /transactions/:hash
GET /private/tx/:hash
```

## Transaction Submission

```text
POST /private/tx
POST /transactions
```

Wallets should submit locally built transaction payloads. Public RPC endpoints
must not receive wallet seed phrases, scan keys, or spend keys.

## Explorer Use

The public explorer can use read endpoints for block height, latest blocks,
block details, transaction hashes, mempool size, private transaction hashes,
and private transaction body availability. It should not turn wallet scan keys
or addresses into public account-history pages.

## Wallet Use

Wallets may read block and private transaction data to reconstruct local wallet
state. Ownership detection happens in the wallet with local account material,
not by asking an RPC server to reveal a user's balance.

## CLI Helpers

```sh
zoka rpc-list --network mainnet
zoka rpc-status --network mainnet
zoka rpc-height --network mainnet
zoka rpc-sync --network mainnet --limit 512
```
