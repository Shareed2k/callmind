use crate::models::TranscriptSegment;
use callmind_audio::ChannelMode;
use callmind_core::{SpeakerId, SpeakerRole};
use std::collections::HashMap;

/// Identifies conversational roles (Agent, Customer, Supervisor) from audio metadata & speech patterns.
pub struct RoleIdentifier;

impl RoleIdentifier {
    /// Infer speaker roles across transcript segments.
    pub fn identify_roles(
        segments: &[TranscriptSegment],
        channel_mode: &ChannelMode,
    ) -> HashMap<SpeakerId, SpeakerRole> {
        let mut roles = HashMap::new();

        // 1. Channel-based identification for Stereo with acoustic greeting check
        if let ChannelMode::StereoSeparated {
            left_channel,
            right_channel,
        } = channel_mode
        {
            let spk_left = SpeakerId::new(*left_channel as u16);
            let spk_right = SpeakerId::new(*right_channel as u16);

            // Check if right channel actually delivered the agent greeting
            let right_is_agent = segments
                .iter()
                .filter(|s| s.speaker_id == spk_right)
                .take(3)
                .any(|s| is_agent_greeting(&s.raw_text.to_lowercase()));

            if right_is_agent {
                roles.insert(spk_right, SpeakerRole::Agent);
                roles.insert(spk_left, SpeakerRole::Customer);
            } else {
                // Default: left channel is agent, right channel is customer
                roles.insert(spk_left, SpeakerRole::Agent);
                roles.insert(spk_right, SpeakerRole::Customer);
            }
            return roles;
        }

        // 2. Phrase-based heuristic identification for Mono audio
        let mut agent_speaker: Option<SpeakerId> = None;

        for segment in segments.iter().take(4) {
            let lower = segment.raw_text.to_lowercase();
            if is_agent_greeting(&lower) {
                agent_speaker = Some(segment.speaker_id);
                break;
            }
        }

        if let Some(agent_id) = agent_speaker {
            roles.insert(agent_id, SpeakerRole::Agent);
            // Any other speaker is attributed as Customer
            for segment in segments {
                if segment.speaker_id != agent_id {
                    roles.insert(segment.speaker_id, SpeakerRole::Customer);
                }
            }
        } else {
            // Default convention: Speaker 0 = Agent, Speaker 1 = Customer
            roles.insert(SpeakerId::new(0), SpeakerRole::Agent);
            roles.insert(SpeakerId::new(1), SpeakerRole::Customer);
        }

        roles
    }
}

/// Helper identifying representative company greeting phrases across Hebrew, Russian, and English.
fn is_agent_greeting(text: &str) -> bool {
    // Hebrew agent opening patterns
    text.contains("במה אפשר לעזור") ||
    text.contains("מדבר") ||
    text.contains("מדברת") ||
    text.contains("תודה שהתקשרת") ||
    text.contains("מוקד שירות") ||
    text.contains("שירות לקוחות") ||
    // Russian agent opening patterns
    text.contains("чем я могу помочь") ||
    text.contains("чем могу помочь") ||
    text.contains("служба поддержки") ||
    text.contains("компания") ||
    text.contains("меня зовут") ||
    // English agent opening patterns
    text.contains("how can i help") ||
    text.contains("thank you for calling") ||
    text.contains("customer support")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{TextDirection, TranscriptSegment};
    use callmind_core::{CallId, Language};
    use uuid::Uuid;

    #[test]
    fn test_role_identification_from_greeting() {
        let seg1 = TranscriptSegment {
            id: Uuid::new_v4(),
            call_id: CallId::generate(),
            sequence: 0,
            speaker_id: SpeakerId::new(1),
            speaker_role: SpeakerRole::Unknown,
            language: Language::Hebrew,
            text_direction: TextDirection::Rtl,
            start_ms: 0,
            end_ms: 2000,
            raw_text: "שלום, מדבר דני מחברת בזק, במה אפשר לעזור?".into(),
            normalized_text: "שלום, מדבר דני מחברת בזק, במה אפשר לעזור?".into(),
            words: Vec::new(),
        };

        let seg2 = TranscriptSegment {
            id: Uuid::new_v4(),
            call_id: CallId::generate(),
            sequence: 1,
            speaker_id: SpeakerId::new(0),
            speaker_role: SpeakerRole::Unknown,
            language: Language::Russian,
            text_direction: TextDirection::Ltr,
            start_ms: 2500,
            end_ms: 4500,
            raw_text: "Здравствуйте, у меня не работает интернет".into(),
            normalized_text: "Здравствуйте, у меня не работает интернет".into(),
            words: Vec::new(),
        };

        let roles = RoleIdentifier::identify_roles(&[seg1, seg2], &ChannelMode::Mono);
        assert_eq!(roles.get(&SpeakerId::new(1)), Some(&SpeakerRole::Agent));
        assert_eq!(roles.get(&SpeakerId::new(0)), Some(&SpeakerRole::Customer));
    }
}
