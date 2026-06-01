#!/usr/bin/env bash
set -euo pipefail

service_name="${1:?usage: migrate-node-secret-env.sh <systemd-service-name>}"
unit="/etc/systemd/system/${service_name}.service"
env_dir="/etc/zoka"
env_file="${env_dir}/node.env"

test -f "$unit"
install -d -m 0700 "$env_dir"

if grep -q '^Environment=ZOKA_NODE_PASSWORD=' "$unit"; then
  cp "$unit" "${unit}.before-envfile-$(date +%s)"
  secret="$(sed -n 's/^Environment=ZOKA_NODE_PASSWORD=//p' "$unit" | head -n1)"
  test -n "$secret"
  umask 077
  printf 'ZOKA_NODE_PASSWORD=%s\n' "$secret" > "$env_file"
  chmod 0600 "$env_file"
  sed -i '/^Environment=ZOKA_NODE_PASSWORD=/d' "$unit"
fi

test -s "$env_file"
chmod 0600 "$env_file"
grep -q '^EnvironmentFile=/etc/zoka/node.env$' "$unit" ||
  sed -i '/^\[Service\]$/a EnvironmentFile=/etc/zoka/node.env' "$unit"

systemctl daemon-reload
systemctl restart "$service_name"
sleep 8
systemctl is-active "$service_name"
stat -c '%a %n' "$env_file"
curl --fail --silent --show-error --max-time 8 http://127.0.0.1:3000/health
