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
