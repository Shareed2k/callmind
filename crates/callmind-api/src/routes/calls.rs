use crate::errors::ApiError;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use callmind_core::{Call, CallDirection, CallFilter, CallId, CreateCallRequest, OrgId};

const DEFAULT_ORG_ID: &str = "00000000-0000-0000-0000-000000000001";

#[utoipa::path(
    post,
    path = "/api/v1/calls",
    tag = "Calls",
    request_body = CreateCallRequest,
    responses(
        (status = 201, description = "Call created successfully", body = Call),
        (status = 200, description = "Call already exists (idempotent)", body = Call),
        (status = 400, description = "Invalid request payload", body = crate::errors::ApiErrorResponse)
    )
)]
pub async fn create_call(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<CreateCallRequest>,
) -> Result<Response, ApiError> {
    let org_id = payload
        .organization_id
        .unwrap_or_else(|| OrgId(uuid::Uuid::parse_str(DEFAULT_ORG_ID).unwrap()));

    // Check idempotency via external_id or Idempotency-Key header
    let idempotency_key = headers
        .get("Idempotency-Key")
        .and_then(|v| v.to_str().ok())
        .map(ToString::to_string)
        .or_else(|| payload.external_id.clone());

    if let Some(ref ext_id) = idempotency_key {
        if let Some(existing) = state.call_repo.get_by_external_id(org_id, ext_id).await? {
            return Ok((StatusCode::OK, Json(existing)).into_response());
        }
    }

    let call = Call::new(
        org_id,
        payload.external_id.or(idempotency_key),
        payload.direction.unwrap_or(CallDirection::Incoming),
        payload.phone_from,
        payload.phone_to,
        payload.started_at,
    );

    state.call_repo.create(&call).await?;

    Ok((StatusCode::CREATED, Json(call)).into_response())
}

#[utoipa::path(
    get,
    path = "/api/v1/calls",
    tag = "Calls",
    params(CallFilter),
    responses(
        (status = 200, description = "List of calls", body = Vec<Call>)
    )
)]
pub async fn list_calls(
    State(state): State<AppState>,
    Query(filter): Query<CallFilter>,
) -> Result<Json<Vec<Call>>, ApiError> {
    let calls = state.call_repo.list(&filter).await?;
    Ok(Json(calls))
}

#[utoipa::path(
    get,
    path = "/api/v1/calls/{id}",
    tag = "Calls",
    params(
        ("id" = CallId, Path, description = "Call UUID identifier")
    ),
    responses(
        (status = 200, description = "Call found", body = Call),
        (status = 404, description = "Call not found", body = crate::errors::ApiErrorResponse)
    )
)]
pub async fn get_call(
    State(state): State<AppState>,
    Path(id): Path<CallId>,
) -> Result<Json<Call>, ApiError> {
    let call = state
        .call_repo
        .get_by_id(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Call {id} not found")))?;

    Ok(Json(call))
}

#[utoipa::path(
    delete,
    path = "/api/v1/calls/{id}",
    tag = "Calls",
    params(
        ("id" = CallId, Path, description = "Call UUID identifier")
    ),
    responses(
        (status = 204, description = "Call deleted"),
        (status = 404, description = "Call not found", body = crate::errors::ApiErrorResponse)
    )
)]
pub async fn delete_call(
    State(state): State<AppState>,
    Path(id): Path<CallId>,
) -> Result<StatusCode, ApiError> {
    // If recording exists, delete from storage
    if let Some(recording) = state.call_repo.get_recording_by_call_id(id).await? {
        let _ = state.storage.delete(&recording.storage_key).await;
    }

    // Clean up full-text search index
    let _ = sqlx::query("DELETE FROM fts_calls WHERE call_id = ?")
        .bind(id.to_string())
        .execute(&state.pool)
        .await;

    let deleted = state.call_repo.delete(id).await?;
    if deleted {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound(format!("Call {id} not found")))
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct UpdateCallRequest {
    pub title: Option<String>,
    pub speaker_names: Option<std::collections::HashMap<u16, String>>,
}

#[utoipa::path(
    patch,
    path = "/api/v1/calls/{id}",
    tag = "Calls",
    params(
        ("id" = CallId, Path, description = "Call UUID identifier")
    ),
    request_body = UpdateCallRequest,
    responses(
        (status = 200, description = "Call updated successfully"),
        (status = 404, description = "Call not found", body = crate::errors::ApiErrorResponse)
    )
)]
pub async fn update_call(
    State(state): State<AppState>,
    Path(id): Path<CallId>,
    Json(payload): Json<UpdateCallRequest>,
) -> Result<Response, ApiError> {
    let _ = state
        .call_repo
        .get_by_id(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Call {id} not found")))?;

    let call_id_str = id.to_string();

    if let Some(ref title) = payload.title {
        sqlx::query("UPDATE call_analyses SET title = ? WHERE call_id = ?")
            .bind(title)
            .bind(&call_id_str)
            .execute(&state.pool)
            .await?;

        sqlx::query("UPDATE fts_calls SET title = ? WHERE call_id = ?")
            .bind(title)
            .bind(&call_id_str)
            .execute(&state.pool)
            .await?;
    }

    if let Some(ref speaker_map) = payload.speaker_names {
        // Update speaker labels in transcript_json if it exists
        let row: Option<(String,)> =
            sqlx::query_as("SELECT transcript_json FROM call_transcripts WHERE call_id = ?")
                .bind(&call_id_str)
                .fetch_optional(&state.pool)
                .await?;

        if let Some((raw_json,)) = row {
            if let Ok(mut json_val) = serde_json::from_str::<serde_json::Value>(&raw_json) {
                if let Some(segments) = json_val
                    .get_mut("segments")
                    .and_then(serde_json::Value::as_array_mut)
                {
                    for seg in segments {
                        if let Some(spk_id) =
                            seg.get("speaker_id").and_then(serde_json::Value::as_u64)
                        {
                            if let Some(new_name) = speaker_map.get(&(spk_id as u16)) {
                                seg["speaker_label"] = serde_json::json!(new_name);
                            }
                        }
                    }
                    let updated_json = serde_json::to_string(&json_val).unwrap_or(raw_json);
                    sqlx::query(
                        "UPDATE call_transcripts SET transcript_json = ? WHERE call_id = ?",
                    )
                    .bind(&updated_json)
                    .bind(&call_id_str)
                    .execute(&state.pool)
                    .await?;
                }
            }
        }
    }

    Ok((
        StatusCode::OK,
        Json(serde_json::json!({ "status": "updated", "call_id": id.to_string() })),
    )
        .into_response())
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct UpdateTagsRequest {
    pub tags: Vec<String>,
}

#[utoipa::path(
    post,
    path = "/api/v1/calls/{id}/favorite",
    tag = "Calls",
    params(
        ("id" = CallId, Path, description = "Call UUID identifier")
    ),
    responses(
        (status = 200, description = "Favorite status toggled")
    )
)]
pub async fn toggle_call_favorite(
    State(state): State<AppState>,
    Path(id): Path<CallId>,
) -> Result<Response, ApiError> {
    let new_fav = state.call_repo.toggle_favorite(id).await?;
    Ok((
        StatusCode::OK,
        Json(serde_json::json!({ "call_id": id.to_string(), "is_favorite": new_fav })),
    )
        .into_response())
}

#[utoipa::path(
    put,
    path = "/api/v1/calls/{id}/tags",
    tag = "Calls",
    params(
        ("id" = CallId, Path, description = "Call UUID identifier")
    ),
    request_body = UpdateTagsRequest,
    responses(
        (status = 200, description = "Tags updated successfully")
    )
)]
pub async fn update_call_tags(
    State(state): State<AppState>,
    Path(id): Path<CallId>,
    Json(payload): Json<UpdateTagsRequest>,
) -> Result<Response, ApiError> {
    state.call_repo.update_tags(id, &payload.tags).await?;
    Ok((
        StatusCode::OK,
        Json(serde_json::json!({ "call_id": id.to_string(), "tags": payload.tags })),
    )
        .into_response())
}

#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
pub struct ExportCallQuery {
    pub format: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/calls/{id}/export",
    tag = "Calls",
    params(
        ("id" = CallId, Path, description = "Call UUID identifier"),
        ExportCallQuery
    ),
    responses(
        (status = 200, description = "Exported file content"),
        (status = 404, description = "Transcript not found", body = crate::errors::ApiErrorResponse)
    )
)]
pub async fn export_transcript(
    State(state): State<AppState>,
    Path(id): Path<CallId>,
    Query(query): Query<ExportCallQuery>,
) -> Result<Response, ApiError> {
    let call_id_str = id.to_string();
    let transcript_row: Option<(String,)> =
        sqlx::query_as("SELECT transcript_json FROM call_transcripts WHERE call_id = ?")
            .bind(&call_id_str)
            .fetch_optional(&state.pool)
            .await
            .map_err(|e| ApiError::Internal(format!("Database error: {e}")))?;

    let (json_str,) = transcript_row
        .ok_or_else(|| ApiError::NotFound(format!("Transcript for call {id} not found")))?;

    let transcript: callmind_transcript::Transcript = serde_json::from_str(&json_str)
        .map_err(|e| ApiError::Internal(format!("Failed to parse transcript: {e}")))?;

    let format_choice = query
        .format
        .as_deref()
        .unwrap_or("txt")
        .to_ascii_lowercase();

    let (content_type, ext, body_text) = match format_choice.as_str() {
        "srt" => (
            "text/plain; charset=utf-8",
            "srt",
            callmind_transcript::TranscriptExporter::to_srt(&transcript),
        ),
        "vtt" => (
            "text/vtt; charset=utf-8",
            "vtt",
            callmind_transcript::TranscriptExporter::to_vtt(&transcript),
        ),
        "md" | "markdown" => (
            "text/markdown; charset=utf-8",
            "md",
            callmind_transcript::TranscriptExporter::to_markdown(&transcript, None),
        ),
        "json" => (
            "application/json; charset=utf-8",
            "json",
            serde_json::to_string_pretty(&transcript).unwrap_or_default(),
        ),
        _ => (
            "text/plain; charset=utf-8",
            "txt",
            callmind_transcript::TranscriptExporter::to_txt(&transcript),
        ),
    };

    let filename = format!("attachment; filename=\"transcript-{id}.{ext}\"");

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(axum::http::header::CONTENT_TYPE, content_type)
        .header(axum::http::header::CONTENT_DISPOSITION, filename)
        .body(axum::body::Body::from(body_text))
        .unwrap())
}

#[utoipa::path(
    post,
    path = "/api/v1/calls/reanalyze-all",
    tag = "Calls",
    responses(
        (status = 202, description = "All completed calls queued for re-analysis")
    )
)]
pub async fn reanalyze_all_calls(State(state): State<AppState>) -> Result<Response, ApiError> {
    let completed_calls = state
        .call_repo
        .list(&CallFilter {
            status: Some(callmind_core::ProcessingStatus::Completed),
            limit: Some(100_000),
            offset: Some(0),
            ..Default::default()
        })
        .await?;

    let mut queued_count = 0;
    for call in completed_calls {
        let enqueue_req = callmind_core::EnqueueJob::new(
            callmind_core::JobKind::IngestRecording,
            serde_json::json!({ "call_id": call.id.to_string() }),
        )
        .with_call_id(call.id);
        if state.job_repo.enqueue(&enqueue_req).await.is_ok() {
            queued_count += 1;
        }
    }

    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "status": "queued", "count": queued_count })),
    )
        .into_response())
}

#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct ReprocessResponse {
    pub call_id: CallId,
    pub status: String,
}

#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
pub struct ReprocessCallQuery {
    pub language: Option<String>,
}

#[utoipa::path(
    post,
    path = "/api/v1/calls/{id}/reprocess",
    tag = "Calls",
    params(
        ("id" = CallId, Path, description = "Call UUID identifier"),
        ReprocessCallQuery
    ),
    responses(
        (status = 202, description = "Reprocessing pipeline scheduled", body = ReprocessResponse),
        (status = 404, description = "Call not found", body = crate::errors::ApiErrorResponse)
    )
)]
pub async fn reprocess_call(
    State(state): State<AppState>,
    Path(id): Path<CallId>,
    Query(query): Query<ReprocessCallQuery>,
) -> Result<Response, ApiError> {
    let _ = state
        .call_repo
        .get_by_id(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Call {id} not found")))?;

    // Reset status to pending
    state
        .call_repo
        .update_status(id, callmind_core::ProcessingStatus::Pending)
        .await?;

    // Re-enqueue job with optional language hint
    let mut payload = serde_json::json!({ "call_id": id.to_string() });
    if let Some(ref lang) = query.language {
        if lang != "auto" && !lang.is_empty() {
            payload["language_hint"] = serde_json::Value::String(lang.clone());
        }
    }

    let enqueue_req =
        callmind_core::EnqueueJob::new(callmind_core::JobKind::IngestRecording, payload)
            .with_call_id(id);

    state.job_repo.enqueue(&enqueue_req).await?;

    let resp = ReprocessResponse {
        call_id: id,
        status: "reprocessing_queued".to_string(),
    };

    Ok((StatusCode::ACCEPTED, Json(resp)).into_response())
}
