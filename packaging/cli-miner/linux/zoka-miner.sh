#!/usr/bin/env bash
# Minero de Zoka Quantic por línea de comandos, para Linux.
#
# Todo queda dentro de la carpeta del paquete: los datos (cadena, wallets, Tor)
# en ./data. Usa puertos propios (API 3180, P2P 28181, Tor 19150), así que puede
# convivir con ZSilent en la misma máquina; pero no tiene sentido minar con los
# dos a la vez: se reparten la CPU.
#
# Uso: ./zoka-miner.sh <create-wallet | restore-wallet | address | mine | balance | xmrig-node | status> [--wallet nombre] [--threads n]
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ZOKA="$ROOT/zoka"
NETWORK="quantic-testnet"
DATA="$ROOT/data"
API_BIND="127.0.0.1:3180"
API="http://$API_BIND"
LISTEN="/ip4/127.0.0.1/tcp/28181"
TOR_SOCKS="127.0.0.1:19150"
STRATUM_PORT=3333

COMMAND="${1:-help}"
shift || true
WALLET="miner"
THREADS=0
while [ $# -gt 0 ]; do
    case "$1" in
        --wallet) WALLET="$2"; shift 2 ;;
        --threads) THREADS="$2"; shift 2 ;;
        --stratum-port) STRATUM_PORT="$2"; shift 2 ;;
        *) echo "Unknown option: $1" >&2; exit 2 ;;
    esac
done

export ZOKA_HOME="$DATA"
mkdir -p "$DATA"

die() { echo "Error: $*" >&2; exit 1; }

read_password() {
    local prompt="$1" value
    read -r -s -p "$prompt: " value
    echo >&2
    printf '%s' "$value"
}

wallet_exists() { [ -f "$DATA/.zoka/$NETWORK/quantic-wallets/$WALLET.json" ]; }

short_address() {
    "$ZOKA" quantic-wallet address --network "$NETWORK" --name "$WALLET" | tr -d '\r\n'
}

node_online() { curl -fsS --max-time 3 "$API/quantic/info" > /dev/null 2>&1; }

TOR_PID=""
stop_tor() {
    if [ -n "$TOR_PID" ]; then kill "$TOR_PID" 2> /dev/null || true; fi
    echo "Node and Tor stopped."
}

# Tor propio, solo para salir: el nodo llega a los demás por sus direcciones
# .onion. Sin Tor, un minero en otra casa no podría publicar sus bloques. El
# registro va por la salida estándar a un archivo: "Log notice file <ruta>" se
# rompe con una ruta con espacios.
start_tor() {
    command -v tor > /dev/null || die "Tor is not installed. On Debian or Ubuntu: sudo apt install tor"
    local tor_data="$DATA/tor" tor_log="$DATA/tor/tor.log"
    mkdir -p "$tor_data"
    chmod 700 "$tor_data"
    cat > "$tor_data/torrc" << EOF
SocksPort $TOR_SOCKS
DataDirectory "$tor_data"
Log notice stdout
EOF
    rm -f "$tor_log"
    tor -f "$tor_data/torrc" > "$tor_log" 2>&1 &
    TOR_PID=$!
    echo "Starting Tor (this takes up to a minute)..."
    local deadline=$((SECONDS + 180))
    while [ $SECONDS -lt $deadline ]; do
        sleep 2
        kill -0 "$TOR_PID" 2> /dev/null || die "Tor stopped. See $tor_log"
        if grep -q "Bootstrapped 100%" "$tor_log" 2> /dev/null; then
            echo "Tor is ready."
            return 0
        fi
    done
    kill "$TOR_PID" 2> /dev/null || true
    die "Tor did not finish starting in 3 minutes. Check your internet connection; details in $tor_log"
}

start_node() {
    wallet_exists || die "There is no wallet '$WALLET' yet. Run ./zoka-miner.sh create-wallet (or restore-wallet) first."
    node_online && die "A node is already running on $API. Close it first."
    local address
    address="$(short_address)"
    start_tor
    trap stop_tor EXIT
    echo
    echo "Mining rewards go to wallet '$WALLET': $address"
    echo "Press Ctrl+C to stop."
    echo
    ZOKA_API_BIND="$API_BIND" "$ZOKA" fullnode --network "$NETWORK" --quantic-mine-to "$address" \
        --listen "$LISTEN" --tor-proxy "$TOR_SOCKS" --min-mine-peers 1 "$@"
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
        ZOKA_CLI_PASSWORD="$p1" "$ZOKA" quantic-wallet new --network "$NETWORK" --name "$WALLET" --password-env ZOKA_CLI_PASSWORD
        ;;
    restore-wallet)
        wallet_exists && die "Wallet '$WALLET' already exists. Use --wallet <other name>."
        phrase="$(read_password "Type your 24 words, separated by spaces (hidden)")"
        p1="$(read_password "Choose a password for this computer")"
        p2="$(read_password "Repeat the password")"
        [ "$p1" = "$p2" ] || die "The two passwords are different. Nothing was restored."
        ZOKA_CLI_PHRASE="$phrase" ZOKA_CLI_PASSWORD="$p1" "$ZOKA" quantic-wallet restore --network "$NETWORK" \
            --name "$WALLET" --phrase-env ZOKA_CLI_PHRASE --password-env ZOKA_CLI_PASSWORD
        ;;
    address)
        echo "Your address (share this one to get paid):"
        short_address
        echo
        ;;
    mine)
        cores="$(nproc)"
        if [ "$THREADS" -le 0 ]; then THREADS=$((cores > 1 ? cores - 1 : 1)); fi
        echo "Mining with $THREADS of $cores CPU threads (use --threads n to change it)."
        start_node --threads "$THREADS"
        ;;
    xmrig-node)
        echo "Node with a Stratum server for XMRig on port $STRATUM_PORT; the node itself does not mine."
        echo "If another program (for example ZSilent's Stratum) already uses it, pick another with --stratum-port <port>."
        echo "XMRig on this machine:  xmrig -o 127.0.0.1:$STRATUM_PORT -u <your address> -p x -a rx/0"
        echo "XMRig on another machine of your network: use this machine's local IP instead of 127.0.0.1."
        start_node --stratum "0.0.0.0:$STRATUM_PORT" --no-mine
        ;;
    balance)
        node_online || die "The node is not running. Start mining first (./zoka-miner.sh mine), then check the balance from another terminal."
        p="$(read_password "Password of wallet '$WALLET'")"
        ZOKA_CLI_PASSWORD="$p" "$ZOKA" quantic-wallet balance --network "$NETWORK" --name "$WALLET" \
            --password-env ZOKA_CLI_PASSWORD --rpc "$API"
        ;;
    status)
        node_online || { echo "The node is not running."; exit 0; }
        curl -fsS --max-time 5 "$API/quantic/info" | sed -E 's/.*"height":([0-9]+).*/Height \1/'
        curl -fsS --max-time 5 "$API/quantic/mining" | sed -E 's/.*"blocks_mined":([0-9]+).*"hashrate_hps":([0-9]+).*/Your blocks this session \1 · hashrate \2 H\/s/'
        ;;
    *)
        cat << 'EOF'
Zoka Quantic CLI miner (testnet). Commands:
  create-wallet   a new wallet (shows its 24 words once)
  restore-wallet  a wallet from its 24 words
  address         the address you share to get paid
  mine            run the node and mine (--threads n)
  balance         your balance (with the node running)
  xmrig-node      run the node for XMRig instead of mining
  status          height and your blocks
Options: --wallet <name> (default: miner), --threads n, --stratum-port n (default: 3333).
Data lives in ./data next to this script.
EOF
        ;;
esac
