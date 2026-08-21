use crate::errors::{ApiErrorResponse, ErrorDetail};
use crate::routes::calls::*;
use crate::routes::health::*;
use crate::routes::recordings::*;
use crate::routes::search::*;
use callmind_core::*;
use callmind_search::{AskCallsRequest, AskCallsResponse, CallCitation, SearchResultItem};
use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    paths(
        health_check,
        readiness_check,
        system_metrics,
        create_call,
        list_calls,
        get_call,
        delete_call,
        update_call,
        toggle_call_favorite,
        update_call_tags,
        export_transcript,
        reprocess_call,
        reanalyze_all_calls,
        upload_recording,
        get_recording,
        search_calls,
        ask_calls,
    ),
    components(
        schemas(
            Call,
            CreateCallRequest,
            UpdateCallRequest,
            UpdateTagsRequest,
            Recording,
            UploadRecordingResponse,
            ReprocessResponse,
            SearchResultItem,
            AskCallsRequest,
            AskCallsResponse,
            CallCitation,
            HealthResponse,
            ReadyResponse,
            SystemMetricsResponse,
            ApiErrorResponse,
            ErrorDetail,
            CallDirection,
            ProcessingStatus,
            JobKind,
            JobStatus,
            SpeakerRole,
            Language,
        )
    ),
    tags(
        (name = "System", description = "Liveness and readiness checks"),
        (name = "Calls", description = "Call records and lifecycle management"),
        (name = "Recordings", description = "Audio recording streaming upload and download"),
        (name = "Search", description = "Multilingual full-text search and analytical QA over calls")
    ),
    info(
        title = "CallMind API",
        version = "0.1.0",
        description = "Self-hosted Multilingual Conversation Intelligence Server REST API"
    )
)]
pub struct ApiDoc;
