#!/usr/bin/env bash
# Instala un nodo de Zoka en un VPS con Ubuntu 24.04. Ver README.md.
#
#   sudo ./instalar.sh --rol wallet|liviano|explorador --binario ./zoka --ip <IPv4>
#        [--dominio datos.zokascan.cc --publicador ./publish.py]   (explorador)
#
# Es idempotente: volver a correrlo con los mismos argumentos actualiza el
# binario y la configuracion sin tocar la base de la cadena.
set -euo pipefail

ROL=""
BINARIO=""
IP=""
DOMINIO=""
PUBLICADOR=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --rol) ROL="${2:-}"; shift 2 ;;
    --binario) BINARIO="${2:-}"; shift 2 ;;
    --ip) IP="${2:-}"; shift 2 ;;
    --dominio) DOMINIO="${2:-}"; shift 2 ;;
    --publicador) PUBLICADOR="${2:-}"; shift 2 ;;
    -h|--help) sed -n '2,9p' "$0"; exit 0 ;;
    *) echo "argumento desconocido: $1" >&2; exit 2 ;;
  esac
done

if [[ $EUID -ne 0 ]]; then echo "correrlo con sudo" >&2; exit 1; fi
case "$ROL" in
  wallet|liviano|explorador) ;;
  *) echo "--rol tiene que ser wallet, liviano o explorador" >&2; exit 2 ;;
esac
if [[ ! -f "$BINARIO" ]]; then echo "--binario: no existe '$BINARIO'" >&2; exit 2; fi
if [[ ! "$IP" =~ ^[0-9]{1,3}(\.[0-9]{1,3}){3}$ ]]; then echo "--ip tiene que ser una IPv4" >&2; exit 2; fi
if [[ "$ROL" == explorador ]]; then
  if [[ -z "$DOMINIO" ]]; then echo "el explorador necesita --dominio" >&2; exit 2; fi
  if [[ ! -f "$PUBLICADOR" ]]; then echo "el explorador necesita --publicador con publish.py" >&2; exit 2; fi
fi

AQUI="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "== paquetes"
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq tor ufw unattended-upgrades python3 >/dev/null
if [[ "$ROL" == explorador ]]; then
  apt-get install -y -qq caddy >/dev/null
fi

echo "== usuario y carpetas"
if ! id zoka >/dev/null 2>&1; then
  useradd --system --home-dir /var/lib/zoka --shell /usr/sbin/nologin zoka
fi
install -d -o zoka -g zoka -m 0750 /var/lib/zoka
install -d -m 0755 /opt/zoka/bin /etc/zoka
# Si el servicio ya corre, se para antes de reemplazar el binario.
systemctl stop zoka-nodo 2>/dev/null || true
install -m 0755 "$BINARIO" /opt/zoka/bin/zoka
echo "binario: $(sha256sum /opt/zoka/bin/zoka | cut -d' ' -f1)"
/opt/zoka/bin/zoka --version

echo "== onion"
TORRC=/etc/tor/torrc
if ! grep -q '^HiddenServiceDir /var/lib/tor/zoka/' "$TORRC"; then
  cat >> "$TORRC" <<'TOR'

# Zoka: onion del nodo (agregado por deploy/vps/instalar.sh)
HiddenServiceDir /var/lib/tor/zoka/
HiddenServicePort 28080 127.0.0.1:28080
TOR
fi
systemctl enable tor >/dev/null
systemctl restart tor
ONION=""
for _ in $(seq 1 30); do
  if [[ -s /var/lib/tor/zoka/hostname ]]; then
    ONION="$(cat /var/lib/tor/zoka/hostname)"
    break
  fi
  sleep 2
done
if [[ -z "$ONION" ]]; then echo "Tor no creo la onion en 60 s" >&2; exit 1; fi
ONION="${ONION%.onion}"

echo "== configuracion del nodo"
# --sync-only: no abre ni crea wallets y no mina.
ARGS="--network testnet --sync-only --tor-proxy 127.0.0.1:9050 --allow-clearnet-seeds"
ARGS="$ARGS --listen /ip4/0.0.0.0/tcp/28080"
ARGS="$ARGS --external-addr /ip4/${IP}/tcp/28080 --external-addr /onion3/${ONION}:28080"
if [[ "$ROL" != explorador ]]; then
  # Un nodo nuevo puede bajar la cadena entera de este en segundos.
  ARGS="$ARGS --serve-iroh-snapshot"
fi
cat > /etc/zoka/nodo.env <<ENV
# Generado por deploy/vps/instalar.sh. Rol: ${ROL}
ZOKA_HOME=/var/lib/zoka
# La API queda solo en el propio servidor.
ZOKA_API_BIND=127.0.0.1:3000
ZOKA_ARGS=${ARGS}
ENV
install -m 0644 "$AQUI/systemd/zoka-nodo.service" /etc/systemd/system/zoka-nodo.service

if [[ "$ROL" == explorador ]]; then
  echo "== explorador"
  install -d -m 0755 /opt/zoka/explorador
  install -m 0644 "$PUBLICADOR" /opt/zoka/explorador/publish.py
  install -d -o zoka -g zoka -m 0755 /srv/zoka/explorador-datos
  install -m 0644 "$AQUI/systemd/zoka-explorador-publicar.service" /etc/systemd/system/
  install -m 0644 "$AQUI/systemd/zoka-explorador-publicar.timer" /etc/systemd/system/
  sed "s/__DOMINIO__/${DOMINIO}/" "$AQUI/caddy/Caddyfile.explorador" > /etc/caddy/Caddyfile
fi

echo "== firewall"
ufw default deny incoming >/dev/null
ufw default allow outgoing >/dev/null
ufw allow OpenSSH >/dev/null
ufw allow 28080/tcp >/dev/null
if [[ "$ROL" == explorador ]]; then
  ufw allow 80/tcp >/dev/null
  ufw allow 443/tcp >/dev/null
fi
ufw --force enable >/dev/null

echo "== servicios"
systemctl daemon-reload
systemctl enable --now zoka-nodo
if [[ "$ROL" == explorador ]]; then
  systemctl enable --now zoka-explorador-publicar.timer
  systemctl restart caddy
fi

cat <<FIN

Listo: nodo '${ROL}' instalado.
Registrar en la red (ver README.md, paso 3):
  onion: /onion3/${ONION}:28080
  TCP:   /ip4/${IP}/tcp/28080   (o /dns4/<dominio>/tcp/28080 con un registro A)
FIN
if [[ "$ROL" == explorador ]]; then
  echo "  datos del explorador: https://${DOMINIO}  (rpc_url de zoka-network.json)"
fi
