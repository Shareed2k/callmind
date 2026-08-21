use crate::errors::SttError;
use crate::models::{ModelInfo, SttRequest, SttResult, SttWord};
use crate::traits::SttEngine;
use async_trait::async_trait;
use callmind_core::Language;

/// Mock STT engine for integration and unit testing.
pub struct MockSttEngine {
    pub info: ModelInfo,
    pub predefined_words: Option<Vec<SttWord>>,
}

impl MockSttEngine {
    pub fn new(id: &str, version: &str) -> Self {
        Self {
            info: ModelInfo {
                id: id.to_string(),
                version: version.to_string(),
                backend: "mock".to_string(),
            },
            predefined_words: None,
        }
    }

    #[must_use]
    pub fn with_words(mut self, words: Vec<SttWord>) -> Self {
        self.predefined_words = Some(words);
        self
    }
}

#[async_trait]
impl SttEngine for MockSttEngine {
    async fn transcribe(&self, request: SttRequest<'_>) -> Result<SttResult, SttError> {
        if request.audio.is_empty() {
            return Err(SttError::InvalidAudio);
        }

        if let Some(ref words) = self.predefined_words {
            return Ok(SttResult::new(words.clone(), request.language_hint));
        }

        // Generate synthetic timestamped words based on audio duration
        let duration_ms = request.audio.duration_ms();
        let lang = request.language_hint.unwrap_or(Language::Hebrew);

        let phrases: Vec<&str> = match lang {
            Language::Hebrew => vec!["שלום", "במה", "אפשר", "לעזור", "לך", "היום"],
            Language::Russian => vec!["Здравствуйте", "у", "меня", "вопрос", "по", "заказу"],
            Language::English => vec!["Hello", "how", "can", "I", "help", "you", "today"],
            _ => vec!["sample", "transcription", "word", "stream"],
        };

        let word_count = phrases.len();
        let step_ms = (duration_ms / (word_count as u64)).max(200);
        let mut words = Vec::with_capacity(word_count);

        for (i, phrase) in phrases.into_iter().enumerate() {
            let start = (i as u64) * step_ms;
            let end = (start + step_ms - 50).min(duration_ms);
            words.push(SttWord::new(
                phrase.to_string(),
                start,
                end,
                Some(0.95),
                Some(lang.clone()),
            ));
        }

        Ok(SttResult::new(words, Some(lang)))
    }

    fn info(&self) -> ModelInfo {
        self.info.clone()
    }
}
