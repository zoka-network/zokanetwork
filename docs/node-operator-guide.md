# ZOKA Node Operator Guide

This guide is for running `zokahd` on a Linux VPS for the public testnet.

## Recommended VPS Baseline

- Linux x86_64.
- 2 or more CPU cores.
- 4 GB RAM minimum, 8 GB preferred for RandomX testing.
- SSD storage.
- System clock synchronized with NTP.
- A dedicated unprivileged `zoka` user.

## Install A Release Package

Download the Linux package from GitHub Releases:

```sh
wget https://github.com/ZokaNetwork/ZokaNetwork/releases/download/testnet-v0.1.0/zoka-testnet-v0.1.0-linux-x64.tar.gz
tar -xzf zoka-testnet-v0.1.0-linux-x64.tar.gz
sudo install -m 0755 zoka /usr/local/bin/zoka
sudo install -m 0755 zokahd /usr/local/bin/zokahd
```

Verify the package before installing it. See
[release-verification.md](release-verification.md).

## First Start

```sh
sudo useradd --system --create-home --shell /usr/sbin/nologin zoka
sudo -u zoka /usr/local/bin/zokahd --network testnet --password "<wallet-password>"
```

When the daemon creates the node wallet, store the recovery phrase offline. The
encrypted wallet file is stored under the `zoka` user's home directory in
`.zoka/testnet/private_wallets/`.

## Systemd Service

Create `/etc/systemd/system/zokad-testnet.service`. The example below shows the
shape of the service, but the wallet password must come from your own secret
manager or operator process. Do not store production passwords in Git.

```ini
[Unit]
Description=ZOKA testnet node
After=network-online.target
Wants=network-online.target

[Service]
User=zoka
Group=zoka
WorkingDirectory=/home/zoka
Environment=RUST_LOG=info
ExecStart=/usr/local/bin/zokahd --network testnet --password CHANGE_ME
Restart=always
RestartSec=10
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
```

Then:

```sh
sudo systemctl daemon-reload
sudo systemctl enable --now zokad-testnet
sudo journalctl -u zokad-testnet -f
```

Before enabling the service, replace `CHANGE_ME` using an OS-level secret
manager or a deployment wrapper appropriate for your VPS. Do not commit
passwords or recovery phrases to this repository.

## Seed Node Publication

After the node is stable:

1. Capture the announced listen address and peer ID from the daemon output.
2. Build a full multiaddr ending in `/p2p/<peer-id>`.
3. Add it to `docs/testnet-network.v1.json` under `seed_nodes`.
4. Mark it `active` only after a clean restart and successful peer connection
   from another host.

## Firewall

Open the TCP port used by the node listen address. The current default is:

```text
/ip4/0.0.0.0/tcp/28080
```

Do not expose wallet files, recovery phrases, or local secret directories.

## Backups

Back up:

- The encrypted wallet file.
- The wallet recovery phrase.
- Any release signing or operator keys.

Do not back up:

- Temporary RocksDB test databases.
- Logs containing accidental secrets.
- Local rehearsal artifacts.
