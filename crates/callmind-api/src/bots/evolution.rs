use crate::bots::formatter::BotResponseFormatter;
use crate::errors::ApiError;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use callmind_config::EvolutionBotConfig;
use callmind_core::{Call, CallDirection, EnqueueJob, JobKind, OrgId, ProcessingStatus, Recording};
use serde_json::{Value, json};
use sha2::Digest;
use std::sync::OnceLock;
use std::time::Duration;
use subtle::ConstantTimeEq as _;
use tokio::sync::Semaphore;
use tracing::{error, info, warn};

/// Concurrent result-waiters. Each one only polls for completion, but an
/// unbounded number of them would still pile onto SQLite.
const MAX_CONCURRENT_DELIVERIES: usize = 16;

/// How often to check whether the pipeline has finished.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

fn delivery_slots() -> &'static Semaphore {
    static SLOTS: OnceLock<Semaphore> = OnceLock::new();
    SLOTS.get_or_init(|| Semaphore::new(MAX_CONCURRENT_DELIVERIES))
}

/// Evolution does not sign its webhooks, so a shared secret is the only thing
/// standing between this route and fabricated calls. Configure it as a custom
/// header on the Evolution webhook, or pass `?token=`.
fn verify_webhook_token(
    config: &EvolutionBotConfig,
    headers: &HeaderMap,
    query_token: Option<&str>,
) -> Result<(), ApiError> {
    let Some(expected) = config.webhook_token.as_deref().map(str::trim) else {
        return Ok(());
    };
    if expected.is_empty() {
        return Ok(());
    }

    let presented = headers
        .get("x-webhook-token")
        .and_then(|v| v.to_str().ok())
        .or(query_token)
        .unwrap_or("");

    if presented
        .trim()
        .as_bytes()
        .ct_eq(expected.as_bytes())
        .into()
    {
        Ok(())
    } else {
        Err(ApiError::Unauthorized(
            "Invalid or missing Evolution webhook token".into(),
        ))
    }
}

/// Strip the WhatsApp JID suffix: `972500000000@s.whatsapp.net` -> `972500000000`.
fn jid_to_number(jid: &str) -> &str {
    jid.split('@').next().unwrap_or(jid)
}

/// Pick a file extension from a WhatsApp audio mimetype.
fn extension_for_mime(mimetype: &str) -> &'static str {
    let base = mimetype
        .split(';')
        .next()
        .unwrap_or(mimetype)
        .trim()
        .to_ascii_lowercase();
    match base.as_str() {
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/mp4" | "audio/m4a" | "audio/x-m4a" | "audio/aac" => "m4a",
        "audio/wav" | "audio/x-wav" => "wav",
        // Baileys voice notes are OGG/Opus.
        _ => "ogg",
    }
}

/// `MESSAGES_UPSERT` webhook, with the event name appended to the path.
///
/// Evolution appends it when the instance is configured with
/// `webhookByEvents: true`, so both shapes have to be routable.
pub async fn handle_evolution_webhook_by_event(
    state: State<AppState>,
    Path(event): Path<String>,
    headers: HeaderMap,
    payload: Json<Value>,
) -> Result<Response, ApiError> {
    info!("Evolution webhook event path: {event}");
    handle_evolution_webhook(state, headers, payload).await
}

/// Inbound webhook from a self-hosted Evolution API instance.
pub async fn handle_evolution_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> Result<Response, ApiError> {
    let config = state.config.bots.evolution.clone();
    if !config.enabled {
        return Err(ApiError::BadRequest(
            "Evolution API integration is disabled".into(),
        ));
    }
    verify_webhook_token(&config, &headers, None)?;

    // Acknowledge fast: Evolution retries on slow responses, and the pipeline
    // takes minutes.
    let ack = (StatusCode::OK, "EVENT_RECEIVED").into_response();

    let event = payload["event"].as_str().unwrap_or_default().to_string();
    if !event.eq_ignore_ascii_case("messages.upsert")
        && !event.eq_ignore_ascii_case("messages_upsert")
    {
        return Ok(ack);
    }

    let data = &payload["data"];
    // Our own outgoing messages come back through the same webhook; replying to
    // them would loop forever.
    if data["key"]["fromMe"].as_bool().unwrap_or(false) {
        return Ok(ack);
    }

    if !data["message"]["audioMessage"].is_object() {
        return Ok(ack);
    }

    let Some(remote_jid) = data["key"]["remoteJid"].as_str() else {
        warn!("Evolution webhook message without remoteJid; ignoring");
        return Ok(ack);
    };
    let sender = jid_to_number(remote_jid).to_string();

    if !config.allowed_numbers.is_empty()
        && !config
            .allowed_numbers
            .iter()
            .any(|allowed| jid_to_number(allowed.trim()) == sender)
    {
        warn!("Ignoring Evolution audio from non-allowlisted number");
        return Ok(ack);
    }

    let Some(message_id) = data["key"]["id"].as_str().map(str::to_string) else {
        warn!("Evolution webhook message without key.id; ignoring");
        return Ok(ack);
    };

    let push_name = data["pushName"].as_str().unwrap_or("WhatsApp").to_string();

    tokio::spawn(async move {
        if let Err(e) = process_voice_note(state, config, message_id, sender, push_name).await {
            error!("Evolution voice note processing failed: {e}");
        }
    });

    Ok(ack)
}

/// Fetch the audio bytes, ingest them, then reply once analysis is available.
async fn process_voice_note(
    state: AppState,
    config: EvolutionBotConfig,
    message_id: String,
    sender: String,
    push_name: String,
) -> anyhow::Result<()> {
    let _slot = delivery_slots().acquire().await?;

    let base_url = config
        .base_url
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("bots.evolution.base_url is not set"))?
        .trim_end_matches('/')
        .to_string();
    let instance = config
        .instance
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("bots.evolution.instance is not set"))?
        .to_string();
    let api_key = config
        .api_key
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("bots.evolution.api_key is not set"))?
        .to_string();

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()?;

    send_text(
        &client,
        &base_url,
        &instance,
        &api_key,
        &sender,
        "⏳ Analyzing your voice note with CallMind…",
    )
    .await?;

    let (audio_bytes, fetched_mime) =
        fetch_audio(&client, &base_url, &instance, &api_key, &message_id).await?;
    let extension = extension_for_mime(&fetched_mime);
    let org_id = OrgId::DEFAULT;
    let call = Call::new(
        org_id,
        Some(format!("wa_{sender}_{message_id}.{extension}")),
        CallDirection::Incoming,
        Some(format!("{push_name} ({sender})")),
        None,
        Some(chrono::Utc::now()),
    );
    state.call_repo.create(&call).await?;

    let storage_key = format!("{org_id}/{}.{extension}", call.id);
    let size_bytes = audio_bytes.len() as u64;
    let sha256 = hex::encode(sha2::Sha256::digest(&audio_bytes));
    let payload = bytes::Bytes::from(audio_bytes);
    let stream = Box::pin(futures_util::stream::once(async move {
        Ok::<bytes::Bytes, std::io::Error>(payload)
    }));
    state.storage.put(&storage_key, stream).await?;

    state
        .call_repo
        .add_recording(&Recording::new(
            call.id,
            storage_key,
            format!("audio/{extension}"),
            size_bytes,
            sha256,
        ))
        .await?;

    state
        .job_repo
        .enqueue(
            &EnqueueJob::new(
                JobKind::IngestRecording,
                json!({ "call_id": call.id.to_string() }),
            )
            .with_call_id(call.id)
            .with_priority(5000),
        )
        .await?;

    // Bounded wait: never an open-ended poll loop.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(config.result_timeout_secs);
    let mut status = ProcessingStatus::Pending;
    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(POLL_INTERVAL).await;
        if let Some(current) = state.call_repo.get_by_id(call.id).await? {
            status = current.processing_status;
            if matches!(
                status,
                ProcessingStatus::Completed | ProcessingStatus::Failed
            ) {
                break;
            }
        }
    }

    let reply = match status {
        ProcessingStatus::Completed => match state.call_repo.get_analysis_json(call.id).await? {
            Some((_title, full_json)) => {
                BotResponseFormatter::format(
                    &call.id.to_string(),
                    &full_json,
                    &state.config.server.bind,
                )
                .text_markdown
            }
            None => "✅ Processed, but no analysis was stored for this call.".to_string(),
        },
        ProcessingStatus::Failed => {
            "⚠️ Processing failed. The audio may be too short or unclear.".to_string()
        }
        _ => format!(
            "⏱️ Still processing after {}s. Check the web UI for the result.",
            config.result_timeout_secs
        ),
    };

    send_text(&client, &base_url, &instance, &api_key, &sender, &reply).await?;
    Ok(())
}

/// Fetch the voice note in its original OGG/Opus form.
///
/// `convertToMp4` is deliberately off: `callmind-audio` decodes Opus natively,
/// so transcoding to AAC would only add a lossy generation before Whisper ever
/// sees the audio, and cost server-side CPU for nothing.
///
/// Returns the bytes together with the mimetype Evolution reports, so the stored
/// extension matches the real container.
async fn fetch_audio(
    client: &reqwest::Client,
    base_url: &str,
    instance: &str,
    api_key: &str,
    message_id: &str,
) -> anyhow::Result<(Vec<u8>, String)> {
    let response = client
        .post(format!(
            "{base_url}/chat/getBase64FromMediaMessage/{instance}"
        ))
        .header("apikey", api_key)
        .json(&json!({
            "message": { "key": { "id": message_id } },
            "convertToMp4": false
        }))
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;

    let encoded = response["base64"].as_str().ok_or_else(|| {
        anyhow::anyhow!("getBase64FromMediaMessage response contained no base64 field")
    })?;
    let mimetype = response["mimetype"]
        .as_str()
        .unwrap_or("audio/ogg")
        .to_string();

    Ok((BASE64.decode(encoded.trim())?, mimetype))
}

async fn send_text(
    client: &reqwest::Client,
    base_url: &str,
    instance: &str,
    api_key: &str,
    number: &str,
    text: &str,
) -> anyhow::Result<()> {
    let response = client
        .post(format!("{base_url}/message/sendText/{instance}"))
        .header("apikey", api_key)
        .json(&json!({ "number": number, "text": text }))
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        // The body can echo the request; keep it out of the log.
        anyhow::bail!("Evolution sendText failed with status {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jid_is_reduced_to_a_bare_number() {
        assert_eq!(jid_to_number("972500000000@s.whatsapp.net"), "972500000000");
        assert_eq!(jid_to_number("972500000000"), "972500000000");
    }

    #[test]
    fn audio_mimetypes_map_to_extensions() {
        assert_eq!(extension_for_mime("audio/ogg; codecs=opus"), "ogg");
        assert_eq!(extension_for_mime("audio/mpeg"), "mp3");
        assert_eq!(extension_for_mime("audio/mp4"), "m4a");
        assert_eq!(extension_for_mime("AUDIO/WAV"), "wav");
        assert_eq!(extension_for_mime("something/unknown"), "ogg");
    }

    #[test]
    fn webhook_token_is_enforced_when_configured() {
        let mut config = EvolutionBotConfig::default();
        let mut headers = HeaderMap::new();

        // No token configured: open, matching the universal webhook's behaviour.
        assert!(verify_webhook_token(&config, &headers, None).is_ok());

        config.webhook_token = Some("s3cret".into());
        assert!(verify_webhook_token(&config, &headers, None).is_err());
        assert!(verify_webhook_token(&config, &headers, Some("wrong")).is_err());
        assert!(verify_webhook_token(&config, &headers, Some("s3cret")).is_ok());

        headers.insert("x-webhook-token", "s3cret".parse().unwrap());
        assert!(verify_webhook_token(&config, &headers, None).is_ok());
    }
}
