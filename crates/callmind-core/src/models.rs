use crate::enums::{CallDirection, JobKind, JobStatus, ProcessingStatus};
use crate::ids::{CallId, JobId, OrgId, RecordingId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Core domain entity representing a telephone call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Call {
    pub id: CallId,
    pub organization_id: OrgId,
    pub external_id: Option<String>,
    pub direction: CallDirection,
    pub phone_from: Option<String>,
    pub phone_to: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<u64>,
    pub processing_status: ProcessingStatus,
    pub is_favorite: bool,
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Call {
    pub fn new(
        organization_id: OrgId,
        external_id: Option<String>,
        direction: CallDirection,
        phone_from: Option<String>,
        phone_to: Option<String>,
        started_at: Option<DateTime<Utc>>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: CallId::generate(),
            organization_id,
            external_id,
            direction,
            phone_from,
            phone_to,
            started_at,
            ended_at: None,
            duration_ms: None,
            processing_status: ProcessingStatus::Pending,
            is_favorite: false,
            tags: Vec::new(),
            created_at: now,
            updated_at: now,
        }
    }
}

/// Request DTO for creating a new Call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CreateCallRequest {
    #[schema(example = "org-default")]
    pub organization_id: Option<OrgId>,
    #[schema(example = "pbx-call-892147")]
    pub external_id: Option<String>,
    #[schema(example = "incoming")]
    pub direction: Option<CallDirection>,
    #[schema(example = "+972501234567")]
    pub phone_from: Option<String>,
    #[schema(example = "+97235551234")]
    pub phone_to: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
}

/// Query filter for listing calls.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::IntoParams)]
pub struct CallFilter {
    pub organization_id: Option<OrgId>,
    pub external_id: Option<String>,
    pub status: Option<ProcessingStatus>,
    pub direction: Option<CallDirection>,
    pub from_date: Option<DateTime<Utc>>,
    pub to_date: Option<DateTime<Utc>>,
    #[param(default = 50)]
    pub limit: Option<u32>,
    #[param(default = 0)]
    pub offset: Option<u32>,
}

/// Domain entity representing an audio recording attached to a Call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Recording {
    pub id: RecordingId,
    pub call_id: CallId,
    pub storage_key: String,
    pub mime_type: String,
    pub file_size_bytes: u64,
    pub sha256: String,
    pub duration_ms: Option<u64>,
    pub channels: Option<u16>,
    pub sample_rate: Option<u32>,
    pub created_at: DateTime<Utc>,
}

impl Recording {
    pub fn new(
        call_id: CallId,
        storage_key: String,
        mime_type: String,
        file_size_bytes: u64,
        sha256: String,
    ) -> Self {
        Self {
            id: RecordingId::generate(),
            call_id,
            storage_key,
            mime_type,
            file_size_bytes,
            sha256,
            duration_ms: None,
            channels: None,
            sample_rate: None,
            created_at: Utc::now(),
        }
    }
}

/// Domain entity representing a Background Processing Job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Job {
    pub id: JobId,
    pub call_id: Option<CallId>,
    pub kind: JobKind,
    pub payload: serde_json::Value,
    pub status: JobStatus,
    pub priority: i32,
    pub attempt: i32,
    pub max_attempts: i32,
    pub run_after: DateTime<Utc>,
    pub locked_at: Option<DateTime<Utc>>,
    pub locked_by: Option<String>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// Request DTO for enqueuing a new Job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnqueueJob {
    pub call_id: Option<CallId>,
    pub kind: JobKind,
    pub payload: serde_json::Value,
    pub priority: i32,
    pub max_attempts: i32,
    pub run_after: Option<DateTime<Utc>>,
}

impl EnqueueJob {
    pub fn new(kind: JobKind, payload: serde_json::Value) -> Self {
        Self {
            call_id: None,
            kind,
            payload,
            priority: 0,
            max_attempts: 3,
            run_after: None,
        }
    }

    #[must_use]
    pub fn with_call_id(mut self, call_id: CallId) -> Self {
        self.call_id = Some(call_id);
        self
    }

    #[must_use]
    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }
}
