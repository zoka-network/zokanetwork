# ZOKA Release Verification

Verify release artifacts before running `zoka` or `zokahd`, especially on a VPS.

## Expected Files

```text
zoka-testnet-v0.1.0-linux-x64.tar.gz
zoka-testnet-v0.1.0-windows-x64.zip
SHA256SUMS.txt
```

If a release also publishes `SHA256SUMS.txt.asc`, verify the GPG signature
against the fingerprint shown in that release's notes.

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

## Verify Source Tag

```sh
git clone https://github.com/zoka-network/zokanetwork.git
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

## Verify Testnet Genesis

```sh
zoka genesis-verify docs/genesis.testnet.v1.json
```

## Checklist

- Checksums pass.
- Source tag matches the release notes.
- Testnet genesis validates.
- Binary was built with `--features randomx`.
