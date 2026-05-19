# ZOKA User CLI Reference

The `zoka` CLI is the user-facing command surface. User flows are private by
default and wallets are stored separately for each network.

## Networks

Use `--network testnet` for the public testnet.

Default local paths:

| Network | Wallets | Chain DB |
|---|---|---|
| testnet | `.zoka/testnet/private_wallets/` | `.zoka/testnet/blockchain.db` |

## Short Commands

```sh
zoka
zoka create <label> --network testnet --password <password>
zoka restore "<recovery phrase>" --network testnet --password <password> --name <label>
zoka wallets --network testnet
zoka balance <wallet> --network testnet --password <password>
zoka send <from-zka1-address> <to-zka1-address> <amount> --network testnet --password <password> --fee <fee> --yes
zoka mine <wallet-zka1-address> --network testnet --password <password>
zoka node --network testnet --password <password>
zoka status --network testnet
```

Long command names such as `wallet-create`, `wallet-list`, `tx-send`, and
`fullnode` remain available for automation.

## Wallets

Create a wallet:

```sh
zoka create alice --network testnet --password "<password>"
```

List wallets:

```sh
zoka wallets --network testnet
```

Without a password, encrypted wallets are shown as locked. With the correct
password, they are shown as unlocked.

Restore from recovery phrase:

```sh
zoka restore "<12 words>" --network testnet --password "<new-password>" --name alice-restored
```

Back up the encrypted wallet file:

```sh
zoka wallet-backup --address <zka1...> --network testnet --out <path> --password "<password>"
```

Restore an encrypted wallet backup:

```sh
zoka wallet-restore-from-backup --in <path> --network testnet --password "<password>" --name alice-restored
```

Export recovery secrets only in an offline environment:

```sh
zoka wallet-export-secrets --address <zka1...> --network testnet --password "<password>" --out <path>
```

## Private Transfers

Send ZKA:

```sh
zoka send <sender-zka1-address> <recipient-zka1-address> 0.000001 --network testnet --password "<password>" --fee 0.0001 --yes
```

Rules:

- `--yes` is required to avoid accidental sends.
- Sender and recipient must be full wallet addresses (`zka1...`); local wallet
  names are not accepted for network transfers.
- Amounts and fees accept up to 6 decimals.
- `0.000001 ZKA` is 1 atom.
- Private fees are paid through private fee claims.
- Recipient and amount are not exposed as a transparent account transfer.

Check pending private transfers:

```sh
zoka pending --network testnet
```

## Mining

Mine with a local wallet:

```sh
zoka mine <wallet-zka1-address> --network testnet --password "<password>"
```

On mainnet, standalone mining first checks the configured public RPC height in
`docs/mainnet-network.v1.json`. If the local DB is behind the Fly bootstrap,
mining is blocked until `zokahd --network mainnet` catches up. Use
`--allow-unsynced-mining` only for isolated development rehearsals.

Repair or bootstrap a local mainnet DB from the configured public RPC:

```sh
zoka rpc-sync --network mainnet --limit 512
```

`zokahd --network mainnet` runs this RPC sync path automatically before and
during node sync, then falls back to peer sync. The command is useful when a
desktop node was offline while the Fly bootstrap kept mining blocks.

Check mining state:

```sh
zoka mine-status --network testnet
```

Coinbase rewards mature after 60 blocks.

## Full Node

Start a full testnet node:

```sh
zokahd --network testnet --password "<password>"
```

Verifier mode:

```sh
zokahd --network testnet --no-mine --password "<password>"
```

Connect to a seed manually:

```sh
zokahd --network testnet --seed <multiaddr> --password "<password>"
```

On Windows, add `--inline` to keep the daemon in the current terminal:

```powershell
target\release\zokahd.exe --network testnet --password "<password>" --inline
```

## Genesis And Network Verification

Verify testnet genesis:

```sh
zoka genesis-verify docs/genesis.testnet.v1.json
```

Inspect configured RPC endpoints:

```sh
zoka rpc-list --network-config docs/testnet-network.v1.json
zoka rpc-list --network mainnet
```

## Safety Rules

- Save the recovery phrase before funding a wallet.
- Keep wallet passwords and recovery phrases out of Git.
- Verify release checksums before running binaries.
- Use testnet coins only for testing.
- Never use local unsafe rehearsal flags on public testnet nodes.
