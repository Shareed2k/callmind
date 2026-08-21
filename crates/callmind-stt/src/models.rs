use callmind_audio::AudioBuffer;
use callmind_core::{Language, SpeakerId};
use serde::{Deserialize, Serialize};

/// Individual transcribed word with precise millisecond timestamps.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SttWord {
    /// Transcribed word text.
    pub text: String,
    /// Word start timestamp in milliseconds from the start of the audio stream.
    pub start_ms: u64,
    /// Word end timestamp in milliseconds from the start of the audio stream.
    pub end_ms: u64,
    /// Speaker identifier attributed to this word.
    pub speaker_id: Option<SpeakerId>,
    /// Token probability / confidence score (0.0 to 1.0).
    pub confidence: Option<f32>,
    /// Language attributed to this specific word.
    pub language: Option<Language>,
}

impl SttWord {
    pub fn new(
        text: String,
        start_ms: u64,
        end_ms: u64,
        confidence: Option<f32>,
        language: Option<Language>,
    ) -> Self {
        Self {
            text,
            start_ms,
            end_ms,
            speaker_id: None,
            confidence,
            language,
        }
    }

    #[must_use]
    pub fn with_speaker(mut self, speaker: SpeakerId) -> Self {
        self.speaker_id = Some(speaker);
        self
    }
}

/// Request parameters passed to an `SttEngine`.
pub struct SttRequest<'a> {
    /// Audio buffer (must be 16kHz Mono f32).
    pub audio: &'a AudioBuffer,
    /// Optional language hint (e.g. "he", "ru", "en").
    pub language_hint: Option<Language>,
    /// Organization custom vocabulary phrases for contextual biasing.
    pub vocabulary: &'a [String],
    /// Whether to generate word-level timestamps.
    pub word_timestamps: bool,
}

/// Result of speech-to-text transcription.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SttResult {
    /// Token/word sequence with millisecond timestamps.
    pub words: Vec<SttWord>,
    /// Dominant detected language.
    pub detected_language: Option<Language>,
    /// Full raw concatenated text.
    pub raw_text: String,
}

impl SttResult {
    pub fn new(words: Vec<SttWord>, detected_language: Option<Language>) -> Self {
        let raw_text = words
            .iter()
            .map(|w| w.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        Self {
            words,
            detected_language,
            raw_text,
        }
    }
}

/// Metadata describing an STT model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ModelInfo {
    pub id: String,
    pub version: String,
    pub backend: String,
}
