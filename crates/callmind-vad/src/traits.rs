use crate::errors::VadError;
use crate::region::SpeechRegion;
use async_trait::async_trait;
use callmind_audio::AudioBuffer;

/// Interface for Voice Activity Detection engines.
#[async_trait]
pub trait VadEngine: Send + Sync {
    /// Analyze an audio buffer and extract speech regions.
    async fn detect(&self, audio: &AudioBuffer) -> Result<Vec<SpeechRegion>, VadError>;
}
