# ── Build stage ───────────────────────────────────────────────────────────────
FROM rust:1.82-slim AS builder

# Install libssl for reqwest's TLS (rustls-tls is used but some crates pull in
# openssl-sys at compile time on Linux).
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Copy the full workspace so cargo can resolve workspace dependencies.
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
# The social-server binary only needs these two workspace crates at link time.
# We don't copy apps/ or vendor/ to keep the image small; they are not deps.

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
EXPOSE 3000

ENTRYPOINT ["./awidat-social-server"]
