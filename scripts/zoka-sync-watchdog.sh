#!/usr/bin/env bash
set -euo pipefail

BOOT_RPC="${BOOT_RPC:-http://159.223.119.216:3000}"
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

require_command curl
require_command flock
require_command python3

exec 9>/tmp/zoka-sync-watchdog.lock
if ! flock --nonblock 9; then
  echo "another zoka sync watchdog run is active"
  exit 0
fi

boot_height="$(rpc_height "$BOOT_RPC")"
local_height="$(rpc_height "$LOCAL_RPC")"
gap=$((boot_height - local_height))

if (( gap <= GAP_THRESHOLD )); then
  echo "sync healthy: local=$local_height boot=$boot_height gap=$gap"
  exit 0
fi

target_height="$boot_height"
if (( target_height - local_height > MAX_PUSH )); then
  target_height=$((local_height + MAX_PUSH))
fi

echo "sync catch-up: local=$local_height boot=$boot_height pushing=$((target_height - local_height))"
for ((height = local_height + 1; height <= target_height; height++)); do
  block="$(curl --fail --silent --show-error \
    --max-time "$CURL_TIMEOUT" \
    "$BOOT_RPC/blocks/$height/raw")"
  result="$(curl --fail --silent --show-error \
    --max-time "$CURL_TIMEOUT" \
    --header 'Content-Type: application/json' \
    --data-binary "$block" \
    "$LOCAL_RPC/sync/block")"
  status="$(python3 -c 'import json, sys; print(json.load(sys.stdin)["status"])' <<<"$result")"
  echo "height=$height status=$status"
done

final_height="$(rpc_height "$LOCAL_RPC")"
echo "sync catch-up complete: local=$final_height boot_at_start=$boot_height"
