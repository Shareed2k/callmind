use callmind_core::{CallDirection, CallId, Language, OrgId, ProcessingStatus};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Query parameters for searching conversations.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, utoipa::IntoParams)]
pub struct SearchFilter {
    pub organization_id: Option<OrgId>,
    pub query: String,
    pub from_date: Option<DateTime<Utc>>,
    pub to_date: Option<DateTime<Utc>>,
    pub status: Option<ProcessingStatus>,
    pub direction: Option<CallDirection>,
    pub language: Option<Language>,
    pub sentiment_min: Option<f32>,
    pub sentiment_max: Option<f32>,
    pub resolved: Option<bool>,
    #[param(default = 20)]
    pub limit: Option<u32>,
    #[param(default = 0)]
    pub offset: Option<u32>,
}

/// Search hit with snippets and ranking information.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SearchResultItem {
    pub call_id: CallId,
    pub title: String,
    pub summary: String,
    pub match_highlight: String,
    pub rank: f64,
    pub created_at: DateTime<Utc>,
}

/// Request to ask analytical questions across calls.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AskCallsRequest {
    /// Natural language question (e.g. "Why are customers asking to cancel subscriptions this week?").
    #[schema(example = "Why are customers canceling subscriptions?")]
    pub question: String,
    pub organization_id: Option<OrgId>,
    #[schema(default = 5)]
    pub max_sources: Option<usize>,
}

/// Precise citation to a specific moment in a call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CallCitation {
    pub call_id: CallId,
    pub text_snippet: String,
    pub relevance_score: f32,
}

/// Answer generated from call evidence with verifiable citations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AskCallsResponse {
    pub answer: String,
    pub citations: Vec<CallCitation>,
}
