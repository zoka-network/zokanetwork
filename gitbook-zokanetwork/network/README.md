# Network Overview

ZOKA Network is composed of wallets, nodes, miners, public RPC services, and a
privacy-safe explorer.

## Ecosystem Components

| Component | Role |
|---|---|
| Wallets | Hold keys, construct private transactions, scan chain data, recover balances. |
| Full nodes | Validate blocks, maintain chain state, gossip data, expose local or public RPC. |
| Miners | Produce proof-of-work blocks and receive rewards. |
| Public RPC | Provides read-only chain data and accepts locally built transaction payloads where enabled. |
| Explorer | Shows public consensus data without revealing private wallet state. |

## Public and Local Roles

Public services help the network stay reachable, but they should not become
custody services. Wallets own recovery, scanning, and spend authorization.
Nodes own validation and chain state. Miners own block production. The explorer
owns public visibility into consensus data.
