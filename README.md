# ZOKA 2.0

ZOKA is a privacy blockchain written in Rust: private wallets, RandomX proof of
work, zero-knowledge proof infrastructure, and a full node you run yourself.
There is no custodian and no server holding your funds — your node and your
wallet live on your machine.

**ZSilent Core** is the desktop application. Download it from
[Releases](https://github.com/ZokaNetwork/ZokaNetwork/releases): `.msi` for
Windows, `.deb` and `.rpm` for Linux, or the standalone node binary if you
prefer the command line.

Version 2.0 adds two sections to ZSilent, Explorer and Mining Pool. This
document covers both. (The P2P Exchange that shipped alongside them was
removed in October 2026.)

---

## Explorer — inspect the chain through your own node

Look up blocks, transactions and addresses against the node you are running.

**It reads from your node, not from a website.** No third party learns what you
are looking at. If you point ZSilent at a remote RPC instead, the section says
so with a privacy warning, because then that operator does see your queries.

**It shows what it cannot know.** Freshness, peer count and sync lag sit
alongside the data. A chain tip reported by a node with no peers is labelled as
such rather than presented as the truth.

**Private transactions stay private.** Blocks report how many private
transactions and nullifiers they contain, and nothing more — no amounts, no
senders, no recipients. The explorer cannot leak them because the node never
serves them.

---

## Mining Pool — combine hashrate without handing over rewards

Team mining lets several machines mine together. Nobody holds anyone else's
coins at any point.

### How it works

Members share a team code and mine the same block template. What rotates is
**which address the coinbase pays**, and it rotates by contributed work: the
member furthest behind relative to what they have contributed takes the next
turn.

**Whole blocks rotate — they are never split.** The coinbase pays one address,
so in a team of five you receive roughly one block in five rather than a fifth
of every block. A weaker miner waits longer between payouts but is never
excluded: turn debt accumulates until their turn arrives.

Payments land in your wallet directly from the chain. There is no pool operator
holding a balance for you, and no withdrawal step.

### Using it

**Create a team** and share the generated code, or **join with a code** someone
gave you. Then start mining from the same section.

Solo mining and team mining are mutually exclusive: the node runs one or the
other, and the section states which is active. Stop one before starting the
other.

The panel shows contributed work per member and the resulting turn order, so
everyone can check that the rotation matches what they are putting in.

---

## Running the node without ZSilent

Everything above also works from the command line.

```sh
zoka fullnode --network mainnet --address <your-zka-address>
zoka wallet-list --network mainnet --json
zoka mine-status --network mainnet --json
zoka sync-status --network mainnet --json
```

`zoka` with no arguments lists every command.

### Upgrading a database created before 2.0

Chain databases created by older builds lack the atomic-economics marker and
refuse to mine, so two accounting units can never be mixed. They still sync and
serve normally. To seal one, with the node stopped:

```sh
zoka chain-economics-migrate --network mainnet --db <path-to-blockchain.db> --dry-run
```

It audits every block's coinbase against the emission formula and refuses if any
of them disagree. If the dry run is clean, repeat with `--yes`.

Pass `--db` explicitly. The default path is relative to the current directory,
and the command refuses to run against a database that does not exist rather
than sealing an empty one and reporting success.

---

## Documentation

| Topic | Document |
| --- | --- |
| Canonical chain anchors | [docs/canonical-chain.md](docs/canonical-chain.md) |
| CLI commands | [docs/user-cli-reference.md](docs/user-cli-reference.md) |
| Run a CLI node over Tor | [docs/cli-node-over-tor.md](docs/cli-node-over-tor.md) |
| Run a VPS node | [docs/node-operator-guide.md](docs/node-operator-guide.md) |
| Verify releases | [docs/release-verification.md](docs/release-verification.md) |
| Protocol overview | [docs/protocol-overview.md](docs/protocol-overview.md) |
| Troubleshooting | [docs/troubleshooting.md](docs/troubleshooting.md) |

## Building from source

```sh
cargo build --release --features randomx --bins
```

The desktop application lives in its own repository,
[ZokaNetwork/zsilent-core](https://github.com/ZokaNetwork/zsilent-core), and
expects the node binaries to be built first.

## Security notice

Do not publish wallet recovery phrases, spend keys, scan keys, or server
secrets. Always verify release checksums before running a binary.
