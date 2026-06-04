# Node TLS / HTTPS setup (reverse proxy)

The node REST API serves plain HTTP. For mainnet it must be reachable over
HTTPS so a network observer can't read request metadata (which blocks/private
tx hashes a wallet queries, timing, etc.) between the app / explorer and the
node.

Recommended approach: terminate TLS at **Caddy** in front of each node. Caddy
auto-provisions and renews Let's Encrypt certificates; the node keeps serving
HTTP but only on loopback. No node code change, no manual cert rotation.

You run this on each droplet (BOOT `159.223.119.216`, VPS2 `157.230.5.103`).
I can't SSH in, so the steps below are for you to execute.

## 1. DNS

Create an A record per node pointing a hostname at the droplet IP, e.g.:

```
boot-rpc.zoka.network   A   159.223.119.216
vps2-rpc.zoka.network   A   157.230.5.103
```

(Any domain you control works — it just needs a public A record so Let's
Encrypt can validate it.)

## 2. Bind the node to loopback only

So the node is reachable ONLY through Caddy, not directly on :3000. In the
systemd unit / launch args, bind the API to `127.0.0.1:3000` instead of
`0.0.0.0:3000`, then:

```bash
systemctl daemon-reload && systemctl restart zoka-bootstrap.service
```

## 3. Install Caddy

```bash
apt install -y debian-keyring debian-archive-keyring apt-transport-https curl
curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' | gpg --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' | tee /etc/apt/sources.list.d/caddy-stable.list
apt update && apt install -y caddy
```

## 4. Caddyfile

`/etc/caddy/Caddyfile` (use this node's hostname):

```
boot-rpc.zoka.network {
    encode zstd gzip
    reverse_proxy 127.0.0.1:3000
}
```

Then:

```bash
systemctl restart caddy
systemctl status caddy --no-pager | head -5
```

Caddy obtains the cert on first request. Verify:

```bash
curl -s https://boot-rpc.zoka.network/chain/height
```

## 5. Firewall

Block the raw API port from the public internet, allow 80/443 (Caddy needs 80
for the ACME challenge):

```bash
ufw allow 80/tcp
ufw allow 443/tcp
ufw deny 3000/tcp
ufw reload
```

## 6. Point the clients at HTTPS (final step — only after the cert is live)

Once `https://<host>/chain/height` works on BOTH nodes, switch the clients:

- **ZKAPriv app** — `zkapriv/core/.../util/Constants.kt`:
  ```kotlin
  const val ZOKA_MAINNET_RPC          = "https://boot-rpc.zoka.network/"
  const val ZOKA_MAINNET_RPC_FALLBACK = "https://vps2-rpc.zoka.network/"
  ```
  Rebuild + ship the APK.

- **Explorer** — `zokaexplorer/vercel.json` rewrite destination:
  ```json
  { "source": "/rpc/:path*", "destination": "https://boot-rpc.zoka.network/:path*" }
  ```
  Push → Vercel redeploys. (Vercel→node is now encrypted too.)

Do step 6 LAST: switching the URLs before the cert is live would break the app
and explorer.

## Alternative: native TLS in the node

If you'd rather not run a reverse proxy, the node binary can be extended to
serve HTTPS directly (rustls + a cert/key path via env). It works but you own
cert renewal. Ask and I'll add it; the Caddy path above is recommended.
