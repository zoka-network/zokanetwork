#!/usr/bin/env bash
# Deja un servidor Linux con IP pública (Ubuntu 22.04+ o Debian 12+) como
# semilla de Zoka Quantic Testnet v2, sin minar:
#
# - SSH solo con llave (sin contraseñas);
# - firewall: entra solo SSH y el puerto UDP del nodo;
# - actualizaciones de seguridad automáticas;
# - el nodo como servicio de systemd, con un usuario propio sin privilegios y
#   el puerto UDP fijo (--bind), para que el manifiesto lo liste en
#   direct_addresses y se lo alcance sin el servicio de direcciones de n0.
#
# La identidad del nodo (iroh + ML-DSA-65) se crea acá y no sale del servidor.
# Al final imprime los dos ids públicos para el manifiesto. Se puede correr
# de nuevo: no pisa la identidad ni la cadena.
#
# Uso (como root): semilla-vps.sh <binario zoka para Linux> [puerto UDP, 4433]
#
# Opcional, por variables de entorno:
# - ZOKA_SEED_API: la API local (127.0.0.1:9180 por defecto);
# - ZOKA_SEED_EXTRA: más opciones de `serve`, por ejemplo las de un nodo de
#   explorador detrás de una .onion: "--public-api 127.0.0.1:9181
#   --stream-listen 127.0.0.1:9444 --stream-address XXXX.onion:9444".
set -euo pipefail

ZOKA_SRC="${1:?usage: semilla-vps.sh <zoka binary> [udp port]}"
PORT="${2:-4433}"
API="${ZOKA_SEED_API:-127.0.0.1:9180}"
EXTRA="${ZOKA_SEED_EXTRA:-}"
SEED_USER=zoka
DATA=/var/lib/zoka-seed
BIN=/usr/local/bin/zoka
ENV_FILE=/etc/zoka-seed.env
UNIT=/etc/systemd/system/zoka-seed.service

[ "$(id -u)" = 0 ] || { echo "Run it as root." >&2; exit 1; }
[ -s /root/.ssh/authorized_keys ] || { echo "No SSH key in /root/.ssh/authorized_keys: password login is about to be turned off." >&2; exit 1; }

export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq ufw unattended-upgrades > /dev/null
printf 'APT::Periodic::Update-Package-Lists "1";\nAPT::Periodic::Unattended-Upgrade "1";\n' \
    > /etc/apt/apt.conf.d/20auto-upgrades

# SSH solo con llave. Va antes que 50-cloud-init.conf: en sshd gana el primer valor.
cat > /etc/ssh/sshd_config.d/10-zoka.conf << 'EOF'
PasswordAuthentication no
KbdInteractiveAuthentication no
PermitRootLogin prohibit-password
EOF
sshd -t
systemctl reload ssh 2> /dev/null || systemctl restart ssh

ufw default deny incoming > /dev/null
ufw default allow outgoing > /dev/null
# Todos los puertos donde escucha sshd (22, y otro si se agregó): activar el
# firewall sin uno de ellos dejaría afuera a quien entra por ese.
for ssh_port in $(sshd -T | awk '$1 == "port" {print $2}'); do
    ufw allow "$ssh_port/tcp" > /dev/null
done
ufw allow "$PORT/udp" > /dev/null
ufw --force enable > /dev/null

id -u "$SEED_USER" > /dev/null 2>&1 \
    || useradd --system --home-dir "$DATA" --create-home --shell /usr/sbin/nologin "$SEED_USER"
install -m 755 "$ZOKA_SRC" "$BIN"
mkdir -p "$DATA"
chown "$SEED_USER:$SEED_USER" "$DATA"
chmod 700 "$DATA"

# La contraseña que cifra la identidad: 32 bytes al azar, solo para root y
# systemd. Nunca se muestra.
if [ ! -s "$ENV_FILE" ]; then
    (umask 077; printf 'ZOKA_PQ_PASSWORD=%s\n' "$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n')" > "$ENV_FILE")
fi
chmod 600 "$ENV_FILE"

# La identidad, antes del servicio: el mismo comando la crea si falta.
ids="$(set -a; . "$ENV_FILE"; set +a; runuser -u "$SEED_USER" -- env ZOKA_PQ_PASSWORD="$ZOKA_PQ_PASSWORD" \
    "$BIN" quantic-pq-node identity --network testnet --identity "$DATA/node.identity")"

cat > "$UNIT" << EOF
[Unit]
Description=Zoka Quantic Testnet v2 seed
After=network-online.target
Wants=network-online.target

[Service]
User=$SEED_USER
Group=$SEED_USER
EnvironmentFile=$ENV_FILE
ExecStart=$BIN quantic-pq-node serve --network testnet --db $DATA/db --identity $DATA/node.identity --api $API --bind 0.0.0.0:$PORT $EXTRA
Restart=always
RestartSec=15
KillSignal=SIGINT
TimeoutStopSec=30
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
ReadWritePaths=$DATA
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
EOF
systemctl daemon-reload
systemctl enable zoka-seed > /dev/null 2>&1
systemctl restart zoka-seed

public_ip="$(hostname -I | awk '{print $1}')"
echo "Seed running: systemctl status zoka-seed, journalctl -u zoka-seed -f"
echo "Endpoint id:   $(sed -n 's/.*"endpoint_id": *"\([0-9a-f]*\)".*/\1/p' <<< "$ids")"
echo "ML-DSA key id: $(sed -n 's/.*"ml_dsa_key_id": *"\([0-9a-f]*\)".*/\1/p' <<< "$ids")"
echo "Direct address for the manifest: $public_ip:$PORT"
