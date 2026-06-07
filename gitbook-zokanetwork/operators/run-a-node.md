# Run a Node

Build from source:

```sh
cargo build --release --features randomx --bin zoka --bin zokahd
```

Start a mainnet daemon:

```sh
zokahd --network mainnet --password "<wallet-password>"
```

Start without mining:

```sh
zokahd --network mainnet --no-mine --password "<wallet-password>"
```

On Windows, add `--inline` to keep the daemon in the current terminal:

```powershell
target\release\zokahd.exe --network mainnet --password "<wallet-password>" --inline
```

## Safety

Do not store production passwords directly in systemd units, Git repositories,
screenshots, or shell history. Use an operator process or secret manager
appropriate for the host.

