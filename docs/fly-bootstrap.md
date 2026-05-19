# Fly.io Bootstrap Node

This guide deploys a ZOKA bootstrap node on Fly.io without connecting GitHub.
The current bootstrap app is `zoka-bootstrap-zokachain`. The node runs as a
persistent Fly Machine in `gru` (Sao Paulo), stores its chain
and node wallet under `/data`, exposes the P2P port `30303`, and exposes the
local API through the Fly app URL.

## Requirements

- Fly CLI installed and logged in.
- A Fly app name that is globally unique.
- A persistent volume named `zoka_data`.
- A dedicated IPv4 if you want a stable `/ip4/.../tcp/30303/p2p/...` bootstrap
  multiaddr for libp2p peers.

## First Deploy

From the repository root:

```powershell
fly auth login
fly launch --no-deploy --name zoka-bootstrap-zokachain --region gru
fly volumes create zoka_data --size 20 --region gru
fly ips allocate-v4 -a zoka-bootstrap-zokachain
fly secrets set ZOKA_NODE_PASSWORD="replace-with-a-strong-node-wallet-password" -a zoka-bootstrap-zokachain
fly deploy -a zoka-bootstrap-zokachain
```

If `zoka-bootstrap` is taken, choose another app name and update `app` in
`fly.toml`.

## Runtime Layout

Inside the Machine:

```text
/data/.zoka/mainnet/              encrypted node wallet files
/data/mainnet/blockchain.db       node chain DB
/data/mainnet-local/blockchain.db imported local mainnet DB for the bootstrap
/data/node_key                    persistent libp2p node identity
/usr/local/bin/zoka               ZOKA CLI/fullnode binary
/usr/local/bin/zokahd             daemon wrapper binary
```

The Docker image starts:

```text
zoka fullnode --network mainnet \
  --password-env ZOKA_NODE_PASSWORD \
  --db ${ZOKA_CHAIN_DB_PATH:-/data/mainnet/blockchain.db} \
  --listen /ip4/0.0.0.0/tcp/30303 \
  --allow-isolated-start \
  --sync-warmup-secs ${ZOKA_SYNC_WARMUP_SECS:-30} \
  --sync-rebroadcast-recent-blocks ${ZOKA_SYNC_REBROADCAST_RECENT_BLOCKS:-32} \
  --no-mine
```

When `ZOKA_MINER_ADDRESS` is empty, the Docker entrypoint runs as a
bootstrap/verifier node with `--no-mine`. This is the recommended production
mode for public bootstrap infrastructure: it keeps the API, peer discovery,
recent block rebroadcast, and chain sync available without collecting mining
rewards. For controlled testnet/mainnet exercises only, set
`ZOKA_MINER_ADDRESS` to a wallet address that has an encrypted wallet file on
the Fly volume and keep the passphrase in the `ZOKA_NODE_PASSWORD` secret.

## Get Logs

```powershell
fly logs -a zoka-bootstrap-zokachain
```

The logs print the listening address and PeerID when the node starts. Combine
the dedicated IPv4 and PeerID into a bootstrap address:

```text
/ip4/YOUR_DEDICATED_IPV4/tcp/30303/p2p/YOUR_PEER_ID
```

Use that address in local node configs or in the network seed file.

## API

The API binds to `0.0.0.0:3000` in Fly using `ZOKA_API_BIND`. Fly exposes it at:

```text
https://zoka-bootstrap-zokachain.fly.dev
http://zoka-bootstrap-zokachain.fly.dev
```

Point the explorer and ZSilent to the HTTPS URL. The CLI read-only RPC commands
use the HTTP URL because the v0.4 CLI has a minimal stdlib HTTP client and no
TLS stack.

For node bootstrap, Fly also exposes raw block endpoints used by `zoka rpc-sync`
and by `zokahd` during initial sync:

```text
/blocks/:height/raw
/blocks/range/:from/:to/raw
```

The range endpoint lets a local node catch up in batches and avoids request
throttling when hundreds of blocks must be imported.

## Updating

After code changes:

```powershell
fly deploy -a zoka-bootstrap-zokachain
```

The volume remains attached, so the node wallet and chain data survive redeploys.
The libp2p node identity is also stored on the volume, so the bootstrap PeerID
stays stable after restarts and image updates.

## Important Notes

- Do not commit the node wallet password.
- Keep `ZOKA_NODE_PASSWORD` as a Fly secret.
- Keep public bootstrap nodes in verifier mode by default. Mine from user nodes
  or dedicated mining machines instead.
- For a production network, run at least three bootstrap nodes in different
  regions/providers.
