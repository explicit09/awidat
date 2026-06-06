# ── Build stage ───────────────────────────────────────────────────────────────
FROM rust:1.95-slim AS builder

# Install libssl for reqwest's TLS (rustls-tls is used but some crates pull in
# openssl-sys at compile time on Linux).
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Copy the full workspace so cargo can resolve workspace dependencies.
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
# Cargo loads every workspace member manifest before building a package.
COPY vendor/ vendor/
COPY apps/desktop/src-tauri/ apps/desktop/src-tauri/
# We don't copy the full desktop app; it is not a social-server dep.

RUN cargo build --release -p awidat-social-server

# ── Runtime stage ─────────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /build/target/release/awidat-social-server ./
# Copy migrations so the boot-time migrator can find them.
COPY crates/social/migrations/ ./migrations/

ENV BIND_ADDR="0.0.0.0:3000"
ENV SOCIAL_FIRING_ENABLED="false"
ENV SOCIAL_MIGRATIONS_DIR="/app/migrations"
EXPOSE 3000

ENTRYPOINT ["./awidat-social-server"]
