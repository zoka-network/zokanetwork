# ZOKA Release Verification

Verify release artifacts before running `zoka` or `zokahd`, especially on a VPS.

## Expected Release Files

GitHub Releases should provide:

```text
zoka-testnet-v0.1.0-linux-x64.tar.gz
zoka-testnet-v0.1.0-windows-x64.zip
SHA256SUMS.txt
SHA256SUMS.txt.asc
```

The source tag is:

```text
testnet-v0.1.0
```

## Verify Checksums

Linux:

```sh
sha256sum -c SHA256SUMS.txt
```

Windows PowerShell:

```powershell
Get-Content .\SHA256SUMS.txt | ForEach-Object {
    $parts = $_ -split '\s+', 2
    if ($parts.Count -lt 2) { return }
    $expected = $parts[0].ToLower()
    $name = $parts[1].Trim()
    $actual = (Get-FileHash -Algorithm SHA256 -Path $name).Hash.ToLower()
    if ($actual -eq $expected) {
        Write-Output "$name`: OK"
    } else {
        Write-Output "$name`: FAILED"
    }
}
```

Do not run a binary if the checksum fails.

## Verify GPG Signature

```sh
gpg --import zoka-release-signing.asc
gpg --verify SHA256SUMS.txt.asc SHA256SUMS.txt
```

The fingerprint must match the fingerprint published by the ZOKA project for
that release. If the fingerprint differs, do not trust the binaries.

## Verify Source Tag

```sh
git clone https://github.com/zokachain/zoka-network.git
cd zoka-network
git fetch --tags
git checkout testnet-v0.1.0
git rev-parse HEAD
```

Compare the commit with the release notes.

## Optional Source Build

```sh
cargo build --release --features randomx --bin zoka --bin zokahd
```

The build should produce:

```text
target/release/zoka
target/release/zokahd
```

On Windows:

```text
target\release\zoka.exe
target\release\zokahd.exe
```

## Genesis Verification

Before starting a node:

```sh
zoka genesis-verify docs/genesis.testnet.v1.json
```

For mainnet candidate checks:

```sh
zoka genesis-verify docs/genesis.mainnet.v1.json
```

## Release Checklist

- Checksums pass.
- Signature validates.
- Fingerprint matches the published release key.
- Source tag matches the release notes.
- Genesis file validates.
- Binary was built with `--features randomx` for public network use.
