use super::formatter::BotResponseFormatter;
use crate::errors::ApiError;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Multipart, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use callmind_core::{Call, CallDirection, EnqueueJob, JobKind, OrgId, ProcessingStatus, Recording};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::time::Duration;

#[derive(Debug, Deserialize)]
pub struct WebhookQuery {
    #[serde(default)]
    pub sync: bool,
    pub title: Option<String>,
    pub secret: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct WebhookAudioResponse {
    pub call_id: String,
    pub status: String,
    pub title: Option<String>,
    pub summary: Option<String>,
    pub action_items: Vec<String>,
    pub key_facts: Vec<String>,
    pub ics_calendar: Option<String>,
    pub text_markdown: Option<String>,
    pub web_player_url: String,
}

/// Universal Audio Webhook Handler (iOS Shortcuts, Siri, Tasker, Zapier, n8n).
pub async fn handle_audio_webhook(
    State(state): State<AppState>,
    Query(query): Query<WebhookQuery>,
    mut multipart: Multipart,
) -> Result<Response, ApiError> {
    // Optional secret verification
    if let Some(ref expected_secret) = state.config.bots.webhook.secret_token {
        if query.secret.as_deref() != Some(expected_secret.as_str()) {
            return Err(ApiError::Unauthorized(
                "Invalid webhook secret token".into(),
            ));
        }
    }

    let mut audio_bytes = Vec::new();
    let mut file_ext = "m4a".to_string();
    let mut filename = "webhook_audio.m4a".to_string();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?
    {
        if let Some(name) = field.name() {
            if name == "audio" || name == "file" || name == "recording" {
                if let Some(fname) = field.file_name() {
                    filename = fname.to_string();
                    if let Some(ext) = fname.split('.').next_back() {
                        file_ext = ext.to_lowercase();
                    }
                }
                audio_bytes = field
                    .bytes()
                    .await
                    .map_err(|e| ApiError::BadRequest(e.to_string()))?
                    .to_vec();
                break;
            }
        }
    }

    if audio_bytes.is_empty() {
        return Err(ApiError::BadRequest(
            "No audio file received in multipart payload. Send multipart field 'audio' or 'file'."
                .into(),
        ));
    }

    let org_id = OrgId::DEFAULT;
    let call = Call::new(
        org_id,
        Some(format!(
            "webhook_{}_{filename}",
            chrono::Utc::now().timestamp()
        )),
        CallDirection::Incoming,
        query
            .title
            .clone()
            .or_else(|| Some("Voice Note".to_string())),
        None,
        Some(chrono::Utc::now()),
    );
    state.call_repo.create(&call).await?;

    let storage_key = format!("{}/{}.{}", org_id, call.id, file_ext);
    let size_bytes = audio_bytes.len() as u64;
    let sha256 = hex::encode(sha2::Sha256::digest(&audio_bytes));

    let stream = Box::pin(futures_util::stream::once(async move {
        Ok::<bytes::Bytes, std::io::Error>(bytes::Bytes::from(audio_bytes))
    }));
    state.storage.put(&storage_key, stream).await?;

    let mime_type = match file_ext.as_str() {
        "ogg" | "opus" => "audio/ogg",
        "mp3" => "audio/mpeg",
        "m4a" | "aac" => "audio/mp4",
        _ => "audio/wav",
    };

    let recording = Recording::new(
        call.id,
        storage_key,
        mime_type.to_string(),
        size_bytes,
        sha256,
    );
    state.call_repo.add_recording(&recording).await?;

    let enqueue_req = EnqueueJob::new(
        JobKind::IngestRecording,
        serde_json::json!({
            "call_id": call.id.to_string(),
            "recording_id": recording.id.to_string(),
        }),
    )
    .with_call_id(call.id)
    .with_priority(5000);

    state.job_repo.enqueue(&enqueue_req).await?;

    let web_player_url = format!("http://{}/calls/{}", state.config.server.bind, call.id);

    if !query.sync {
        return Ok((
            StatusCode::ACCEPTED,
            Json(WebhookAudioResponse {
                call_id: call.id.to_string(),
                status: "queued".to_string(),
                title: query.title,
                summary: None,
                action_items: Vec::new(),
                key_facts: Vec::new(),
                ics_calendar: None,
                text_markdown: None,
                web_player_url,
            }),
        )
            .into_response());
    }

    // Sync mode: await pipeline completion (poll up to 120s)
    let call_id_str = call.id.to_string();
    let mut attempts = 0;
    while attempts < 60 {
        tokio::time::sleep(Duration::from_secs(2)).await;
        attempts += 1;

        if let Some(current) = state.call_repo.get_by_id(call.id).await? {
            match current.processing_status {
                ProcessingStatus::Completed => break,
                ProcessingStatus::Failed => {
                    return Err(ApiError::Internal("Voice processing failed.".into()));
                }
                _ => {}
            }
        }
    }

    if let Some((_title, full_json)) = state.call_repo.get_analysis_json(call.id).await? {
        let formatted =
            BotResponseFormatter::format(&call_id_str, &full_json, &state.config.server.bind);

        let parsed: serde_json::Value = serde_json::from_str(&full_json).unwrap_or_default();
        let action_items = parsed["action_items"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|i| i["text"].as_str().map(ToString::to_string))
                    .collect()
            })
            .unwrap_or_default();

        let key_facts = parsed["key_facts"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|f| f.as_str().map(ToString::to_string))
                    .collect()
            })
            .unwrap_or_default();

        Ok((
            StatusCode::OK,
            Json(WebhookAudioResponse {
                call_id: call.id.to_string(),
                status: "completed".to_string(),
                title: Some(formatted.title),
                summary: Some(formatted.summary),
                action_items,
                key_facts,
                ics_calendar: formatted.ics_content,
                text_markdown: Some(formatted.text_markdown),
                web_player_url,
            }),
        )
            .into_response())
    } else {
        Ok((
            StatusCode::OK,
            Json(WebhookAudioResponse {
                call_id: call.id.to_string(),
                status: "completed".to_string(),
                title: Some("Voice Note".to_string()),
                summary: None,
                action_items: Vec::new(),
                key_facts: Vec::new(),
                ics_calendar: None,
                text_markdown: None,
                web_player_url,
            }),
        )
            .into_response())
    }
}
