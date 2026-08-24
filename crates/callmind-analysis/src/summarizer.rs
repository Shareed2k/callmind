use callmind_core::Language;
use callmind_transcript::Transcript;

/// Smart language-aware conversation summarizer and topic classifier.
pub struct ConversationSummarizer;

pub struct HeuristicSummary {
    pub title: String,
    pub summary: String,
    pub reason: Option<String>,
    pub resolution: Option<String>,
    pub intent: Option<String>,
    pub topics: Vec<String>,
}

impl ConversationSummarizer {
    /// A stand-in used when the model fails, and to fill single fields it left
    /// out. It states only what the transcript states.
    ///
    /// It used to match keywords onto canned templates, which asserted things the
    /// recording never said: a call about buying a laptop, containing the word
    /// "order" once, came back titled "delivery coordination and courier arrival"
    /// with the resolution "delivery details were successfully coordinated". None
    /// of it was in the audio, and because the analyzer also uses this to fill
    /// missing fields, the invention reached otherwise-correct analyses.
    ///
    /// Anything that cannot be known without reading the call is `None`. An empty
    /// field is a correct answer; a plausible guess is a wrong one.
    #[must_use]
    pub fn summarize(transcript: &Transcript, primary_lang: &Language) -> HeuristicSummary {
        HeuristicSummary {
            title: Self::label(primary_lang).to_string(),
            summary: Self::opening(transcript),
            reason: None,
            resolution: None,
            intent: None,
            topics: Vec::new(),
        }
    }

    /// The one claim true of every recording, in the language of the call.
    fn label(language: &Language) -> &'static str {
        match language {
            Language::Hebrew => "שיחה מוקלטת",
            Language::Russian => "Записанный разговор",
            _ => "Recorded conversation",
        }
    }

    /// The start of the conversation, verbatim, so the call is still
    /// recognisable in a list without claiming to summarise it.
    fn opening(transcript: &Transcript) -> String {
        const MAX_CHARS: usize = 280;

        let mut out = String::new();
        for segment in &transcript.segments {
            let text = segment.normalized_text.trim();
            if text.is_empty() {
                continue;
            }
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(text);
            if out.chars().count() >= MAX_CHARS {
                break;
            }
        }

        if out.chars().count() > MAX_CHARS {
            let mut truncated: String = out.chars().take(MAX_CHARS).collect();
            truncated.push('…');
            return truncated;
        }
        out
    }
}

#[cfg(test)]
mod honest_fallback_tests {
    use super::*;
    use callmind_core::{CallId, SpeakerId, SpeakerRole};
    use callmind_transcript::{TextDirection, TranscriptSegment};
    use uuid::Uuid;

    fn transcript(language: &Language, lines: &[&str]) -> Transcript {
        Transcript {
            call_id: CallId::generate(),
            languages: Vec::new(),
            speakers: Vec::new(),
            segments: lines
                .iter()
                .enumerate()
                .map(|(i, text)| TranscriptSegment {
                    id: Uuid::new_v4(),
                    call_id: CallId::generate(),
                    sequence: u32::try_from(i).unwrap_or(0),
                    speaker_id: SpeakerId::new(u16::try_from(i % 2).unwrap_or(0)),
                    speaker_role: SpeakerRole::Customer,
                    language: language.clone(),
                    text_direction: TextDirection::Rtl,
                    start_ms: (i as u64) * 1000,
                    end_ms: (i as u64) * 1000 + 900,
                    raw_text: (*text).to_string(),
                    normalized_text: (*text).to_string(),
                    words: Vec::new(),
                })
                .collect(),
        }
    }

    /// The keyword templates asserted things the recording never said: a call
    /// about buying a laptop, containing the word "order" once, was titled
    /// "delivery coordination and courier arrival" with the resolution "delivery
    /// details were successfully coordinated" -- an invented outcome. This path
    /// also fills individual fields the LLM left out, so the invention reached
    /// otherwise-good analyses.
    #[test]
    fn it_does_not_claim_anything_the_recording_did_not_say() {
        let call = transcript(
            &Language::Hebrew,
            &[
                "שלום, יש לכם במלאי את הדגם Asus New 14 Ultra 7?",
                "אני רוצה לבצע הזמנה טלפונית ולשלם במזומן.",
            ],
        );

        let summary = ConversationSummarizer::summarize(&call, &Language::Hebrew);

        for invented in ["שליח", "חבילה", "כתובת"] {
            assert!(
                !summary.title.contains(invented) && !summary.summary.contains(invented),
                "{invented} was never said: {} / {}",
                summary.title,
                summary.summary
            );
        }
        assert!(
            summary.resolution.is_none(),
            "no outcome can be known without reading the call: {:?}",
            summary.resolution
        );
        assert!(
            summary.reason.is_none() && summary.intent.is_none(),
            "a guessed reason is a wrong answer, an absent one is correct"
        );
        assert!(summary.topics.is_empty(), "topics require actually reading");
    }

    /// What it may say is what the transcript says, so the fallback still leaves
    /// something a person can recognise the call by.
    #[test]
    fn it_quotes_the_conversation_it_was_given() {
        let call = transcript(&Language::Russian, &["Привет, я по поводу счёта."]);
        let summary = ConversationSummarizer::summarize(&call, &Language::Russian);
        assert!(
            summary.summary.contains("Привет, я по поводу счёта."),
            "the opening is the one thing we know was said: {}",
            summary.summary
        );
        assert!(!summary.title.is_empty(), "the list still needs a label");
    }
}
