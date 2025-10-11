# ---------- Stage 1: Builder ----------
# Use Rust 1.90+ so Cargo understands the 2024 edition used by this crate.
FROM rust:1.90-slim-bookworm AS builder

# Build-time dependencies needed for linking openssl and friends.
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy manifest and source files into the builder image.
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY migrations ./migrations
COPY robots.txt ./robots.txt
COPY entrypoint.sh ./entrypoint.sh

# Build the release binary we will ship to production.
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

# Copy the compiled binary and migrations from the builder image.
COPY --from=builder /app/target/release/ai-toolkit /app/ai-toolkit
COPY --from=builder /app/migrations /app/migrations
COPY --from=builder /app/entrypoint.sh /app/entrypoint.sh

# Prepare runtime directories.
RUN mkdir -p /app/storage \
 && chmod 755 /app/ai-toolkit \
 && chmod 755 /app/entrypoint.sh

# Railway injects $PORT; use 8080 locally by default.
EXPOSE 8080
ENV RUST_LOG=info

# Run the service.
CMD ["/app/entrypoint.sh"]
