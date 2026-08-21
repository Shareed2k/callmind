use crate::errors::ApiError;
use crate::state::AppState;
use axum::Json;
use axum::body::Body;
use axum::extract::{Path, Query, Request, State};
use axum::http::header::{ACCEPT_RANGES, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, RANGE};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use callmind_core::{CallId, EnqueueJob, JobKind, Recording, RecordingId};
use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize};
use std::io::SeekFrom;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct UploadRecordingResponse {
    pub recording_id: RecordingId,
    pub call_id: CallId,
    pub status: String,
    pub file_size_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct GetRecordingQuery {
    pub format: Option<String>,
}

#[utoipa::path(
    post,
    path = "/api/v1/calls/{id}/recording",
    tag = "Recordings",
    params(
        ("id" = CallId, Path, description = "Call UUID identifier")
    ),
    responses(
        (status = 202, description = "Audio recording uploaded and processing queued", body = UploadRecordingResponse),
        (status = 404, description = "Call not found", body = crate::errors::ApiErrorResponse),
        (status = 409, description = "Recording already exists for this call", body = crate::errors::ApiErrorResponse)
    )
)]
pub async fn upload_recording(
    State(state): State<AppState>,
    Path(id): Path<CallId>,
    headers: HeaderMap,
    request: Request,
) -> Result<Response, ApiError> {
    let call = state
        .call_repo
        .get_by_id(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Call {id} not found")))?;

    if state
        .call_repo
        .get_recording_by_call_id(id)
        .await?
        .is_some()
    {
        return Err(ApiError::Conflict(format!(
            "Call {id} already has a recording attached"
        )));
    }

    let raw_mime = headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("audio/wav");

    let clean_mime = raw_mime
        .split(';')
        .next()
        .unwrap_or("audio/wav")
        .trim()
        .to_ascii_lowercase();

    let ext = match clean_mime.as_str() {
        "audio/webm" => "webm",
        "audio/ogg" | "audio/opus" => "ogg",
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/m4a" | "audio/mp4" | "audio/aac" => "m4a",
        "audio/flac" => "flac",
        _ => "wav",
    };

    let upload_nonce = uuid::Uuid::new_v4();
    let storage_key = format!("{}/{}_{}.{}", call.organization_id, id, upload_nonce, ext);

    let stream = request
        .into_body()
        .into_data_stream()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));

    let put_res = state.storage.put(&storage_key, Box::pin(stream)).await?;

    if put_res.size_bytes == 0 {
        let _ = state.storage.delete(&storage_key).await;
        return Err(ApiError::BadRequest(
            "Uploaded audio stream is empty".into(),
        ));
    }

    // Validate that the uploaded audio file can be decoded
    let local_path = state.storage.get_local_path(&storage_key).await?;
    if let Err(e) = callmind_audio::AudioDecoder::decode_file(&local_path) {
        let _ = state.storage.delete(&storage_key).await;
        return Err(ApiError::BadRequest(format!(
            "Unsupported or corrupted audio stream: {e}"
        )));
    }

    let recording = Recording::new(
        id,
        storage_key.clone(),
        clean_mime,
        put_res.size_bytes,
        put_res.sha256.clone(),
    );

    if let Err(e) = state.call_repo.add_recording(&recording).await {
        let _ = state.storage.delete(&storage_key).await;
        return Err(ApiError::from(e));
    }

    // Enqueue Ingest job for downstream VAD / STT processing
    let enqueue_req = EnqueueJob::new(
        JobKind::IngestRecording,
        serde_json::json!({
            "call_id": id.to_string(),
            "recording_id": recording.id.to_string(),
        }),
    )
    .with_call_id(id);

    if let Err(e) = state.job_repo.enqueue(&enqueue_req).await {
        let _ = state.storage.delete(&storage_key).await;
        let _ = sqlx::query("DELETE FROM call_recordings WHERE id = ?")
            .bind(recording.id.to_string())
            .execute(&state.pool)
            .await;
        return Err(ApiError::from(e));
    }

    let resp = UploadRecordingResponse {
        recording_id: recording.id,
        call_id: id,
        status: "queued".to_string(),
        file_size_bytes: put_res.size_bytes,
        sha256: put_res.sha256,
    };

    Ok((StatusCode::ACCEPTED, Json(resp)).into_response())
}

pub fn normalize_mime_type(mime: &str, storage_key: &str) -> &'static str {
    let lower = mime
        .split(';')
        .next()
        .unwrap_or(mime)
        .trim()
        .to_ascii_lowercase();
    if lower == "audio/webm" {
        "audio/webm"
    } else if lower == "audio/m4a"
        || lower == "audio/x-m4a"
        || lower == "audio/aac"
        || lower == "audio/mp4"
    {
        "audio/mp4"
    } else if lower == "audio/mp3" || lower == "audio/mpeg" {
        "audio/mpeg"
    } else if lower == "audio/ogg" || lower == "audio/opus" {
        "audio/ogg"
    } else if lower == "audio/flac" {
        "audio/flac"
    } else if lower == "audio/wav" || lower == "audio/x-wav" || lower == "audio/wave" {
        "audio/wav"
    } else {
        let path = std::path::Path::new(storage_key);
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        match ext.as_str() {
            "webm" | "mkv" => "audio/webm",
            "m4a" | "mp4" | "aac" => "audio/mp4",
            "mp3" => "audio/mpeg",
            "ogg" | "opus" => "audio/ogg",
            "flac" => "audio/flac",
            _ => "audio/wav",
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/calls/{id}/recording",
    tag = "Recordings",
    params(
        ("id" = CallId, Path, description = "Call UUID identifier"),
        GetRecordingQuery
    ),
    responses(
        (status = 200, description = "Full audio stream"),
        (status = 206, description = "Partial audio stream for range request"),
        (status = 404, description = "Recording not found", body = crate::errors::ApiErrorResponse)
    )
)]
pub async fn get_recording(
    State(state): State<AppState>,
    Path(id): Path<CallId>,
    Query(query): Query<GetRecordingQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let recording = state
        .call_repo
        .get_recording_by_call_id(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Recording for call {id} not found")))?;

    // If WAV format is requested, transcode on the fly to standard PCM WAV (100% browser compatible)
    if query.format.as_deref() == Some("wav") {
        let mut file = state.storage.get(&recording.storage_key).await?;
        let mut raw_bytes = Vec::with_capacity(recording.file_size_bytes as usize);
        file.read_to_end(&mut raw_bytes).await.map_err(|e| {
            ApiError::Internal(format!("Failed to read recording from storage: {e}"))
        })?;

        let ext = recording
            .storage_key
            .split('.')
            .next_back()
            .unwrap_or("m4a");
        let audio_buffer = callmind_audio::AudioDecoder::decode_bytes(&raw_bytes, Some(ext))
            .map_err(|e| ApiError::Internal(format!("Failed to decode audio to WAV: {e}")))?;

        let wav_bytes = audio_buffer.to_wav_bytes();
        let total_size = wav_bytes.len() as u64;
        let content_disposition = format!("inline; filename=\"call-{id}.wav\"");

        if let Some(range_header) = headers.get(RANGE).and_then(|v| v.to_str().ok()) {
            if let Some((start, end)) = parse_range_header(range_header, total_size) {
                let start_idx = start as usize;
                let end_idx = (end as usize) + 1;
                let chunk = wav_bytes[start_idx..end_idx].to_vec();
                let chunk_size = chunk.len();
                let content_range = format!("bytes {start}-{end}/{total_size}");

                return Ok(Response::builder()
                    .status(StatusCode::PARTIAL_CONTENT)
                    .header(CONTENT_TYPE, "audio/wav")
                    .header(CONTENT_LENGTH, chunk_size.to_string())
                    .header(CONTENT_RANGE, content_range)
                    .header(ACCEPT_RANGES, "bytes")
                    .header(axum::http::header::CONTENT_DISPOSITION, content_disposition)
                    .body(Body::from(chunk))
                    .unwrap());
            }

            return Ok(Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(CONTENT_RANGE, format!("bytes */{total_size}"))
                .body(Body::empty())
                .unwrap());
        }

        return Ok(Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, "audio/wav")
            .header(CONTENT_LENGTH, total_size.to_string())
            .header(ACCEPT_RANGES, "bytes")
            .header(axum::http::header::CONTENT_DISPOSITION, content_disposition)
            .body(Body::from(wav_bytes))
            .unwrap());
    }

    let mut file = state.storage.get(&recording.storage_key).await?;
    let total_size = recording.file_size_bytes;
    let mime_type = normalize_mime_type(&recording.mime_type, &recording.storage_key);

    let ext = match mime_type {
        "audio/mp4" => "m4a",
        "audio/mpeg" => "mp3",
        "audio/ogg" => "ogg",
        "audio/flac" => "flac",
        _ => "wav",
    };
    let content_disposition = format!("inline; filename=\"call-{id}.{ext}\"");

    if let Some(range_header) = headers.get(RANGE).and_then(|v| v.to_str().ok()) {
        if let Some((start, end)) = parse_range_header(range_header, total_size) {
            let chunk_size = end - start + 1;
            file.seek(SeekFrom::Start(start))
                .await
                .map_err(|e| ApiError::Internal(format!("Failed to seek file: {e}")))?;

            let limited_reader = file.take(chunk_size);
            let stream = ReaderStream::new(limited_reader);
            let body = Body::from_stream(stream);

            let content_range = format!("bytes {start}-{end}/{total_size}");

            return Ok(Response::builder()
                .status(StatusCode::PARTIAL_CONTENT)
                .header(CONTENT_TYPE, mime_type)
                .header(CONTENT_LENGTH, chunk_size.to_string())
                .header(CONTENT_RANGE, content_range)
                .header(ACCEPT_RANGES, "bytes")
                .header(axum::http::header::CONTENT_DISPOSITION, content_disposition)
                .body(body)
                .unwrap());
        }

        return Ok(Response::builder()
            .status(StatusCode::RANGE_NOT_SATISFIABLE)
            .header(CONTENT_RANGE, format!("bytes */{total_size}"))
            .body(Body::empty())
            .unwrap());
    }

    // Full audio stream
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, mime_type)
        .header(CONTENT_LENGTH, total_size.to_string())
        .header(ACCEPT_RANGES, "bytes")
        .header(axum::http::header::CONTENT_DISPOSITION, content_disposition)
        .body(body)
        .unwrap())
}

/// Helper function to parse standard HTTP Range headers (RFC 7233).
fn parse_range_header(range_str: &str, total_size: u64) -> Option<(u64, u64)> {
    if total_size == 0 || !range_str.starts_with("bytes=") {
        return None;
    }

    let range_part = range_str.strip_prefix("bytes=")?;
    let parts: Vec<&str> = range_part.split('-').collect();
    if parts.len() != 2 {
        return None;
    }

    if parts[0].is_empty() {
        // Suffix byte range: "-500" means last 500 bytes
        let suffix_len = parts[1].parse::<u64>().ok()?;
        if suffix_len == 0 {
            return None;
        }
        let start = total_size.saturating_sub(suffix_len);
        let end = total_size.saturating_sub(1);
        Some((start, end))
    } else {
        let start = parts[0].parse::<u64>().ok()?;
        if start >= total_size {
            return None;
        }
        let end = if parts[1].is_empty() {
            total_size.saturating_sub(1)
        } else {
            let req_end = parts[1].parse::<u64>().ok()?;
            req_end.min(total_size.saturating_sub(1))
        };

        if start <= end {
            Some((start, end))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_range_header() {
        assert_eq!(parse_range_header("bytes=0-499", 1000), Some((0, 499)));
        assert_eq!(parse_range_header("bytes=500-", 1000), Some((500, 999)));
        assert_eq!(parse_range_header("bytes=0-2000", 1000), Some((0, 999)));
        assert_eq!(parse_range_header("bytes=-500", 1000), Some((500, 999)));
        assert_eq!(parse_range_header("bytes=-1500", 1000), Some((0, 999)));
        assert_eq!(parse_range_header("bytes=1000-", 1000), None);
        assert_eq!(parse_range_header("invalid", 1000), None);
    }

    #[test]
    fn test_normalize_mime_type() {
        assert_eq!(normalize_mime_type("audio/m4a", "call.m4a"), "audio/mp4");
        assert_eq!(normalize_mime_type("audio/x-m4a", "call.m4a"), "audio/mp4");
        assert_eq!(normalize_mime_type("audio/mp4", "call.m4a"), "audio/mp4");
        assert_eq!(normalize_mime_type("audio/mpeg", "call.mp3"), "audio/mpeg");
        assert_eq!(normalize_mime_type("audio/wav", "call.wav"), "audio/wav");
    }
}
