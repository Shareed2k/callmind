use crate::models::ConversationMetrics;
use callmind_core::SpeakerRole;
use callmind_transcript::Transcript;

/// Calculates objective conversation timing and speech interaction metrics.
pub struct ConversationMetricsCalculator;

impl ConversationMetricsCalculator {
    /// Compute conversation metrics from a structured `Transcript`.
    pub fn calculate(transcript: &Transcript) -> ConversationMetrics {
        if transcript.segments.is_empty() {
            return ConversationMetrics {
                total_duration_ms: 0,
                agent_talk_ms: 0,
                customer_talk_ms: 0,
                silence_ms: 0,
                agent_talk_ratio: 0.0,
                customer_talk_ratio: 0.0,
                longest_agent_monologue_ms: 0,
                longest_customer_monologue_ms: 0,
                speaker_switches: 0,
                agent_words_per_minute: 0.0,
                customer_words_per_minute: 0.0,
            };
        }

        let total_duration_ms = transcript.segments.last().map_or(0, |s| s.end_ms);
        let mut agent_talk_ms: u64 = 0;
        let mut customer_talk_ms: u64 = 0;
        let mut agent_word_count: usize = 0;
        let mut customer_word_count: usize = 0;

        let mut longest_agent_monologue_ms: u64 = 0;
        let mut longest_customer_monologue_ms: u64 = 0;
        let mut current_monologue_role: Option<SpeakerRole> = None;
        let mut current_monologue_duration: u64 = 0;

        let mut speaker_switches: u32 = 0;
        let mut previous_speaker_id: Option<callmind_core::SpeakerId> = None;
        let mut silence_ms: u64 = 0;
        let mut previous_end_ms: u64 = 0;

        for segment in &transcript.segments {
            let seg_dur = segment.duration_ms();
            let word_count = segment.words.len();

            // Track silence gaps
            if segment.start_ms > previous_end_ms {
                let gap = segment.start_ms - previous_end_ms;
                if gap >= 500 {
                    silence_ms += gap;
                }
            }
            previous_end_ms = segment.end_ms;

            // Track speaker switches
            if let Some(prev) = previous_speaker_id {
                if prev != segment.speaker_id {
                    speaker_switches += 1;
                }
            }
            previous_speaker_id = Some(segment.speaker_id);

            // Track talk times by role
            match segment.speaker_role {
                SpeakerRole::Agent => {
                    agent_talk_ms += seg_dur;
                    agent_word_count += word_count;
                }
                SpeakerRole::Customer => {
                    customer_talk_ms += seg_dur;
                    customer_word_count += word_count;
                }
                _ => {}
            }

            // Track monologues
            if current_monologue_role == Some(segment.speaker_role) {
                current_monologue_duration += seg_dur;
            } else {
                if let Some(role) = current_monologue_role {
                    match role {
                        SpeakerRole::Agent => {
                            longest_agent_monologue_ms =
                                longest_agent_monologue_ms.max(current_monologue_duration);
                        }
                        SpeakerRole::Customer => {
                            longest_customer_monologue_ms =
                                longest_customer_monologue_ms.max(current_monologue_duration);
                        }
                        _ => {}
                    }
                }
                current_monologue_role = Some(segment.speaker_role);
                current_monologue_duration = seg_dur;
            }
        }

        // Final monologue flush
        if let Some(role) = current_monologue_role {
            match role {
                SpeakerRole::Agent => {
                    longest_agent_monologue_ms =
                        longest_agent_monologue_ms.max(current_monologue_duration);
                }
                SpeakerRole::Customer => {
                    longest_customer_monologue_ms =
                        longest_customer_monologue_ms.max(current_monologue_duration);
                }
                _ => {}
            }
        }

        let total_talk_ms = (agent_talk_ms + customer_talk_ms).max(1);
        let agent_talk_ratio = (agent_talk_ms as f32) / (total_talk_ms as f32);
        let customer_talk_ratio = (customer_talk_ms as f32) / (total_talk_ms as f32);

        let agent_words_per_minute = if agent_talk_ms > 0 {
            ((agent_word_count as f32) * 60_000.0) / (agent_talk_ms as f32)
        } else {
            0.0
        };

        let customer_words_per_minute = if customer_talk_ms > 0 {
            ((customer_word_count as f32) * 60_000.0) / (customer_talk_ms as f32)
        } else {
            0.0
        };

        ConversationMetrics {
            total_duration_ms,
            agent_talk_ms,
            customer_talk_ms,
            silence_ms,
            agent_talk_ratio,
            customer_talk_ratio,
            longest_agent_monologue_ms,
            longest_customer_monologue_ms,
            speaker_switches,
            agent_words_per_minute,
            customer_words_per_minute,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use callmind_core::{CallId, Language, SpeakerId, SpeakerRole};
    use callmind_transcript::{TextDirection, TranscriptSegment, TranscriptWord};
    use uuid::Uuid;

    #[test]
    fn test_calculate_metrics() {
        let seg1 = TranscriptSegment {
            id: Uuid::new_v4(),
            call_id: CallId::generate(),
            sequence: 0,
            speaker_id: SpeakerId::new(0),
            speaker_role: SpeakerRole::Agent,
            language: Language::Hebrew,
            text_direction: TextDirection::Rtl,
            start_ms: 0,
            end_ms: 5000,
            raw_text: "שלום במה אפשר לעזור".into(),
            normalized_text: "שלום במה אפשר לעזור".into(),
            words: vec![
                TranscriptWord {
                    text: "שלום".into(),
                    start_ms: 0,
                    end_ms: 1000,
                    speaker_id: SpeakerId::new(0),
                    speaker_role: SpeakerRole::Agent,
                    language: Language::Hebrew,
                    confidence: None,
                },
                TranscriptWord {
                    text: "במה".into(),
                    start_ms: 1000,
                    end_ms: 2000,
                    speaker_id: SpeakerId::new(0),
                    speaker_role: SpeakerRole::Agent,
                    language: Language::Hebrew,
                    confidence: None,
                },
            ],
        };

        let seg2 = TranscriptSegment {
            id: Uuid::new_v4(),
            call_id: CallId::generate(),
            sequence: 1,
            speaker_id: SpeakerId::new(1),
            speaker_role: SpeakerRole::Customer,
            language: Language::Russian,
            text_direction: TextDirection::Ltr,
            start_ms: 6000,
            end_ms: 10000,
            raw_text: "Здравствуйте проблема с заказом".into(),
            normalized_text: "Здравствуйте проблема с заказом".into(),
            words: vec![TranscriptWord {
                text: "Здравствуйте".into(),
                start_ms: 6000,
                end_ms: 8000,
                speaker_id: SpeakerId::new(1),
                speaker_role: SpeakerRole::Customer,
                language: Language::Russian,
                confidence: None,
            }],
        };

        let transcript = Transcript {
            call_id: CallId::generate(),
            languages: Vec::new(),
            speakers: Vec::new(),
            segments: vec![seg1, seg2],
        };

        let metrics = ConversationMetricsCalculator::calculate(&transcript);
        assert_eq!(metrics.agent_talk_ms, 5000);
        assert_eq!(metrics.customer_talk_ms, 4000);
        assert_eq!(metrics.silence_ms, 1000);
        assert_eq!(metrics.speaker_switches, 1);
        assert!((metrics.agent_talk_ratio - 0.555).abs() < 0.01);
    }
}
