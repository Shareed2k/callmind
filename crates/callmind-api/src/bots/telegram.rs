use super::formatter::BotResponseFormatter;
use callmind_config::AppConfig;
use callmind_core::{Call, CallDirection, EnqueueJob, JobKind, OrgId, ProcessingStatus, Recording};
use callmind_db::{CallRepository, JobRepository};
use callmind_storage::RecordingStorage;
use serde_json::Value;
use sha2::Digest;
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info, warn};

const DEFAULT_ORG_ID: &str = "00000000-0000-0000-0000-000000000001";

/// Telegram Bot Listener for personal / family voice intelligence.
pub struct TelegramBotService;

impl TelegramBotService {
    /// Start long-polling Telegram Bot listener in background task.
    pub fn start(
        config: Arc<AppConfig>,
        call_repo: Arc<dyn CallRepository>,
        job_repo: Arc<dyn JobRepository>,
        storage: Arc<dyn RecordingStorage>,
        pool: sqlx::SqlitePool,
    ) {
        if !config.bots.telegram.enabled {
            return;
        }

        let Some(token) = config.bots.telegram.bot_token.clone() else {
            warn!("Telegram bot is enabled, but bot_token is empty.");
            return;
        };

        if token.trim().is_empty() {
            warn!("Telegram bot token is empty. Skipping Telegram bot startup.");
            return;
        }

        info!("Starting CallMind Telegram Bot Assistant...");

        tokio::spawn(async move {
            let client = reqwest::Client::new();
            let mut offset: i64 = 0;

            // Verify bot credentials
            let get_me_url = format!("https://api.telegram.org/bot{token}/getMe");
            match client.get(&get_me_url).send().await {
                Ok(resp) => {
                    if let Ok(json) = resp.json::<Value>().await {
                        let bot_name = json["result"]["username"].as_str().unwrap_or("CallMindBot");
                        info!("Connected to Telegram Bot: @{bot_name}");
                    }
                }
                Err(e) => {
                    error!("Failed to connect to Telegram Bot API: {e}");
                    return;
                }
            }

            loop {
                let updates_url = format!(
                    "https://api.telegram.org/bot{token}/getUpdates?offset={offset}&timeout=25"
                );
                match client.get(&updates_url).send().await {
                    Ok(resp) => {
                        if let Ok(json) = resp.json::<Value>().await {
                            if let Some(updates) = json["result"].as_array() {
                                for update in updates {
                                    if let Some(update_id) = update["update_id"].as_i64() {
                                        offset = update_id + 1;
                                    }

                                    let client_clone = client.clone();
                                    let token_clone = token.clone();
                                    let config_clone = config.clone();
                                    let call_repo_clone = call_repo.clone();
                                    let job_repo_clone = job_repo.clone();
                                    let storage_clone = storage.clone();
                                    let pool_clone = pool.clone();
                                    let update_clone = update.clone();

                                    tokio::spawn(async move {
                                        if let Err(e) = handle_telegram_update(
                                            &client_clone,
                                            &token_clone,
                                            &config_clone,
                                            call_repo_clone,
                                            job_repo_clone,
                                            storage_clone,
                                            &pool_clone,
                                            &update_clone,
                                        )
                                        .await
                                        {
                                            error!("Error processing Telegram update: {e}");
                                        }
                                    });
                                }
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Telegram long-poll connection error: {e}. Retrying in 5s...");
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                }
            }
        });
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_telegram_update(
    client: &reqwest::Client,
    token: &str,
    config: &AppConfig,
    call_repo: Arc<dyn CallRepository>,
    job_repo: Arc<dyn JobRepository>,
    storage: Arc<dyn RecordingStorage>,
    pool: &sqlx::SqlitePool,
    update: &Value,
) -> anyhow::Result<()> {
    let message = update.get("message").or_else(|| update.get("channel_post"));
    let Some(message) = message else {
        return Ok(());
    };

    let chat_id = message["chat"]["id"].as_i64().unwrap_or(0);
    if chat_id == 0 {
        return Ok(());
    }

    // Optional allowed chat IDs filter
    if !config.bots.telegram.allowed_chat_ids.is_empty()
        && !config.bots.telegram.allowed_chat_ids.contains(&chat_id)
    {
        warn!("Ignoring Telegram message from unauthorized chat_id: {chat_id}");
        return Ok(());
    }

    let text = message["text"].as_str().unwrap_or("");
    if text == "/start" || text == "/help" {
        let welcome = r#"👋 *Welcome to CallMind Personal Voice & Call Assistant!*

Forward or send me any:
• 🎙️ *Voice message* (голосовое сообщение / הודעה קולית)
• 📞 *Call recording* (.wav, .m4a, .mp3, .ogg, .opus)
• 📝 *Audio note*

I will automatically:
1. ✍️ Transcribe speech with word-level accuracy
2. 📌 Extract factual summary & key details
3. 📝 Build your Smart To-Do & Grocery checklist
4. 📅 Detect appointments and create a Calendar invite (.ics)!"#;

        send_telegram_message(client, token, chat_id, welcome).await?;
        return Ok(());
    }

    // Check for voice, audio, or document
    let (file_id, original_ext) = if let Some(voice) = message.get("voice") {
        (voice["file_id"].as_str(), "ogg")
    } else if let Some(audio) = message.get("audio") {
        let ext = audio["file_name"]
            .as_str()
            .and_then(|n| n.split('.').next_back())
            .unwrap_or("mp3");
        (audio["file_id"].as_str(), ext)
    } else if let Some(doc) = message.get("document") {
        let ext = doc["file_name"]
            .as_str()
            .and_then(|n| n.split('.').next_back())
            .unwrap_or("m4a");
        (doc["file_id"].as_str(), ext)
    } else {
        (None, "wav")
    };

    let Some(file_id) = file_id else {
        return Ok(());
    };

    send_telegram_message(
        client,
        token,
        chat_id,
        "⏳ *Analyzing your conversation with CallMind AI...*",
    )
    .await?;

    // 1. Get File info from Telegram
    let get_file_url = format!("https://api.telegram.org/bot{token}/getFile?file_id={file_id}");
    let file_info_resp = client
        .get(&get_file_url)
        .send()
        .await?
        .json::<Value>()
        .await?;
    let file_path = file_info_resp["result"]["file_path"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing file_path in Telegram getFile response"))?;

    // 2. Download audio file bytes
    let download_url = format!("https://api.telegram.org/file/bot{token}/{file_path}");
    let audio_resp = client.get(&download_url).send().await?;
    let audio_bytes = audio_resp.bytes().await?;

    // 3. Save into RecordingStorage
    let org_id = OrgId(uuid::Uuid::parse_str(DEFAULT_ORG_ID).unwrap());
    let call = Call::new(
        org_id,
        Some(format!(
            "tg_voice_{}_{file_id}.{original_ext}",
            chrono::Utc::now().timestamp()
        )),
        CallDirection::Incoming,
        Some(format!("Telegram User {chat_id}")),
        None,
        Some(chrono::Utc::now()),
    );
    call_repo.create(&call).await?;

    let storage_key = format!("{}/{}.{}", org_id, call.id, original_ext);
    let size_bytes = audio_bytes.len() as u64;
    let sha256 = hex::encode(sha2::Sha256::digest(&audio_bytes));

    let stream = Box::pin(futures_util::stream::once(async move {
        Ok::<bytes::Bytes, std::io::Error>(audio_bytes)
    }));
    storage.put(&storage_key, stream).await?;

    let mime_type = match original_ext {
        "ogg" | "oga" | "opus" => "audio/ogg",
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
    call_repo.add_recording(&recording).await?;

    // 4. Enqueue Ingestion Job with High Priority
    let enqueue_req = EnqueueJob::new(
        JobKind::IngestRecording,
        serde_json::json!({
            "call_id": call.id.to_string(),
            "recording_id": recording.id.to_string(),
        }),
    )
    .with_call_id(call.id)
    .with_priority(5000);

    job_repo.enqueue(&enqueue_req).await?;

    // 5. Await analysis completion (poll SQLite every 2s for up to 180s)
    let mut poll_attempts = 0;
    let call_id_str = call.id.to_string();

    while poll_attempts < 90 {
        tokio::time::sleep(Duration::from_secs(2)).await;
        poll_attempts += 1;

        let status_row: Option<(String,)> =
            sqlx::query_as("SELECT processing_status FROM calls WHERE id = ?")
                .bind(&call_id_str)
                .fetch_optional(pool)
                .await?;

        if let Some((status,)) = status_row {
            if status == ProcessingStatus::Completed.as_str() {
                break;
            }
            if status == ProcessingStatus::Failed.as_str() {
                send_telegram_message(
                    client,
                    token,
                    chat_id,
                    "⚠️ *Processing encountered an issue.* Please check audio clarity.",
                )
                .await?;
                return Ok(());
            }
        }
    }

    // 6. Fetch Analysis result
    let analysis_row: Option<(String, String)> =
        sqlx::query_as("SELECT title, full_analysis_json FROM call_analyses WHERE call_id = ?")
            .bind(&call_id_str)
            .fetch_optional(pool)
            .await?;

    let Some((_title, full_json)) = analysis_row else {
        send_telegram_message(
            client,
            token,
            chat_id,
            "⚠️ Analysis completed, but summary could not be retrieved.",
        )
        .await?;
        return Ok(());
    };

    let formatted = BotResponseFormatter::format(&call_id_str, &full_json, &config.server.bind);

    send_telegram_message(client, token, chat_id, &formatted.text_markdown).await?;

    Ok(())
}

async fn send_telegram_message(
    client: &reqwest::Client,
    token: &str,
    chat_id: i64,
    text: &str,
) -> anyhow::Result<()> {
    let url = format!("https://api.telegram.org/bot{token}/sendMessage");
    client
        .post(&url)
        .json(&serde_json::json!({
            "chat_id": chat_id,
            "text": text,
            "parse_mode": "Markdown",
            "disable_web_page_preview": false,
        }))
        .send()
        .await?;
    Ok(())
}
