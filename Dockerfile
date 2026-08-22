# ------------------------------------------------------------------------------
# Stage 1: Builder
# ------------------------------------------------------------------------------
FROM rust:bookworm AS builder

# Build argument for hardware acceleration: "cpu" or "vulkan"
ARG ACCELERATION=cpu

WORKDIR /usr/src/callmind

# Install build dependencies & Vulkan SDK headers
RUN apt-get update && apt-get install -y --no-install-recommends \
    cmake \
    pkg-config \
    libasound2-dev \
    libclang-dev \
    libvulkan-dev \
    glslang-tools \
    libshaderc-dev \
    libssl-dev \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Ensure glslc compiler binary is available for Vulkan shader compilation
RUN if ! command -v glslc >/dev/null 2>&1; then \
        printf '#!/bin/sh\nexec glslangValidator -V "$@"\n' > /usr/local/bin/glslc && \
        chmod +x /usr/local/bin/glslc; \
    fi

# Copy workspace sources
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

# Build release binary based on selected acceleration target
RUN if [ "$ACCELERATION" = "vulkan" ]; then \
        echo "Building CallMind with Vulkan GPU acceleration..." && \
        cargo build --release --no-default-features --features vulkan; \
    else \
        echo "Building CallMind with universal CPU backend..." && \
        cargo build --release --no-default-features --features cpu; \
    fi

# ------------------------------------------------------------------------------
# Stage 2: Minimal Runtime
# ------------------------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

WORKDIR /app

# Install runtime dependencies, CA certificates, and Vulkan drivers
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    libasound2 \
    libvulkan1 \
    mesa-vulkan-drivers \
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
