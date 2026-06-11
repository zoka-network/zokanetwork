# Dashboard and Metrics

ZSilent Core is designed as an operational dashboard, not only as a wallet
screen. The goal is to let a desktop user understand wallet, node, and mining
state without switching between terminals and external tools.

## Dashboard Areas

The implementation tracks several categories of local state:

- Wallet availability, lock state, balance, and recent history.
- Local node online/offline state.
- RPC connectivity.
- Network and bootstrap status.
- Mining active/inactive state.
- Mining session status and configured thread count.
- Pending private transfer visibility where supported by the local CLI.
- Runtime logs with sensitive-line redaction.

## Mining Metrics

The mining controller polls local status periodically and reads mining snapshots
from the Zoka CLI. This lets the app display whether mining is active and keep
the UI aligned with the local node process.

Mining is started through the running local node, not by launching a separate
untracked background process from the UI. That keeps node status and mining
status connected.

## Node Metrics

ZSilent Core reads local RPC state to detect whether the node is online and to
surface network status. This is especially important for miners because mining
should be monitored together with node connectivity and sync health.

## Wallet Metrics

Wallet balance and history are shown as wallet state, not as public explorer
state. Private wallet history depends on local wallet scanning and local wallet
material.

The dashboard can also support privacy-oriented display behavior, such as
hiding balances at the UI level when the user wants to reduce shoulder-surfing
or screen exposure risk.
