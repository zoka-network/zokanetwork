# CPU Mining and RandomX

RandomX is designed to favor general-purpose CPUs and make specialized mining
hardware less dominant than in simple hash-only proof-of-work systems.

## Why CPU Mining

- Broader access for independent miners.
- Lower barrier to entry.
- Better alignment with open participation.

## ASIC and GPU Considerations

RandomX is commonly used for CPU-oriented mining and ASIC resistance. It does
not make permanent immunity promises against specialized hardware, but it makes
the network design less dependent on narrow mining infrastructure.

## Build Requirement

Public network binaries should be built with RandomX enabled:

```sh
cargo build --release --features randomx --bin zoka --bin zokahd
```

In the current Rust package, RandomX is also part of the default feature set so
a normal release build is intended to include the public PoW backend. The
explicit `--features randomx` form remains useful for release verification and
operator instructions.

## Practical Notes

RandomX mining is CPU and memory intensive by design. Operators should monitor
system load, temperature, node sync state, and peer connectivity. A miner that
is not synced to the public chain risks wasting work on a local view that other
nodes will reject.

ZSilent Core exposes mining controls from the desktop app, while the CLI flow
remains available for servers and automation.
