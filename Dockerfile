# ------------------------------------------------------------------------------
# Stage 1: Builder
# ------------------------------------------------------------------------------
FROM ubuntu:24.04 AS builder

ARG DEBIAN_FRONTEND=noninteractive
# Build argument for hardware acceleration: "cpu" or "vulkan"
ARG ACCELERATION=cpu

# Portability, not speed. whisper.cpp defaults to `-march=native`, which bakes
# in whatever the machine building the image supports -- and an image is meant
# to run somewhere else. With this off, ggml enables SSE4.2/AVX/AVX2/BMI2
# explicitly and leaves AVX-512 alone, so the image runs on any x86_64 since
# Haswell instead of dying with SIGILL on an older host.
ENV GGML_NATIVE=OFF

WORKDIR /usr/src/callmind

# Install build dependencies, Vulkan SDK headers & glslc shader compiler
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    cmake \
    pkg-config \
    build-essential \
    libasound2-dev \
    libclang-dev \
    libopus-dev \
    libssl-dev \
    libvulkan-dev \
    glslc \
    && rm -rf /var/lib/apt/lists/*

# Install Rust toolchain
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --default-toolchain stable

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
FROM ubuntu:24.04 AS runtime

ARG DEBIAN_FRONTEND=noninteractive
WORKDIR /app

# Install runtime dependencies, CA certificates, and Vulkan drivers
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    libasound2t64 \
    libopus0 \
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
