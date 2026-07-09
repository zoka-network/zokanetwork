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

1. Download the official Tor Project Expert Bundle for Windows x86_64.
2. Verify it before packaging. The helper script requires a pinned SHA256 and can also verify `.asc` with GPG:

```powershell
powershell -ExecutionPolicy Bypass -File zsilent-core/scripts/prepare-tor-expert-bundle-windows.ps1 `
  -Url "<official-tor-expert-bundle-url>" `
  -Sha256 "<pinned-sha256>" `
  -AscUrl "<official-asc-url>" `
  -VerifySignature
```

3. Build the Windows installer with Tor required:

```powershell
cd zsilent-core
.\gradlew.bat packageMsi -Pzsilent.requireTorBundle=true
```

The Gradle package task copies the prepared bundle from `third_party/tor/windows-x86_64` or from `-Pzsilent.torBundleDir=<dir>` into `common/tor/windows-x86_64/`.

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
