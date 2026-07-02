#!/usr/bin/env bash
set -euo pipefail

# Comma-separated list of reference RPC endpoints (tried in order until one responds).
BOOT_RPC="${BOOT_RPC:-http://159.223.119.216:3000,http://157.230.5.103:3000}"
LOCAL_RPC="${LOCAL_RPC:-http://127.0.0.1:3000}"
GAP_THRESHOLD="${GAP_THRESHOLD:-2}"
MAX_PUSH="${MAX_PUSH:-64}"
CURL_TIMEOUT="${CURL_TIMEOUT:-30}"

require_command() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "missing required command: $1" >&2
    exit 1
  }
}

rpc_height() {
  curl --fail --silent --show-error \
    --max-time "$CURL_TIMEOUT" \
    "$1/chain/height" |
    python3 -c 'import json, sys; print(json.load(sys.stdin)["height"])'
}

# Try each comma-separated BOOT_RPC endpoint until one succeeds.
# Prints "<endpoint> <height>" on success so the caller can capture both
# via `read` from a single command substitution (a function invoked as
# `x="$(fn)"` runs in a subshell, so side-effect variable assignments
# inside it never reach the caller).
reference_rpc_height() {
  IFS=',' read -ra endpoints <<< "$BOOT_RPC"
  for ep in "${endpoints[@]}"; do
    ep="${ep// /}"
    if h="$(rpc_height "$ep" 2>/dev/null)"; then
      echo "$ep $h"
      return 0
    fi
  done
  echo "all reference endpoints unreachable: $BOOT_RPC" >&2
  return 1
}

require_command curl
require_command flock
require_command python3

exec 9>/tmp/zoka-sync-watchdog.lock
if ! flock --nonblock 9; then
  echo "another zoka sync watchdog run is active"
  exit 0
fi

read -r ACTIVE_BOOT_RPC boot_height <<< "$(reference_rpc_height)"
local_height="$(rpc_height "$LOCAL_RPC")"
gap=$((boot_height - local_height))

if (( gap <= GAP_THRESHOLD )); then
  echo "sync healthy: local=$local_height reference=$boot_height gap=$gap (via $ACTIVE_BOOT_RPC)"
  exit 0
fi

target_height="$boot_height"
if (( target_height - local_height > MAX_PUSH )); then
  target_height=$((local_height + MAX_PUSH))
fi

echo "sync catch-up: local=$local_height reference=$boot_height pushing=$((target_height - local_height)) (via $ACTIVE_BOOT_RPC)"
for ((height = local_height + 1; height <= target_height; height++)); do
  block="$(curl --fail --silent --show-error \
    --max-time "$CURL_TIMEOUT" \
    "$ACTIVE_BOOT_RPC/blocks/$height/raw")"
  result="$(curl --fail --silent --show-error \
    --max-time "$CURL_TIMEOUT" \
    --header 'Content-Type: application/json' \
    --data-binary "$block" \
    "$LOCAL_RPC/sync/block")"
  status="$(python3 -c 'import json, sys; print(json.load(sys.stdin)["status"])' <<<"$result")"
  echo "height=$height status=$status"
done

final_height="$(rpc_height "$LOCAL_RPC")"
echo "sync catch-up complete: local=$final_height reference_at_start=$boot_height"
