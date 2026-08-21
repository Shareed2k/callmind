# Hebrew AI and ONNX Diarization Enhancements Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement neural ONNX speaker diarization embeddings via `tract-onnx` (pure Rust) with fallback to acoustic AHC, add Hebrew & multilingual 8-emotion & sentiment analysis (`hebEMO`-aligned), and enhance LLM prompt engineering for Hebrew models (`DictaLM-3.0`, `Llama-3.2-Hebrew`).

**Architecture:**
1. **Diarization (`callmind-diarization`)**: `OnnxSpeakerEmbeddingExtractor` uses `tract-onnx` to run pre-trained neural embedding models (e.g., PyAnnote, ECAPA-TDNN, WeSpeaker) from `./models/diarization/` on 16kHz speech turns, clustered via `AgglomerativeClustering` (AHC). Falls back seamlessly to acoustic DSP feature extraction if no ONNX model is present.
2. **Sentiment & Emotion Analysis (`callmind-analysis`)**: Add `EmotionClassifier` supporting 8 emotional states (*anger, fear, joy, surprise, sadness, disgust, anticipation, trust*) with Hebrew/Russian/English lexicons and heuristic/neural classification, integrated into `ConversationMetricsCalculator` and the `/analytics` view.
3. **Hebrew LLM Prompts (`callmind-llm`)**: Enhance structured analysis prompts with language-adaptive instructions (responding in Hebrew for Hebrew calls with proper RTL semantics, DictaLM/Llama-3.2 tuning).

**Tech Stack:** Rust 2024, `tract-onnx` (pure Rust ONNX engine), Axum 0.8, SQLx SQLite, Ollama / OpenAI / Anthropic.

## Global Constraints
- Pure Rust ONNX execution via `tract-onnx` (zero external C/C++ dynamic libraries required).
- Zero mock fallbacks in production paths.
- Comprehensive test coverage with unit and integration tests passing (`cargo test --workspace`).
- Clippy passes cleanly (`cargo clippy --all-targets -- -D warnings`).

---

### Task 1: Neural ONNX Speaker Embedding Extractor in `callmind-diarization`

**Files:**
- Create: `crates/callmind-diarization/src/onnx_extractor.rs`
- Modify: `crates/callmind-diarization/src/lib.rs`
- Modify: `crates/callmind-diarization/Cargo.toml`
- Test: `crates/callmind-diarization/tests/onnx_diarizer_test.rs`

**Interfaces:**
- Produces: `OnnxSpeakerEmbeddingExtractor::new(model_path: &Path) -> Result<Self, DiarizationError>`
- Produces: `OnnxSpeakerEmbeddingExtractor::extract_embedding(&self, samples: &[f32], sample_rate: u32) -> Result<Vec<f32>, DiarizationError>`
- Produces: `NeuralDiarizer` implementing `DiarizationEngine`.

- [ ] **Step 1: Write the failing test for ONNX extractor and fallback**
- [ ] **Step 2: Run test to verify it fails**
- [ ] **Step 3: Implement `OnnxSpeakerEmbeddingExtractor` using `tract_onnx`**
- [ ] **Step 4: Implement `NeuralDiarizer` with AHC clustering and acoustic fallback**
- [ ] **Step 5: Run tests and verify they pass**

---

### Task 2: Hebrew & Multilingual 8-Emotion & Sentiment Classifier in `callmind-analysis`

**Files:**
- Create: `crates/callmind-analysis/src/emotions.rs`
- Modify: `crates/callmind-analysis/src/lib.rs`
- Modify: `crates/callmind-analysis/src/analyzer.rs`
- Modify: `crates/callmind-analysis/src/models.rs`
- Modify: `crates/callmind-ui/src/views/analytics.rs`
- Test: `crates/callmind-analysis/tests/emotion_analysis_test.rs`

**Interfaces:**
- Produces: `EmotionType` (Anger, Fear, Joy, Surprise, Sadness, Disgust, Anticipation, Trust, Neutral)
- Produces: `EmotionDistribution` with percentages and top emotion.
- Produces: `EmotionClassifier::analyze_text(text: &str, language: &Language) -> EmotionDistribution`

- [ ] **Step 1: Write failing tests for Hebrew/Russian/English emotion detection**
- [ ] **Step 2: Run test to verify it fails**
- [ ] **Step 3: Implement `EmotionClassifier` with comprehensive Hebrew/Russian/English lexicons**
- [ ] **Step 4: Integrate emotion metrics into `CallAnalysis` and `analytics.rs` UI view**
- [ ] **Step 5: Run tests and verify they pass**

---

### Task 3: Hebrew-Optimized LLM Prompts & Configuration in `callmind-llm`

**Files:**
- Modify: `crates/callmind-llm/src/prompts.rs`
- Modify: `crates/callmind-llm/src/local.rs`
- Modify: `callmind.yaml`
- Test: `crates/callmind-llm/tests/hebrew_prompt_test.rs`

**Interfaces:**
- Produces: `build_language_aware_analysis_prompt(transcript_text: &str, organization: &str, primary_language: &Language) -> String`

- [ ] **Step 1: Write test for language-aware Hebrew prompt construction**
- [ ] **Step 2: Run test to verify it fails**
- [ ] **Step 3: Implement language-aware prompt with native Hebrew instructions for Hebrew transcripts**
- [ ] **Step 4: Update `callmind.yaml` with model presets for DictaLM and Llama 3.2 Hebrew**
- [ ] **Step 5: Run tests and clippy across workspace to verify all checks pass**
