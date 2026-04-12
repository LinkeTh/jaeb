# ─── Stage 1: builder ──────────────────────────────────────────────────────
FROM rust:1.94-bookworm AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev protobuf-compiler && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY . .
RUN cargo build --release -p observability-stack

# ─── Stage 2: runtime ──────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libssl3 && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder \
    /app/target/release/observability-stack \
    /usr/local/bin/observability-stack

EXPOSE 3000
ENTRYPOINT ["/usr/local/bin/observability-stack"]
