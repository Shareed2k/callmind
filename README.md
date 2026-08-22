# CallMind

<div align="center">

**High-Performance Autonomous Conversation Intelligence Platform**

[![CI](https://github.com/callmind/callmind/actions/workflows/ci.yml/badge.svg)](https://github.com/callmind/callmind/actions/workflows/ci.yml)
[![Rust Version](https://img.shields.io/badge/rust-1.85%2B-blue.svg)](https://www.rust-lang.org)
[![Edition](https://img.shields.io/badge/edition-2024-purple.svg)](https://doc.rust-lang.org/edition-guide/rust-2024/index.html)
[![Acceleration](https://img.shields.io/badge/GPU-Metal%20%7C%20Vulkan%20%7C%20CUDA-green.svg)](#hardware-acceleration)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-informational.svg)](LICENSE)

[Features](#-key-features) • [Quickstart](#-quickstart) • [Hardware Acceleration](#-hardware-acceleration) • [Personal Voice Assistant](#-personal-voice-assistant--bots) • [Docker](#-docker--docker-compose) • [API & Swagger](#-api--documentation)

</div>

---

## 🌟 Overview

**CallMind** is a conversation intelligence platform written in pure **Rust (2024 edition)**. It delivers speech-to-text transcription, neural speaker diarization, fact-based conversation intelligence analysis, smart to-do extraction, calendar event synchronization, and omnichannel voice bot capabilities.

Whether used as a backend for call analytics or as a private voice assistant for family, health, and personal to-dos, CallMind operates autonomously without cloud vendor lock-in.

---

## 🚀 Key Features

- **🎙️ Multilingual Speech Recognition (STT)**:
  - Automatic language identification (LID) across Hebrew, Russian, English, Arabic, and 90+ languages.
  - Dynamic STT routing: runs specialized [ivrit-ai](https://huggingface.co/ivrit-ai) models for Hebrew and Whisper Large-v3 for multilingual audio.
  - Word-level millisecond timestamp synchronization for audio playback.

- **👥 Pure Rust Neural Speaker Diarization**:
  - Neural speaker embedding inference (`tract-onnx`) using WeSpeaker ResNet34 / ECAPA-TDNN with 80-channel log Mel filterbanks.
  - Agglomerative Hierarchical Clustering (AHC) with automatic DSP fallback.
  - Support for multi-channel and separated stereo telephony with PBX channel role mapping.

- **🧠 Deep Factual Conversation Intelligence**:
  - Structured extraction of facts: **WHO** spoke, **WHAT** happened, **WHERE** (floors, rooms, addresses), **WHEN**, and **WHAT WAS DECIDED**.
  - Localized analytical summaries generated strictly in the language of the conversation (Russian, Hebrew, English).
  - Sentiment scoring, intent classification, compliance checks, and agreement tracking.

- **📝 Smart To-Do & Grocery List**:
  - Automatically extracts action items, grocery lists, and commitments.
  - One-click copy to **Apple Reminders**, **Todoist**, **Markdown**, and **WhatsApp**.

- **📅 Calendar & Appointment Sync (.ICS RFC 5545)**:
  - Detects appointments, dates, times, and medical/service visits.
  - Generates standard `.ics` calendar files importable into Apple Calendar, Google Calendar, and Microsoft Outlook.

- **🤖 Omnichannel Personal Voice Assistant**:
  - **Telegram Bot**: forward voice notes or audio files; receive summaries, to-do lists, and calendar invites.
  - **Meta WhatsApp Cloud API**: webhook integration for WhatsApp voice notes.
  - **Universal Voice Webhook**: one-tap audio processing for **iOS Shortcuts / Siri**, Android Tasker, n8n, and Zapier.

- **⚡ Interactive Web UI & Deep Search**:
  - Audio player with word-by-word active highlighting and click-to-seek.
  - Sub-millisecond full-text search (SQLite FTS5) across transcripts, summaries, entities, and tags.
  - Subtitle exports: **SRT**, **VTT**, **TXT**, **Markdown**, **JSON**, and **ICS**.

---

## ⚡ Hardware Acceleration

CallMind supports hardware acceleration across all major platforms:

| Platform | Backend Feature | Supported Hardware |
| :--- | :--- | :--- |
| **macOS (Apple Silicon)** | `metal` *(default)* | 🍏 Apple M1, M2, M3, M4, M5 (Unified Memory Metal GPU) |
| **Linux (Cross-GPU)** | `vulkan` | 🎮 **AMD Radeon** (RX 6000/7000/8000), **Intel Arc**, **Nvidia** |
| **Linux (Nvidia GPU)** | `cuda` | 🚀 **Nvidia RTX / Tesla / A100 / H100** (Tensor Cores) |
| **Linux / Server (Universal)** | `cpu` | ⚡ Any x86_64 or ARM64 CPU (AVX2 / FMA / NEON) |

---

## 📦 Quickstart

### Option 1: Pre-built Release Binaries

Download the binary for your platform from [GitHub Releases](https://github.com/callmind/callmind/releases):

```bash
# macOS (Apple Silicon M-series)
curl -L -o callmind.tar.gz https://github.com/callmind/callmind/releases/latest/download/callmind-macos-arm64.tar.gz
tar -xzf callmind.tar.gz

# Linux (x86_64 CPU)
curl -L -o callmind.tar.gz https://github.com/callmind/callmind/releases/latest/download/callmind-linux-x86_64.tar.gz
tar -xzf callmind.tar.gz
```

### Option 2: Build from Source

Ensure you have Rust 1.85+ installed:

```bash
# Clone the repository
git clone https://github.com/callmind/callmind.git
cd callmind

# Build optimized release binary (default uses Apple Metal on macOS)
cargo build --release

# On Linux for Vulkan GPU acceleration:
# cargo build --release --no-default-features --features vulkan

# On Linux for Nvidia CUDA GPU acceleration:
# cargo build --release --no-default-features --features cuda

# On Linux for CPU-only:
# cargo build --release --no-default-features --features cpu
```

---

## 🧠 Model Management

CallMind manages AI model weights via a built-in CLI with download resume and SHA-256 integrity verification:

```bash
# List available models and status
./target/release/callmind models list

# Download all required models (Whisper Large-v3, ivrit-ai Turbo, ONNX Diarization, LLM)
./target/release/callmind models download all

# Verify SHA-256 checksums
./target/release/callmind models verify all
```

---

## 🏃 Running the Server

1. **Start Ollama** (or configure OpenAI / Anthropic in `callmind.yaml`):
   ```bash
   ollama run llama3.2:3b
   ```

2. **Start CallMind**:
   ```bash
   ./target/release/callmind serve
   ```

3. **Open the Web Interface**:
   - Web UI: [http://localhost:8080](http://localhost:8080)
   - Interactive Swagger API: [http://localhost:8080/swagger-ui](http://localhost:8080/swagger-ui)
   - Health Check: [http://localhost:8080/health](http://localhost:8080/health)

---

## 🤖 Personal Voice Assistant & Bots

### Telegram Bot Setup
1. Create a bot with [@BotFather](https://t.me/BotFather) and obtain your token.
2. Add the token to `callmind.yaml`:
   ```yaml
   bots:
     telegram:
       enabled: true
       bot_token: "123456789:ABCdefGhIJKlmNoPQRstuVWXyz"
       allowed_chat_ids: [123456789] # Optional: restrict access
   ```
   Or set environment variable: `export CALLMIND_TELEGRAM_BOT_TOKEN="your-token"`
3. Start CallMind. Send any voice message or forward an audio recording to your bot to receive instant summaries, action items, and calendar files.

### iOS Shortcuts / Siri Webhook
Send voice notes directly from your iPhone:
```bash
curl -X POST "http://localhost:8080/api/v1/bots/webhook?sync=true" \
  -F "audio=@recording.m4a"
```

---

## 🐳 Docker & Docker Compose

Run CallMind with hardware-accelerated speech processing and Ollama in containers:

```bash
# 1. Universal CPU Mode (Default)
docker compose up -d

# 2. Vulkan GPU Acceleration (AMD Radeon, Intel Arc, Nvidia on Linux)
docker compose --profile vulkan up -d

# 3. Nvidia CUDA GPU Acceleration (Nvidia RTX / Tesla with nvidia-container-toolkit)
docker compose --profile cuda up -d

# Pull the default LLM in the Ollama container
docker compose exec ollama ollama pull llama3.2:3b

# Download speech & diarization models inside CallMind
docker compose exec callmind ./callmind models download all
```

---

## ⚙️ Configuration Reference (`callmind.yaml`)

```yaml
server:
  bind: "127.0.0.1:8080"
  body_limit_mb: 500

database:
  driver: "sqlite"
  url: "./data/callmind.db"
  max_connections: 16
  busy_timeout_ms: 5000

storage:
  driver: "filesystem"
  path: "./data/recordings"

jobs:
  workers: 4
  poll_interval_ms: 500
  lock_timeout_secs: 600
  max_attempts: 3

models:
  models_dir: "./models"

llm:
  provider: "ollama" # "ollama", "openai", "anthropic", "heuristic"
  endpoint: "http://localhost:11434"
  model: "llama3.2:3b"

auth:
  enabled: false
  api_key: null # or set CALLMIND_AUTH_API_KEY
  allowed_origins: [] # CORS allowed origins

bots:
  telegram:
    enabled: false
    bot_token: null # or set CALLMIND_TELEGRAM_BOT_TOKEN
    allowed_chat_ids: []
  whatsapp:
    enabled: false
    phone_number_id: null
    access_token: null
    verify_token: null
  webhook:
    enabled: true
    secret_token: null
```

---

## 📁 Workspace Architecture

CallMind is structured as 18 decoupled, domain-focused Rust crates:

```
callmind/
├── crates/
│   ├── callmind-core          # Domain entities, IDs, enums, ChannelMapping
│   ├── callmind-config        # Configuration loader, validation, env overrides
│   ├── callmind-storage       # Recording storage abstraction (Filesystem / S3)
│   ├── callmind-db            # SQL database repositories (SQLite WAL / migrations)
│   ├── callmind-jobs          # Worker pool, leasing, heartbeats, background queue
│   ├── callmind-audio         # Symphonia decoder, resampler, channel analyzer
│   ├── callmind-vad           # Voice Activity Detection (Energy / Silero VAD)
│   ├── callmind-language      # Multi-window acoustic Language Identification (LID)
│   ├── callmind-stt           # Whisper.cpp engine, ivrit-ai router, token timestamps
│   ├── callmind-diarization   # Pure Rust ONNX speaker embedding & AHC clustering
│   ├── callmind-transcript    # Transcript builder, RTL/LTR normalization, .ics exporter
│   ├── callmind-llm           # Localized prompt templates (RU/HE/EN) & LLM adapters
│   ├── callmind-analysis      # Structured intelligence parser, scorecard, entities
│   ├── callmind-search        # SQLite FTS5 multilingual search & Ask Q&A engine
│   ├── callmind-ui            # Server-rendered HTML5 UI, word-level audio sync
│   ├── callmind-api           # Axum REST API, Omnichannel bots, Swagger OpenAPI
│   └── callmind               # Main CLI binary (serve, import, models, doctor)
```

---

## 🧪 Testing & Verification

```bash
# Check code formatting
cargo fmt --check

# Run Clippy lints
cargo clippy --all-targets -- -D warnings

# Execute full workspace test suite
cargo test --workspace
```

---

## 📄 License

Dual-licensed under either of:
- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.
