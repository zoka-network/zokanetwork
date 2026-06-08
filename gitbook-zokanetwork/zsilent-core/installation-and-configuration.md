# Installation and Configuration

ZSilent Core is distributed as a desktop application. The current implementation
uses Kotlin/JVM, Compose Desktop, Material 3, Gradle Kotlin DSL, Ktor, and
Windows MSI packaging.

## Local Development

```powershell
.\gradlew.bat run
.\gradlew.bat build
.\gradlew.bat packageMsi
```

## Binary Discovery

The app looks for Zoka binaries in common local build paths such as:

```text
../target/release/zoka.exe
../target/release/zokahd.exe
../target/debug/zoka.exe
../target/debug/zokahd.exe
```

## First-Run Direction

The desktop flow is designed to guide users through the local setup sequence:

- Create or restore a wallet.
- Protect the wallet with a local password.
- Configure the local node.
- Choose whether to enable mining after setup.
- Save recovery material offline.
- Enter the dashboard with wallet, node, mining, and log visibility.

The setup flow should keep secrets local. Recovery phrases and wallet passwords
belong on the user's machine, not in public forms, support messages, or
explorer searches.
