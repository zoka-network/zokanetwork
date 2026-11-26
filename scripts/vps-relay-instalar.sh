#!/usr/bin/env bash
# Instala el relay de iroh de Zoka en un VPS (relay1 en el VPS 1, relay2 en el
# VPS 2). Se corre como root en el VPS, con el binario ya subido:
#
#   bash vps-relay-instalar.sh <relayN.zoka.network> <ruta del binario> <sha256>
#
# El relay usa el 443/TCP (HTTPS, con certificado de Let's Encrypt que saca
# solo), el 80/TCP (HTTP y la prueba de portal cautivo) y el 7842/UDP (QUIC,
# para que cada nodo vea su dirección pública). SSH se queda en el 22 y el 2222:
# antes de sacarlo del 443 se comprueba que el 2222 esté escuchando. No borra
# nada: el binario anterior y la configuración de SSH quedan con otro nombre.
set -euo pipefail

host="$1"
binary="$2"
sha="$3"
case "$host" in
    relay1.zoka.network|relay2.zoka.network) ;;
    *) echo "nombre inesperado: $host" >&2; exit 1 ;;
esac

echo "$sha  $binary" | sha256sum -c -
chmod 0755 "$binary"
"$binary" --help > /dev/null

if [ -e /usr/local/bin/iroh-relay ]; then
    cp -a /usr/local/bin/iroh-relay "/usr/local/bin/iroh-relay.antes-$(date +%Y%m%d%H%M%S)"
fi
install -m 0755 "$binary" /usr/local/bin/iroh-relay

id iroh-relay > /dev/null 2>&1 || useradd --system --no-create-home --shell /usr/sbin/nologin iroh-relay
install -d -m 0755 /etc/iroh-relay

cat > /etc/iroh-relay/config.toml <<EOF
# Relay de iroh de Zoka: https://$host (scripts/vps-relay-instalar.sh).
enable_relay = true
http_bind_addr = "[::]:80"
enable_quic_addr_discovery = true
enable_metrics = false

[tls]
https_bind_addr = "[::]:443"
quic_bind_addr = "[::]:7842"
cert_mode = "LetsEncrypt"
hostname = "$host"
cert_dir = "/var/lib/iroh-relay/certs"
prod_tls = true

[limits]
accept_conn_limit = 50.0
accept_conn_burst = 200

[limits.client.rx]
bytes_per_second = 4194304
max_burst_bytes = 16777216
EOF

cat > /etc/systemd/system/iroh-relay.service <<EOF
[Unit]
Description=Zoka iroh relay ($host)
After=network-online.target
Wants=network-online.target

[Service]
User=iroh-relay
Group=iroh-relay
ExecStart=/usr/local/bin/iroh-relay --config-path /etc/iroh-relay/config.toml
Restart=always
RestartSec=5
Environment=RUST_LOG=info
StateDirectory=iroh-relay
AmbientCapabilities=CAP_NET_BIND_SERVICE
CapabilityBoundingSet=CAP_NET_BIND_SERVICE
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
EOF

ufw allow 80/tcp > /dev/null
ufw allow 7842/udp > /dev/null

# SSH sale del 443 recién cuando el 2222 está escuchando.
ss -tlnH | awk '{print $4}' | grep -qE ':2222$' || { echo "SSH no escucha en el 2222: no se toca" >&2; exit 1; }
conf=/etc/ssh/sshd_config.d/05-puertos.conf
if grep -q '^Port 443$' "$conf"; then
    cp -a "$conf" "/root/05-puertos.conf.antes-relay-$(date +%Y%m%d%H%M%S)"
    sed -i '/^Port 443$/d' "$conf"
    sshd -t
    systemctl reload ssh
    sleep 2
fi
if ss -tlnpH | grep -q 'sshd.*:443 '; then echo "SSH sigue en el 443" >&2; exit 1; fi

systemctl daemon-reload
systemctl enable --now iroh-relay
sleep 5
systemctl is-active iroh-relay
journalctl -u iroh-relay --no-pager -o cat -n 15
echo LISTO="$host"
