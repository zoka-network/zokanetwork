# Architecture Overview

## Core

The Rust core includes:

- CLI wallet commands.
- Full node daemon.
- Mining and proof-of-work.
- Chain state and fork choice.
- RPC API.
- Private wallet flows.
- Zero-knowledge proof infrastructure.

## Wallet Apps

ZKAPriv is Android/Kotlin/Jetpack Compose. ZSilent Core is Kotlin/JVM and
Compose Desktop.

## Frontends

zokaweb and zokaexplorer are TypeScript/React frontends.

## Important Boundaries

| Boundary | Why It Matters |
|---|---|
| Wallet app to RPC | RPC should receive chain requests and transaction payloads, not wallet secrets. |
| Explorer to private data | The public explorer should not expose address balances, senders, receivers, or private histories. |
| Desktop UI to local binaries | ZSilent Core coordinates local commands; private wallet work stays local. |
| Consensus constants to releases | Supply, PoW, activation heights, and validation rules need coordinated release handling. |

## Protocol Modules

The core repository separates mining, state, network, private wallet logic, ZK
proof infrastructure, and encrypted wallet storage. This separation is useful
when reviewing risk: a UI text change should not affect consensus, and a node
sync change should not alter wallet secret handling.
