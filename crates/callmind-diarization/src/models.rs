use callmind_audio::AudioBuffer;
use callmind_core::SpeakerId;
use serde::{Deserialize, Serialize};

/// Represents a continuous time interval spoken by an individual speaker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SpeakerTurn {
    /// Speaker identifier (0, 1, ...).
    pub speaker: SpeakerId,
    /// Turn start timestamp in milliseconds.
    pub start_ms: u64,
    /// Turn end timestamp in milliseconds.
    pub end_ms: u64,
    /// Attribution confidence (0.0 to 1.0).
    pub confidence: Option<f32>,
}

impl SpeakerTurn {
    pub fn new(speaker: SpeakerId, start_ms: u64, end_ms: u64, confidence: Option<f32>) -> Self {
        Self {
            speaker,
            start_ms,
            end_ms,
            confidence,
        }
    }

    /// Turn duration in milliseconds.
    pub fn duration_ms(&self) -> u64 {
        self.end_ms.saturating_sub(self.start_ms)
    }

    /// Calculate temporal overlap in milliseconds with another time range `[start, end]`.
    pub fn overlap_ms(&self, start: u64, end: u64) -> u64 {
        let overlap_start = self.start_ms.max(start);
        let overlap_end = self.end_ms.min(end);
        overlap_end.saturating_sub(overlap_start)
    }
}

/// Request parameters passed to a `DiarizationEngine`.
pub struct DiarizationRequest<'a> {
    pub audio: &'a AudioBuffer,
    pub expected_speakers: Option<usize>,
}

impl<'a> DiarizationRequest<'a> {
    pub fn new(audio: &'a AudioBuffer) -> Self {
        Self {
            audio,
            expected_speakers: None,
        }
    }

    #[must_use]
    pub fn with_expected_speakers(mut self, count: usize) -> Self {
        self.expected_speakers = Some(count);
        self
    }
}

/// Result of speaker diarization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DiarizationResult {
    /// Total distinct speakers identified.
    pub speakers: usize,
    /// Chronological list of speaker turns.
    pub turns: Vec<SpeakerTurn>,
    /// One voice print per speaker, when the engine produced embeddings.
    ///
    /// Carried out of diarization rather than recomputed later: the expensive
    /// part -- an ONNX pass per window -- has already happened here, and these
    /// used to be discarded the moment clustering finished.
    pub speaker_embeddings: Vec<(SpeakerId, Vec<f32>)>,
}

impl DiarizationResult {
    pub fn new(speakers: usize, mut turns: Vec<SpeakerTurn>) -> Self {
        turns.sort_by_key(|t| t.start_ms);
        Self {
            speakers,
            turns,
            speaker_embeddings: Vec::new(),
        }
    }

    /// Attach the per-speaker voice prints.
    #[must_use]
    pub fn with_speaker_embeddings(mut self, embeddings: Vec<(SpeakerId, Vec<f32>)>) -> Self {
        self.speaker_embeddings = embeddings;
        self
    }
}
