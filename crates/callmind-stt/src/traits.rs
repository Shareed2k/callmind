use crate::errors::SttError;
use crate::models::{ModelInfo, SttRequest, SttResult};
use async_trait::async_trait;

/// Interface for speech-to-text engines (Whisper Large v3, ivrit-ai, etc.).
#[async_trait]
pub trait SttEngine: Send + Sync {
    /// Transcribe an audio stream into timestamped words.
    async fn transcribe(&self, request: SttRequest<'_>) -> Result<SttResult, SttError>;

    /// Return model identifier, version, and backend metadata.
    fn info(&self) -> ModelInfo;
}
