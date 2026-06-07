# Mining and Blocks

ZOKA uses proof-of-work mining. Public network binaries use RandomX.

## Build With RandomX

```sh
cargo build --release --features randomx --bin zoka --bin zokahd
```

The development CPU fallback is for CI and isolated testing. It is not the
public network proof-of-work backend.

## Mining Rewards

Mining rewards are paid as coinbase outputs and mature after 60 blocks.

## Explorer Visibility

Blocks expose public consensus metadata such as height, timestamp, hash,
difficulty, nonce, transaction counts, and private transaction hashes. Private
participants and amounts remain hidden.

