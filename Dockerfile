# ---------- Stage 1: Builder ----------
FROM rust:1.82-slim-bookworm AS builder

# Build deps
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev \
 && rm -rf /var/lib/apt/lists/*

# Pin nightly for stability (adjust date if needed)
ARG RUST_TOOLCHAIN=nightly-2024-09-10
RUN rustup toolchain install ${RUST_TOOLCHAIN} && rustup default ${RUST_TOOLCHAIN}

WORKDIR /app

# Cache deps
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src && echo "fn main() {}" > src/main.rs
RUN cargo build --release --locked
RUN rm -rf src

# Actual sources
COPY src ./src
COPY migrations ./migrations

# Build the real binary
RUN cargo build --release --locked && ls -lh target/release/ai-toolkit

# ---------- Stage 2: Runtime ----------
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libreoffice-writer libreoffice-core libreoffice-common \
    libssl3 \
    fonts-liberation fonts-dejavu \
    curl \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Binary + migrations
COPY --from=builder /app/target/release/ai-toolkit /app/ai-toolkit
COPY --from=builder /app/migrations /app/migrations

# App state dir
RUN mkdir -p /app/storage && chmod 755 /app/ai-toolkit

# Railway sets $PORT; expose 8080 for local clarity (optional)
EXPOSE 8080
ENV RUST_LOG=info

# Optional: basic liveness check (expects you to serve /healthz)
HEALTHCHECK --interval=10s --timeout=3s --retries=3 \
  CMD curl -fsS "http://127.0.0.1:${PORT:-8080}/healthz" || exit 1

# Run as non-root (optional, add a user if you prefer)
# USER 10001

CMD ["/app/ai-toolkit"]
