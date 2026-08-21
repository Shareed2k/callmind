use crate::classifiers::Classifier;
use crate::metrics::ConversationMetricsCalculator;
use crate::models::{
    ActionItem, CallAnalysis, ComplianceResult, Entity, Evidence, ScoreRuleResult, ScorecardResult,
    SentimentPoint, Topic,
};
use callmind_core::SpeakerRole;
use callmind_llm::{
    CONVERSATION_ANALYSIS_SYSTEM_PROMPT, LlmEngine, LlmError, build_language_aware_analysis_prompt,
};
use callmind_transcript::Transcript;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fmt::Write;
use std::sync::Arc;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum AnalysisError {
    #[error("LLM execution error: {0}")]
    Llm(#[from] LlmError),

    #[error("Transcript has no speech segments to analyze")]
    EmptyTranscript,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LlmRawAnalysis {
    title: Option<String>,
    summary: Option<String>,
    reason: Option<String>,
    resolution: Option<String>,
    resolved: Option<bool>,
    customer_intent: Option<String>,
    topics: Option<serde_json::Value>,
    key_facts: Option<serde_json::Value>,
    action_items: Option<serde_json::Value>,
    entities: Option<serde_json::Value>,
    sentiment_score: Option<f32>,
    scorecard: Option<serde_json::Value>,
    compliance: Option<serde_json::Value>,
}

/// Orchestrator for conversation intelligence analysis.
pub struct AnalysisEngine {
    llm: Arc<dyn LlmEngine>,
}

impl AnalysisEngine {
    pub fn new(llm: Arc<dyn LlmEngine>) -> Self {
        Self { llm }
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

        // 3. Prompt LLM for structured analysis
        let prompt = build_language_aware_analysis_prompt(
            &formatted_transcript,
            organization_name,
            primary_lang,
        );
        let llm_res: Result<LlmRawAnalysis, LlmError> = self
            .llm
            .generate_structured(&prompt, Some(CONVERSATION_ANALYSIS_SYSTEM_PROMPT))
            .await;

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
                {
                    heuristic.summary.clone()
                } else {
                    parsed_summary
                };

                let final_title = raw
                    .title
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
