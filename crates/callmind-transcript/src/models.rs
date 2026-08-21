use callmind_core::{CallId, Language, SpeakerId, SpeakerRole};
use callmind_language::LanguageProbability;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Text rendering direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum TextDirection {
    Ltr,
    Rtl,
}

impl TextDirection {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ltr => "ltr",
            Self::Rtl => "rtl",
        }
    }
}

/// A single token in the transcript with precise timing and attribution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct TranscriptWord {
    pub text: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub speaker_id: SpeakerId,
    pub speaker_role: SpeakerRole,
    pub language: Language,
    pub confidence: Option<f32>,
}

/// A formatted conversational segment spoken by a single speaker in a single language.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct TranscriptSegment {
    pub id: Uuid,
    pub call_id: CallId,
    pub sequence: u32,
    pub speaker_id: SpeakerId,
    pub speaker_role: SpeakerRole,
    pub language: Language,
    pub text_direction: TextDirection,
    pub start_ms: u64,
    pub end_ms: u64,
    pub raw_text: String,
    pub normalized_text: String,
    pub words: Vec<TranscriptWord>,
}

impl TranscriptSegment {
    pub fn duration_ms(&self) -> u64 {
        self.end_ms.saturating_sub(self.start_ms)
    }
}

/// Metadata describing an individual speaker in the conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SpeakerMetadata {
    pub speaker_id: SpeakerId,
    pub role: SpeakerRole,
    pub talk_time_ms: u64,
    pub word_count: usize,
}

/// Complete conversation transcript contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Transcript {
    pub call_id: CallId,
    pub languages: Vec<LanguageProbability>,
    pub speakers: Vec<SpeakerMetadata>,
    pub segments: Vec<TranscriptSegment>,
}

impl Transcript {
    /// Calculate total conversation duration in milliseconds from segments.
    pub fn duration_ms(&self) -> u64 {
        self.segments.iter().map(|s| s.end_ms).max().unwrap_or(0)
    }

    /// Concatenate full normalized text of the conversation.
    pub fn full_text(&self) -> String {
        self.segments
            .iter()
            .map(|s| format!("{}: {}", s.speaker_role.as_str(), s.normalized_text))
            .collect::<Vec<_>>()
            .join("\n")
    }
}
