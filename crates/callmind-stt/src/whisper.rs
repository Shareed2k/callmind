use crate::errors::SttError;
use crate::models::{ModelInfo, SttRequest, SttResult, SttWord};
use crate::traits::SttEngine;
use async_trait::async_trait;
use callmind_core::Language;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tracing::info;

/// Production STT engine supporting Whisper.cpp GGML model weights with in-memory caching.
pub struct WhisperCppEngine {
    pub model_path: PathBuf,
    pub info: ModelInfo,
    cached_ctx: Arc<Mutex<Option<Arc<whisper_rs::WhisperContext>>>>,
}

impl WhisperCppEngine {
    pub fn new<P: AsRef<Path>>(model_path: P, id: &str, version: &str) -> Self {
        let path = model_path.as_ref().to_path_buf();
        Self {
            model_path: path.clone(),
            info: ModelInfo {
                id: id.to_string(),
                version: version.to_string(),
                backend: "whisper.cpp (Metal/CUDA)".to_string(),
            },
            cached_ctx: Arc::new(Mutex::new(None)),
        }
    }

    fn get_or_load_context(&self) -> Result<Arc<whisper_rs::WhisperContext>, SttError> {
        Self::load_context(&self.cached_ctx, &self.model_path)
    }

    /// Load (or return the cached) Whisper context.
    ///
    /// Takes the pieces it needs rather than `&self` so that callers can run it
    /// inside `spawn_blocking`: this reads multi-GB weights off disk and
    /// initialises the GPU backend while holding the mutex, which must never
    /// happen on an async runtime thread.
    fn load_context(
        cached_ctx: &Mutex<Option<Arc<whisper_rs::WhisperContext>>>,
        model_path: &Path,
    ) -> Result<Arc<whisper_rs::WhisperContext>, SttError> {
        let mut lock = cached_ctx
            .lock()
            .map_err(|e| SttError::Inference(e.to_string()))?;
        if let Some(ref ctx) = *lock {
            return Ok(ctx.clone());
        }

        let model_path_str = model_path.to_string_lossy().to_string();
        if !model_path.exists() {
            return Err(SttError::ModelLoad {
                path: model_path_str,
                message: "Whisper model weights file not found on disk. Please download required model before transcribing.".to_string(),
            });
        }

        info!("Loading Whisper model into memory from {}", model_path_str);

        // 1. Try loading with GPU acceleration (Metal on macOS, CUDA on Linux)
        let mut gpu_params = whisper_rs::WhisperContextParameters::default();
        gpu_params.use_gpu(true);
        gpu_params.flash_attn(false);

        let ctx = match whisper_rs::WhisperContext::new_with_params(&model_path_str, gpu_params) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    "GPU acceleration failed for Whisper ({e}). Gracefully falling back to CPU backend."
                );
                let mut cpu_params = whisper_rs::WhisperContextParameters::default();
                cpu_params.use_gpu(false);
                whisper_rs::WhisperContext::new_with_params(&model_path_str, cpu_params).map_err(
                    |cpu_err| SttError::ModelLoad {
                        path: model_path_str.clone(),
                        message: format!(
                            "Failed to load Whisper model on GPU ({e}) and CPU ({cpu_err})"
                        ),
                    },
                )?
            }
        };

        let arc_ctx = Arc::new(ctx);
        *lock = Some(arc_ctx.clone());
        Ok(arc_ctx)
    }

    /// Fast acoustic language identification probe using Whisper decoder state.
    pub fn detect_language_probe(
        &self,
        audio: &callmind_audio::AudioBuffer,
    ) -> Result<Vec<(Language, f32)>, SttError> {
        let ctx = self.get_or_load_context()?;
        let mono = audio.to_mono();
        if mono.is_empty() {
            return Ok(Vec::new());
        }
        let sample_len = (15 * mono.sample_rate as usize).min(mono.samples.len());
        let samples = mono.samples[..sample_len].to_vec();

        let mut state = ctx
            .create_state()
            .map_err(|e| SttError::Inference(e.to_string()))?;

        let mut params =
            whisper_rs::FullParams::new(whisper_rs::SamplingStrategy::Greedy { best_of: 1 });
        params.set_translate(false);
        params.set_detect_language(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_single_segment(true);
        params.set_max_len(1);

        state
            .full(params, &samples)
            .map_err(|e| SttError::Inference(e.to_string()))?;

        let lang_id = state.full_lang_id_from_state();
        let detected_lang_str = whisper_rs::get_lang_str(lang_id).unwrap_or("en");
        let lang: Language = detected_lang_str.parse().unwrap_or(Language::English);
        Ok(vec![(lang, 0.95)])
    }
}

#[async_trait]
impl SttEngine for WhisperCppEngine {
    async fn transcribe(&self, request: SttRequest<'_>) -> Result<SttResult, SttError> {
        if request.audio.is_empty() {
            return Err(SttError::InvalidAudio);
        }

        let audio_samples = request.audio.to_mono().samples;
        let lang_hint = request.language_hint.as_ref().map(|l| l.code().to_string());
        let cached_ctx = self.cached_ctx.clone();
        let model_path = self.model_path.clone();

        let result = tokio::task::spawn_blocking(move || -> Result<SttResult, SttError> {
            // Loaded here, not before the spawn: on a cold cache this reads the
            // model off disk and spins up Metal/CUDA, which stalled every task
            // on the runtime — including the HTTP handlers.
            let ctx = Self::load_context(&cached_ctx, &model_path)?;
            let mut state = ctx
                .create_state()
                .map_err(|e| SttError::Inference(e.to_string()))?;

            let mut params =
                whisper_rs::FullParams::new(whisper_rs::SamplingStrategy::Greedy { best_of: 1 });
            params.set_translate(false); // Transcribe in authentic spoken language, no auto-translation
            params.set_token_timestamps(true); // Enable real token/word level timestamps from Whisper decoder
            params.set_split_on_word(true);

            if let Some(ref l) = lang_hint {
                if l != "und" && l != "unknown" {
                    params.set_language(Some(l.as_str()));
                } else {
                    params.set_language(None);
                }
            } else {
                params.set_language(None);
            }

            params.set_print_special(false);
            params.set_print_progress(false);
            params.set_print_realtime(false);
            params.set_print_timestamps(false);

            state
                .full(params, &audio_samples)
                .map_err(|e| SttError::Inference(e.to_string()))?;

            // Detect language directly from Whisper decoder state
            let lang_id = state.full_lang_id_from_state();
            let detected_lang_str = whisper_rs::get_lang_str(lang_id).unwrap_or("en");
            let detected_language: Option<Language> = detected_lang_str
                .parse()
                .ok()
                .or_else(|| lang_hint.as_deref().and_then(|l| l.parse().ok()));

            let num_segments = state.full_n_segments();
            let mut words = Vec::new();

            for i in 0..num_segments {
                if let Some(seg) = state.get_segment(i) {
                    let seg_t0 = (seg.start_timestamp() as u64) * 10;
                    let seg_t1 = (seg.end_timestamp() as u64) * 10;
                    let num_tokens = seg.n_tokens();

                    let mut current_word_tokens: Vec<(Vec<u8>, u64, u64, f32)> = Vec::new();
                    let mut seg_words_count = 0;

                    for j in 0..num_tokens {
                        if let Some(token) = seg.get_token(j) {
                            let t_data = token.token_data();
                            let p = token.token_probability();
                            let token_bytes = token.to_bytes().unwrap_or(&[]).to_vec();

                            if token_bytes.starts_with(b"[_") {
                                continue; // Skip control tokens
                            }

                            let t0 = ((t_data.t0 as u64) * 10).max(seg_t0);
                            let t1 = ((t_data.t1 as u64) * 10).min(seg_t1).max(t0 + 30);

                            let is_word_boundary = token_bytes.starts_with(b" ")
                                || token_bytes.starts_with(b"\n")
                                || token_bytes.starts_with(b"\t");

                            if is_word_boundary && !current_word_tokens.is_empty() {
                                let mut raw_bytes = Vec::new();
                                for (tb, _, _, _) in &current_word_tokens {
                                    raw_bytes.extend_from_slice(tb);
                                }
                                let word_text =
                                    String::from_utf8_lossy(&raw_bytes).trim().to_string();
                                if !word_text.is_empty() {
                                    let word_start =
                                        current_word_tokens.first().map_or(seg_t0, |t| t.1);
                                    let word_end = current_word_tokens
                                        .last()
                                        .map_or(seg_t1, |t| t.2)
                                        .max(word_start + 40);
                                    let avg_conf =
                                        current_word_tokens.iter().map(|t| t.3).sum::<f32>()
                                            / current_word_tokens.len() as f32;

                                    words.push(SttWord::new(
                                        word_text,
                                        word_start,
                                        word_end,
                                        Some(avg_conf),
                                        detected_language.clone(),
                                    ));
                                    seg_words_count += 1;
                                }
                                current_word_tokens.clear();
                            }

                            let clean_bytes = if current_word_tokens.is_empty() {
                                token_bytes
                                    .strip_prefix(b" ")
                                    .unwrap_or(&token_bytes)
                                    .to_vec()
                            } else {
                                token_bytes
                            };

                            current_word_tokens.push((clean_bytes, t0, t1, p));
                        }
                    }

                    if !current_word_tokens.is_empty() {
                        let mut raw_bytes = Vec::new();
                        for (tb, _, _, _) in &current_word_tokens {
                            raw_bytes.extend_from_slice(tb);
                        }
                        let word_text = String::from_utf8_lossy(&raw_bytes).trim().to_string();
                        if !word_text.is_empty() {
                            let word_start = current_word_tokens.first().map_or(seg_t0, |t| t.1);
                            let word_end = current_word_tokens
                                .last()
                                .map_or(seg_t1, |t| t.2)
                                .max(word_start + 40);
                            let avg_conf = current_word_tokens.iter().map(|t| t.3).sum::<f32>()
                                / current_word_tokens.len() as f32;

                            words.push(SttWord::new(
                                word_text,
                                word_start,
                                word_end,
                                Some(avg_conf),
                                detected_language.clone(),
                            ));
                            seg_words_count += 1;
                        }
                    }

                    // If token extraction yielded 0 words, fallback to segment text
                    if seg_words_count == 0 {
                        let seg_text = seg.to_str_lossy().unwrap_or_default();
                        let fallback_words: Vec<&str> = seg_text.split_whitespace().collect();
                        if !fallback_words.is_empty() {
                            let total_dur = seg_t1.saturating_sub(seg_t0);
                            let per_word = (total_dur / fallback_words.len() as u64).max(50);
                            for (w_idx, &w_str) in fallback_words.iter().enumerate() {
                                let w_start = seg_t0 + (w_idx as u64 * per_word);
                                let w_end = (w_start + per_word).min(seg_t1).max(w_start + 40);
                                words.push(SttWord::new(
                                    w_str.to_string(),
                                    w_start,
                                    w_end,
                                    Some(0.90),
                                    detected_language.clone(),
                                ));
                            }
                        }
                    }
                }
            }

            Ok(SttResult::new(words, detected_language))
        })
        .await
        .map_err(|e| SttError::Inference(format!("Whisper task execution failed: {e}")))?;

        let stt_res = result?;
        info!(
            "Whisper real inference transcribed {} words from audio",
            stt_res.words.len()
        );
        Ok(stt_res)
    }

    fn info(&self) -> ModelInfo {
        self.info.clone()
    }
}
