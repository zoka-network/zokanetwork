# Introduction

ZOKA Network is built around one core idea: financial activity should not become
public surveillance data by default.

Most blockchains make transaction graphs, balances, and address relationships
easy to inspect. ZOKA takes the opposite direction for user payments: wallet
state is recovered locally by the owner, while public infrastructure exposes
only the data needed for consensus, synchronization, and verification.

## Core Principles

- **Privacy by default** for user-facing transfer flows.
- **Self-custody** through local wallet keys and recovery phrases.
- **Public verifiability** through blocks, proof-of-work, commitments,
  nullifiers, and transaction hashes.
- **Open participation** for nodes and miners.
- **Practical clients** across desktop and Android.

ZOKA's public surfaces should explain what is verifiable without exposing what
is personal. The explorer, website, and docs can show network status, blocks,
mining, supply, and downloads, while wallets remain the place where private
account history and recovery material live.
