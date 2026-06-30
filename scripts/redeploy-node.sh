#!/usr/bin/env bash
#
# Redeploy the Zoka node on a droplet from the latest pushed source.
# Run this ON each droplet (BOOT 159.223.119.216, VPS2 157.230.5.103) as root.
#
#   curl -fsSL <raw-url>/scripts/redeploy-node.sh -o redeploy-node.sh
#   bash redeploy-node.sh
#
# It: ensures build swap (the droplets are RAM-light), updates the repo to the
# target branch, builds the `zoka` core and `zokahd` wrapper release binaries
# with a low-memory job count, installs both, restarts the service, and verifies
# /chain/height.
#
# Confirm these match your droplet before running:
set -euo pipefail

REPO_DIR="${REPO_DIR:-/root/ZOKA}"        # where the source is (cloned if missing)
REPO_URL="${REPO_URL:-https://github.com/ZokaNetwork/ZokaNetwork.git}"
BRANCH="${BRANCH:-codex/network-sync-encrypted-send}"
SERVICE="${SERVICE:-zoka-bootstrap.service}"
BIN_NAME="${BIN_NAME:-zokahd}"
BIN_DEST="${BIN_DEST:-/usr/local/bin/zokahd}"
CORE_BIN_NAME="${CORE_BIN_NAME:-zoka}"
CORE_BIN_DEST="${CORE_BIN_DEST:-/usr/local/bin/zoka}"
RPC_PORT="${RPC_PORT:-3000}"
BUILD_JOBS="${BUILD_JOBS:-1}"             # 1 = lowest memory; raise if RAM allows

log() { printf '\n=== %s ===\n' "$*"; }

# 1) Swap (release builds of rocksdb/randomx/bellman need several GB).
if [ "$(free -m | awk '/^Mem:/{print $2}')" -lt 3000 ] && ! swapon --show | grep -q .; then
  log "Adding 4G build swap (low RAM detected)"
  fallocate -l 4G /swapfile || dd if=/dev/zero of=/swapfile bs=1M count=4096
  chmod 600 /swapfile && mkswap /swapfile && swapon /swapfile
  CLEANUP_SWAP=1
fi

# 2) Get the latest source on the target branch.
if [ -d "$REPO_DIR/.git" ]; then
  log "Updating $REPO_DIR to $BRANCH"
  git -C "$REPO_DIR" fetch --depth 1 origin "$BRANCH"
  git -C "$REPO_DIR" checkout -B "$BRANCH" "origin/$BRANCH"
else
  log "Cloning $REPO_URL into $REPO_DIR ($BRANCH)"
  git clone --depth 1 --branch "$BRANCH" "$REPO_URL" "$REPO_DIR"
fi

# 3) Build the node wrapper and the core CLI. `zokahd` delegates to a sibling
# `zoka` binary, so installing only the wrapper can leave old consensus code
# in production.
log "Building $CORE_BIN_NAME + $BIN_NAME (release, -j$BUILD_JOBS) - this is slow on a small droplet"
cd "$REPO_DIR"
cargo build --release --bin "$CORE_BIN_NAME" --bin "$BIN_NAME" -j "$BUILD_JOBS"

# 4) Swap in the new binaries and restart.
log "Installing $CORE_BIN_NAME + $BIN_NAME and restarting $SERVICE"
systemctl stop "$SERVICE" || true
install -m 0755 "target/release/$CORE_BIN_NAME" "$CORE_BIN_DEST"
install -m 0755 "target/release/$BIN_NAME" "$BIN_DEST"
systemctl start "$SERVICE"

# 5) Verify.
sleep 4
log "Health check"
systemctl --no-pager status "$SERVICE" | head -4 || true
curl -fsS "http://127.0.0.1:${RPC_PORT}/chain/height" && echo
# Confirm the new block field is live (B7):
curl -fsS "http://127.0.0.1:${RPC_PORT}/blocks/$(curl -fsS http://127.0.0.1:${RPC_PORT}/chain/height | grep -oE '[0-9]+' | head -1)" \
  | grep -o 'private_nullifier_count' && echo "OK: private_nullifier_count present" || echo "note: private_nullifier_count not found (check block)"

if [ "${CLEANUP_SWAP:-0}" = "1" ]; then
  log "Removing build swap"
  swapoff /swapfile && rm -f /swapfile
fi

log "Done. Repeat on the other droplet."
