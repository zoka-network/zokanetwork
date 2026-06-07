# ZSilent Core Desktop

ZSilent Core is the desktop control center for the ZOKA Network. It combines
wallet operations, full-node control, mining, logs, local settings, and network
operations in a desktop interface.

## Stack

- Kotlin/JVM 17+
- Compose Desktop
- Material 3
- Gradle Kotlin DSL
- Ktor HTTP/WebSocket client
- Windows MSI packaging

## First Run Flow

1. Choose create wallet or restore seed.
2. Select 12 or 24 recovery words.
3. Set a master password and auto-lock policy.
4. Configure local node and optional mining.
5. Create or restore the Zoka wallet through the local CLI bridge.
6. Save the generated recovery phrase when creating a new wallet.
7. Enter the dashboard only after setup is complete.

## Process Secret Handling

ZSilent Core uses protected process channels instead of command-line secrets:

```text
--password-env ZSILENT_WALLET_PASSWORD
--seed-env ZSILENT_RECOVERY_PHRASE
--password-env ZSILENT_NODE_PASSWORD
```

## Privacy Defaults

- No telemetry.
- No private keys in logs.
- Wallet history is local-first.
- Remote node support must not weaken local key custody.

