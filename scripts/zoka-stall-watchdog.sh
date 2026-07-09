#!/bin/bash
#
# Restarts the local node only when it's genuinely stuck (local height hasn't
# moved across STALL_THRESHOLD consecutive runs), not just when it's behind a
# reference peer. A node that's behind but actively backfilling via IBD will
# catch up on its own -- restarting it mid-catch-up is pure disruption.
#
# Replaces the older per-droplet watchdogs (zoka-bootstrap-watchdog.sh on
# BOOT, zoka-watchdog.sh on VPS2), which restarted on gap > 10 regardless of
# whether the node was still making progress, and targeted a stale unit name
# (zoka-bootstrap.service / zoka-rpc.service) left over from before the
# project switched to zoka-node.service via deploy-mainnet.yml. That stale
# unit was never touched by later deploys, so it sat there dormant until this
# cron fired it -- starting a second zoka process that fought the real
# zoka-node.service for the RocksDB lock and crash-looped it for hours
# (see incident 2026-07-09).
#
# Install (run on each droplet as root):
#   cp zoka-stall-watchdog.sh /usr/local/bin/zoka-stall-watchdog.sh
#   chmod +x /usr/local/bin/zoka-stall-watchdog.sh
#   echo '*/5 * * * * REFERENCE_RPC=http://<other-seed-ip>:3000 /usr/local/bin/zoka-stall-watchdog.sh' | crontab -
#
# On BOOT (159.223.119.216), set REFERENCE_RPC to VPS2's RPC.
# On VPS2 (157.230.5.103), set REFERENCE_RPC to BOOT's RPC.

set -uo pipefail

LOCAL_RPC="${LOCAL_RPC:-http://127.0.0.1:3000}"
REFERENCE_RPC="${REFERENCE_RPC:?set REFERENCE_RPC to the other seed RPC URL}"
SERVICE="${SERVICE:-zoka-node.service}"
GAP_THRESHOLD="${GAP_THRESHOLD:-10}"
STALL_THRESHOLD="${STALL_THRESHOLD:-3}"   # consecutive no-progress runs (>= ~15 min at */5) before restarting
STATE_FILE="${STATE_FILE:-/var/lib/zoka-stall-watchdog.state}"
LOG_FILE="${LOG_FILE:-/var/log/zoka-stall-watchdog.log}"

log() { echo "$(date -u +%Y-%m-%dT%H:%M:%SZ): $*" >>"$LOG_FILE"; }

rpc_height() {
  curl -sf --max-time 5 "$1/chain/height" | python3 -c 'import json,sys; print(json.load(sys.stdin)["height"])' 2>/dev/null
}

local_height="$(rpc_height "$LOCAL_RPC")"
if [ -z "$local_height" ]; then
  log "local RPC unreachable, skipping this run"
  exit 0
fi

reference_height="$(rpc_height "$REFERENCE_RPC")"
if [ -z "$reference_height" ]; then
  log "local=$local_height reference=unreachable, skipping this run"
  exit 0
fi

gap=$((reference_height - local_height))

prev_height=0
stall_count=0
if [ -f "$STATE_FILE" ]; then
  read -r prev_height stall_count < "$STATE_FILE" 2>/dev/null || true
fi

if [ "$gap" -le "$GAP_THRESHOLD" ]; then
  log "healthy: local=$local_height reference=$reference_height gap=$gap"
  echo "$local_height 0" > "$STATE_FILE"
  exit 0
fi

if [ "$local_height" -gt "$prev_height" ]; then
  log "behind but progressing: local=$local_height reference=$reference_height gap=$gap (prev=$prev_height)"
  echo "$local_height 0" > "$STATE_FILE"
  exit 0
fi

stall_count=$((stall_count + 1))
log "no progress: local=$local_height reference=$reference_height gap=$gap stall_count=$stall_count/$STALL_THRESHOLD"

if [ "$stall_count" -ge "$STALL_THRESHOLD" ]; then
  log "RESTART: $SERVICE stuck at height=$local_height for $stall_count consecutive checks"
  systemctl restart "$SERVICE"
  echo "$local_height 0" > "$STATE_FILE"
else
  echo "$local_height $stall_count" > "$STATE_FILE"
fi
