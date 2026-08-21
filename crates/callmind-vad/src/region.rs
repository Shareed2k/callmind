use serde::{Deserialize, Serialize};

/// Represents a detected segment of human speech activity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SpeechRegion {
    /// Start timestamp in milliseconds.
    pub start_ms: u64,
    /// End timestamp in milliseconds.
    pub end_ms: u64,
    /// Detection confidence score (0.0 to 1.0).
    pub confidence: f32,
}

impl SpeechRegion {
    pub fn new(start_ms: u64, end_ms: u64, confidence: f32) -> Self {
        Self {
            start_ms,
            end_ms,
            confidence: confidence.clamp(0.0, 1.0),
        }
    }

    /// Duration of the speech region in milliseconds.
    pub fn duration_ms(&self) -> u64 {
        self.end_ms.saturating_sub(self.start_ms)
    }
}
