# ZOKA 3.0

**Network status (21 October 2026):** Zoka has two networks, both started from
zero: **Zoka Quantic Testnet** (`--network quantic-testnet`, the default; test
coins) and **Zoka Quantic Mainnet** (`--network quantic-mainnet`). Zenith, the
previous network, was retired: nothing from its chain, balances or addresses
carries over, and it runs only on releases before 3.0.

The testnet runs Ironwood transactions with X-Wing note envelopes: private
against a future quantum computer, but spend authorization is not post-quantum
yet ([what is and is not](docs/QUANTIC-INVENTARIO-CRIPTOGRAFICO.md)). It will
restart from zero, with new addresses, when the fully post-quantum shielded pool
is ready ([design](docs/QUANTIC-POOL-POSTCUANTICO.md)); the mainnet launches only
with that complete protocol. Launch steps: [docs/LANZAMIENTO-quantic.md](docs/LANZAMIENTO-quantic.md).

ZOKA is a privacy blockchain written in Rust: shielded transfers (Ironwood:
Halo 2 with no trusted setup), notes sealed with X-Wing (X25519 + ML-KEM-768),
RandomX proof of work, and a full node you run yourself. There is no custodian
and no server holding your funds: your node and your wallet live on your
machine.

**ZSilent 3.0** is the desktop application, for Zoka Quantic only. Download it
from [Releases](https://github.com/zoka-network/zokanetwork/releases): `.msi`
for Windows, `.deb` and `.rpm` for Linux, or the standalone node binary if you
prefer the command line.

---

## What ZSilent does

- **Wallet.** A new wallet gets a 24-word recovery phrase (256 bits). Balances,
  coinbase rewards that are still maturing, and a history with the amounts only
  your wallet can read. Sending shows the exact fee first (0.0001 ZKA per
  action, at least two) and takes an optional memo, up to 512 bytes, that only
  the recipient can read. The receive address is about 1,730 characters long
  (it carries the X-Wing key), so it is shown as an animated QR code in parts.
- **Node and mining.** ZSilent runs its own node and mines into the active
  wallet's address. The node never receives the wallet password: wallets are
  opened by `zoka quantic-wallet` on each request.
- **Tor.** The node can be reached through an onion service on its P2P port
  (28180 on the testnet).

---

## Running the node without ZSilent

```sh
zoka quantic-wallet new --network quantic-testnet --name main --password-stdin
zoka quantic-wallet balance --network quantic-testnet --name main --password-stdin
zoka fullnode --network quantic-testnet --quantic-mine-to <your-zqtest1-address>
zoka mine-status --network quantic-testnet --json
```

`zoka help` lists every command. The node API listens on 127.0.0.1:3100 on the
testnet and 127.0.0.1:3300 on the mainnet.

---

## Documentation

| Topic | Document |
| --- | --- |
| Launching the testnet and the mainnet | [docs/LANZAMIENTO-quantic.md](docs/LANZAMIENTO-quantic.md) |
| Protocol specification (for review) | [docs/REVIEW-zoka-quantic.md](docs/REVIEW-zoka-quantic.md) |
| What is and is not post-quantum | [docs/QUANTIC-INVENTARIO-CRIPTOGRAFICO.md](docs/QUANTIC-INVENTARIO-CRIPTOGRAFICO.md) |
| The fully post-quantum pool (design) | [docs/QUANTIC-POOL-POSTCUANTICO.md](docs/QUANTIC-POOL-POSTCUANTICO.md) |
| Mobile wallet engine | [docs/QUANTIC-MOTOR-MOVIL.md](docs/QUANTIC-MOTOR-MOVIL.md) |
| Run a CLI node over Tor | [docs/cli-node-over-tor.md](docs/cli-node-over-tor.md) |
| Verify releases | [docs/release-verification.md](docs/release-verification.md) |

## Building from source

```sh
cargo build --release --features randomx,quantic --bins
```

Without `quantic` the binary cannot join either network. The desktop
application lives in its own repository,
[zoka-network/zsilent-core](https://github.com/zoka-network/zsilent-core), and
expects the node binaries to be built first.

## Security notice

Do not publish wallet recovery phrases or server secrets. Always verify release
checksums before running a binary.
