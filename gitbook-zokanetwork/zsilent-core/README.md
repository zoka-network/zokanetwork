# ZSilent Core

ZSilent Core is the native desktop control center for ZOKA Network. It is the
all-in-one application for users who want wallet access, local node operation,
mining control, logs, and operational metrics from a single interface.

It combines:

- Self-custody wallet.
- Local full node control.
- Mining control.
- Dashboard metrics.
- Runtime logs with sensitive-line redaction.
- Local settings and backup flows.
- Network and RPC status.

## Purpose

ZSilent Core gives desktop users a single interface for self-custody, node
operation, and mining without requiring every action to be performed manually
through the terminal.

The application is not a remote hosted wallet. It controls local Zoka binaries
through a process bridge and reads node state through local RPC. The UI is a
coordinator for local services: wallet, node, mining, encrypted settings,
privacy handling, and dashboard state.

## Why It Matters

ZSilent Core is important because it makes full participation in the network
practical from a desktop:

- A user can run a local node instead of depending only on public RPC services.
- Mining can be started, stopped, and monitored from the same application.
- Wallet actions can stay on the user's machine.
- Node, wallet, and mining status can be viewed together.
- Logs and metrics help diagnose network participation without exposing wallet
  secrets.

## Privacy Defaults

- No telemetry.
- No private keys in logs.
- Wallet history is local-first.
- Secrets are handled through protected process channels where possible.
- Remote node usage is treated as an explicit mode, while local node operation
  is the privacy-oriented default.

## Current Scope

ZSilent Core is designed for desktop operators and advanced users. Its current
implementation focuses on local control of Zoka Core binaries, wallet setup and
restore flows, node status, mining control, logs, and dashboard visibility. It
does not turn public infrastructure into a custodian of user keys.
