use crate::models::Transcript;
use std::fmt::Write;

/// Exporter for converting transcripts to various subtitle and document formats.
pub struct TranscriptExporter;

impl TranscriptExporter {
    /// Export transcript as SubRip (.srt) subtitle format.
    #[must_use]
    pub fn to_srt(transcript: &Transcript) -> String {
        let mut out = String::new();
        for (i, seg) in transcript.segments.iter().enumerate() {
            let start = format_srt_time(seg.start_ms);
            let end = format_srt_time(seg.end_ms);
            let speaker = seg
                .speaker_role
                .display_label(Some(seg.speaker_id.as_u16()));
            let _ = writeln!(
                out,
                "{}\n{} --> {}\n[{}] {}\n",
                i + 1,
                start,
                end,
                speaker,
                seg.normalized_text
            );
        }
        out
    }

    /// Export transcript as WebVTT (.vtt) subtitle format.
    #[must_use]
    pub fn to_vtt(transcript: &Transcript) -> String {
        let mut out = String::from("WEBVTT - CallMind Transcript\n\n");
        for (i, seg) in transcript.segments.iter().enumerate() {
            let start = format_vtt_time(seg.start_ms);
            let end = format_vtt_time(seg.end_ms);
            let speaker = seg
                .speaker_role
                .display_label(Some(seg.speaker_id.as_u16()));
            let _ = writeln!(
                out,
                "{}\n{} --> {}\n<v {}>{}\n",
                i + 1,
                start,
                end,
                speaker,
                seg.normalized_text
            );
        }
        out
    }

    /// Export transcript as clean dialogue Plain Text (.txt).
    #[must_use]
    pub fn to_txt(transcript: &Transcript) -> String {
        let mut out = String::new();
        for seg in &transcript.segments {
            let time = format_mm_ss(seg.start_ms);
            let speaker = seg
                .speaker_role
                .display_label(Some(seg.speaker_id.as_u16()));
            let _ = writeln!(out, "[{time}] {speaker}: {}", seg.normalized_text);
        }
        out
    }

    /// Export transcript as formatted Markdown (.md).
    #[must_use]
    pub fn to_markdown(transcript: &Transcript, title: Option<&str>) -> String {
        let mut out = String::new();
        let doc_title = title.unwrap_or("Conversation Transcript");
        let duration = format_mm_ss(transcript.duration_ms());

        let _ = writeln!(out, "# {doc_title}\n");
        let _ = writeln!(out, "- **Duration:** {duration}");
        let _ = writeln!(out, "- **Total Segments:** {}\n", transcript.segments.len());
        let _ = writeln!(out, "## Transcript\n");

        for seg in &transcript.segments {
            let time = format_mm_ss(seg.start_ms);
            let speaker = seg
                .speaker_role
                .display_label(Some(seg.speaker_id.as_u16()));
            let _ = writeln!(out, "- **[{time}] {speaker}**: {}", seg.normalized_text);
        }
        out
    }
}

fn format_srt_time(ms: u64) -> String {
    let hours = ms / 3_600_000;
    let mins = (ms % 3_600_000) / 60_000;
    let secs = (ms % 60_000) / 1_000;
    let millis = ms % 1_000;
    format!("{hours:02}:{mins:02}:{secs:02},{millis:03}")
}

fn format_vtt_time(ms: u64) -> String {
    let hours = ms / 3_600_000;
    let mins = (ms % 3_600_000) / 60_000;
    let secs = (ms % 60_000) / 1_000;
    let millis = ms % 1_000;
    format!("{hours:02}:{mins:02}:{secs:02}.{millis:03}")
}

fn format_mm_ss(ms: u64) -> String {
    let mins = ms / 60_000;
    let secs = (ms % 60_000) / 1_000;
    format!("{mins:02}:{secs:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{TextDirection, TranscriptSegment};
    use callmind_core::{CallId, Language, SpeakerId, SpeakerRole};
    use uuid::Uuid;

    #[test]
    fn test_exporters() {
        let seg = TranscriptSegment {
            id: Uuid::new_v4(),
            call_id: CallId::generate(),
            sequence: 0,
            speaker_id: SpeakerId::new(0),
            speaker_role: SpeakerRole::Speaker1,
            language: Language::Hebrew,
            text_direction: TextDirection::Rtl,
            start_ms: 1500,
            end_ms: 4500,
            raw_text: "שלום עליכם".into(),
            normalized_text: "שלום עליכם".into(),
            words: Vec::new(),
        };

        let transcript = Transcript {
            call_id: seg.call_id,
            languages: Vec::new(),
            speakers: Vec::new(),
            segments: vec![seg],
        };

        let srt = TranscriptExporter::to_srt(&transcript);
        assert!(srt.contains("00:00:01,500"));
        assert!(srt.contains("[Speaker 1] שלום עליכם"));

        let vtt = TranscriptExporter::to_vtt(&transcript);
        assert!(vtt.starts_with("WEBVTT"));
        assert!(vtt.contains("<v Speaker 1>שלום עליכם"));

        let txt = TranscriptExporter::to_txt(&transcript);
        assert!(txt.contains("[00:01] Speaker 1: שלום עליכם"));

        let md = TranscriptExporter::to_markdown(&transcript, Some("Test Call"));
        assert!(md.contains("# Test Call"));
    }
}
