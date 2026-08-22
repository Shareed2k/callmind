use crate::models::{SpeakerMetadata, Transcript, TranscriptSegment, TranscriptWord};
use crate::normalizer::TextNormalizer;
use crate::roles::RoleIdentifier;
use crate::rtl::RtlDetector;
use crate::vocabulary::VocabularyEntry;
use callmind_audio::ChannelMode;
use callmind_core::{CallId, Language, SpeakerId, SpeakerRole};
use callmind_language::LanguageProbability;
use callmind_stt::SttWord;
use std::collections::HashMap;
use uuid::Uuid;

/// Builds and structures raw STT tokens into full conversational transcripts.
pub struct TranscriptBuilder;

impl TranscriptBuilder {
    /// Build a complete `Transcript` from transcribed words, channel mode, and vocabulary.
    pub fn build(
        call_id: CallId,
        words: &[SttWord],
        channel_mode: &ChannelMode,
        vocabulary: &[VocabularyEntry],
        languages: Vec<LanguageProbability>,
    ) -> Transcript {
        Self::build_with_mapping(call_id, words, channel_mode, None, vocabulary, languages)
    }

    /// Build a complete `Transcript` with optional explicit PBX channel mapping.
    pub fn build_with_mapping(
        call_id: CallId,
        words: &[SttWord],
        channel_mode: &ChannelMode,
        channel_mapping: Option<&callmind_core::ChannelMapping>,
        vocabulary: &[VocabularyEntry],
        languages: Vec<LanguageProbability>,
    ) -> Transcript {
        if words.is_empty() {
            return Transcript {
                call_id,
                languages,
                speakers: Vec::new(),
                segments: Vec::new(),
            };
        }

        // 1. Group consecutive words into initial segments (max pause gap 1500ms, or speaker/language change)
        let mut raw_segments: Vec<(SpeakerId, Language, Vec<SttWord>)> = Vec::new();
        let mut current_speaker = SpeakerId::new(0);
        let mut current_lang = Language::Unknown;
        let mut current_group: Vec<SttWord> = Vec::new();

        for word in words {
            let spk = word.speaker_id.unwrap_or(SpeakerId::new(0));
            let lang = word.language.clone().unwrap_or(Language::Unknown);

            let is_new_group = current_group.is_empty()
                || spk != current_speaker
                || lang != current_lang
                || (word.start_ms > current_group.last().unwrap().end_ms + 1500);

            if is_new_group {
                if !current_group.is_empty() {
                    raw_segments.push((current_speaker, current_lang, current_group));
                }
                current_speaker = spk;
                current_lang = lang;
                current_group = vec![word.clone()];
            } else {
                current_group.push(word.clone());
            }
        }

        if !current_group.is_empty() {
            raw_segments.push((current_speaker, current_lang, current_group));
        }

        // 2. Build initial segments
        let mut segments = Vec::with_capacity(raw_segments.len());

        for (seq, (speaker_id, mut language, group_words)) in raw_segments.into_iter().enumerate() {
            let start_ms = group_words.first().map_or(0, |w| w.start_ms);
            let end_ms = group_words.last().map_or(0, |w| w.end_ms);

            let raw_text = group_words
                .iter()
                .map(|w| w.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");

            let text_language = RtlDetector::detect_language(&raw_text);
            if language == Language::Unknown
                || matches!(
                    text_language,
                    Language::Hebrew | Language::Russian | Language::Arabic
                )
            {
                language = text_language;
            }

            let normalized_text = TextNormalizer::normalize(&raw_text, vocabulary);
            let text_direction = RtlDetector::detect(&raw_text, &language);

            let transcript_words: Vec<TranscriptWord> = group_words
                .into_iter()
                .map(|w| TranscriptWord {
                    text: w.text,
                    start_ms: w.start_ms,
                    end_ms: w.end_ms,
                    speaker_id,
                    speaker_role: SpeakerRole::Unknown,
                    language: language.clone(),
                    confidence: w.confidence,
                })
                .collect();

            segments.push(TranscriptSegment {
                id: Uuid::new_v4(),
                call_id,
                sequence: seq as u32,
                speaker_id,
                speaker_role: SpeakerRole::Unknown,
                language,
                text_direction,
                start_ms,
                end_ms,
                raw_text,
                normalized_text,
                words: transcript_words,
            });
        }

        // 3. Infer speaker roles (respecting explicit channel mapping)
        let role_map = RoleIdentifier::identify_roles(&segments, channel_mode, channel_mapping);

        for segment in &mut segments {
            if let Some(&role) = role_map.get(&segment.speaker_id) {
                segment.speaker_role = role;
                for w in &mut segment.words {
                    w.speaker_role = role;
                }
            }
        }

        // 4. Calculate speaker metrics / metadata
        let mut speaker_talk_time: HashMap<SpeakerId, (SpeakerRole, u64, usize)> = HashMap::new();

        for segment in &segments {
            let entry =
                speaker_talk_time
                    .entry(segment.speaker_id)
                    .or_insert((segment.speaker_role, 0, 0));
            entry.1 += segment.duration_ms();
            entry.2 += segment.words.len();
        }

        let mut speakers: Vec<SpeakerMetadata> = speaker_talk_time
            .into_iter()
            .map(
                |(speaker_id, (role, talk_time_ms, word_count))| SpeakerMetadata {
                    speaker_id,
                    role,
                    talk_time_ms,
                    word_count,
                },
            )
            .collect();

        speakers.sort_by_key(|s| s.speaker_id.0);

        Transcript {
            call_id,
            languages,
            speakers,
            segments,
        }
    }
}
