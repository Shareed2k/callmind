use crate::errors::ApiError;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use callmind_core::{Call, CallDirection, CallFilter, CallId, CreateCallRequest, OrgId};
use tracing::warn;

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
    // The server's own organization, never the caller's choice. See
    // `CreateCallRequest`.
    let org_id = OrgId::DEFAULT;

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
    // If recording exists, delete from storage along with the cached WAV
    // transcode the player may have generated for it.
    if let Some(recording) = state.call_repo.get_recording_by_call_id(id).await? {
        let cache_key = format!("{}.16k.wav", recording.storage_key);
        if let Err(e) = state.storage.delete(&recording.storage_key).await {
            warn!("Failed to delete recording {}: {e}", recording.storage_key);
        }
        // Absent unless the browser ever requested the fallback.
        if state.storage.exists(&cache_key).await.unwrap_or(false) {
            if let Err(e) = state.storage.delete(&cache_key).await {
                warn!("Failed to delete cached transcode {cache_key}: {e}");
            }
        }
    }

    // Clean up full-text search index
    if let Err(e) = state.search.delete_index(id).await {
        warn!("Failed to remove call {id} from the search index: {e}");
    }

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

    if let Some(ref title) = payload.title {
        state.call_repo.update_analysis_title(id, title).await?;

        state.search.update_indexed_title(id, title).await?;
    }

    if let Some(ref speaker_map) = payload.speaker_names {
        // Two things happen here, and the second is the point: the name is
        // written into this call's transcript, and the speaker's voice print is
        // named so the same person is recognised in later calls without anybody
        // labelling them again.
        if let Some(raw_json) = state.call_repo.get_transcript_json(id).await? {
            if let Ok(mut json_val) = serde_json::from_str::<serde_json::Value>(&raw_json) {
                let changed =
                    callmind_transcript::labels::apply_speaker_labels(&mut json_val, speaker_map);
                if changed > 0 {
                    let updated = serde_json::to_string(&json_val).unwrap_or(raw_json);
                    state.call_repo.update_transcript_json(id, &updated).await?;
                }
            }
        }

        for (speaker, name) in speaker_map {
            if let Err(e) = state
                .speaker_repo
                .name_speaker(id, callmind_core::SpeakerId::new(*speaker), name)
                .await
            {
                // No stored voice print for that speaker -- an older call, or a
                // run without the embedding model. The transcript is still
                // renamed; only cross-call recognition is unavailable.
                tracing::debug!("Speaker {speaker} of call {id} has no voice print: {e}");
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
    let transcript_row = state.call_repo.get_transcript_json(id).await?;

    let json_str = transcript_row
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
        "ics" | "calendar" => {
            let analysis_row = state.call_repo.get_analysis_json(id).await.unwrap_or(None);

            let (title, summary, location) = if let Some((_t, raw_json)) = analysis_row {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw_json) {
                    let t = val
                        .get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or("CallMind Event");
                    let s = val
                        .get("summary")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Event generated from CallMind conversation");
                    let loc = val
                        .get("entities")
                        .and_then(|e| e.as_array())
                        .and_then(|arr| {
                            arr.iter()
                                .find(|item| {
                                    item.get("entity_type")
                                        .and_then(|t| t.as_str())
                                        .is_some_and(|t| {
                                            t.contains("loc")
                                                || t.contains("place")
                                                || t.contains("address")
                                        })
                                })
                                .and_then(|item| item.get("value").and_then(|v| v.as_str()))
                        });
                    (t.to_string(), s.to_string(), loc.map(ToString::to_string))
                } else {
                    (
                        "CallMind Event".to_string(),
                        "Event generated from CallMind".to_string(),
                        None,
                    )
                }
            } else {
                (
                    "CallMind Event".to_string(),
                    "Event generated from CallMind".to_string(),
                    None,
                )
            };

            let ics_body = callmind_transcript::TranscriptExporter::to_ics(
                &call_id_str,
                &title,
                &summary,
                location.as_deref(),
                None,
            );

            ("text/calendar; charset=utf-8", "ics", ics_body)
        }
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
    /// Page size for walking completed calls. The previous implementation asked
    /// for up to 100_000 in one go and held every `Call` in memory at once.
    const PAGE: u32 = 500;

    let mut queued = 0usize;
    let mut failed = 0usize;
    let mut offset = 0u32;

    loop {
        let page = state
            .call_repo
            .list(&CallFilter {
                status: Some(callmind_core::ProcessingStatus::Completed),
                limit: Some(PAGE),
                offset: Some(offset),
                ..Default::default()
            })
            .await?;

        if page.is_empty() {
            break;
        }
        let page_len = u32::try_from(page.len()).unwrap_or(PAGE);

        for call in page {
            let enqueue_req = callmind_core::EnqueueJob::new(
                callmind_core::JobKind::IngestRecording,
                serde_json::json!({ "call_id": call.id.to_string() }),
            )
            .with_call_id(call.id);

            // Errors were swallowed by `.is_ok()`, so a half-failed sweep
            // reported a plausible count and no reason.
            match state.job_repo.enqueue(&enqueue_req).await {
                Ok(_) => queued += 1,
                Err(e) => {
                    failed += 1;
                    warn!("Failed to enqueue reanalysis for call {}: {e}", call.id);
                }
            }
        }

        if page_len < PAGE {
            break;
        }
        offset += page_len;
    }

    // Stored transcripts are reused, so this re-runs the analysis stage only.
    // It used to redo speech-to-text as well, which on an archive of this size
    // is days of GPU time.
    warn!(
        "Queued reanalysis of {queued} call(s) ({failed} failed to enqueue). \
         Stored transcripts are reused; only the analysis stage re-runs."
    );

    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "status": "queued",
            "count": queued,
            "failed": failed,
        })),
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
    /// How many people are on the recording, when the caller knows.
    ///
    /// The count cannot be recovered from the audio -- measured in
    /// `callmind-diarization/tests/onnx_centroid_probe.rs` -- so a recording that
    /// came back with the wrong number of speakers is fixed by saying so here.
    /// `1` for a voice note or a microphone test.
    pub speakers: Option<usize>,
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

    // Re-enqueue with an optional language hint.
    //
    // Reprocessing one call means redoing it properly, including transcription —
    // that is the point of the endpoint, and a language hint only takes effect
    // during transcription. `reanalyze-all` deliberately omits this flag so it
    // reuses stored transcripts and only re-runs the analysis.
    let mut payload = serde_json::json!({
        "call_id": id.to_string(),
        "force_retranscribe": true,
    });
    if let Some(ref lang) = query.language {
        if lang != "auto" && !lang.is_empty() {
            payload["language_hint"] = serde_json::Value::String(lang.clone());
        }
    }
    if let Some(speakers) = query.speakers {
        payload["expected_speakers"] = serde_json::Value::from(speakers.clamp(1, 10));
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
