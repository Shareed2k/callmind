//! Conversions between the wire contract and the service's domain types.
//!
//! Kept explicit rather than deriving the wire format from the Rust types: the
//! `.proto` is a published contract that third-party workers generate from, so
//! it must be able to evolve separately from internal refactors. The round-trip
//! test at the bottom is what keeps the two in step.

use crate::v1;
use callmind_core::{CallId, Language, SpeakerId, SpeakerRole};
use callmind_language::LanguageProbability;
use callmind_transcript::{TextDirection, Transcript, TranscriptSegment, TranscriptWord};
use std::str::FromStr;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum TranscriptConversionError {
    #[error("invalid call id {value:?}: {source}")]
    CallId {
        value: String,
        #[source]
        source: uuid::Error,
    },
    #[error("invalid segment id {value:?}: {source}")]
    SegmentId {
        value: String,
        #[source]
        source: uuid::Error,
    },
    #[error("speaker id {0} exceeds the supported range")]
    SpeakerIdRange(u32),
    #[error("transcript contains no segments")]
    Empty,
}

fn role_to_proto(role: SpeakerRole) -> v1::SpeakerRole {
    match role {
        SpeakerRole::Speaker1 => v1::SpeakerRole::Speaker1,
        SpeakerRole::Speaker2 => v1::SpeakerRole::Speaker2,
        SpeakerRole::Participant => v1::SpeakerRole::Participant,
        SpeakerRole::Agent => v1::SpeakerRole::Agent,
        SpeakerRole::Customer => v1::SpeakerRole::Customer,
        SpeakerRole::Supervisor => v1::SpeakerRole::Supervisor,
        SpeakerRole::Unknown => v1::SpeakerRole::Unknown,
    }
}

fn role_from_proto(role: i32) -> SpeakerRole {
    match v1::SpeakerRole::try_from(role) {
        Ok(v1::SpeakerRole::Speaker1) => SpeakerRole::Speaker1,
        Ok(v1::SpeakerRole::Speaker2) => SpeakerRole::Speaker2,
        Ok(v1::SpeakerRole::Participant) => SpeakerRole::Participant,
        Ok(v1::SpeakerRole::Agent) => SpeakerRole::Agent,
        Ok(v1::SpeakerRole::Customer) => SpeakerRole::Customer,
        Ok(v1::SpeakerRole::Supervisor) => SpeakerRole::Supervisor,
        // Unspecified, Unknown, or a value from a newer contract version.
        _ => SpeakerRole::Unknown,
    }
}

fn direction_to_proto(direction: TextDirection) -> v1::TextDirection {
    match direction {
        TextDirection::Ltr => v1::TextDirection::Ltr,
        TextDirection::Rtl => v1::TextDirection::Rtl,
    }
}

fn direction_from_proto(direction: i32) -> TextDirection {
    match v1::TextDirection::try_from(direction) {
        Ok(v1::TextDirection::Rtl) => TextDirection::Rtl,
        _ => TextDirection::Ltr,
    }
}

fn speaker_id_from_proto(id: u32) -> Result<SpeakerId, TranscriptConversionError> {
    u16::try_from(id)
        .map(SpeakerId)
        .map_err(|_| TranscriptConversionError::SpeakerIdRange(id))
}

/// Domain transcript to wire.
#[must_use]
pub fn transcript_to_proto(transcript: &Transcript) -> v1::Transcript {
    v1::Transcript {
        call_id: transcript.call_id.to_string(),
        languages: transcript
            .languages
            .iter()
            .map(|l| v1::LanguageProbability {
                // `Language::code()` and `FromStr` round-trip, including the
                // open-ended `Other` variant, so a string is lossless here.
                language: l.language.code().to_string(),
                probability: l.probability,
            })
            .collect(),
        speakers: transcript
            .speakers
            .iter()
            .map(|s| v1::SpeakerMetadata {
                speaker_id: u32::from(s.speaker_id.as_u16()),
                role: role_to_proto(s.role) as i32,
                talk_time_ms: s.talk_time_ms,
                word_count: s.word_count as u64,
            })
            .collect(),
        segments: transcript
            .segments
            .iter()
            .map(|seg| v1::TranscriptSegment {
                id: seg.id.to_string(),
                call_id: seg.call_id.to_string(),
                sequence: seg.sequence,
                speaker_id: u32::from(seg.speaker_id.as_u16()),
                speaker_role: role_to_proto(seg.speaker_role) as i32,
                language: seg.language.code().to_string(),
                text_direction: direction_to_proto(seg.text_direction) as i32,
                start_ms: seg.start_ms,
                end_ms: seg.end_ms,
                raw_text: seg.raw_text.clone(),
                normalized_text: seg.normalized_text.clone(),
                words: seg
                    .words
                    .iter()
                    .map(|w| v1::TranscriptWord {
                        text: w.text.clone(),
                        start_ms: w.start_ms,
                        end_ms: w.end_ms,
                        speaker_id: u32::from(w.speaker_id.as_u16()),
                        speaker_role: role_to_proto(w.speaker_role) as i32,
                        language: w.language.code().to_string(),
                        confidence: w.confidence,
                    })
                    .collect(),
            })
            .collect(),
    }
}

/// Wire transcript to domain, rejecting anything the service cannot store.
pub fn transcript_from_proto(
    proto: v1::Transcript,
) -> Result<Transcript, TranscriptConversionError> {
    if proto.segments.is_empty() {
        return Err(TranscriptConversionError::Empty);
    }

    let parse_call_id = |value: &str| {
        CallId::from_str(value).map_err(|source| TranscriptConversionError::CallId {
            value: value.to_string(),
            source,
        })
    };

    let mut segments = Vec::with_capacity(proto.segments.len());
    for seg in proto.segments {
        let mut words = Vec::with_capacity(seg.words.len());
        for w in seg.words {
            words.push(TranscriptWord {
                text: w.text,
                start_ms: w.start_ms,
                end_ms: w.end_ms,
                speaker_id: speaker_id_from_proto(w.speaker_id)?,
                speaker_role: role_from_proto(w.speaker_role),
                // `FromStr` is infallible: an unknown code becomes `Other`.
                language: Language::from_str(&w.language).unwrap_or(Language::Unknown),
                confidence: w.confidence,
            });
        }

        segments.push(TranscriptSegment {
            id: Uuid::from_str(&seg.id).map_err(|source| TranscriptConversionError::SegmentId {
                value: seg.id.clone(),
                source,
            })?,
            call_id: parse_call_id(&seg.call_id)?,
            sequence: seg.sequence,
            speaker_id: speaker_id_from_proto(seg.speaker_id)?,
            speaker_role: role_from_proto(seg.speaker_role),
            language: Language::from_str(&seg.language).unwrap_or(Language::Unknown),
            text_direction: direction_from_proto(seg.text_direction),
            start_ms: seg.start_ms,
            end_ms: seg.end_ms,
            raw_text: seg.raw_text,
            normalized_text: seg.normalized_text,
            words,
        });
    }

    Ok(Transcript {
        call_id: parse_call_id(&proto.call_id)?,
        languages: proto
            .languages
            .into_iter()
            .map(|l| LanguageProbability {
                language: Language::from_str(&l.language).unwrap_or(Language::Unknown),
                probability: l.probability,
            })
            .collect(),
        speakers: proto
            .speakers
            .into_iter()
            .map(|s| {
                Ok(callmind_transcript::SpeakerMetadata {
                    speaker_id: speaker_id_from_proto(s.speaker_id)?,
                    role: role_from_proto(s.role),
                    talk_time_ms: s.talk_time_ms,
                    word_count: s.word_count as usize,
                })
            })
            .collect::<Result<Vec<_>, TranscriptConversionError>>()?,
        segments,
    })
}

/// Acoustic emotion results to the JSON shape the core stores and renders.
///
/// Deliberately a plain JSON document rather than a Rust type: plugin results
/// share one storage column, and the renderer reads this shape by convention.
#[must_use]
pub fn speaker_emotions_to_json(emotions: &v1::SpeakerEmotions) -> serde_json::Value {
    let scores = |scores: &[v1::EmotionScore]| {
        scores
            .iter()
            .map(|s| serde_json::json!({ "emotion": s.emotion, "score": s.score }))
            .collect::<Vec<_>>()
    };

    serde_json::json!({
        "kind": "speaker_emotions",
        "model": emotions.model,
        "call_id": emotions.call_id,
        "summaries": emotions.summaries.iter().map(|s| serde_json::json!({
            "speaker_id": s.speaker_id,
            "dominant": s.dominant,
            "scores": scores(&s.scores),
        })).collect::<Vec<_>>(),
        "spans": emotions.spans.iter().map(|s| serde_json::json!({
            "speaker_id": s.speaker_id,
            "start_ms": s.start_ms,
            "end_ms": s.end_ms,
            "scores": scores(&s.scores),
        })).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use callmind_transcript::SpeakerMetadata;

    fn sample() -> Transcript {
        let call_id = CallId::generate();
        Transcript {
            call_id,
            languages: vec![
                LanguageProbability {
                    language: Language::Hebrew,
                    probability: 0.7,
                },
                // The open-ended variant has to survive the string round-trip.
                LanguageProbability {
                    language: Language::Other("yi".into()),
                    probability: 0.3,
                },
            ],
            speakers: vec![SpeakerMetadata {
                speaker_id: SpeakerId(1),
                role: SpeakerRole::Agent,
                talk_time_ms: 4200,
                word_count: 12,
            }],
            segments: vec![TranscriptSegment {
                id: Uuid::new_v4(),
                call_id,
                sequence: 0,
                speaker_id: SpeakerId(1),
                speaker_role: SpeakerRole::Customer,
                language: Language::Hebrew,
                text_direction: TextDirection::Rtl,
                start_ms: 0,
                end_ms: 1500,
                raw_text: "שלום".into(),
                normalized_text: "שלום".into(),
                words: vec![TranscriptWord {
                    text: "שלום".into(),
                    start_ms: 0,
                    end_ms: 500,
                    speaker_id: SpeakerId(1),
                    speaker_role: SpeakerRole::Customer,
                    language: Language::Hebrew,
                    confidence: Some(0.94),
                }],
            }],
        }
    }

    /// The published `.proto` and the internal types must stay in step. If a
    /// field is added to one and not the other, this fails.
    #[test]
    fn transcript_round_trips_through_the_wire_format() {
        let original = sample();
        let restored = transcript_from_proto(transcript_to_proto(&original)).unwrap();
        assert_eq!(restored, original);
    }

    #[test]
    fn unknown_enum_values_degrade_instead_of_failing() {
        let mut proto = transcript_to_proto(&sample());
        // A value from a newer contract version, or a client that sent garbage.
        proto.segments[0].speaker_role = 9999;
        proto.segments[0].text_direction = 9999;
        let restored = transcript_from_proto(proto).unwrap();
        assert_eq!(restored.segments[0].speaker_role, SpeakerRole::Unknown);
        assert_eq!(restored.segments[0].text_direction, TextDirection::Ltr);
    }

    #[test]
    fn malformed_input_is_rejected() {
        let mut proto = transcript_to_proto(&sample());
        proto.call_id = "not-a-uuid".into();
        assert!(matches!(
            transcript_from_proto(proto),
            Err(TranscriptConversionError::CallId { .. })
        ));

        let mut proto = transcript_to_proto(&sample());
        proto.segments.clear();
        assert!(matches!(
            transcript_from_proto(proto),
            Err(TranscriptConversionError::Empty)
        ));

        // SpeakerId is a u16 on the domain side.
        let mut proto = transcript_to_proto(&sample());
        proto.segments[0].speaker_id = 70_000;
        assert!(matches!(
            transcript_from_proto(proto),
            Err(TranscriptConversionError::SpeakerIdRange(70_000))
        ));
    }
}
