use crate::models::{DiarizationResult, SpeakerTurn};
use callmind_core::SpeakerId;
use std::collections::HashMap;
use std::fmt::Write;

/// A single time interval in a ground truth or predicted diarization.
#[derive(Debug, Clone, PartialEq)]
pub struct GroundTruthTurn {
    pub speaker: String,
    pub start_ms: u64,
    pub end_ms: u64,
}

impl GroundTruthTurn {
    pub fn new(speaker: &str, start_ms: u64, end_ms: u64) -> Self {
        Self {
            speaker: speaker.to_string(),
            start_ms,
            end_ms,
        }
    }

    pub fn duration_ms(&self) -> u64 {
        self.end_ms.saturating_sub(self.start_ms)
    }
}

/// Detailed results of a Diarization Error Rate (DER) evaluation.
#[derive(Debug, Clone, PartialEq)]
pub struct DerEvaluation {
    /// Overall Diarization Error Rate (0.0 to 1.0, or >1.0 if false alarms exceed total speech).
    pub der: f64,
    /// Ratio of reference speech missed by the system.
    pub missed_ratio: f64,
    /// Ratio of non-speech falsely detected as speech.
    pub false_alarm_ratio: f64,
    /// Ratio of speech attributed to the wrong speaker.
    pub speaker_confusion_ratio: f64,
    /// Total reference speech duration in milliseconds.
    pub total_speech_ms: u64,
}

/// Utility for NIST RTTM parsing, generation, and Diarization Error Rate (DER) computation.
pub struct DerCalculator;

impl DerCalculator {
    /// Export speaker turns to standard NIST RTTM format string.
    #[must_use]
    pub fn to_rttm(uri: &str, turns: &[SpeakerTurn]) -> String {
        let mut out = String::new();
        for turn in turns {
            let start_sec = turn.start_ms as f64 / 1000.0;
            let dur_sec = turn.duration_ms() as f64 / 1000.0;
            let spk = turn.speaker;
            let _ = writeln!(
                out,
                "SPEAKER {uri} 1 {start_sec:.3} {dur_sec:.3} <NA> <NA> {spk} <NA> <NA>"
            );
        }
        out
    }

    /// Parse a standard NIST RTTM string into ground truth turns.
    #[must_use]
    pub fn from_rttm(rttm_content: &str) -> Vec<GroundTruthTurn> {
        let mut turns = Vec::new();
        for line in rttm_content.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 8 && parts[0] == "SPEAKER" {
                if let (Ok(start_sec), Ok(dur_sec)) =
                    (parts[3].parse::<f64>(), parts[4].parse::<f64>())
                {
                    let start_ms = (start_sec * 1000.0).round() as u64;
                    let end_ms = start_ms + (dur_sec * 1000.0).round() as u64;
                    let speaker = parts[7];
                    turns.push(GroundTruthTurn::new(speaker, start_ms, end_ms));
                }
            }
        }
        turns.sort_by_key(|t| t.start_ms);
        turns
    }

    /// Evaluate Diarization Error Rate (DER) comparing predicted turns against ground truth.
    #[must_use]
    pub fn evaluate(
        reference: &[GroundTruthTurn],
        hypothesis: &DiarizationResult,
    ) -> DerEvaluation {
        let mut total_ref_ms: u64 = 0;
        for turn in reference {
            total_ref_ms += turn.duration_ms();
        }

        if total_ref_ms == 0 {
            return DerEvaluation {
                der: 0.0,
                missed_ratio: 0.0,
                false_alarm_ratio: 0.0,
                speaker_confusion_ratio: 0.0,
                total_speech_ms: 0,
            };
        }

        // Build confusion matrix between reference and hypothesis speakers
        let mut overlap_matrix: HashMap<(String, SpeakerId), u64> = HashMap::new();
        let mut hyp_total_ms: u64 = 0;

        for hyp in &hypothesis.turns {
            hyp_total_ms += hyp.duration_ms();
            for r in reference {
                let start = hyp.start_ms.max(r.start_ms);
                let end = hyp.end_ms.min(r.end_ms);
                if start < end {
                    let overlap = end - start;
                    *overlap_matrix
                        .entry((r.speaker.clone(), hyp.speaker))
                        .or_insert(0) += overlap;
                }
            }
        }

        // Greedy / Maximum Bipartite Matching
        let mut matched_overlap_total: u64 = 0;
        let mut sorted_overlaps: Vec<((String, SpeakerId), u64)> =
            overlap_matrix.into_iter().collect();
        sorted_overlaps.sort_by_key(|b| std::cmp::Reverse(b.1));

        let mut used_ref = std::collections::HashSet::new();
        let mut used_hyp = std::collections::HashSet::new();

        for ((ref_spk, hyp_spk), overlap) in sorted_overlaps {
            if !used_ref.contains(&ref_spk) && !used_hyp.contains(&hyp_spk) {
                used_ref.insert(ref_spk);
                used_hyp.insert(hyp_spk);
                matched_overlap_total += overlap;
            }
        }

        // Total speech overlap between reference and hypothesis
        let mut speech_overlap_total: u64 = 0;
        for hyp in &hypothesis.turns {
            for r in reference {
                let start = hyp.start_ms.max(r.start_ms);
                let end = hyp.end_ms.min(r.end_ms);
                if start < end {
                    speech_overlap_total += end - start;
                }
            }
        }

        let missed_ms = total_ref_ms.saturating_sub(speech_overlap_total);
        let false_alarm_ms = hyp_total_ms.saturating_sub(speech_overlap_total);
        let confusion_ms = speech_overlap_total.saturating_sub(matched_overlap_total);

        let missed_ratio = missed_ms as f64 / total_ref_ms as f64;
        let false_alarm_ratio = false_alarm_ms as f64 / total_ref_ms as f64;
        let speaker_confusion_ratio = confusion_ms as f64 / total_ref_ms as f64;

        let der = missed_ratio + false_alarm_ratio + speaker_confusion_ratio;

        DerEvaluation {
            der,
            missed_ratio,
            false_alarm_ratio,
            speaker_confusion_ratio,
            total_speech_ms: total_ref_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rttm_parser_and_der() {
        let rttm = r#"
SPEAKER call_01 1 0.000 2.000 <NA> <NA> Alice <NA> <NA>
SPEAKER call_01 1 2.500 2.000 <NA> <NA> Bob <NA> <NA>
SPEAKER call_01 1 5.000 2.000 <NA> <NA> Alice <NA> <NA>
"#;
        let reference = DerCalculator::from_rttm(rttm);
        assert_eq!(reference.len(), 3);
        assert_eq!(reference[0].speaker, "Alice");
        assert_eq!(reference[1].speaker, "Bob");

        // Hypothesis with correct speaker mapping
        let hyp_turns = vec![
            SpeakerTurn::new(SpeakerId::new(0), 0, 2000, Some(0.95)),
            SpeakerTurn::new(SpeakerId::new(1), 2500, 4500, Some(0.95)),
            SpeakerTurn::new(SpeakerId::new(0), 5000, 7000, Some(0.95)),
        ];
        let hypothesis = DiarizationResult::new(2, hyp_turns);

        let eval = DerCalculator::evaluate(&reference, &hypothesis);
        assert!(eval.der < 1e-6, "Perfect alignment should have 0.0 DER");
        assert!(eval.speaker_confusion_ratio < 1e-6);
    }
}
