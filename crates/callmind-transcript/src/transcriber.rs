use callmind_audio::{AudioBuffer, AudioResampler, ChannelAnalyzer, ChannelMode};
use callmind_core::{CallId, Language};
use callmind_diarization::{
    DiarizationEngine, DiarizationRequest, StereoChannelDiarizer, TranscriptAligner,
};
use callmind_language::LanguageEngine;
use callmind_stt::SttRouter;
use callmind_vad::VadEngine;
use std::sync::Arc;
use thiserror::Error;

use crate::builder::TranscriptBuilder;
use crate::models::Transcript;

#[derive(Debug, Error)]
pub enum TranscriberError {
    #[error("Audio processing error: {0}")]
    Audio(#[from] callmind_audio::AudioError),

    #[error("VAD error: {0}")]
    Vad(#[from] callmind_vad::VadError),

    #[error("Language identification error: {0}")]
    Language(#[from] callmind_language::LanguageError),

    #[error("STT error: {0}")]
    Stt(String),

    #[error("Diarization error: {0}")]
    Diarization(String),
}

/// Deep Conversation Audio Transcription Engine.
/// Encapsulates resampling, VAD segmentation, language routing, parallel STT + Diarization,
/// word alignment, role detection, and RTL normalization behind a single call.
pub struct AudioTranscriber {
    pub vad: Arc<dyn VadEngine>,
    pub language_engine: Arc<dyn LanguageEngine>,
    pub stt_router: Arc<SttRouter>,
    pub stereo_diarizer: Arc<StereoChannelDiarizer>,
    pub clustering_diarizer: Arc<dyn DiarizationEngine>,
    pub gpu_semaphore: Arc<tokio::sync::Semaphore>,
}

impl AudioTranscriber {
    pub fn new(
        vad: Arc<dyn VadEngine>,
        language_engine: Arc<dyn LanguageEngine>,
        stt_router: Arc<SttRouter>,
        stereo_diarizer: Arc<StereoChannelDiarizer>,
        clustering_diarizer: Arc<dyn DiarizationEngine>,
        gpu_semaphore: Arc<tokio::sync::Semaphore>,
    ) -> Self {
        Self {
            vad,
            language_engine,
            stt_router,
            stereo_diarizer,
            clustering_diarizer,
            gpu_semaphore,
        }
    }

    /// Transcribe a complete decoded conversation audio buffer into an aligned, structured `Transcript`.
    /// Executes Diarization and STT concurrently via `tokio::try_join!`.
    pub async fn transcribe_conversation(
        &self,
        call_id: CallId,
        decoded_audio: &AudioBuffer,
        explicit_language_hint: Option<Language>,
        channel_mapping: Option<&callmind_core::ChannelMapping>,
        vocabulary: &[crate::vocabulary::VocabularyEntry],
        expected_speakers: Option<usize>,
    ) -> Result<Transcript, TranscriberError> {
        // 1. Channel Analysis & Resampling
        let channel_mode = ChannelAnalyzer::analyze(decoded_audio);
        let resampled_mono = AudioResampler::resample_to_16k_mono(decoded_audio)?;

        // 2. VAD Detection
        let speech_regions = self.vad.detect(&resampled_mono).await?;

        // 3. Language Probe
        let language_detection = if let Some(ref l) = explicit_language_hint {
            callmind_language::LanguageDetection::new(
                l.clone(),
                vec![callmind_language::LanguageProbability {
                    language: l.clone(),
                    probability: 1.0,
                }],
                false,
            )
        } else {
            self.language_engine
                .detect(&resampled_mono, &speech_regions)
                .await?
        };

        // 4. Parallel STT and Diarization execution
        let diarization_fut = async {
            match channel_mode {
                ChannelMode::StereoSeparated { .. } => self
                    .stereo_diarizer
                    .diarize(DiarizationRequest {
                        audio: decoded_audio,
                        expected_speakers: Some(2),
                    })
                    .await
                    .map_err(|e| TranscriberError::Diarization(e.to_string())),
                // The count comes from whoever ingested the audio, because that
                // is the only place it is actually known: a voice note is one
                // person by construction, a phone recording is two. Hardcoding
                // `Some(2)` here split every single-speaker recording in half,
                // and no acoustic threshold can tell the two cases apart --
                // measured in `tests/onnx_centroid_probe.rs`, where the centroid
                // gap for two real speakers runs as low as 0.104 while one voice
                // forced apart reaches 0.394.
                _ => self
                    .clustering_diarizer
                    .diarize(DiarizationRequest {
                        audio: &resampled_mono,
                        expected_speakers,
                    })
                    .await
                    .map_err(|e| TranscriberError::Diarization(e.to_string())),
            }
        };

        let raw_vocab_words: Vec<String> = vocabulary.iter().map(|v| v.phrase.clone()).collect();

        let stt_fut = async {
            let _gpu_permit = self
                .gpu_semaphore
                .acquire()
                .await
                .map_err(|_| TranscriberError::Stt("GPU scheduler semaphore closed".into()))?;

            let (stt_result, _stt_profile) = self
                .stt_router
                .transcribe_routed(&resampled_mono, &language_detection, &raw_vocab_words)
                .await
                .map_err(|e| TranscriberError::Stt(e.to_string()))?;

            Ok(stt_result)
        };

        let (diarization_res, stt_res) = tokio::try_join!(diarization_fut, stt_fut)?;

        // 5. Word-to-Speaker Alignment
        let aligned_words = TranscriptAligner::align(&stt_res.words, &diarization_res.turns);

        // 6. Build Structured Transcript with Roles, Normalization & RTL/LTR
        let mut transcript = TranscriptBuilder::build_with_mapping(
            call_id,
            &aligned_words,
            &channel_mode,
            channel_mapping,
            vocabulary,
            language_detection.distribution,
        );

        // The short language probe is only a routing hint. Persist the language
        // distribution derived from the final transcript so UI filters and
        // downstream analysis reflect the text Whisper actually produced.
        let mut language_counts = std::collections::HashMap::new();
        let mut total_words = 0_u32;
        for segment in &transcript.segments {
            let word_count = segment.words.len() as u32;
            if word_count > 0 {
                *language_counts
                    .entry(segment.language.clone())
                    .or_insert(0_u32) += word_count;
                total_words += word_count;
            }
        }
        if total_words > 0 {
            transcript.languages = language_counts
                .into_iter()
                .map(|(language, count)| callmind_language::LanguageProbability {
                    language,
                    probability: count as f32 / total_words as f32,
                })
                .collect();
            transcript.languages.sort_by(|a, b| {
                b.probability
                    .partial_cmp(&a.probability)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }

        Ok(transcript)
    }
}
