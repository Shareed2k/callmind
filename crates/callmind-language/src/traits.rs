use crate::errors::LanguageError;
use crate::models::LanguageDetection;
use async_trait::async_trait;
use callmind_audio::AudioBuffer;
use callmind_vad::SpeechRegion;

/// Interface for language detection engines.
#[async_trait]
pub trait LanguageEngine: Send + Sync {
    /// Detect primary language and distribution across speech regions.
    async fn detect(
        &self,
        audio: &AudioBuffer,
        speech_regions: &[SpeechRegion],
    ) -> Result<LanguageDetection, LanguageError>;
}
