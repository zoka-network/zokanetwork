FROM rust:1-bookworm AS builder

WORKDIR /app

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential \
        clang \
        cmake \
        pkg-config \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY docs ./docs
COPY trusted-setup ./trusted-setup

RUN cargo build --release --features randomx --bin zoka --bin zokahd

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        libstdc++6 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/zoka /usr/local/bin/zoka
COPY --from=builder /app/target/release/zokahd /usr/local/bin/zokahd
COPY --from=builder /app/docs /app/docs
COPY --from=builder /app/trusted-setup /app/trusted-setup

ENV ZOKA_NETWORK=mainnet
ENV ZOKA_API_BIND=0.0.0.0:3000
ENV ZOKA_NODE_KEY_PATH=/data/node_key
ENV ZOKA_TRUSTED_SETUP_DIR=/app/trusted-setup
ENV ZOKA_EXPECTED_SETUP_DIGEST=219fdb5d5e47e115ca899d13a6de284df03c86eaaef05eb4ff4adb395cfb0d8e
ENV ZOKA_MINER_ADDRESS=
ENV ZOKA_MINE_INTERVAL_SECS=30
ENV ZOKA_MINE_BLOCKS=1
ENV ZOKA_MINING_THREADS=1
ENV ZOKA_SYNC_WARMUP_SECS=30
ENV ZOKA_SYNC_REBROADCAST_RECENT_BLOCKS=32
ENV RUST_LOG=info

WORKDIR /data

CMD ["sh", "-lc", "db_path=\"${ZOKA_CHAIN_DB_PATH:-/data/${ZOKA_NETWORK:-mainnet}/blockchain.db}\"; mkdir -p \"$(dirname \"$db_path\")\"; sync_args=\"--sync-warmup-secs ${ZOKA_SYNC_WARMUP_SECS:-30} --sync-rebroadcast-recent-blocks ${ZOKA_SYNC_REBROADCAST_RECENT_BLOCKS:-32}\"; mine_args=\"--no-mine\"; if [ -n \"${ZOKA_MINER_ADDRESS:-}\" ]; then mine_args=\"--address ${ZOKA_MINER_ADDRESS} --mine-interval-secs ${ZOKA_MINE_INTERVAL_SECS:-30} --mine-blocks ${ZOKA_MINE_BLOCKS:-1} --threads ${ZOKA_MINING_THREADS:-1}\"; fi; exec zoka fullnode --network ${ZOKA_NETWORK:-mainnet} --password-env ZOKA_NODE_PASSWORD --db \"$db_path\" --listen /ip4/0.0.0.0/tcp/30303 --allow-isolated-start $sync_args $mine_args"]
