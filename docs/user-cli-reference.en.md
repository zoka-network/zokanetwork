# ZOKA User CLI Reference

This is the minimum command set expected in every mainnet release. User flows
are private by default; there is no transparent transfer mode in the user CLI.

## Recommended Short Commands

```powershell
zoka
zoka create <label> --network mainnet|testnet --password <pwd>
zoka restore "<recovery phrase>" --network mainnet|testnet --password <pwd> [--name <label>]
zoka wallets [--network mainnet|testnet] [--password <pwd>]
zoka balance <wallet> [--network mainnet|testnet] [--password <pwd>]
zoka send <from> <to> <amount> [--network mainnet|testnet] [--password <pwd>] --yes
zoka mine <wallet> [--network mainnet|testnet] [--password <pwd>]
zoka node [--network mainnet|testnet] [--password <pwd>] [--seed <multiaddr>]
zoka status
zoka rpc-list
zoka rpc-status [--rpc http://host:port]
```

Long command names (`wallet-create`, `tx-send`, `fullnode`, and so on) remain
available for scripts, but the short commands are the primary user UX.
Wallets and local mining state are stored per network inside the project:
`.zoka/mainnet/private_wallets`, `.zoka/testnet/private_wallets`,
`.zoka/mainnet/user_mining_state.json`, and
`.zoka/testnet/user_mining_state.json`.

## Read-Only Mainnet RPC

```powershell
zoka rpc-list [--network-config <file>] [--json]
zoka rpc-status [--rpc http://host:port] [--network-config <file>] [--timeout-secs <n>] [--json]
zoka rpc-height [--rpc http://host:port] [--network-config <file>] [--timeout-secs <n>] [--json]
zoka rpc-mempool [--rpc http://host:port] [--network-config <file>] [--timeout-secs <n>] [--json]
```

Read-only ZOKA RPC queries. They do not read wallets, sign, or submit
transactions. If `--rpc` is omitted, the CLI uses the first active RPC in
`docs/mainnet-network.v1.json` or the file passed with `--network-config`.

In v0.4, the embedded client supports `http://host:port` with short timeouts.
For public HTTPS/onion/I2P RPC, use a trusted local HTTP proxy or wait for the
dedicated TLS/proxy client.

## Full Node

```powershell
zokahd [--network mainnet|testnet] [--address <wallet-address>] [--password <pwd>] [--network-config docs/mainnet-network.v1.json] [--seed <multiaddr> ...] [--genesis <file>]
```

`zokahd` starts a full node, loads or creates the local node wallet, syncs from
peers, and mines when the wallet and chain state are ready. If the daemon needs
to create a wallet, `--password` is required.

By default, the daemon looks for `docs/mainnet-network.v1.json`, validates the
mainnet `chain_id` and `PROTOCOL_FREEZE_ID`, and connects to the first usable
active seed node. `--seed <multiaddr>` overrides the network config for
operators and can be repeated for fallback bootstrap peers. Clearnet seed
endpoints are skipped unless the operator explicitly passes
`--allow-clearnet-seeds`.

## Wallet

Create a wallet:

```powershell
zoka wallet-create --password <pwd> [--name <label>] [--export-secrets <path>]
```

The command prints the `zka1...` address, recovery phrase, encrypted local file
path and next steps. The encrypted file is stored under
`.zoka/<network>/private_wallets/`. A user can create more than one wallet; each wallet
keeps its own password, keys and recovery phrase. Full hex keys are printed
only with `--show-keys` or exported with `--export-secrets <path>`.

Restore from seed:

```powershell
zoka wallet-restore-from-seed --seed "<words>" --password <pwd> [--name <label>] [--restore-height <n>]
```

List wallets:

```powershell
zoka wallet-list --password <pwd>
```

Check balance:

```powershell
zoka wallet-balance --address <zka1...> --password <pwd>
```

Back up the encrypted wallet file:

```powershell
zoka wallet-backup --address <zka1...> --out <path> --password <pwd>
```

Export recovery secrets only when offline:

```powershell
zoka wallet-export-secrets --address <zka1...> --password <pwd> --out <path>
```

## Transfers

Send private ZKA:

```powershell
zoka tx-send --from <wallet-address|name> --to <wallet|address> --amount <n> --priority normal --password <pwd> --yes
```

Estimate fee:

```powershell
zoka estimate-fee --amount <n> --priority normal
```

The normal transfer path is private by default. Amounts and recipients are not
published as transparent account transfers.

Amounts and optional `--fee` values accept up to 6 decimals:
`0.000001 ZKA` is 1 atom. On a quiet chain the estimator may return `0`; under
load it increases in `0.0001 ZKA` steps. Private fees are paid to the block
producer through encrypted `private_fee_claims`, without publishing the clear
fee in the transaction body.

## Mining

Start assisted mining:

```powershell
zoka mine --address <zka1...> --password <pwd>
```

Check mining state:

```powershell
zoka mine-status
```

Open the live mining console:

```powershell
zoka mine-console
```

## Chain And Sync

Check chain height:

```powershell
zoka height
```

Check wallet sync:

```powershell
zoka sync-status --address <zka1...> --password <pwd>
```

Verify genesis before running mainnet:

```powershell
zoka genesis-verify docs/genesis.mainnet.v1.json
```

## Fast Sync Snapshots

Import a signed snapshot:

```powershell
zoka snapshot-import <snapshot.json> --require-signature --signer-key <hex>
```

Every block inside the snapshot is replayed through the same consensus block
validation path used by normal sync.

## Chain Pruning

Drop block bodies older than `head - N` from RocksDB to reclaim disk while
keeping recent state for difficulty retarget and short reorgs.

```powershell
# Dry-run first to see what would be removed.
zoka chain-prune --keep-last 2880 --dry-run

# Apply.
zoka chain-prune --keep-last 2880
```

`--keep-last` is silently floored at `chain::PRUNE_MIN_RETAINED_BLOCKS` (8)
because the difficulty retarget reads the timestamps of the parent and
grandparent â€” pruning below that floor would break the next-block difficulty
calculation. The chain head metadata, the height index for retained blocks,
and the per-block `chain:block_meta:<hash>` entries for retained blocks are
NEVER touched. Pruning a node turns it into a non-archival node: it cannot
serve IBD for heights below `head - keep_last`.

## Safety Rules

- Write down the recovery phrase before funding the wallet.
- Keep the spend key offline unless you are using the wallet.
- Verify `SHA256SUMS.txt` and its GPG signature before running a release binary.
- Verify the genesis digest before starting a mainnet node.
- Treat remote RPC endpoints as untrusted unless explicitly documented.
