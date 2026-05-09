# ZOKA Troubleshooting

This guide covers common failures when running wallets, mining, or full nodes.

## RandomX Is Not Compiled

Error text may include:

```text
not compiled into this binary
```

Build a public-network binary with RandomX:

```sh
cargo build --release --features randomx --bin zoka --bin zokahd
```

## Wallet Is Locked

`zoka wallets` can list encrypted wallets without a password, but they remain
locked. Pass the correct password to unlock wallet details:

```sh
zoka wallets --network testnet --password "<password>"
```

If the password is lost, restore from the recovery phrase:

```sh
zoka restore "<12 words>" --network testnet --password "<new-password>" --name restored
```

## Wallet Has No Spendable Funds

Mining rewards are not spendable until coinbase maturity:

```text
60 blocks
```

Wait for maturity or receive confirmed testnet funds.

## Node Starts Standalone

If no active seed node is configured, `zokahd` starts standalone until peers
connect. This is expected before the first public VPS seed is added to
`docs/testnet-network.v1.json`.

Use a manual seed:

```sh
zokahd --network testnet --seed <multiaddr> --password "<password>"
```

## Genesis Fails Verification

Run:

```sh
zoka genesis-verify docs/genesis.testnet.v1.json
```

If verification fails, confirm that the file came from the same release tag as
the binary.

## Transfer Does Not Appear In A Block

Check:

- The local node is running.
- The wallet has spendable funds.
- The transaction is pending locally.
- A miner is producing blocks.
- The fee is valid for the current network.

Useful commands:

```sh
zoka pending --network testnet
zoka status --network testnet
zoka mine-status --network testnet
```

## Windows Daemon Opens Another Window

Run with `--inline`:

```powershell
target\release\zokahd.exe --network testnet --password "<password>" --inline
```

## Local Rehearsal Flags Are Rejected

`--unsafe-rehearsal-difficulty-bits` requires:

```sh
ZOKA_ALLOW_UNSAFE_REHEARSAL=1
```

This flag is for local rehearsal only. Do not use it on public testnet nodes.
