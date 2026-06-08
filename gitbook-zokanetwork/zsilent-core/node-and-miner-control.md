# Node and Miner Control

ZSilent Core can control the existing Rust binaries through a local process
bridge and read node state through the local RPC API.

## Full Node

Users can configure and run a local ZOKA node from the desktop interface.

The node controller starts the local `zoka` binary in full-node mode and keeps
mining disabled at launch. This lets the node come online, sync, and expose
local status before mining is toggled separately.

Supported runtime configuration includes network selection, optional network
configuration files, bootstrap seed settings, Tor proxy options, mining thread
preferences, and the reward address used when mining is enabled.

## Miner

Users can configure mining behavior and route rewards to their wallet address.

Mining is controlled through the local node API after the node is running. The
desktop app can request `/mining/start` and `/mining/stop`, which allows mining
to be toggled without shutting down the node process.

The mining controller validates the reward address and thread count before
starting a mining session. It also polls mining status so the dashboard can show
whether mining is active and report current session state.

## Operational Flow

1. Start the local full node.
2. Wait for RPC and sync status.
3. Configure the reward wallet address.
4. Start mining with a selected thread count.
5. Monitor node, mining, and wallet state from the dashboard.
6. Stop mining without stopping the node when needed.

## Logs

Logs are useful for diagnosing node and mining state, but must never include
private keys, seed phrases, or wallet passwords.

ZSilent Core sanitizes runtime logs before showing them in the UI. Lines that
match obvious sensitive terms are redacted instead of displayed directly.
