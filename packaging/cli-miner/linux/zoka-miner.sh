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
# Uso: ./zoka-miner.sh <create-wallet | restore-wallet | address | mine | balance | status> [--wallet nombre] [--threads n] [--stratum-port n] [--xmrig ruta]
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

COMMAND="${1:-help}"
shift || true
WALLET="miner"
THREADS=0
while [ $# -gt 0 ]; do
    case "$1" in
        --wallet) WALLET="$2"; shift 2 ;;
        --threads) THREADS="$2"; shift 2 ;;
        --stratum-port) STRATUM_PORT="$2"; shift 2 ;;
        --xmrig) XMRIG="$2"; shift 2 ;;
        *) echo "Unknown option: $1" >&2; exit 2 ;;
    esac
done
STRATUM="127.0.0.1:$STRATUM_PORT"

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

NODE_PID=""
stop_node() {
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

mine() {
    wallet_exists || die "There is no wallet '$WALLET' yet. Run ./zoka-miner.sh create-wallet (or restore-wallet) first."
    node_online && die "A node is already running on $API. Close it first."
    local full short cores miner log err_log secret
    secret="$(node_secret)"
    full="$(address --full)"
    short="$(address)"
    cores="$(nproc)"
    if [ "$THREADS" -le 0 ]; then THREADS=$((cores > 1 ? cores - 1 : 1)); fi
    miner="$(find_xmrig)"

    # El nodo nunca pisa un archivo: el descriptor de la corrida anterior se borra.
    rm -f "$DESCRIPTOR"
    log="$V2/node.log"
    err_log="$V2/node-errors.log"
    ZOKA_PQ_PASSWORD="$secret" \
        "$ZOKA" quantic-pq-node serve --network "$NETWORK" --db "$NODE_DIR/db" \
        --identity "$NODE_DIR/node.identity" --api "127.0.0.1:$API_PORT" --descriptor-out "$DESCRIPTOR" \
        --stratum "$STRATUM" --mine-to "$full" > "$log" 2> "$err_log" &
    NODE_PID=$!
    trap stop_node EXIT
    trap 'exit 130' INT TERM

    echo "Starting the node and joining the network..."
    local deadline=$((SECONDS + 180))
    until node_online; do
        kill -0 "$NODE_PID" 2> /dev/null || die "The node stopped. Details in $err_log"
        [ $SECONDS -lt $deadline ] || die "The node did not start in 3 minutes. Details in $err_log"
        sleep 2
    done
    # Publica la dirección corta en la red, así cualquiera puede pagarte con ella.
    "$ZOKA" quantic-pq-wallet publish-address --network "$NETWORK" --dir "$V2" --name "$WALLET" --api "$API" > /dev/null 2>&1 \
        || echo "Your short address could not be published yet; the full one always works."

    echo
    echo "Mining rewards go to wallet '$WALLET': $short"
    echo "The node's log is $log"
    if [ -n "$miner" ]; then
        echo "Mining with XMRig, $THREADS of $cores CPU threads (--threads n changes it). Ctrl+C stops both."
        echo
        local code=0
        "$miner" -o "$STRATUM" -u "$WALLET" -p x -a rx/0 -t "$THREADS" || code=$?
        echo "XMRig stopped (exit code $code)."
    else
        echo "XMRig was not found next to this script, so the node is waiting for miners. Start it with:"
        echo "  xmrig -o $STRATUM -u $WALLET -p x -a rx/0 -t $THREADS"
        echo "Ctrl+C stops the node."
        echo
        tail -n +1 -f --pid="$NODE_PID" "$log"
        echo "The node stopped. Details in $err_log"
    fi
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
    mine)
        mine
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
    *)
        cat << 'EOF'
Zoka Quantic Testnet v2 CLI miner. Commands:
  create-wallet   a new wallet (shows its 24 words once)
  restore-wallet  a wallet from its 24 words
  address         the address you share to get paid
  mine            run the node, and XMRig if it is next to this script (--threads n)
  balance         your balance
  status          height, peers and miners
Options: --wallet <name> (default: miner), --threads n, --stratum-port n (default: 3333),
--xmrig <path to xmrig>.
Data lives in ./data next to this script.
EOF
        ;;
esac
