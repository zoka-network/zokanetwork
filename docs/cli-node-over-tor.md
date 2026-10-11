# Running a ZOKA node from the command line (Linux and Windows)

This is for operators who run the `zoka` binary directly instead of the
ZSilent desktop app. ZSilent bundles Tor and passes the right flags for you;
from the CLI you supply both yourself.

Nothing here needs an open port or a public IP. Reachability comes from a Tor
v3 onion service, and chain transfers use iroh (QUIC with NAT hole-punching).

## 1. Build

No CLI binaries are published; build from source.

```sh
git clone https://github.com/zoka-network/zokanetwork.git
cd ZokaNetwork
cargo build --release --features randomx --bins
```

The binaries land in `target/release/` (`zoka`, `zoka.exe` on Windows).

You also need the network manifest, `docs/mainnet-network.v1.json`, which is
in the repository you just cloned.

## 2. Set up Tor

The node needs two things from Tor: a **SOCKS proxy** to dial other nodes, and
a **hidden service** so other nodes can dial you.

### Linux

Install Tor from your distribution, then add to `/etc/tor/torrc`:

```
SocksPort 9050
HiddenServiceDir /var/lib/tor/zoka/
HiddenServicePort 28080 127.0.0.1:28080
```

```sh
sudo systemctl restart tor
sudo cat /var/lib/tor/zoka/hostname     # -> <something>.onion
```

### Windows

Download the Tor Expert Bundle, then create a `torrc` next to `tor.exe`:

```
SocksPort 9050
HiddenServiceDir C:\Users\<you>\zoka-tor\hs
HiddenServicePort 28080 127.0.0.1:28080
```

```powershell
.\tor.exe -f .\torrc
type C:\Users\<you>\zoka-tor\hs\hostname
```

Leave Tor running. Keep the `hostname` value — that is your onion address.

## 3. Run the node

Take the hostname from the previous step and **drop the `.onion` suffix** —
the multiaddr format is `/onion3/<base32>:<port>`, not `/onion3/<...>.onion`.

Linux:

```sh
./target/release/zoka fullnode \
  --network mainnet \
  --network-config docs/mainnet-network.v1.json \
  --tor-proxy 127.0.0.1:9050 \
  --tor-only \
  --external-addr /onion3/<your-onion-without-.onion>:28080 \
  --serve-iroh-snapshot \
  --no-mine \
  --password "<your-wallet-password>"
```

Windows (PowerShell — note the backtick continuation):

```powershell
.\target\release\zoka.exe fullnode `
  --network mainnet `
  --network-config docs\mainnet-network.v1.json `
  --tor-proxy 127.0.0.1:9050 `
  --tor-only `
  --external-addr /onion3/<your-onion-without-.onion>:28080 `
  --serve-iroh-snapshot `
  --no-mine `
  --password "<your-wallet-password>"
```

What the flags do:

| Flag | Why |
|---|---|
| `--tor-proxy 127.0.0.1:9050` | how the node dials out. **Required** by `--tor-only` — without it the node refuses to start |
| `--tor-only` | refuse every non-onion destination, so nothing leaks to clearnet |
| `--external-addr` | the address you advertise so peers can dial *you* |
| `--serve-iroh-snapshot` | serve your chain to nodes that need to bootstrap. Costs upload bandwidth; it is what lets newcomers start without a central seed |
| `--no-mine` | node only. Drop it and add `--address <your-zka1-address>` to mine |

To mine, replace `--no-mine` with:

```sh
  --address <your-zka1-address> --min-mine-peers 1
```

## 4. Check it worked

The node exposes a local API. `ZOKA_API_BIND` sets the port (default comes from
the manifest's `desktop.local_rpc_url`, `127.0.0.1:3000`).

```sh
curl -s http://127.0.0.1:3000/sync/status
curl -s http://127.0.0.1:3000/peers
```

You want `"status":"caught_up"` and a non-zero `tor` count under
`by_transport`. Reaching the first peer over Tor can take a couple of minutes
while circuits are built — `seed reconnect warmup failed` during that window is
normal and not an error.

## Notes

- **A fresh node does not crawl the chain block by block.** It pulls a full
  snapshot over iroh — from the manifest seed, or, failing that, from any peer
  that offers a heavier chain. Every block is still re-validated against
  consensus on import, so a snapshot from an untrusted peer cannot introduce an
  invalid or side-branch chain.
- **Running two nodes against the same data directory will corrupt it.** Use a
  separate `ZOKA_HOME` for each instance.
- The canonical chain is anchored at block 3127 (see the README). A node whose
  block 3127 differs is on a side branch and its blocks are rejected.
