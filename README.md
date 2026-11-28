# Zoka Quantic

Zoka Quantic is a proof-of-work blockchain with private payments by default
and a post-quantum private pool. Every payment carries a STARK proof (Plonky3,
Poseidon2, no trusted setup), each note is sealed for its recipient with
X-Wing (X25519 + ML-KEM-768), and nodes and checkpoints sign with ML-DSA-65.
It is written in Rust and mined with RandomX through XMRig.

**Network status (27 November 2026)**

- **Zoka Quantic Testnet** (`zoka-quantic-testnet-v3`) is live, with test
  coins of no value. It changes through network upgrades at an announced
  height and is never restarted: payments carry proof v4 since block 10,000.
- **Zoka Quantic Mainnet** launches with the complete protocol, once an
  external review passes. Target: Q4 2027. Testnet coins never become mainnet
  coins.
- Zenith and testnets v1 and v2 are retired. Nothing from them carries over.

## Get started

| | |
| --- | --- |
| Desktop wallet with its own node (Windows) | [ZSilent](https://github.com/zoka-network/zsilent-core/releases/latest) |
| CLI miner: node, wallet and XMRig scripts (Windows, Linux) | [Releases](https://github.com/zoka-network/zokanetwork/releases/latest) · [guide](packaging/cli-miner/README.md) |
| Android wallet | [ZKAPriv](https://github.com/zoka-network/zkapriv/releases/latest) |
| Web wallet | [wallet.zoka.network](https://wallet.zoka.network) |
| Test coins without mining | [faucet.zoka.network](https://faucet.zoka.network) |
| Explorer and public API | [zokascan.cc](https://zokascan.cc) |
| Guides, roadmap and papers | [zoka.network](https://zoka.network/#docs) |

Every release publishes `SHA256SUMS.txt` and `SHA256SUMS.txt.sig`, a signature
made with Zoka's release key (Ed25519 and ML-DSA-65 at once). Check it with
`zoka release verify SHA256SUMS.txt` (5.1.1 and later) or with OpenSSL 3.5:
[docs/release-verification.md](docs/release-verification.md).

## What the testnet runs

- **Payments.** A payment spends two, four or eight notes (padded with notes
  worth nothing) and creates two. It pays the exact fee of its shape, 0.0002,
  0.0004 or 0.0008 ZKA, the same for every wallet. Its STARK proof shows that
  the spender owns the notes, that they exist in the commitment tree and that
  inputs equal outputs plus the fee. A nullifier stops double spends without
  saying which note was spent. Since block 10,000 the proof (v4) weighs about
  0.74 MB with two notes and under 0.8 MB with eight.
- **Notes and memos.** Each output is encrypted for its recipient with X-Wing.
  A memo of up to 512 bytes travels inside it.
- **Addresses.** A short address (`zqtest2…`, 57 characters) is a BLAKE2b-256
  fingerprint of the full address, which carries the receiving keys. Wallets
  publish the full address to the nodes' directory, and a payer checks the
  fingerprint before paying. One 24-word recovery phrase gives up to eight
  addresses. An exchange gives each customer an integrated address
  (`zqtesti2…`); the payer's wallet writes the customer's code into the
  encrypted memo. View keys (`zqtestview2…`, or `zqtestviewin2…` for incoming
  payments only) show a wallet's history and cannot spend.
- **Network.** Nodes talk over QUIC (iroh). Every connection negotiates hybrid
  X25519MLKEM768 TLS and carries the server's ML-DSA-65 signature. Zoka's relays
  (relay1 and relay2.zoka.network) and n0's let nodes behind NAT reach each
  other, so nobody opens a router port. Seeds are pinned by endpoint id and key
  fingerprint. Tor mode is optional (`--tor`).
- **Consensus.** RandomX `rx/0` with a 76-byte work blob that XMRig mines
  through the node's Stratum server. ASERT difficulty around 120 seconds. 12 ZKA
  per block, spendable after 60 blocks; 23,000,000 ZKA at most, of which
  5,000,000 is the team's public allocation at genesis. Every 30 blocks, two of
  the network's three authority keys sign a checkpoint (Ed25519 and ML-DSA-65).
  Nodes never follow a branch below it, and still validate every block.

## Command line

The CLI miner's scripts run these commands for you. By hand, on the testnet:

```sh
# A node with its local API and a Stratum server for XMRig.
# ZOKA_PQ_PASSWORD encrypts the node's identity file.
zoka quantic-pq-node serve --network testnet --db node/db --identity node/node.identity \
  --api 127.0.0.1:9180 --stratum 127.0.0.1:3333 --mine-to <your-zqtest2-address>

# A wallet. --password-stdin and --password-env keep the password out of the command line.
zoka quantic-pq-wallet new --network testnet --dir wallets --name main --password-stdin
zoka quantic-pq-wallet address --network testnet --dir wallets --name main
zoka quantic-pq-wallet balance --network testnet --dir wallets --name main --password-stdin --api http://127.0.0.1:9180
zoka quantic-pq-wallet send --network testnet --dir wallets --name main --password-stdin \
  --api http://127.0.0.1:9180 --to <address> --amount 1.5 --memo "thanks"
```

`zoka quantic-pq-node help` and `zoka quantic-pq-wallet help` list every flag.
In 5.1.1, `zoka help` still lists the commands of the retired testnet v1: use
these two instead. Keep the node API on loopback.

Exchanges run `zoka quantic-pq-wallet-rpc`, a JSON-RPC 2.0 wallet server with
integrated addresses and withdrawals that are never paid twice:
[docs/EXCHANGE-INTEGRATION.md](docs/EXCHANGE-INTEGRATION.md).

## Building from source

```sh
git clone https://github.com/zoka-network/zokanetwork.git
cd zokanetwork
git checkout v5.1.1
cargo build --release --locked --features randomx,quantic-pq --bins
```

RandomX needs a C toolchain and CMake: on Linux, gcc or clang, cmake 3.5 or
later, make and pkg-config; on Windows, Visual Studio Build Tools (MSVC v143 or
later) with cmake on the PATH. Linux releases are built on Debian 11 with
[scripts/build-linux-bullseye.sh](scripts/build-linux-bullseye.sh). The
desktop wallet lives in
[zoka-network/zsilent-core](https://github.com/zoka-network/zsilent-core) and
bundles this node.

## Documentation

| Topic | Document |
| --- | --- |
| Proof v4 and network upgrades | [docs/PROOF-V4.md](docs/PROOF-V4.md) |
| Integrating an exchange | [docs/EXCHANGE-INTEGRATION.md](docs/EXCHANGE-INTEGRATION.md) |
| Zoka ID, the fingerprint of an address | [docs/ZOKA-ID.md](docs/ZOKA-ID.md) |
| Verifying releases | [docs/release-verification.md](docs/release-verification.md) |
| The CLI miner | [packaging/cli-miner/README.md](packaging/cli-miner/README.md) |
| Whitepaper and litepaper | [zoka.network](https://zoka.network/#docs/papers) |
| Internal security review | [zoka-network/audit](https://github.com/zoka-network/audit) |

Design notes in Spanish, with the reasoning behind each decision, are in
[docs/](docs/).

## Security

Never publish recovery phrases, passwords or server secrets. Verify a release's
checksums and signature before running it.

Found a vulnerability? Write to **zokanetwork@proton.me** instead of opening a
public issue. The security policy, with what is in scope and the rules for
testing, is in [SECURITY.md](SECURITY.md); the scope for an external review is
in [docs/EXTERNAL-REVIEW.md](docs/EXTERNAL-REVIEW.md).
