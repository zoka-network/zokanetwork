# Privacy and Local Cryptography

ZSilent Core is built around a strict boundary: the desktop UI orchestrates
local services, but it should not turn into a place where private cryptographic
material is casually logged, exposed, or sent to remote services.

## Local Service Boundary

The application coordinates several local modules:

- Wallet lifecycle and transaction commands.
- Local full-node process control.
- Mining control.
- RPC state polling.
- Encrypted local settings.
- Sensitive value handling and log redaction.
- Backup and restore flows.

This separation matters because wallet operations, node operations, and mining
operations have different risk profiles.

## Secret Handling

ZSilent Core avoids passing wallet passwords and recovery phrases as plain
command-line arguments. The bridge uses protected process channels such as:

```text
--password-env ZSILENT_WALLET_PASSWORD
--seed-env ZSILENT_RECOVERY_PHRASE
--password-env ZSILENT_NODE_PASSWORD
```

The local Zoka binaries then perform wallet and transaction operations on the
user's machine. The UI coordinates those operations and clears sensitive
in-memory values where the implementation supports it.

## Local Wallet Operations

The wallet service supports local flows such as wallet creation, recovery from
seed, encrypted backup export, encrypted backup restore, balance reading,
history reading, fee estimation, and private send commands.

For private send flows, the UI provides the command inputs to the local wallet
binary. The transaction operation is performed locally before the result is
broadcast through the configured node path.

## Logs and Redaction

Runtime logs are important for node and mining diagnosis. They are also a
privacy risk if they expose seed phrases, private keys, wallet passwords, or
other secrets.

ZSilent Core includes log sanitization before display. Lines containing obvious
sensitive terms are replaced with a redacted message instead of being shown
verbatim.

## Honest Limits

Local-first design reduces reliance on remote services, but it does not make a
compromised device safe. Users still need to protect their operating system,
wallet backups, recovery phrase, passwords, and network environment.
