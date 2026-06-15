# Multi-stage build for the trading-api binary (used here for capture-only
# market-data ingestion on Hetzner). Build ON the x86_64 host so the arch
# matches — no cross-compilation from the arm64 dev machine.
#
#   docker build -f deploy/Dockerfile -t trading-capture:latest .
#
# Migrations are embedded at compile time via sqlx::migrate!("../../migrations"),
# so the runtime image does not need the migrations/ directory; with
# RUN_MIGRATIONS=true the binary applies them to the target DB on startup.

# Latest stable 1.x: the workspace's rust-version=1.78 is only a MINIMUM, and the
# current dependency tree pulls crates that need edition2024 (stabilized in Rust
# 1.85), so the builder must be newer than 1.78. Tracking stable 1.x keeps the
# image building as dependencies bump their MSRV.
FROM rust:1-slim-bookworm AS builder
WORKDIR /build
# Build deps: rustls is used for TLS (no OpenSSL needed), but the build still
# needs pkg-config/clang-free std crates; slim already covers them. Copy the
# whole workspace so the embedded-migration macro sees migrations/ at build time.
COPY . .
RUN cargo build --release --bin trading-api

FROM debian:bookworm-slim AS runtime
# rustls verifies against the system root store at runtime → needs ca-certificates.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /build/target/release/trading-api /usr/local/bin/trading-api
# API/health server port (API_PORT, default 8080).
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/trading-api"]
