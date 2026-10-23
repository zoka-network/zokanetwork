# Zoka Quantic CLI miner (testnet)

Mine **Zoka Quantic Testnet** from a terminal, without the desktop wallet. This
package is the Zoka node (`zoka`, the same one ZSilent runs inside) plus a few
scripts that create a wallet, mine, and show your balance.

> **Prefer an app?** Use **ZSilent**, the official desktop wallet: same node,
> same network, with buttons instead of commands.

- **Test coins with no value.** The testnet restarts from zero when the fully
  post-quantum private pool is ready.
- **Private by default.** Rewards are shielded notes: the chain does not show
  which address received them.
- **No open ports.** The node reaches the network through Tor, which is
  included (Windows) or installed from your distribution (Linux).

## What is inside

| File | What it does |
| --- | --- |
| `zoka` / `zoka.exe` | The Zoka Quantic node and wallet (version 3.0.4) |
| `zoka-miner.ps1` / `zoka-miner.sh` | One script for every step below |
| `*.cmd` (Windows) | Double-click shortcuts to that script |
| `tor/` (Windows) | Tor 0.4.9.14, from the official Tor Expert Bundle 15.0.25 |
| `SHA256SUMS.txt` | Checksums of every file in the package |

Everything the miner writes (chain, wallet, Tor) stays in a `data` folder next
to the scripts.

## Requirements

- A 64-bit CPU and **4 GB of RAM or more** (the RandomX dataset takes about 2 GB).
- **Windows 10 or 11** (x64), or **Linux x86-64** with `tor` installed
  (`sudo apt install tor` on Debian or Ubuntu).
- An internet connection. Nothing to open on your router.

## 1. Verify the download

Compare the SHA-256 of the archive with the one published next to it in the
release:

```powershell
# Windows (PowerShell)
Get-FileHash .\zoka-cli-miner-3.0.4-windows-x64.zip -Algorithm SHA256
```

```bash
# Linux
sha256sum zoka-cli-miner-3.0.4-linux-x64.tar.gz
```

If they do not match, do not run it: download it again from the official release.

## 2. Create your wallet

- **Windows:** unzip the package anywhere (for example, in Documents) and
  double-click **`1-create-wallet.cmd`**.
- **Linux:** `tar xzf zoka-cli-miner-3.0.4-linux-x64.tar.gz`, `cd` into the
  folder and run `./zoka-miner.sh create-wallet`.

Choose a password (at least 8 characters). You will then see **24 words**:
**write them down on paper and keep them safe.** They are the only way to
recover the wallet, they are shown only once, and anyone who has them can spend
your coins.

You also get your **address** (57 characters, starting with `zqtest1`). That is
the one you share to get paid.

Already have a wallet (for example, from ZSilent)? Use **`restore-wallet.cmd`**
or `./zoka-miner.sh restore-wallet` with your 24 words.

## 3. Mine

- **Windows:** double-click **`2-start-mining.cmd`**.
- **Linux:** `./zoka-miner.sh mine`

The script starts Tor, then the node. The node downloads the chain, waits until
it is connected to at least one other node (so that your blocks reach the
network) and starts mining. It uses all CPU threads but one; to change it, run
`zoka-miner.ps1 mine -Threads 2` (Windows) or `./zoka-miner.sh mine --threads 2`
(Linux). Press **Ctrl+C** to stop.

Each block pays **12 ZKA** to your wallet. They can be spent after **60 blocks**,
about two hours at the target pace of one block every two minutes.

## 4. Check your balance

With the miner running, open another window and run **`check-balance.cmd`**
(Windows) or `./zoka-miner.sh balance` (Linux). It asks for your wallet password.

Other commands: **`show-address.cmd`** / `./zoka-miner.sh address` shows your
address, and **`status.cmd`** / `./zoka-miner.sh status` shows the height and
your blocks.

Your address is published to the network automatically while the node runs, so
anyone can pay it.

## Mining with XMRig (optional)

Zoka Quantic uses RandomX (`rx/0`), the same algorithm as Monero, so
[XMRig](https://xmrig.com) can mine it through the node's Stratum server:

1. Run **`start-node-for-xmrig.cmd`** (Windows) or `./zoka-miner.sh xmrig-node`
   (Linux). The node opens a Stratum server on port 3333 and does not mine by itself.
2. Start XMRig with your address:

   ```text
   xmrig -o 127.0.0.1:3333 -u <your address> -p x -a rx/0
   ```

   From another computer on your local network, use this computer's local IP
   instead of `127.0.0.1`. Windows may ask whether `zoka.exe` can accept
   connections: allow private networks only.

Each miner logs in with its own address and gets the whole reward of every block
it finds. It is solo mining through your node, not a shared pool.

XMRig is a separate program; it is not included. Download it only from its
official releases and check its SHA-256. Antivirus programs, Windows Defender
included, flag CPU miners such as XMRig and stop them. A GPU can also mine
RandomX, but it is slower and uses more power than a CPU.

## Your data and backups

- Everything lives in **`data`** next to the scripts. Do not delete it unless you
  have your 24 words.
- Your **24 words** are the backup. With them you can restore the wallet in this
  package or in ZSilent, on any computer.
- An encrypted backup file is also possible:
  `zoka quantic-wallet backup --network quantic-testnet --name miner --out <file>`
  (with `ZOKA_HOME` pointing at the `data` folder).

## Ports

| Port | What for |
| --- | --- |
| `127.0.0.1:3180` | The node's local API (the scripts use it) |
| `127.0.0.1:28181` | The node's P2P port, local only (peers come through Tor) |
| `127.0.0.1:19150` | Tor's SOCKS port |
| `0.0.0.0:3333` | Stratum for XMRig, only with `xmrig-node` |

They do not collide with ZSilent, so both can be installed on the same computer.
Do not mine with both at once: they share the same CPU.

## Troubleshooting

- **"A node is already running":** another window is running the miner. Close it first.
- **Tor does not start:** check your internet connection and any proxy or
  firewall; details are in `data/tor/tor.log`.
- **It never starts mining:** the node waits until it is connected to another
  node, through Tor. Leave it a few minutes and check `status`.
- **Windows warns about the program:** the binaries are not code-signed yet.
  Verify the SHA-256 (step 1); if it matches the release, the file is the
  official one.

## Using the commands directly

The scripts only call `zoka`. To use it by hand, point `ZOKA_HOME` at the `data`
folder and pass `--network quantic-testnet`. For example, to send:

```text
zoka quantic-wallet send --network quantic-testnet --name miner --to <address> --amount 5 --password-env ZOKA_CLI_PASSWORD --rpc http://127.0.0.1:3180
```

`zoka --help` lists every command. The password always goes through an
environment variable (`--password-env`), never as an argument.
