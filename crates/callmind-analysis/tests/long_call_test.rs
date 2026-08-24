//! A long call must not be analysed from its opening minutes.
//!
//! The prompt used to be the whole transcript, and Ollama silently drops what
//! does not fit its context window -- so a 13-minute call produced a summary of
//! the first couple of minutes with no error anywhere.

use callmind_analysis::{AnalysisEngine, Classifier};
use callmind_core::{CallId, Language, SpeakerId, SpeakerRole};
use callmind_llm::{LlmEngine, LlmError};
use callmind_transcript::{TextDirection, Transcript, TranscriptSegment};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// Records every prompt it is given, so a test can assert on what the model
/// actually received rather than on the analyser's own reporting.
#[derive(Default)]
struct RecordingLlm {
    json_prompts: Mutex<Vec<String>>,
    text_prompts: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl LlmEngine for RecordingLlm {
    async fn generate_json(
        &self,
        prompt: &str,
        _system: Option<&str>,
    ) -> Result<serde_json::Value, LlmError> {
        self.json_prompts.lock().unwrap().push(prompt.to_string());
        Ok(serde_json::json!({
            "title": "Заголовок",
            "summary": "Резюме разговора.",
            "resolved": true,
            "sentiment_score": 0.1
        }))
    }

    async fn generate_text(&self, prompt: &str, _system: Option<&str>) -> Result<String, LlmError> {
        self.text_prompts.lock().unwrap().push(prompt.to_string());
        Ok("сжатое изложение фрагмента".to_string())
    }
}

fn long_transcript(segments: usize) -> Transcript {
    let call_id = CallId::generate();
    let mut out = Vec::with_capacity(segments);
    for i in 0..segments {
        let text =
            format!("реплика номер {i}, достаточно длинная чтобы занять место в контексте модели");
        out.push(TranscriptSegment {
            id: Uuid::new_v4(),
            call_id,
            sequence: i as u32,
            speaker_id: SpeakerId::new(u16::from(i % 2 == 0)),
            speaker_role: if i % 2 == 0 {
                SpeakerRole::Agent
            } else {
                SpeakerRole::Customer
            },
            language: Language::Russian,
            text_direction: TextDirection::Ltr,
            start_ms: (i as u64) * 4000,
            end_ms: (i as u64) * 4000 + 3500,
            raw_text: text.clone(),
            normalized_text: text,
            words: Vec::new(),
        });
    }
    Transcript {
        call_id,
        languages: Vec::new(),
        speakers: Vec::new(),
        segments: out,
    }
}

/// Roughly what the analyser uses internally; kept pessimistic on purpose.
fn estimated_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(3)
}

#[tokio::test]
async fn a_transcript_over_the_context_budget_is_compressed_first() {
    let llm = Arc::new(RecordingLlm::default());
    let budget = 1024;
    let engine = AnalysisEngine::new(llm.clone()).with_context_tokens(budget);

    let transcript = long_transcript(400);
    let analysis = engine
        .analyze(&transcript, "Организация", &[] as &[Classifier])
        .await
        .expect("analysis succeeds");

    let text_calls = llm.text_prompts.lock().unwrap().len();
    assert!(
        text_calls > 1,
        "a transcript this long must be compressed window by window, got {text_calls} calls"
    );

    let json_prompts = llm.json_prompts.lock().unwrap();
    assert_eq!(json_prompts.len(), 1, "one structured analysis at the end");
    let tokens = estimated_tokens(&json_prompts[0]);
    assert!(
        tokens <= budget,
        "the final prompt is {tokens} tokens, over the {budget} the model was given"
    );

    assert!(!analysis.summary.is_empty(), "an analysis still comes out");
}

/// A short call must not pay for compression it does not need.
#[tokio::test]
async fn a_short_transcript_goes_straight_to_analysis() {
    let llm = Arc::new(RecordingLlm::default());
    let engine = AnalysisEngine::new(llm.clone()).with_context_tokens(8192);

    engine
        .analyze(&long_transcript(3), "Организация", &[] as &[Classifier])
        .await
        .expect("analysis succeeds");

    assert_eq!(
        llm.text_prompts.lock().unwrap().len(),
        0,
        "nothing to compress, so no compression calls"
    );
    assert_eq!(llm.json_prompts.lock().unwrap().len(), 1);
}
