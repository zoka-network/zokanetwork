# CLI Bridge

ZSilent Core talks to Zoka Core binaries through a process bridge.

The bridge uses protected process channels instead of command-line secrets:

```text
--password-env ZSILENT_WALLET_PASSWORD
--seed-env ZSILENT_RECOVERY_PHRASE
--password-env ZSILENT_NODE_PASSWORD
```

The Zoka CLI also supports `--password-stdin` for terminal and automation
flows.

## Why Environment Channels Are Used

Command-line arguments can be visible through process lists, diagnostic tools,
shell history, or crash reports. ZSilent Core therefore avoids placing recovery
phrases, wallet passwords, or node passwords directly in command arguments.

Environment-backed channels are not magic, and they still require a trusted
local machine, but they are a safer boundary than exposing secrets in the
visible command line.

## Wallet Commands

The wallet service uses the bridge for actions such as:

- Creating a wallet.
- Restoring from a recovery phrase.
- Reading balance and history.
- Estimating fees.
- Sending private transactions.
- Exporting encrypted backups.
- Restoring encrypted backups.
- Changing the wallet passphrase.

Sensitive character arrays are cleared after use where the implementation
supports it.

## Node and Mining Commands

The node controller starts the local node with node-only behavior first, then
mining is controlled through the running node API. This keeps the node process,
mining state, and dashboard status aligned.
