# Running a Miner

Build the binaries:

```sh
cargo build --release --features randomx --bin zoka --bin zokahd
```

Run a mainnet miner using the network configuration:

```sh
zoka fullnode \
  --network mainnet \
  --network-config docs/mainnet-network.v1.json \
  --mine-interval-secs 30 \
  --mine-blocks 1 \
  --min-mine-peers 1 \
  --address <your-zka1-address> \
  --password <your-password>
```

For production operation, prefer protected password channels where supported,
such as environment-backed password options or stdin, instead of placing real
passwords directly in shell history.

## Sync Gate

Standalone mining checks whether the local chain is aligned with configured
public RPC state before mining. If the local database is behind the network,
the miner should sync first instead of producing blocks from a stale view.

The usual repair/bootstrap command is:

```sh
zoka rpc-sync --network mainnet --limit 512
```

## Desktop Mining

ZSilent Core is the preferred desktop control surface for users who want wallet,
node, and miner controls in one application.

From the desktop app, the node can stay online while mining is started and
stopped through local node control. This is useful for users who want to keep
sync and monitoring active without relaunching the node process.

## Mining Pools

Mining pools are not part of the current documented public flow. Future pool
support should be documented only after the implementation and payout model are
reviewed.
