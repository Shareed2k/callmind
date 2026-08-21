use callmind_core::{CallId, SpeakerId, SpeakerRole};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Citation evidence pointing to specific transcript segment indices.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Evidence {
    pub segment_indices: Vec<usize>,
}

impl Evidence {
    pub fn new(indices: Vec<usize>) -> Self {
        Self {
            segment_indices: indices,
        }
    }
}

/// Extracted conversational topic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Topic {
    pub name: String,
    pub confidence: f32,
    pub evidence: Evidence,
}

/// Action item or next step commitment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ActionItem {
    pub text: String,
    pub owner: Option<SpeakerRole>,
    pub deadline: Option<String>,
    pub evidence: Evidence,
}

/// Extracted business entity (person, phone, amount, tracking number, date).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Entity {
    pub entity_type: String,
    pub value: String,
    pub evidence: Evidence,
}

/// Timestamped sentiment trajectory data point.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SentimentPoint {
    pub timestamp_ms: u64,
    pub speaker_id: SpeakerId,
    pub score: f32,
}

/// Result of evaluating an individual QA scorecard rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ScoreRuleResult {
    pub rule_name: String,
    pub score_awarded: u32,
    pub max_score: u32,
    pub explanation: String,
    pub evidence: Evidence,
}

/// Result of evaluating a full QA scorecard (e.g. 0-100 score).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ScorecardResult {
    pub total_score: u32,
    pub max_possible_score: u32,
    pub rules: Vec<ScoreRuleResult>,
}

/// Compliance policy verification result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ComplianceResult {
    pub rule_name: String,
    pub passed: bool,
    pub explanation: String,
    pub evidence: Evidence,
}

/// Custom classifier output evaluation result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ClassifierResult {
    pub classifier_id: Uuid,
    pub name: String,
    pub value: serde_json::Value,
    pub evidence: Evidence,
}

/// Objective mathematical speech and interaction metrics (calculated without LLM).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ConversationMetrics {
    pub total_duration_ms: u64,
    pub agent_talk_ms: u64,
    pub customer_talk_ms: u64,
    pub silence_ms: u64,
    pub agent_talk_ratio: f32,
    pub customer_talk_ratio: f32,
    pub longest_agent_monologue_ms: u64,
    pub longest_customer_monologue_ms: u64,
    pub speaker_switches: u32,
    pub agent_words_per_minute: f32,
    pub customer_words_per_minute: f32,
}

/// Complete conversation intelligence analysis result for a call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CallAnalysis {
    pub id: Uuid,
    pub call_id: CallId,
    pub title: String,
    pub summary: String,
    pub reason: Option<String>,
    pub resolution: Option<String>,
    pub resolved: bool,
    pub customer_intent: Option<String>,
    pub topics: Vec<Topic>,
    pub action_items: Vec<ActionItem>,
    pub entities: Vec<Entity>,
    pub sentiment_score: f32,
    pub sentiment_trajectory: Vec<SentimentPoint>,
    pub metrics: ConversationMetrics,
    pub scorecard: Option<ScorecardResult>,
    pub compliance: Vec<ComplianceResult>,
    pub classifiers: Vec<ClassifierResult>,
    #[serde(default)]
    pub key_facts: Vec<String>,
    #[serde(default)]
    pub emotions: Option<crate::emotions::EmotionDistribution>,
    pub created_at: DateTime<Utc>,
}
