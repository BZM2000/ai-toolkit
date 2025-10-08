# Multi-stage build for ai-toolkit with LibreOffice support

# Stage 1: Builder
FROM rust:nightly-slim-bookworm AS builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy manifest files first for better caching
COPY Cargo.toml Cargo.lock ./

# Create a dummy main.rs to build dependencies
RUN mkdir -p src && \
    echo "fn main() {}" > src/main.rs && \
    cargo build --release && \
    rm -rf src

# Copy the actual source code
COPY src ./src
COPY migrations ./migrations

# Build the application
RUN cargo build --release

# Stage 2: Runtime
FROM debian:bookworm-slim

# Install runtime dependencies including LibreOffice
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libreoffice-writer \
    libreoffice-core \
    libreoffice-common \
    libssl3 \
    fonts-liberation \
    fonts-dejavu \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy the built binary from builder
COPY --from=builder /app/target/release/ai-toolkit /app/ai-toolkit

# Copy migrations directory
COPY migrations /app/migrations

# Create storage directory
RUN mkdir -p /app/storage

# Expose port (Railway will set PORT env var)
EXPOSE 3000

# Set environment variables
ENV RUST_LOG=info

# Run the application
CMD ["/app/ai-toolkit"]
