#!/usr/bin/env bash
# Campaña aislada: solo fuentes y datos sintéticos, nunca una billetera real.
set -euo pipefail
export PATH="${HOME}/.cargo/bin:$PATH"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
export RAYON_NUM_THREADS="${RAYON_NUM_THREADS:-1}"
source_dir=$(cd "$(dirname "$0")/.." && pwd)
cd "$source_dir"
seconds=${FUZZ_SECONDS:-1200}
seed=${FUZZ_SEED:-20261110}
out=${FUZZ_RESULTS:-"$source_dir/fuzz/results"}
mkdir -p "$out"
[[ "$seconds" =~ ^[1-9][0-9]*$ && "$seed" =~ ^[0-9]+$ ]] || exit 2

# Install tools before running this script. Keep the resolved toolchain and
# dependency lock with the results so a finding can be reproduced later.
rustc +nightly -Vv > "$out/toolchain.txt"
cargo fuzz --version >> "$out/toolchain.txt"
sha256sum fuzz/Cargo.lock > "$out/lock.sha256"
printf 'target\tseed\tseconds\tstarted_utc\tfinished_utc\texit_code\n' > "$out/runs.tsv"
cargo run --locked --release --manifest-path crates/zoka-pq-pool/Cargo.toml \
  --features fuzzing --example fuzz_seeds -- "$source_dir/fuzz/corpus" \
  > "$out/seeds.log" 2>&1
cargo +nightly fuzz build > "$out/build.log" 2>&1

failed=0
for target in text envelope wallet_state scan_page proof transaction; do
  started=$(date -u +%FT%TZ)
  status=0
  cargo +nightly fuzz run "$target" -- \
    -seed="$seed" -max_total_time="$seconds" -max_len=4000000 \
    -timeout=60 -rss_limit_mb=1800 -print_final_stats=1 \
    > "$out/$target.log" 2>&1 || status=$?
  printf '%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$target" "$seed" "$seconds" "$started" "$(date -u +%FT%TZ)" "$status" \
    >> "$out/runs.tsv"
  if (( status != 0 )); then
    failed=1
  fi
done
printf '%s\n' "$failed" > "$out/exit-code"
exit "$failed"
