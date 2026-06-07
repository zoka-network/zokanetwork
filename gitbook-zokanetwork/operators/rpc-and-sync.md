# RPC and Sync

ZOKA wallets, explorers, and nodes can use public RPC endpoints for read-only
chain data and bootstrap sync.

## Useful CLI Commands

List configured RPC endpoints:

```sh
zoka rpc-list --network mainnet
```

Check RPC height:

```sh
zoka rpc-height --network mainnet
```

Bootstrap or repair a local mainnet database from configured public RPC:

```sh
zoka rpc-sync --network mainnet --limit 512
```

## Privacy Rule

Public RPC must not receive wallet seed phrases, spend keys, or scan keys.
Wallet-side private transaction construction keeps secrets local and submits
only the transaction data needed by the network.

