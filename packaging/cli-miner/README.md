# Zoka Quantic Testnet v2 CLI miner

Mine the **Zoka Quantic Testnet v2** from a terminal, without the desktop
wallet. This package is the Zoka node (`zoka`, the same one ZSilent 4.0 runs
inside) plus a few scripts that create a wallet, start the node so XMRig can
mine through it, and show your balance.

> **Prefer an app?** Use **ZSilent 4.0**, the official desktop wallet for
> Windows: the same node and the same network, with buttons instead of
> commands.

- **Test coins, no value.** Testnet v2 replaces testnet v1 (the ZSilent 3.0
  one): v1 wallets and coins do not carry over.
- **Private and post-quantum.** Rewards and payments are shielded notes, with
  STARK proofs and X-Wing encryption: the chain does not show which address
  receives them.
- **No open ports.** The node reaches its peers over QUIC, through n0's relays.
  Nothing to change on your router, and no Tor to install.

## What's inside

| File | What for |
| --- | --- |
| `zoka` / `zoka.exe` | The Zoka Quantic node and wallet (version 4.0.2) |
| `zoka-miner.ps1` / `zoka-miner.sh` | One script for every step below |
| `*.cmd` (Windows) | Double-click shortcuts to that script |
| `SHA256SUMS.txt` | The hashes of every file in the package |

**XMRig is not included** (see step 3). Everything the miner writes (the chain,
the node's identity, the wallets) stays in a `data` folder next to the scripts.

## Requirements

- A 64-bit processor and **4 GB of RAM or more**: XMRig uses about 2.5 GB.
- **Windows 10 or 11** (x64), or **Linux x86-64** with glibc 2.31 or newer
  (Ubuntu 20.04 or Debian 11 onwards).
- An internet connection. Nothing to open on your router.

## 1. Verify the download

Compare the file's SHA-256 with the one published next to it in the release:

```powershell
# Windows (PowerShell)
Get-FileHash .\zoka-cli-miner-4.0.2-windows-x64.zip -Algorithm SHA256
```

```bash
# Linux
sha256sum zoka-cli-miner-4.0.2-linux-x64.tar.gz
```

If they do not match, do not run it: download it again from the official
release.

## 2. Create your wallet

- **Windows:** unzip the package anywhere (for example in Documents) and
  double-click **`1-create-wallet.cmd`**.
- **Linux:** `tar xzf zoka-cli-miner-4.0.2-linux-x64.tar.gz`, `cd` into the
  folder and run `./zoka-miner.sh create-wallet`.

Choose a password (at least 8 characters). Then you will see **24 words**:
**write them down on paper and keep them safe.** They are the only way to
recover the wallet, they are shown only once, and whoever has them can spend
your coins.

You also get your **short address** (57 characters, starting with `zqtest2`).
That is the one you share to get paid.

Already have a testnet v2 wallet, for example in ZSilent 4.0? Use
**`restore-wallet.cmd`** or `./zoka-miner.sh restore-wallet` with your 24 words.

## 3. Get XMRig

On testnet v2 the node does not mine by itself: [XMRig](https://xmrig.com), the
RandomX miner (`rx/0`, Monero's algorithm), mines through the node's Stratum
server. Every block it finds pays your wallet in full: this is solo mining
through your own node, not a shared pool.

1. Download XMRig only from its [official releases](https://github.com/xmrig/xmrig/releases)
   (`xmrig-<version>-windows-x64.zip`, or `linux-static-x64` on Linux) and check
   its SHA-256 against the `SHA256SUMS` published next to it.
2. Unpack it **inside this package's folder**. The folder the archive comes
   with (`xmrig-<version>`) works as it is. You can also say where it is:
   `zoka-miner.ps1 mine -Xmrig C:\path\xmrig.exe` or
   `./zoka-miner.sh mine --xmrig /path/xmrig`.

Antivirus programs, Windows Defender included, flag CPU miners such as XMRig
and stop them.

## 4. Mine

- **Windows:** double-click **`2-start-mining.cmd`**.
- **Linux:** `./zoka-miner.sh mine`

The script starts the node, which connects to the network and downloads the
chain. Then it publishes your short address, so anyone can pay you with it, and
starts XMRig. XMRig uses every CPU thread but one. To change that:
`zoka-miner.ps1 mine -Threads 2` (Windows) or `./zoka-miner.sh mine --threads 2`
(Linux). **Ctrl+C** stops both.

If the script does not find XMRig, it leaves the node waiting and shows the
command to start XMRig yourself in another window:

```text
xmrig -o 127.0.0.1:3333 -u miner -p x -a rx/0
```

Each block pays **12 ZKA** to your wallet. They can be spent after **60
blocks**, about two hours at the intended pace of one block every two minutes.

## 5. Check your balance

With the miner running, open another window and run **`check-balance.cmd`**
(Windows) or `./zoka-miner.sh balance` (Linux). It asks for the wallet's
password. With the node off, it shows the balance as of the last time it ran.

Other commands: **`show-address.cmd`** / `./zoka-miner.sh address` shows your
address, and **`status.cmd`** / `./zoka-miner.sh status` shows the height, the
peers and the connected miners.

## 6. See your balance anywhere: the view key

**`show-view-key.cmd`** (Windows) or `./zoka-miner.sh view-key` (Linux) shows
your wallet's **view key** (`zqtestview2…`). Paste it in
[zokascan.cc/wallet](https://zokascan.cc/wallet): the explorer downloads the
chain's encrypted outputs and opens them inside your browser, so it shows your
balance and every payment without the key ever leaving the page.

- It **cannot spend**: spending needs your 24 words. A Zoka Quantic wallet has
  no separate spend key.
- Whoever holds it sees every payment of the wallet, past and future. Share it
  only as you would a bank statement: it cannot be revoked. If it leaks, send
  your coins to a new wallet.

## Help the network: keep a node running

Testnet v2 relies today on a single seed node, the team's. Every node that stays
online all day makes the network sturdier, and an always-on node can become a
**seed**, one of the nodes compiled into Zoka so new nodes find the network.

- Leave **`2-start-mining.cmd`** / `./zoka-miner.sh mine` running. Without XMRig
  the node still runs and serves the chain; with it, it also mines.
- To offer it as a seed, run **`show-node-id.cmd`** / `./zoka-miner.sh node-id`
  and send both ids to the Zoka team on Discord. They are public; they never
  give access to your wallet or your node. The team adds them to the network
  document of the next release.
- A seed's IP address is visible to the nodes that connect to it, like any
  node's. Keep the `node` folder: the ids live in it, and a new folder means a
  new identity.

## Your data and your backups

- Everything is in **`data/quantic-testnet-v2`**, next to the scripts:
  - `node`: the chain and the node's identity;
  - `wallets`: your wallets, encrypted with their password;
  - `wallet-mirror`: the copy of the chain the wallets read.
- **Do not copy the `node` folder to another computer.** Two nodes with the same
  identity get in each other's way on the network. Every install creates its
  own.
- Your **24 words** are the backup. With them you can restore the wallet in this
  package or in ZSilent 4.0, on any computer.
- You can also make an encrypted copy in a file:
  `zoka quantic-pq-wallet backup --network testnet --dir data/quantic-testnet-v2 --name miner --out <file>`

## Ports

| Port | What for |
| --- | --- |
| `127.0.0.1:9180` | The node's local API (the scripts use it) |
| `127.0.0.1:3333` | The Stratum for XMRig (`-StratumPort` / `--stratum-port` changes it) |

Both listen on this computer only. Towards the internet the node uses QUIC
(UDP) and n0's relays, with no open ports.

The API does not clash with ZSilent 4.0's. The Stratum uses the same port as
ZSilent's, though: if ZSilent's Stratum is on, pick another one with
`zoka-miner.ps1 mine -StratumPort 3334` or `./zoka-miner.sh mine --stratum-port 3334`.
Either way, do not mine with both at once: they share the same CPU.

## Troubleshooting

- **"A node is already running":** another window has the miner running. Close
  it first.
- **"The node stopped", or the node does not start:** the details are in
  `data/quantic-testnet-v2/node-errors.log`.
- **XMRig does not connect:** check that no other program (ZSilent, for example)
  uses the Stratum port and that the node is still running (`status`).
- **XMRig closes right after starting:** usually the antivirus. The script shows
  its exit code.
- **Windows warns about the program:** the binaries are not digitally signed
  yet. Verify the SHA-256 (step 1): if it matches the release's, the file is the
  official one.

## Running the commands by hand

The scripts only call `zoka`. To use it by hand, pass
`--network testnet --dir data/quantic-testnet-v2`. For example, to send, with
the node running:

```text
zoka quantic-pq-wallet send --network testnet --dir data/quantic-testnet-v2 --name miner --to <address> --amount 5 --password-env ZOKA_CLI_PASSWORD --peer data/quantic-testnet-v2/node/peer.json --api http://127.0.0.1:9180
```

`zoka quantic-pq-wallet help` lists the wallet's commands, and
`zoka quantic-pq-node help` the node's. The password always goes through an
environment variable (`--password-env`), never as an argument.
