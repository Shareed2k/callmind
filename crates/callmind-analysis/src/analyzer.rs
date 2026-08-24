use crate::classifiers::Classifier;
use crate::metrics::ConversationMetricsCalculator;
use crate::models::{
    ActionItem, CallAnalysis, ComplianceResult, Entity, Evidence, ScoreRuleResult, ScorecardResult,
    SentimentPoint, Topic,
};
use callmind_core::SpeakerRole;
use callmind_llm::{
    CONVERSATION_ANALYSIS_SYSTEM_PROMPT, LlmEngine, LlmError, build_language_aware_analysis_prompt,
    build_window_compression_prompt,
};
use callmind_transcript::Transcript;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fmt::Write;
use std::sync::Arc;
use thiserror::Error;
use tracing::warn;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum AnalysisError {
    #[error("LLM execution error: {0}")]
    Llm(#[from] LlmError),

    #[error("Transcript has no speech segments to analyze")]
    EmptyTranscript,
}

/// The analysis shape the prompt asks the model for.
///
/// Every scalar field goes through [`crate::lenient`] rather than serde's strict
/// deserialization. A local model slips on a type now and then -- a list of
/// bullet points where the schema says string, `"0.8"` where it says number --
/// and because this is one struct, one such slip used to discard every other
/// field the model got right and fall back to the regex summarizer. The
/// `Value`-typed fields below were already coerced by hand where they are used,
/// so this makes the whole struct consistently forgiving instead of leaving the
/// scalars as the one strict thing in it.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct LlmRawAnalysis {
    #[serde(default, deserialize_with = "crate::lenient::string")]
    title: Option<String>,
    #[serde(default, deserialize_with = "crate::lenient::string")]
    summary: Option<String>,
    #[serde(default, deserialize_with = "crate::lenient::string")]
    reason: Option<String>,
    #[serde(default, deserialize_with = "crate::lenient::string")]
    resolution: Option<String>,
    #[serde(default, deserialize_with = "crate::lenient::boolean")]
    resolved: Option<bool>,
    #[serde(default, deserialize_with = "crate::lenient::string")]
    customer_intent: Option<String>,
    #[serde(default)]
    topics: Option<serde_json::Value>,
    #[serde(default)]
    key_facts: Option<serde_json::Value>,
    #[serde(default)]
    action_items: Option<serde_json::Value>,
    #[serde(default)]
    entities: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "crate::lenient::number")]
    sentiment_score: Option<f32>,
    #[serde(default)]
    scorecard: Option<serde_json::Value>,
    #[serde(default)]
    compliance: Option<serde_json::Value>,
}

/// Orchestrator for conversation intelligence analysis.
pub struct AnalysisEngine {
    llm: Arc<dyn LlmEngine>,
    context_tokens: usize,
}

/// Whether generated prose is one phrase repeated rather than a summary.
///
/// A model can return valid JSON whose contents are degenerate -- the retry in
/// the LLM engine only catches answers that fail to parse. Measured on a real
/// call whose hold announcement repeats a company name, the stored summary read
/// `OPC, OPC, OPC, OPC, OPC`.
///
/// Rejects only text where words repeat about three times over on average, so
/// ordinary prose, which repeats function words, is left alone.
fn is_degenerate(text: &str) -> bool {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() < 4 {
        return false;
    }
    let distinct: std::collections::HashSet<String> = words
        .iter()
        .map(|word| {
            word.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .collect();
    distinct.len() * 3 <= words.len()
}

/// Whether generated prose wandered out of the alphabets this project handles.
///
/// A local model drifts toward its dominant language mid-answer: qwen2.5:7b
/// finished a Hebrew summary in Chinese, and the JSON stayed valid and the text
/// did not repeat, so neither the retry in the LLM engine nor
/// [`is_degenerate`] noticed. One archived analysis carried 33 CJK characters
/// among 194 letters.
///
/// Latin inside Hebrew or Russian is ordinary -- product names, model numbers --
/// so only Hebrew, Latin, Cyrillic and Arabic count as at home here. A lone
/// stray character is cosmetic and a summary may legitimately quote a foreign
/// name, so a run is what counts: at least three characters and more than two
/// percent of the letters, which is well under the 17% that reached the archive.
fn has_foreign_script(text: &str) -> bool {
    const MIN_RUN: usize = 3;
    const MAX_SHARE: f32 = 0.02;

    let mut letters = 0usize;
    let mut foreign = 0usize;

    for ch in text.chars() {
        if !ch.is_alphabetic() {
            continue;
        }
        letters += 1;
        if !is_expected_script(ch) {
            foreign += 1;
        }
    }

    foreign >= MIN_RUN && foreign as f32 > MAX_SHARE * letters as f32
}

/// Whether a letter belongs to an alphabet this project transcribes into.
///
/// Checked by block rather than by Unicode script property, which the standard
/// library does not expose, and which is not worth a dependency for four ranges.
fn is_expected_script(ch: char) -> bool {
    ch.is_ascii_alphabetic()
        || matches!(ch,
            '\u{00C0}'..='\u{024F}'   // Latin, accented
            | '\u{0400}'..='\u{052F}' // Cyrillic
            | '\u{0590}'..='\u{05FF}' // Hebrew
            | '\u{0600}'..='\u{06FF}' // Arabic
            | '\u{0750}'..='\u{077F}' // Arabic supplement
        )
}

impl AnalysisEngine {
    pub fn new(llm: Arc<dyn LlmEngine>) -> Self {
        Self {
            llm,
            context_tokens: 8192,
        }
    }

    /// Tokens the model is given.
    ///
    /// A transcript that does not fit is summarised window by window rather than
    /// cut, so the whole call is represented.
    #[must_use]
    pub fn with_context_tokens(mut self, tokens: usize) -> Self {
        self.context_tokens = tokens;
        self
    }

    /// Reduce a transcript until the analysis prompt fits the context window.
    ///
    /// Each window is summarised on its own and the summaries take the
    /// transcript's place, so the whole call is represented rather than its
    /// first few minutes. Repeats if the summaries are themselves too long, and
    /// gives up rather than looping when a round stops shrinking anything --
    /// a truncated prompt beats no analysis, but only as a last resort and it is
    /// logged.
    async fn fit_to_context(
        &self,
        transcript: &str,
        organization_name: &str,
        language: &callmind_core::Language,
    ) -> String {
        // The template itself costs tokens, so measure it rather than guess.
        let overhead = estimated_tokens(&build_language_aware_analysis_prompt(
            "",
            organization_name,
            language,
        ));
        let budget = self.context_tokens.saturating_sub(overhead);
        if budget == 0 {
            tracing::warn!(
                context_tokens = self.context_tokens,
                overhead,
                "the analysis prompt template alone fills the context window"
            );
            return transcript.to_string();
        }
        if estimated_tokens(transcript) <= budget {
            return transcript.to_string();
        }

        let mut current = transcript.to_string();
        for round in 0..MAX_COMPRESSION_ROUNDS {
            let windows = windows_for_budget(&current, budget);
            tracing::debug!(
                round,
                windows = windows.len(),
                tokens = estimated_tokens(&current),
                budget,
                "compressing a transcript that does not fit the context window"
            );

            let mut compressed = String::new();
            for window in &windows {
                let summary = match self
                    .llm
                    .generate_text(
                        &build_window_compression_prompt(window, language),
                        Some(CONVERSATION_ANALYSIS_SYSTEM_PROMPT),
                    )
                    .await
                {
                    Ok(text) => text,
                    Err(e) => {
                        // Keeping the window verbatim is better than losing it;
                        // a later round may still bring the total down.
                        tracing::warn!("Window compression failed ({e}); keeping it verbatim");
                        (*window).to_string()
                    }
                };
                let _ = writeln!(compressed, "{}", summary.trim());
            }

            if estimated_tokens(&compressed) <= budget {
                return compressed;
            }
            if compressed.len() >= current.len() {
                tracing::warn!(
                    "Compression stopped making progress; truncating the transcript to fit"
                );
                break;
            }
            current = compressed;
        }

        // Last resort, and now a deliberate, logged decision rather than a
        // silent one inside the model.
        let windows = windows_for_budget(&current, budget);
        windows
            .first()
            .map_or(current.clone(), |w| (*w).to_string())
    }

    /// Run full conversation analysis on a transcript.
    pub async fn analyze(
        &self,
        transcript: &Transcript,
        organization_name: &str,
        classifiers: &[Classifier],
    ) -> Result<CallAnalysis, AnalysisError> {
        if transcript.segments.is_empty() {
            return Err(AnalysisError::EmptyTranscript);
        }

        // 1. Compute objective metrics directly from timestamps
        let metrics = ConversationMetricsCalculator::calculate(transcript);
        let primary_lang = transcript
            .segments
            .first()
            .map_or(&callmind_core::Language::Hebrew, |s| &s.language);
        let heuristic =
            crate::summarizer::ConversationSummarizer::summarize(transcript, primary_lang);

        // 2. Format transcript with clean timestamps and speaker labels
        let mut formatted_transcript = String::new();
        for (i, seg) in transcript.segments.iter().enumerate() {
            let speaker = seg
                .speaker_role
                .display_label(Some(seg.speaker_id.as_u16()));
            let time = format!(
                "{:02}:{:02}",
                (seg.start_ms / 60000),
                (seg.start_ms % 60000) / 1000
            );
            let _ = writeln!(
                formatted_transcript,
                "[{i}] ({time}) {speaker}: {}",
                seg.normalized_text
            );
        }

        // 3. Prompt LLM for structured analysis, compressing first if the
        //    transcript will not fit. Ollama drops whatever exceeds its context
        //    window without a word: measured on this archive, a 13-minute call
        //    formats to ~4160 tokens, so at Ollama's 2048 default about half of
        //    it never reached the model and the summary described only what did.
        let transcript_for_prompt = self
            .fit_to_context(&formatted_transcript, organization_name, primary_lang)
            .await;
        let prompt = build_language_aware_analysis_prompt(
            &transcript_for_prompt,
            organization_name,
            primary_lang,
        );
        let transcript_word_count: usize = transcript
            .segments
            .iter()
            .map(|segment| segment.normalized_text.split_whitespace().count())
            .sum();
        let llm_res: Result<LlmRawAnalysis, LlmError> = if transcript_word_count < 4 {
            Err(LlmError::Inference(
                "Transcript is too short for reliable generative analysis".into(),
            ))
        } else {
            // Fetched and parsed in two steps rather than through
            // `generate_structured`, so a rejection can report *which* field was
            // the wrong shape. Serde names the type it wanted and not the field,
            // which is the one thing needed to fix the prompt.
            match self
                .llm
                .generate_json(&prompt, Some(CONVERSATION_ANALYSIS_SYSTEM_PROMPT))
                .await
            {
                Ok(value) => serde_json::from_value(value.clone()).map_err(|e| {
                    tracing::warn!(
                        shape = %crate::lenient::describe_shape(&value),
                        "LLM analysis JSON did not fit the schema"
                    );
                    LlmError::JsonParse(e)
                }),
                Err(e) => Err(e),
            }
        };

        let (
            title,
            summary,
            reason,
            resolution,
            resolved,
            customer_intent,
            topics,
            key_facts,
            action_items,
            entities,
            sentiment_score,
            scorecard,
            compliance,
        ) = match llm_res {
            Ok(raw) => {
                let parsed_summary = raw.summary.unwrap_or_else(|| heuristic.summary.clone());
                let final_summary = if parsed_summary.contains("misunderstandings,")
                    || parsed_summary.contains("neither nor")
                    || is_degenerate(&parsed_summary)
                    || has_foreign_script(&parsed_summary)
                {
                    warn!("Model returned an unusable summary; using the transcript instead");
                    heuristic.summary.clone()
                } else {
                    parsed_summary
                };

                let final_title = raw
                    .title
                    .filter(|t| !is_degenerate(t) && !has_foreign_script(t))
                    .filter(|t| {
                        !t.eq_ignore_ascii_case("Conversation")
                            && !t.eq_ignore_ascii_case("Customer Service Call")
                            && !t.eq_ignore_ascii_case("Customer Service Conversation")
                    })
                    .unwrap_or_else(|| heuristic.title.clone());

                let final_topics = match raw.topics {
                    Some(serde_json::Value::Array(arr)) => {
                        let list: Vec<Topic> = arr
                            .into_iter()
                            .filter_map(|v| {
                                v.as_str().map(|s| Topic {
                                    name: s.to_string(),
                                    confidence: 0.90,
                                    evidence: Evidence::default(),
                                })
                            })
                            .collect();
                        if list.is_empty() {
                            heuristic
                                .topics
                                .iter()
                                .map(|t| Topic {
                                    name: t.clone(),
                                    confidence: 0.90,
                                    evidence: Evidence::default(),
                                })
                                .collect()
                        } else {
                            list
                        }
                    }
                    Some(serde_json::Value::Object(obj)) => obj
                        .values()
                        .filter_map(|v| {
                            v.as_str().map(|s| Topic {
                                name: s.to_string(),
                                confidence: 0.90,
                                evidence: Evidence::default(),
                            })
                        })
                        .collect(),
                    Some(serde_json::Value::String(s)) => vec![Topic {
                        name: s,
                        confidence: 0.90,
                        evidence: Evidence::default(),
                    }],
                    _ => heuristic
                        .topics
                        .iter()
                        .map(|t| Topic {
                            name: t.clone(),
                            confidence: 0.90,
                            evidence: Evidence::default(),
                        })
                        .collect(),
                };

                let final_key_facts: Vec<String> = match raw.key_facts {
                    Some(serde_json::Value::Array(arr)) => arr
                        .into_iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect(),
                    Some(serde_json::Value::Object(obj)) => obj
                        .values()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect(),
                    Some(serde_json::Value::String(s)) => vec![s],
                    _ => Vec::new(),
                };

                let scorecard = raw.scorecard.and_then(|s| {
                    if let Some(total) = s.get("total_score").and_then(serde_json::Value::as_u64) {
                        let rules = s
                            .get("rules")
                            .and_then(|r| r.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .map(|r_obj| ScoreRuleResult {
                                        rule_name: r_obj
                                            .get("name")
                                            .or_else(|| r_obj.get("rule_name"))
                                            .and_then(|n| n.as_str())
                                            .unwrap_or("Rule")
                                            .to_string(),
                                        score_awarded: r_obj
                                            .get("score")
                                            .or_else(|| r_obj.get("score_awarded"))
                                            .and_then(serde_json::Value::as_u64)
                                            .unwrap_or(0)
                                            as u32,
                                        max_score: r_obj
                                            .get("max_score")
                                            .and_then(serde_json::Value::as_u64)
                                            .unwrap_or(100)
                                            as u32,
                                        explanation: r_obj
                                            .get("explanation")
                                            .and_then(|e| e.as_str())
                                            .unwrap_or("")
                                            .to_string(),
                                        evidence: Evidence::default(),
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();

                        Some(ScorecardResult {
                            total_score: total as u32,
                            max_possible_score: 100,
                            rules,
                        })
                    } else {
                        None
                    }
                });

                (
                    final_title,
                    final_summary,
                    raw.reason.or(heuristic.reason),
                    raw.resolution.or(heuristic.resolution),
                    raw.resolved.unwrap_or(true),
                    raw.customer_intent.or(heuristic.intent),
                    final_topics,
                    final_key_facts,
                    raw.action_items
                        .map(|v| match v {
                            serde_json::Value::Array(arr) => arr,
                            serde_json::Value::Object(obj) => obj.into_values().collect(),
                            other => vec![other],
                        })
                        .unwrap_or_default()
                        .into_iter()
                        .filter_map(|val| {
                            if let Some(s) = val.as_str() {
                                if s.trim().is_empty() || s.eq_ignore_ascii_case("none") {
                                    None
                                } else {
                                    Some(ActionItem {
                                        text: s.to_string(),
                                        owner: None,
                                        deadline: None,
                                        evidence: Evidence::default(),
                                    })
                                }
                            } else if let Some(obj) = val.as_object() {
                                let text = obj
                                    .get("text")
                                    .or_else(|| obj.get("task"))
                                    .or_else(|| obj.get("description"))
                                    .or_else(|| obj.get("action"))
                                    .or_else(|| obj.get("item"))
                                    .and_then(|t| t.as_str())
                                    .unwrap_or_default()
                                    .to_string();

                                if text.trim().is_empty() || text.eq_ignore_ascii_case("none") {
                                    return None;
                                }

                                let owner = obj
                                    .get("owner")
                                    .or_else(|| obj.get("assignee"))
                                    .or_else(|| obj.get("who"))
                                    .and_then(|o| o.as_str())
                                    .map(|o| match o.to_lowercase().as_str() {
                                        "speaker_1" | "speaker1" | "speaker 0" | "speaker_0"
                                        | "agent" => SpeakerRole::Speaker1,
                                        "speaker_2" | "speaker2" | "customer" => {
                                            SpeakerRole::Speaker2
                                        }
                                        "speaker_3" | "speaker3" | "supervisor" => {
                                            SpeakerRole::Supervisor
                                        }
                                        _ => SpeakerRole::Participant,
                                    });

                                let deadline = obj
                                    .get("deadline")
                                    .and_then(|d| d.as_str())
                                    .map(String::from);
                                let evidence = obj
                                    .get("evidence_segments")
                                    .and_then(|e| e.as_array())
                                    .map(|arr| {
                                        arr.iter()
                                            .filter_map(|v| v.as_u64().map(|n| n as usize))
                                            .collect()
                                    })
                                    .unwrap_or_default();

                                Some(ActionItem {
                                    text,
                                    owner,
                                    deadline,
                                    evidence: Evidence::new(evidence),
                                })
                            } else {
                                None
                            }
                        })
                        .collect(),
                    raw.entities
                        .map(|v| match v {
                            serde_json::Value::Array(arr) => arr,
                            serde_json::Value::Object(obj) => obj.into_values().collect(),
                            other => vec![other],
                        })
                        .unwrap_or_default()
                        .into_iter()
                        .filter_map(|val| {
                            if let Some(s) = val.as_str() {
                                if s.trim().is_empty() || s.eq_ignore_ascii_case("none") {
                                    None
                                } else {
                                    Some(Entity {
                                        entity_type: "entity".to_string(),
                                        value: s.to_string(),
                                        evidence: Evidence::default(),
                                    })
                                }
                            } else if let Some(obj) = val.as_object() {
                                let value = obj
                                    .get("value")
                                    .or_else(|| obj.get("name"))
                                    .or_else(|| obj.get("text"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or_default()
                                    .to_string();

                                if value.trim().is_empty() || value.eq_ignore_ascii_case("none") {
                                    return None;
                                }

                                let entity_type = obj
                                    .get("entity_type")
                                    .or_else(|| obj.get("type"))
                                    .or_else(|| obj.get("category"))
                                    .and_then(|t| t.as_str())
                                    .unwrap_or("entity")
                                    .to_string();

                                let evidence = obj
                                    .get("evidence_segments")
                                    .and_then(|e| e.as_array())
                                    .map(|arr| {
                                        arr.iter()
                                            .filter_map(|v| v.as_u64().map(|n| n as usize))
                                            .collect()
                                    })
                                    .unwrap_or_default();

                                Some(Entity {
                                    entity_type,
                                    value,
                                    evidence: Evidence::new(evidence),
                                })
                            } else {
                                None
                            }
                        })
                        .collect(),
                    raw.sentiment_score.unwrap_or(0.0),
                    scorecard,
                    raw.compliance
                        .and_then(|c| {
                            c.as_array().map(|arr| {
                                arr.iter()
                                    .map(|c_obj| ComplianceResult {
                                        rule_name: c_obj
                                            .get("name")
                                            .and_then(|n| n.as_str())
                                            .unwrap_or("Policy")
                                            .to_string(),
                                        passed: c_obj
                                            .get("passed")
                                            .and_then(serde_json::Value::as_bool)
                                            .unwrap_or(true),
                                        explanation: c_obj
                                            .get("explanation")
                                            .and_then(|e| e.as_str())
                                            .unwrap_or("")
                                            .to_string(),
                                        evidence: Evidence::default(),
                                    })
                                    .collect()
                            })
                        })
                        .unwrap_or_default(),
                )
            }
            Err(e) => {
                tracing::warn!("LLM structured analysis error, using fallback: {e}");
                let topics_list = heuristic
                    .topics
                    .iter()
                    .map(|t| Topic {
                        name: t.clone(),
                        confidence: 0.90,
                        evidence: Evidence::default(),
                    })
                    .collect();
                (
                    heuristic.title,
                    heuristic.summary,
                    heuristic.reason,
                    heuristic.resolution,
                    true,
                    heuristic.intent,
                    topics_list,
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    0.0,
                    None,
                    Vec::new(),
                )
            }
        };

        // 4. Generate sentiment trajectory across segments
        let mut sentiment_trajectory = Vec::with_capacity(transcript.segments.len());
        for seg in &transcript.segments {
            // Default trajectory estimation based on overall sentiment score
            sentiment_trajectory.push(SentimentPoint {
                timestamp_ms: seg.start_ms,
                speaker_id: seg.speaker_id,
                score: sentiment_score,
            });
        }

        // 5. Evaluate custom classifiers & emotion distribution
        let full_text = transcript.full_text();
        let emotions = crate::emotions::EmotionClassifier::analyze_text(&full_text, primary_lang);

        let mut classifier_results = Vec::new();
        for classifier in classifiers {
            if let Some(res) = classifier.evaluate_heuristic(&full_text) {
                classifier_results.push(res);
            }
        }

        Ok(CallAnalysis {
            id: Uuid::new_v4(),
            call_id: transcript.call_id,
            title,
            summary,
            reason,
            resolution,
            resolved,
            customer_intent,
            topics,
            key_facts,
            action_items,
            entities,
            sentiment_score,
            sentiment_trajectory,
            metrics,
            scorecard,
            compliance,
            classifiers: classifier_results,
            emotions: Some(emotions),
            created_at: Utc::now(),
        })
    }
}

/// How many times a transcript may be summarised before the result is truncated.
const MAX_COMPRESSION_ROUNDS: usize = 3;

/// Rough token count for a piece of transcript.
///
/// Deliberately pessimistic: three characters per token rather than the four
/// usually quoted for English, because Hebrew and Cyrillic tokenize worse and
/// under-estimating here is what silently truncates a call.
fn estimated_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(3)
}

/// Split a formatted transcript into pieces that each fit the token budget.
///
/// Cuts on line boundaries, so a speaker turn is never severed mid-sentence. A
/// single line larger than the budget is emitted alone rather than dropped --
/// losing transcript is the failure this exists to prevent.
fn windows_for_budget(transcript: &str, budget_tokens: usize) -> Vec<&str> {
    if transcript.is_empty() {
        return vec![transcript];
    }

    let mut windows = Vec::new();
    let mut start = 0usize;
    let mut used = 0usize;
    let mut cursor = 0usize;

    while cursor < transcript.len() {
        let rest = &transcript[cursor..];
        let line_len = rest.find('\n').map_or(rest.len(), |i| i + 1);
        let line = &rest[..line_len];
        let cost = estimated_tokens(line);

        // Cut before this line when it would overflow a window that already
        // holds something. An oversized line with nothing before it goes out on
        // its own.
        if used > 0 && used + cost > budget_tokens {
            windows.push(&transcript[start..cursor]);
            start = cursor;
            used = 0;
        }

        used += cost;
        cursor += line_len;
    }

    if start < transcript.len() {
        windows.push(&transcript[start..]);
    }
    windows
}

#[cfg(test)]
mod budget_tests {
    use super::*;

    /// A transcript that fits the budget is analysed whole, in one piece.
    #[test]
    fn a_short_transcript_is_one_window() {
        let transcript = "[0] (00:00) agent: короткая реплика\n";
        let windows = windows_for_budget(transcript, 8192);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0], transcript);
    }

    /// The bug this exists for: a long call used to be handed to the model whole
    /// and silently cut to the context window, so the analysis described the
    /// opening minutes. Every window must fit, and no line may be lost.
    #[test]
    fn a_long_transcript_is_split_into_windows_that_fit() {
        let line = "[0] (00:00) customer: одна реплика примерно такой длины\n";
        let transcript = line.repeat(4000);
        let budget = 512;

        let windows = windows_for_budget(&transcript, budget);

        assert!(windows.len() > 1, "expected a split, got {}", windows.len());
        for (i, w) in windows.iter().enumerate() {
            assert!(
                estimated_tokens(w) <= budget,
                "window {i} is {} tokens, over the {budget} budget",
                estimated_tokens(w)
            );
        }
        assert_eq!(
            windows.concat(),
            transcript,
            "splitting must not drop or duplicate any part of the transcript"
        );
    }

    /// A single line longer than the budget still has to go somewhere rather
    /// than being dropped or looping forever.
    #[test]
    fn an_oversized_single_line_is_kept() {
        let line = format!("{}\n", "x".repeat(10_000));
        let windows = windows_for_budget(&line, 16);
        assert_eq!(windows.concat(), line);
        assert!(!windows.is_empty());
    }
}

#[cfg(test)]
mod degenerate_output_tests {
    use super::*;

    /// A model can return perfectly valid JSON whose contents are one phrase
    /// repeated. The retry in the LLM engine only catches answers that fail to
    /// parse, so this got through: an archived call's page reads
    /// `OPC, OPC, OPC, OPC, OPC`, echoed from a hold announcement.
    #[test]
    fn one_phrase_repeated_is_not_a_summary() {
        for repeated in [
            "OPC, OPC, OPC, OPC, OPC",
            "OPC OPC OPC OPC",
            "да да да да да да да да",
            "כן, כן, כן, כן, כן",
        ] {
            assert!(is_degenerate(repeated), "should be rejected: {repeated}");
        }
    }

    /// The exact text that reached an archived call's page.
    #[test]
    fn the_summary_that_actually_shipped_is_rejected() {
        let stored = "OPC, OPC, OPC, OPC, OPC, OPC, OPC, OPC, OPC, OPC, OPC, OPC, OPC, OPC, OPC, OPC, OPC, OPC, OPC";
        assert!(is_degenerate(stored), "stored summary must be rejected");
    }

    /// Real summaries repeat words too -- rejecting them would replace a good
    /// analysis with the fallback, which is worse than the problem.
    #[test]
    fn ordinary_prose_is_left_alone() {
        for real in [
            "הלקוח שאל אם יש במלאי את הדגם Asus New 14 Ultra 7 וביקש הצעת מחיר.",
            "Разговор о покупке продуктов и поездке в супермаркет.",
            "Разговор о пианино",
            "The customer asked about the order and the courier was already there.",
            "",
            "OPC",
        ] {
            assert!(!is_degenerate(real), "should be kept: {real}");
        }
    }
}

#[cfg(test)]
mod foreign_script_tests {
    use super::*;

    /// qwen2.5:7b drifts out of Hebrew into its dominant language mid-answer.
    /// The JSON stays valid and the text does not repeat, so neither the retry
    /// in the LLM engine nor the degeneracy check sees it -- one archived
    /// analysis carries 33 CJK characters among 194 letters.
    #[test]
    fn an_answer_that_wandered_into_another_script_is_rejected() {
        for drifted in [
            "הלקוח שאל אם יש במלאי את הדגם Asus New 14 Ultra 7 וקיבל תשובה肯定，以下是中文翻译结果：",
            "客户询问了华硕笔记本电脑的库存情况",
        ] {
            assert!(has_foreign_script(drifted), "should be rejected: {drifted}");
        }
    }

    /// Latin inside Hebrew or Russian is ordinary -- product names, model
    /// numbers, a company written in its own alphabet. Rejecting those would
    /// throw away correct analyses, which is worse than the drift.
    #[test]
    fn the_languages_this_project_handles_are_left_alone() {
        for real in [
            "הלקוח שאל אם יש במלאי את המודל Asus New 14 Ultra 7 ותשלום במזומן.",
            "Разговор о полете в Китай, договоренности о смене и охране.",
            "The customer asked about the order and the courier had already arrived.",
            "שעות פעילות החברה בין 9:00 ל-18:30",
            "",
        ] {
            assert!(!has_foreign_script(real), "should be kept: {real}");
        }
    }

    /// A single stray character is cosmetic, and a summary can legitimately
    /// quote a foreign name. What matters is a run of it.
    #[test]
    fn a_lone_stray_character_is_not_worth_discarding_an_analysis_over() {
        let mostly_hebrew =
            "הלקוח ביקש הזמנה טלפונית ותשלום במזומן, וגם שאל על זיכרונות DDR5 ו받ה במלאי";
        assert!(!has_foreign_script(mostly_hebrew));
    }
}
