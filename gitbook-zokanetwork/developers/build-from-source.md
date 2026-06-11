# Build From Source

## Core

```sh
cargo build --release --features randomx --bin zoka --bin zokahd
```

## ZKAPriv Android

```sh
./gradlew assembleRelease
```

The release APK is generated at:

```text
app/build/outputs/apk/release/
```

## ZSilent Core

```powershell
.\gradlew.bat run
.\gradlew.bat build
.\gradlew.bat packageMsi
```

## zokaweb

```sh
npm install
npm run build
```

## zokaexplorer

```sh
npm install
npm run build
```

## Verification

Release builds should verify more than successful compilation:

- RandomX support is included for public-network binaries.
- The expected binaries are produced: `zoka` and `zokahd`.
- Wallet apps do not commit signing keys, keystore properties, APKs, or local
  secret files.
- Android release artifacts are signed and checked before publication.
- Desktop packaging keeps local binary discovery and secret handling intact.

For consensus or wallet changes, run the relevant Rust, Android, or desktop
tests before tagging a release.
