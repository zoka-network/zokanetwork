# RPC API

The node exposes public read endpoints and selected write endpoints.

## Common Read Endpoints

```text
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

## CLI Helpers

```sh
zoka rpc-list --network mainnet
zoka rpc-status --network mainnet
zoka rpc-height --network mainnet
zoka rpc-sync --network mainnet --limit 512
```

