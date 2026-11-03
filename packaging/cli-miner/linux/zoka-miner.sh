#!/usr/bin/env bash
# Minero de Zoka Quantic Testnet v2 por línea de comandos, para Linux.
#
# Todo queda dentro de la carpeta del paquete, en ./data/quantic-testnet-v2,
# con el mismo orden que ZSilent 4.0: node (la cadena y la identidad del nodo),
# wallets y wallet-mirror (la copia de la cadena que leen las wallets). El nodo
# llega a la red por los relays de n0, sin abrir puertos. Su API va en
# 127.0.0.1:9180 y el Stratum para XMRig en 127.0.0.1:3333 (--stratum-port lo
# cambia).
#
# En la testnet el nodo no mina solo: mina XMRig a través de su Stratum. Si
# XMRig está al lado de este script, "mine" lo arranca.
#
# "mine" queda vigilado: si XMRig se cierra vuelve a los 10 s, y si se cae el
# nodo se rearman los dos. "autostart" lo instala como servicio de usuario de
# systemd para que arranque con el sistema.
#
# Una semilla con IP pública (un VPS) fija su puerto UDP con --bind 0.0.0.0:PUERTO
# y lo abre en su firewall: así se la alcanza directo, sin el servicio de
# direcciones ni los relays de n0 (docs/SEMILLAS.md).
#
# Uso: ./zoka-miner.sh <create-wallet | restore-wallet | address | view-key | mine | autostart [--off] | balance | status | node-id> [--wallet nombre] [--threads n] [--stratum-port n] [--xmrig ruta] [--tor host:puerto] [--bind ip:puerto]
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ZOKA="$ROOT/zoka"
NETWORK="testnet"
V2="$ROOT/data/quantic-testnet-v2"
NODE_DIR="$V2/node"
DESCRIPTOR="$NODE_DIR/peer.json"
API_PORT=9180
API="http://127.0.0.1:$API_PORT"
STRATUM_PORT=3333
XMRIG=""
# Modo Tor (opcional): el SOCKS de un Tor que ya corre (127.0.0.1:9050 el del
# sistema, 127.0.0.1:9150 el de Tor Browser).
TOR=""
# Puerto UDP fijo del nodo (opcional), para una semilla con IP pública.
BIND=""

COMMAND="${1:-help}"
shift || true
WALLET="miner"
THREADS=0
AUTOSTART_OFF=0
while [ $# -gt 0 ]; do
    case "$1" in
        --wallet) WALLET="$2"; shift 2 ;;
        --threads) THREADS="$2"; shift 2 ;;
        --stratum-port) STRATUM_PORT="$2"; shift 2 ;;
        --xmrig) XMRIG="$2"; shift 2 ;;
        --tor) TOR="$2"; shift 2 ;;
        --bind) BIND="$2"; shift 2 ;;
        --off) AUTOSTART_OFF=1; shift ;;
        *) echo "Unknown option: $1" >&2; exit 2 ;;
    esac
done
STRATUM="127.0.0.1:$STRATUM_PORT"
NODE_LOG="$V2/node.log"
NODE_ERR_LOG="$V2/node-errors.log"

# La identidad del nodo y las wallets son solo para este usuario.
umask 077
mkdir -p "$NODE_DIR"

die() { echo "Error: $*" >&2; exit 1; }

read_password() {
    local prompt="$1" value
    read -r -s -p "$prompt: " value
    echo >&2
    printf '%s' "$value"
}

wallet_exists() { [ -f "$V2/wallets/$WALLET.json" ]; }

address() {
    "$ZOKA" quantic-pq-wallet address --network "$NETWORK" --dir "$V2" --name "$WALLET" "$@" | tr -d '\r\n'
}

# La API, por /dev/tcp de bash: no hace falta curl.
api_get() {
    local reply
    exec 3<> "/dev/tcp/127.0.0.1/$API_PORT" || return 1
    printf 'GET %s HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n' "$1" >&3
    reply="$(cat <&3)"
    exec 3<&-
    printf '%s' "${reply#*$'\r\n\r\n'}"
}

node_online() { api_get /status > /dev/null 2>&1; }

# Un número de /status, por su clave (las de abajo no se repiten).
status_number() { grep -o "\"$1\":[0-9]*" <<< "$2" | head -1 | cut -d: -f2; }

# La identidad del nodo se cifra con 32 bytes al azar que se crean la primera
# vez, como en ZSilent 4.0. Nunca se muestran.
node_secret() {
    local file="$NODE_DIR/node.identity.secret"
    if [ ! -f "$file" ]; then
        od -An -N32 -tx1 /dev/urandom | tr -d ' \n' > "$file"
    fi
    [ -s "$file" ] || die "$file is empty"
    tr -d '\r\n' < "$file"
}

# XMRig no viene en el paquete: --xmrig, o xmrig al lado de este script (suelto
# o en la carpeta del archivo oficial, xmrig-<versión>), o en el PATH.
find_xmrig() {
    if [ -n "$XMRIG" ]; then
        [ -x "$XMRIG" ] || die "XMRig not found at $XMRIG"
        printf '%s' "$XMRIG"
        return
    fi
    local candidate
    for candidate in "$ROOT/xmrig" "$ROOT"/xmrig*/xmrig; do
        if [ -f "$candidate" ] && [ -x "$candidate" ]; then
            printf '%s' "$candidate"
            return
        fi
    done
    command -v xmrig 2> /dev/null || true
}

XMRIG_PID=""
stop_xmrig() {
    if [ -n "$XMRIG_PID" ] && kill -0 "$XMRIG_PID" 2> /dev/null; then
        kill -TERM "$XMRIG_PID" 2> /dev/null || true
        wait "$XMRIG_PID" 2> /dev/null || true
    fi
    XMRIG_PID=""
}

NODE_PID=""
stop_node() {
    # XMRig primero: sin el nodo no tiene a quién minar.
    stop_xmrig
    if [ -n "$NODE_PID" ] && kill -0 "$NODE_PID" 2> /dev/null; then
        # Como Ctrl+C: el nodo cierra su base solo; si no termina, se corta.
        kill -INT "$NODE_PID" 2> /dev/null || true
        for _ in $(seq 15); do
            kill -0 "$NODE_PID" 2> /dev/null || break
            sleep 1
        done
        kill -KILL "$NODE_PID" 2> /dev/null || true
    fi
    echo "Node stopped."
}

start_node() {
    # El nodo nunca pisa un archivo: el descriptor de la corrida anterior se borra.
    rm -f "$DESCRIPTOR"
    ZOKA_PQ_PASSWORD="$1" \
        "$ZOKA" quantic-pq-node serve --network "$NETWORK" --db "$NODE_DIR/db" \
        --identity "$NODE_DIR/node.identity" --api "127.0.0.1:$API_PORT" --descriptor-out "$DESCRIPTOR" \
        --stratum "$STRATUM" --mine-to "$2" ${TOR:+--tor "$TOR"} ${BIND:+--bind "$BIND"} > "$NODE_LOG" 2> "$NODE_ERR_LOG" &
    NODE_PID=$!
}

# Mina vigilado: si XMRig se cierra, vuelve a los 10 s; si se cae el nodo, se
# rearman los dos a los 15 s. Solo Ctrl+C (o detener el servicio) lo para.
mine() {
    wallet_exists || die "There is no wallet '$WALLET' yet. Run ./zoka-miner.sh create-wallet (or restore-wallet) first."
    node_online && die "A node is already running on $API. Close it first."
    local full short cores miner secret round=0 online code
    secret="$(node_secret)"
    full="$(address --full)"
    short="$(address)"
    cores="$(nproc)"
    if [ "$THREADS" -le 0 ]; then THREADS=$((cores > 1 ? cores - 1 : 1)); fi
    miner="$(find_xmrig)"
    trap stop_node EXIT
    trap 'exit 130' INT TERM
    [ -n "$TOR" ] && echo "Tor mode: the node connects only through Tor at $TOR, to the network's onion seeds."

    while true; do
        round=$((round + 1))
        start_node "$secret" "$full"
        echo "Starting the node and joining the network..."
        local deadline=$((SECONDS + 180))
        online=0
        until node_online; do
            if ! kill -0 "$NODE_PID" 2> /dev/null || [ $SECONDS -ge $deadline ]; then
                # La primera vez es un error para ver; después, se reintenta solo.
                [ "$round" -eq 1 ] && die "The node did not start. Details in $NODE_ERR_LOG"
                break
            fi
            sleep 2
        done
        node_online && online=1
        if [ "$online" -eq 1 ]; then
            # Publica la dirección corta en la red, así cualquiera puede pagarte con ella.
            if ! "$ZOKA" quantic-pq-wallet publish-address --network "$NETWORK" --dir "$V2" --name "$WALLET" \
                --api "$API" > /dev/null 2>&1 && [ "$round" -eq 1 ]; then
                echo "Your short address could not be published yet; the full one always works."
            fi
            if [ "$round" -eq 1 ]; then
                echo
                echo "Mining rewards go to wallet '$WALLET': $short"
                echo "The node's log is $NODE_LOG"
            fi
            if [ -n "$miner" ]; then
                if [ "$round" -eq 1 ]; then
                    echo "Mining with XMRig, $THREADS of $cores CPU threads (--threads n changes it). Ctrl+C stops both."
                    echo
                fi
                # XMRig no termina si el nodo se cae (reintenta el Stratum
                # para siempre): corre aparte y se vigilan los dos.
                while kill -0 "$NODE_PID" 2> /dev/null; do
                    "$miner" -o "$STRATUM" -u "$WALLET" -p x -a rx/0 -t "$THREADS" &
                    XMRIG_PID=$!
                    while kill -0 "$NODE_PID" 2> /dev/null && kill -0 "$XMRIG_PID" 2> /dev/null; do
                        sleep 2
                    done
                    if ! kill -0 "$NODE_PID" 2> /dev/null; then
                        stop_xmrig
                        break
                    fi
                    code=0
                    wait "$XMRIG_PID" || code=$?
                    XMRIG_PID=""
                    echo "$(date '+%F %T') XMRig stopped (exit code $code). Starting it again in 10 s; Ctrl+C stops everything."
                    sleep 10
                done
            else
                if [ "$round" -eq 1 ]; then
                    echo "XMRig was not found next to this script, so the node is waiting for miners. Start it with:"
                    echo "  xmrig -o $STRATUM -u $WALLET -p x -a rx/0 -t $THREADS"
                    echo "Ctrl+C stops the node."
                    echo
                fi
                tail -n +1 -f --pid="$NODE_PID" "$NODE_LOG" || true
            fi
        fi
        echo "$(date '+%F %T') The node stopped. Details in $NODE_ERR_LOG. Starting it again in 15 s; Ctrl+C stops."
        stop_node > /dev/null
        NODE_PID=""
        sleep 15
    done
}

# Arranque con el sistema: un servicio de usuario de systemd que corre "mine"
# con las mismas opciones y lo vuelve a levantar si termina.
autostart() {
    local unit="zoka-miner-$WALLET.service"
    local dir="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
    command -v systemctl > /dev/null || die "This system has no systemd. Add this line with 'crontab -e' instead:
  @reboot $ROOT/zoka-miner.sh mine --wallet $WALLET >> $V2/autostart.log 2>&1"
    if [ "$AUTOSTART_OFF" -eq 1 ]; then
        systemctl --user disable --now "$unit" 2> /dev/null || true
        rm -f "$dir/$unit"
        systemctl --user daemon-reload || true
        echo "Mining no longer starts with the system (wallet '$WALLET')."
        return
    fi
    wallet_exists || die "There is no wallet '$WALLET' yet. Run ./zoka-miner.sh create-wallet (or restore-wallet) first."
    local args="mine --wallet $WALLET"
    [ "$THREADS" -gt 0 ] && args="$args --threads $THREADS"
    [ "$STRATUM_PORT" != 3333 ] && args="$args --stratum-port $STRATUM_PORT"
    [ -n "$XMRIG" ] && args="$args --xmrig $(cd "$(dirname "$XMRIG")" && pwd)/$(basename "$XMRIG")"
    [ -n "$TOR" ] && args="$args --tor $TOR"
    [ -n "$BIND" ] && args="$args --bind $BIND"
    mkdir -p "$dir"
    cat > "$dir/$unit" << UNIT
[Unit]
Description=Zoka Quantic CLI miner (wallet $WALLET)
After=network-online.target
Wants=network-online.target

[Service]
WorkingDirectory=$ROOT
ExecStart=$ROOT/zoka-miner.sh $args
Restart=always
RestartSec=15
KillSignal=SIGINT
TimeoutStopSec=30

[Install]
WantedBy=default.target
UNIT
    systemctl --user daemon-reload
    systemctl --user enable --now "$unit"
    echo "Mining now runs as the user service $unit and starts with the system."
    echo "Its output: journalctl --user -u $unit -f   Stop it from starting: ./zoka-miner.sh autostart --off"
    echo "To keep it running when you are not logged in (a server), run once: sudo loginctl enable-linger $USER"
}

# La contraseña viaja a zoka por una variable de entorno de este proceso: nunca
# como argumento (la vería cualquiera que liste los procesos) ni en pantalla.
case "$COMMAND" in
    create-wallet)
        wallet_exists && die "Wallet '$WALLET' already exists. Use --wallet <other name> for another one."
        echo "A new wallet is protected by a password on this computer."
        p1="$(read_password "Choose a password")"
        p2="$(read_password "Repeat the password")"
        [ "$p1" = "$p2" ] || die "The two passwords are different. Nothing was created."
        [ ${#p1} -ge 8 ] || die "Use at least 8 characters. Nothing was created."
        echo
        echo "WRITE DOWN THE 24 WORDS BELOW, ON PAPER, AND KEEP THEM SAFE."
        echo "They are the only way to recover this wallet. They are shown only once,"
        echo "and anyone who has them can spend your coins. Never type them on a website."
        echo
        ZOKA_CLI_PASSWORD="$p1" "$ZOKA" quantic-pq-wallet new --network "$NETWORK" --dir "$V2" --name "$WALLET" \
            --password-env ZOKA_CLI_PASSWORD
        ;;
    restore-wallet)
        wallet_exists && die "Wallet '$WALLET' already exists. Use --wallet <other name>."
        phrase="$(read_password "Type your 24 words, separated by spaces (hidden)")"
        p1="$(read_password "Choose a password for this computer")"
        p2="$(read_password "Repeat the password")"
        [ "$p1" = "$p2" ] || die "The two passwords are different. Nothing was restored."
        [ ${#p1} -ge 8 ] || die "Use at least 8 characters. Nothing was restored."
        echo "Restored. Your address:"
        ZOKA_CLI_PHRASE="$phrase" ZOKA_CLI_PASSWORD="$p1" "$ZOKA" quantic-pq-wallet restore --network "$NETWORK" \
            --dir "$V2" --name "$WALLET" --phrase-env ZOKA_CLI_PHRASE --password-env ZOKA_CLI_PASSWORD
        ;;
    address)
        echo "Your address (share this one to get paid):"
        address
        echo
        ;;
    view-key)
        wallet_exists || die "There is no wallet '$WALLET' yet."
        p="$(read_password "Password of wallet '$WALLET'")"
        echo "Your view key. It shows every payment and the balance of this wallet (paste it in"
        echo "https://zokascan.cc/wallet, which reads it inside your browser); it cannot spend."
        echo "Share it only as you would a bank statement: it cannot be revoked."
        ZOKA_CLI_PASSWORD="$p" "$ZOKA" quantic-pq-wallet show-view-key --network "$NETWORK" --dir "$V2" \
            --name "$WALLET" --password-env ZOKA_CLI_PASSWORD --json | sed -n 's/.*"view_key":"\([^"]*\)".*/\1/p'
        ;;
    mine)
        mine
        ;;
    autostart)
        autostart
        ;;
    balance)
        wallet_exists || die "There is no wallet '$WALLET' yet."
        # Con el nodo andando, la copia de la wallet se pone al día con él antes de contar.
        peer=()
        if node_online && [ -f "$DESCRIPTOR" ]; then
            peer=(--peer "$DESCRIPTOR")
        else
            echo "The node is not running: this is the balance as of the last time it ran."
        fi
        p="$(read_password "Password of wallet '$WALLET'")"
        ZOKA_CLI_PASSWORD="$p" "$ZOKA" quantic-pq-wallet balance --network "$NETWORK" --dir "$V2" --name "$WALLET" \
            --password-env ZOKA_CLI_PASSWORD --api "$API" "${peer[@]}"
        ;;
    status)
        node_online || { echo "The node is not running."; exit 0; }
        s="$(api_get /status)"
        # La altura de la punta va después de su hash; la del checkpoint, no.
        height="$(grep -o '"hash":"[0-9a-f]*","height":[0-9]*' <<< "$s" | head -1 | cut -d: -f3)"
        echo "Height ${height:-?} | peers $(status_number outbound "$s") out, $(status_number inbound "$s") in" \
            "| XMRig miners $(status_number miners "$s") | blocks found this session $(status_number blocks_found "$s")"
        ;;
    node-id)
        # Los dos identificadores públicos que fija el manifiesto de una semilla.
        # Crea la identidad del nodo si todavía no existe; no muestra secretos.
        mkdir -p "$NODE_DIR"
        secret="$(node_secret)"
        out="$(ZOKA_PQ_PASSWORD="$secret" "$ZOKA" quantic-pq-node identity --network "$NETWORK" \
            --identity "$NODE_DIR/node.identity")" || die "Could not read the node's identity: $out"
        echo "Endpoint id:   $(sed -n 's/.*"endpoint_id": *"\([0-9a-f]*\)".*/\1/p' <<< "$out")"
        echo "ML-DSA key id: $(sed -n 's/.*"ml_dsa_key_id": *"\([0-9a-f]*\)".*/\1/p' <<< "$out")"
        echo "Both are public. To offer this node as a seed, keep it running all day and send them to the Zoka team."
        ;;
    *)
        cat << 'EOF'
Zoka Quantic Testnet v2 CLI miner. Commands:
  create-wallet   a new wallet (shows its 24 words once)
  restore-wallet  a wallet from its 24 words
  address         the address you share to get paid
  view-key        a key that shows your balance (zokascan.cc/wallet) and cannot spend
  mine            run the node, and XMRig if it is next to this script (--threads n); restarts them if they stop
  autostart       run mining as a systemd user service that starts with the system (autostart --off undoes it)
  balance         your balance
  status          height, peers and miners
  node-id         this node's public ids, to offer it as a seed
Options: --wallet <name> (default: miner), --threads n, --stratum-port n (default: 3333),
--xmrig <path to xmrig>, --tor 127.0.0.1:9050 (optional Tor mode, through a running Tor).
Data lives in ./data next to this script.
EOF
        ;;
esac
