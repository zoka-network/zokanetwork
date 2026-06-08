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

## Desktop Mining

ZSilent Core is the preferred desktop control surface for users who want wallet,
node, and miner controls in one application.

## Mining Pools

Mining pools are not part of the current documented public flow. Future pool
support should be documented only after the implementation and payout model are
reviewed.

