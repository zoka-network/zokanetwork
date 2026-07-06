# Node config file (`zoka.config.json`)

Instead of a long command line (or a `.bat` wrapper), a full node can be
driven by an editable JSON file, the way most networks ship a `bitcoin.conf`
or `config.toml`. Edit the file, save, run the binary.

## Where it's loaded from

`zoka fullnode` resolves the config in this order:

1. `--config <path>` on the command line (error if the named file is missing).
2. `$ZOKA_CONFIG` environment variable (error if the named file is missing).
3. `zoka.config.json` next to the executable, then in the current directory
   (optional — if none exists the node just runs from flags/defaults as before).

When a config is loaded the node prints `Loaded node config: <path>` at startup.

## Precedence

Config values are expanded into the same flags the CLI already accepts, and
**the command line always wins**: anything you pass explicitly (e.g.
`--threads 8`) overrides the file. `--seed` values from the file and the CLI
are combined (union), not replaced.

## Fields

All fields are optional. Unknown fields are ignored (forward-compatible).

| Field | Maps to | Notes |
|-------|---------|-------|
| `network` | `--network` | `mainnet` (default) or `testnet`. |
| `address` | `--address` | Your `zka1…` reward address. |
| `password` | (see below) | Plaintext wallet password. Leave empty to be prompted. |
| `password_env` | `--password-env` | Name of an env var holding the password. Used only if `password` is empty. |
| `threads` | `--threads` | CPU threads for mining. Accepts a number (`3`) or string (`"3"`). |
| `power` | `--power` | e.g. `normal`. |
| `mine` | `--no-mine` when `false` | Default mines; set `false` to run node-only. |
| `mining_console` | `--dashboard` when `false` | The fluid streaming console is the default; set `false` to opt back into the old full-screen dashboard. |
| `allow_isolated_start` | `--allow-isolated-start` | Start even if no peers are reachable yet. |
| `network_config` | `--network-config` | Path to the network manifest (seeds + RPC). |
| `db` | `--db` | Override the data/db path. |
| `seeds` | `--seed` (repeated) | Extra P2P seed multiaddrs, unioned with the manifest's. |

## Password handling (flexible)

The password is only taken from the config when you didn't already pass one on
the command line (`--password`, `--password-env`, `--password-stdin`). Then:

1. If `password` is a non-empty string → it's used (exported to a private env
   var internally rather than placed on the process command line).
2. Else if `password_env` names an env var → that variable is read.
3. Else, if this run will mine and an `address` is set → the node prompts for
   the wallet password on the console.

For maximum safety, prefer `password_env` (or leaving `password` empty to be
prompted) over storing the plaintext password in the file.

## Trusted setup

When `ZOKA_TRUSTED_SETUP_DIR` is not set, the node auto-discovers a
`trusted-setup/` folder next to the executable (then in the current directory),
so a shipped folder works with no env var or wrapper script. The bundle's
digest is still verified, so a tampered or foreign bundle is rejected.

## Example

```json
{
  "network": "mainnet",
  "address": "zka1REPLACE_WITH_YOUR_ADDRESS",
  "password": "",
  "threads": 3,
  "power": "normal",
  "mine": true,
  "allow_isolated_start": true,
  "network_config": "mainnet-network.v1.json"
}
```

With this file and the `trusted-setup/` folder alongside the binary, the whole
run is just:

```
zoka fullnode
```

(It prompts for the wallet password because `password` is empty.)
