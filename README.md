# CallMind

<div align="center">

**High-Performance Autonomous Conversation Intelligence Platform**

[![CI](https://github.com/callmind/callmind/actions/workflows/ci.yml/badge.svg)](https://github.com/callmind/callmind/actions/workflows/ci.yml)
[![Rust Version](https://img.shields.io/badge/rust-1.94%2B-blue.svg)](https://www.rust-lang.org)
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
  - **The number of speakers is measured, not assumed.** With the optional
    pyannote segmentation model (`models download diarization-segmentation`),
    each 10-second chunk is classified into a powerset of active speakers, so a
    voice note is not split into two people and a conference is not forced into
    two. Measured on labelled recordings: 4/4 single-speaker recordings and 23/24
    two-party calls, against 0/4 and 24/24 for the two-party assumption it
    replaces. Without the model the assumption still applies, and
    `POST /api/v1/calls/{id}/reprocess?speakers=N` — or the selector on the call
    page — overrides either way.
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
  - **WhatsApp via [Evolution API](https://evolution-api.com)**: self-hosted gateway that pairs with an ordinary WhatsApp account over QR — no Meta business verification required.
  - **Universal Voice Webhook**: one-tap audio processing for **iOS Shortcuts / Siri**, Android Tasker, n8n, and Zapier.

- **⚡ Interactive Web UI & Deep Search**:
  - Audio player with word-by-word active highlighting and click-to-seek.
  - Sub-millisecond full-text search across transcripts, summaries, entities, and tags — SQLite FTS5 or a Postgres `tsvector` with a GIN index, from the same query code.
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

Ensure you have Rust 1.94+ and `libopus` installed (`brew install opus` on macOS, `apt install libopus-dev` on Debian/Ubuntu):

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

### WhatsApp via Evolution API

Uses a self-hosted [Evolution API](https://evolution-api.com) instance, so no Meta
business verification or `phone_number_id` is needed — it pairs with an ordinary
WhatsApp account over QR.

1. Run Evolution API and create an instance, then pair it by scanning the QR code.
2. Point CallMind at it:
   ```yaml
   bots:
     evolution:
       enabled: true
       base_url: "http://localhost:8080"
       instance: "my-instance"
       api_key: "your-evolution-api-key"
       webhook_token: "pick-a-long-random-string"
       allowed_numbers: ["972500000000"] # Optional: restrict senders
   ```
   Or use `CALLMIND_EVOLUTION_BASE_URL`, `CALLMIND_EVOLUTION_INSTANCE`,
   `CALLMIND_EVOLUTION_API_KEY` and `CALLMIND_EVOLUTION_WEBHOOK_TOKEN`.
3. Configure the instance webhook to call back into CallMind. Evolution does not
   sign its webhooks, so send the shared secret as a header:
   ```bash
   curl -X POST "http://localhost:8080/webhook/set/my-instance" \
     -H "apikey: your-evolution-api-key" -H 'Content-Type: application/json' \
     -d '{"webhook":{"enabled":true,
                     "url":"http://callmind-host:8080/api/v1/bots/evolution",
                     "headers":{"X-Webhook-Token":"pick-a-long-random-string"},
                     "byEvents":false,
                     "events":["MESSAGES_UPSERT"]}}'
   ```
   Both `byEvents` modes work: when it is `true`, Evolution appends the event name
   to the path and CallMind accepts that form too.
4. Send a voice note to the paired number. CallMind acknowledges immediately, then
   replies with the summary, action items and calendar file once analysis finishes.

> **Note on audio:** WhatsApp and Telegram voice notes are OGG/Opus. Symphonia
> demuxes the OGG container but has no Opus decoder (there is no
> `symphonia-codec-opus`), so `callmind-audio` decodes Opus directly via
> `libopus`. Building therefore needs `libopus-dev` (`brew install opus` on
> macOS); the Docker images already include it.

### iOS Shortcuts / Siri Webhook
Send voice notes directly from your iPhone:
```bash
curl -X POST "http://localhost:8080/api/v1/bots/webhook?sync=true" \
  -F "audio=@recording.m4a"
```

---

## 🐳 Docker & Docker Compose

Pre-built Docker images are automatically published to **GitHub Container Registry (GHCR)**:
- `ghcr.io/callmind/callmind:latest` (Universal CPU)
- `ghcr.io/callmind/callmind:vulkan` (AMD Radeon, Intel Arc, Nvidia Vulkan GPU)
- `ghcr.io/callmind/callmind:cuda` (Nvidia CUDA Tensor Core GPU)

Run with Docker Compose:

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
  evolution: # WhatsApp via self-hosted Evolution API
    enabled: false
    base_url: null # or set CALLMIND_EVOLUTION_BASE_URL
    instance: null # or set CALLMIND_EVOLUTION_INSTANCE
    api_key: null # or set CALLMIND_EVOLUTION_API_KEY
    webhook_token: null # shared secret for the inbound webhook
    allowed_numbers: []
    result_timeout_secs: 600
  slack:
    enabled: false # config only; no handler implemented yet
    bot_token: null
    signing_secret: null
  watcher:
    enabled: false
    watch_dir: "./incoming"
    poll_secs: 5
  webhook:
    enabled: true
    secret_token: null
```

---

## 📁 Workspace Architecture

CallMind is structured as 17 decoupled, domain-focused Rust crates:

```
callmind/
├── crates/
│   ├── callmind-core          # Domain entities, IDs, enums, ChannelMapping
│   ├── callmind-config        # Configuration loader, validation, env overrides
│   ├── callmind-storage       # Recording storage abstraction (Filesystem)
│   ├── callmind-db            # SQL repositories, one implementation for SQLite and Postgres
│   ├── callmind-jobs          # Worker pool, leasing, heartbeats, background queue
│   ├── callmind-audio         # Symphonia decoder, resampler, channel analyzer
│   ├── callmind-vad           # Voice Activity Detection (Energy VAD)
│   ├── callmind-language      # Multi-window acoustic Language Identification (LID)
│   ├── callmind-stt           # Whisper.cpp engine, ivrit-ai router, token timestamps
│   ├── callmind-diarization   # Pure Rust ONNX speaker embedding & AHC clustering
│   ├── callmind-transcript    # Transcript builder, RTL/LTR normalization, .ics exporter
│   ├── callmind-llm           # Localized prompt templates (RU/HE/EN) & LLM adapters
│   ├── callmind-analysis      # Structured intelligence parser, scorecard, entities
│   ├── callmind-search        # Multilingual search & Ask Q&A engine (FTS5 or tsvector)
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

# The Postgres schema tests need a database. Without it they skip, so the line
# above needs nothing running.
docker compose -f docker-compose.test.yml up -d
CALLMIND_TEST_POSTGRES_URL=postgres://callmind:callmind@127.0.0.1:55432/callmind_test \
  cargo test --workspace
docker compose -f docker-compose.test.yml down -v
```

---

## 📄 License

Dual-licensed under either of:
- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.
