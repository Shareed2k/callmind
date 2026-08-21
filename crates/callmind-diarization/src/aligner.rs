use crate::models::SpeakerTurn;
use callmind_core::SpeakerId;
use callmind_stt::SttWord;

/// Word-to-speaker temporal alignment engine.
pub struct TranscriptAligner;

impl TranscriptAligner {
    /// Attribute each transcribed word to the speaker turn with the maximum temporal overlap.
    pub fn align(words: &[SttWord], turns: &[SpeakerTurn]) -> Vec<SttWord> {
        if turns.is_empty() {
            // Default all words to Speaker 0 if no diarization turns exist
            return words.to_vec();
        }

        let mut aligned_words = Vec::with_capacity(words.len());

        for word in words {
            let mut best_speaker: Option<SpeakerId> = None;
            let mut max_overlap: u64 = 0;
            let mut min_distance: u64 = u64::MAX;
            let mut closest_speaker: Option<SpeakerId> = None;

            for turn in turns {
                let overlap = turn.overlap_ms(word.start_ms, word.end_ms);
                if overlap > max_overlap {
                    max_overlap = overlap;
                    best_speaker = Some(turn.speaker);
                }

                // Track closest turn in case of non-overlapping edge cases
                let dist = if word.end_ms <= turn.start_ms {
                    turn.start_ms.saturating_sub(word.end_ms)
                } else {
                    word.start_ms.saturating_sub(turn.end_ms)
                };

                if dist < min_distance {
                    min_distance = dist;
                    closest_speaker = Some(turn.speaker);
                }
            }

            let speaker = best_speaker.or(closest_speaker);

            let mut aligned = word.clone();
            aligned.speaker_id = speaker;
            aligned_words.push(aligned);
        }

        aligned_words
    }

    /// Match a specific time range to the best matching speaker.
    pub fn find_speaker_for_range(start_ms: u64, end_ms: u64, turns: &[SpeakerTurn]) -> SpeakerId {
        let mut best_speaker = SpeakerId::new(0);
        let mut max_overlap = 0;
        let mut min_distance = u64::MAX;
        let mut closest_speaker = SpeakerId::new(0);

        for turn in turns {
            let overlap = turn.overlap_ms(start_ms, end_ms);
            if overlap > max_overlap {
                max_overlap = overlap;
                best_speaker = turn.speaker;
            }

            let dist = if end_ms <= turn.start_ms {
                turn.start_ms.saturating_sub(end_ms)
            } else {
                start_ms.saturating_sub(turn.end_ms)
            };

            if dist < min_distance {
                min_distance = dist;
                closest_speaker = turn.speaker;
            }
        }

        if max_overlap > 0 {
            best_speaker
        } else {
            closest_speaker
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_speaker_for_range() {
        let turns = vec![
            SpeakerTurn::new(SpeakerId::new(0), 0, 1000, Some(0.9)),
            SpeakerTurn::new(SpeakerId::new(1), 1200, 2500, Some(0.95)),
        ];

        // Within Speaker 0 turn
        assert_eq!(
            TranscriptAligner::find_speaker_for_range(100, 400, &turns),
            SpeakerId::new(0)
        );

        // Within Speaker 1 turn
        assert_eq!(
            TranscriptAligner::find_speaker_for_range(1300, 1800, &turns),
            SpeakerId::new(1)
        );

        // Close to Speaker 1
        assert_eq!(
            TranscriptAligner::find_speaker_for_range(1100, 1150, &turns),
            SpeakerId::new(1)
        );
    }
}
