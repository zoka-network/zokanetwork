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

