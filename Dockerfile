# ------------------------------------------------------------------------------
# Stage 1: Builder
# ------------------------------------------------------------------------------
FROM rust:1.85-bookworm AS builder

WORKDIR /usr/src/callmind

# Install build dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    cmake \
    pkg-config \
    libasound2-dev \
    libclang-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy workspace sources
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

# Build release binary (CPU backend for standard Linux container)
RUN cargo build --release --no-default-features --features cpu

# ------------------------------------------------------------------------------
# Stage 2: Minimal Runtime
# ------------------------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

WORKDIR /app

# Install runtime dependencies & CA certificates
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    libasound2 \
    && rm -rf /var/lib/apt/lists/*

# Copy compiled binary from builder
COPY --from=builder /usr/src/callmind/target/release/callmind /app/callmind

# Copy default configuration
COPY callmind.yaml /app/callmind.yaml

# Create data and models directories
RUN mkdir -p /app/data /app/models

# Expose HTTP API & Web UI port
EXPOSE 8080

# Configure environment defaults
ENV RUST_LOG=info \
    CALLMIND_SERVER_BIND="0.0.0.0:8080" \
    CALLMIND_DATABASE_URL="/app/data/callmind.db" \
    CALLMIND_STORAGE_PATH="/app/data/recordings"

# Healthcheck
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -f http://127.0.0.1:8080/health || exit 1

# Default command
CMD ["/app/callmind", "serve"]
