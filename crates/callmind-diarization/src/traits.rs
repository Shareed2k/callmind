use crate::errors::DiarizationError;
use crate::models::{DiarizationRequest, DiarizationResult};
use async_trait::async_trait;

/// Interface for Speaker Diarization engines.
#[async_trait]
pub trait DiarizationEngine: Send + Sync {
    /// Segment audio into speaker turns.
    async fn diarize(
        &self,
        request: DiarizationRequest<'_>,
    ) -> Result<DiarizationResult, DiarizationError>;
}
