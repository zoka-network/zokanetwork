# ZSilent Managed Tor Onion Seed

ZSilent can run a bundled Tor process for Windows and expose the local P2P listener through a v3 onion service. This is meant for users behind CGNAT or routers where port forwarding is impossible.

## Product Defaults

- Managed Tor uses `127.0.0.1:19050` for SOCKS so it does not fight Tor Browser or a system Tor on `9050`.
- The onion service maps virtual port `28080` to the local node listener `127.0.0.1:28080`.
- Hybrid mode is the normal mode: direct TCP/DNS stays enabled and onion dialing is available when Tor is running.
- Public seed publication is manual opt-in. ZSilent can show a seed onion address, but it does not automatically add every user to `docs/mainnet-network.v1.json`.

Managed `torrc`:

```text
SocksPort 127.0.0.1:19050
DataDirectory <ZokaData>/tor/data
HiddenServiceDir <ZokaData>/tor/zoka-p2p
HiddenServiceVersion 3
HiddenServicePort 28080 127.0.0.1:28080
Log notice stdout
```

## Release Packaging

### Step 1 — Prepare and verify the Tor Expert Bundle

The helper script downloads the bundle, checksum-verifies it against a pinned
SHA256, optionally verifies the `.asc` signature with GPG, and extracts it into
`third_party/tor/windows-x86_64`.

Pinned bundle (Tor Expert Bundle 15.0.17, tor 0.4.9.11):

- URL: `https://archive.torproject.org/tor-package-archive/torbrowser/15.0.17/tor-expert-bundle-windows-x86_64-15.0.17.tar.gz`
- SHA256: `5f91e9426bf641dfe539dc28029088c72bed0b1d8f1c79104a0f89273cb3ebe1`
- Signature: `https://archive.torproject.org/tor-package-archive/torbrowser/15.0.17/tor-expert-bundle-windows-x86_64-15.0.17.tar.gz.asc`

```powershell
powershell -ExecutionPolicy Bypass -File zsilent-core/scripts/prepare-tor-expert-bundle-windows.ps1 `
  -Url "https://archive.torproject.org/tor-package-archive/torbrowser/15.0.17/tor-expert-bundle-windows-x86_64-15.0.17.tar.gz" `
  -Sha256 "5f91e9426bf641dfe539dc28029088c72bed0b1d8f1c79104a0f89273cb3ebe1" `
  -AscUrl "https://archive.torproject.org/tor-package-archive/torbrowser/15.0.17/tor-expert-bundle-windows-x86_64-15.0.17.tar.gz.asc" `
  -VerifySignature
```

When bumping the bundle, update all three values together (URL, SHA256, `.asc`). Compute the SHA256 with `Get-FileHash -Algorithm SHA256`.

### Step 2 — Build the Windows installer with Tor required

```powershell
cd zsilent-core
.\gradlew.bat packageMsi -Pzsilent.requireTorBundle=true
```

The Gradle package task copies the prepared bundle from `third_party/tor/windows-x86_64` or from `-Pzsilent.torBundleDir=<dir>` into `common/tor/windows-x86_64/`. `-Pzsilent.requireTorBundle=true` makes the build fail (instead of warn) if the bundle is missing, so a release can't silently ship without Tor.

## Operating A Public Onion Seed

1. Open ZSilent → Privacy.
2. Keep `Managed Tor bundled with ZSilent` enabled.
3. Enable `Publish this node as an onion seed`.
4. Start the node.
5. Wait for `Seed address` to show a value like:

```text
/onion3/<56-char-onion-id>:28080/p2p/<peer_id>
```

6. From another machine with Tor enabled, start a node using that multiaddr as `--seed`.
7. Confirm it connects and sync/gossip works.
8. Only then add the onion multiaddr to `docs/mainnet-network.v1.json` as an active seed.

Keep existing TCP seeds in the manifest for speed and compatibility.
