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

Pinned bundle (Tor Expert Bundle 15.0.24, tor 0.4.9.13 — security release of
2026-09-22; bundle signed 2026-09-29):

- URL: `https://archive.torproject.org/tor-package-archive/torbrowser/15.0.24/tor-expert-bundle-windows-x86_64-15.0.24.tar.gz`
- SHA256: `e9dc6ccc93cd6afa507193f4de284d6424233ff5102155cd2c94b259e8a22b65`
- Signature: `https://archive.torproject.org/tor-package-archive/torbrowser/15.0.24/tor-expert-bundle-windows-x86_64-15.0.24.tar.gz.asc`
- Signed by the Tor Browser Developers key, primary fingerprint
  `EF6E 286D DA85 EA2A 4BA7 DE68 4E2C 6E87 9329 8290` (fetch it with
  `gpg --auto-key-locate nodefault,wkd --locate-keys torbrowser@torproject.org`
  and check the fingerprint before trusting the "Good signature").

```powershell
powershell -ExecutionPolicy Bypass -File zsilent-core/scripts/prepare-tor-expert-bundle-windows.ps1 `
  -Url "https://archive.torproject.org/tor-package-archive/torbrowser/15.0.24/tor-expert-bundle-windows-x86_64-15.0.24.tar.gz" `
  -Sha256 "e9dc6ccc93cd6afa507193f4de284d6424233ff5102155cd2c94b259e8a22b65" `
  -AscUrl "https://archive.torproject.org/tor-package-archive/torbrowser/15.0.24/tor-expert-bundle-windows-x86_64-15.0.24.tar.gz.asc" `
  -ExpectedTorVersion "0.4.9.13" `
  -VerifySignature
```

When bumping the bundle, update **four** places together, or the next release
ships the old Tor:

1. this document;
2. `zsilent-core/scripts/prepare-release.ps1` (`$expectedTorBundleVersion`,
   `$expectedTorVersion`, `$expectedTorUrl`, `$expectedTorArchiveSha256`);
3. `zsilent-core/.github/workflows/release.yml` (URL, SHA256,
   `-ExpectedTorVersion`) — CI downloads Tor itself, so updating only the
   local install leaves the next build on the old version;
4. the installed copies (ZSilent `resources/tor/windows-x86_64`, Zveil
   `tor/`), which need the node stopped because Windows locks a running
   `tor.exe`.

The SHA256 is what the script *expects*, not proof of origin: take it from an
archive whose GPG signature you already verified. Compute it with
`Get-FileHash -Algorithm SHA256`.

Tor moved to a two-week security-release cadence in September 2026, so expect
to repeat this. The Expert Bundle trails the tor release: 0.4.9.14 came out on
2026-10-07, but on 2026-10-11 the newest bundle (15.0.24) still carried
0.4.9.13. Check the version inside the archive (the script does, via
`-ExpectedTorVersion`) instead of assuming the newest bundle has the newest tor.

`-VerifySignature` needs `gpg` on the PATH. On a Windows box where it only
comes with Git, prepend `C:\Program Files\Git\usr\bin` to `$env:PATH` for that
run.

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
