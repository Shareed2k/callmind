use crate::errors::JobExecutionError;
use crate::handler::{JobContext, JobHandler};
use async_trait::async_trait;
use callmind_analysis::AnalysisEngine;
use callmind_audio::AudioDecoder;
use callmind_core::{CallId, ProcessingStatus};
use callmind_db::CallRepository;
use callmind_search::SearchEngine;
use callmind_storage::RecordingStorage;
use callmind_transcript::AudioTranscriber;
use sqlx::SqlitePool;
use std::str::FromStr;
use std::sync::Arc;
use tracing::info;

/// Complete multi-stage call intelligence processing pipeline handler.
pub struct CallPipelineHandler {
    pub call_repo: Arc<dyn CallRepository>,
    pub storage: Arc<dyn RecordingStorage>,
    pub transcriber: Arc<AudioTranscriber>,
    pub analyzer: Arc<AnalysisEngine>,
    pub search_engine: Arc<SearchEngine>,
    pub pool: SqlitePool,
}

#[async_trait]
impl JobHandler for CallPipelineHandler {
    async fn execute(&self, ctx: JobContext) -> Result<(), JobExecutionError> {
        let call_id = if let Some(id) = ctx.job.call_id {
            id
        } else if let Some(id_str) = ctx.job.payload.get("call_id").and_then(|v| v.as_str()) {
            CallId::from_str(id_str).map_err(|e| JobExecutionError::Failed(e.to_string()))?
        } else {
            return Err(JobExecutionError::Failed(
                "No call_id provided in job payload".into(),
            ));
        };

        info!("Starting full intelligence pipeline for Call {}", call_id);

        let call = self
            .call_repo
            .get_by_id(call_id)
            .await
            .map_err(|e| JobExecutionError::Retryable(e.to_string()))?
            .ok_or_else(|| {
                JobExecutionError::Failed(format!("Call {call_id} not found in database"))
            })?;

        let recording = self
            .call_repo
            .get_recording_by_call_id(call_id)
            .await
            .map_err(|e| JobExecutionError::Retryable(e.to_string()))?
            .ok_or_else(|| {
                JobExecutionError::Failed(format!("No recording found for Call {call_id}"))
            })?;

        // Update state to processing
        let _ = self
            .call_repo
            .update_status(call_id, ProcessingStatus::Processing)
            .await;

        // 1. Load and decode audio file
        let local_path = self
            .storage
            .get_local_path(&recording.storage_key)
            .await
            .map_err(|e| {
                JobExecutionError::Retryable(format!("Failed to locate audio recording: {e}"))
            })?;

        let decoded_audio = AudioDecoder::decode_file(&local_path)
            .map_err(|e| JobExecutionError::Failed(format!("Audio decoding failed: {e}")))?;

        if ctx.cancellation_token.is_cancelled() {
            return Err(JobExecutionError::Cancelled);
        }

        // 2. Deep Audio Transcription (VAD, LID, Parallel STT + Diarization, Alignment, Normalization)
        let explicit_lang: Option<callmind_core::Language> = ctx
            .job
            .payload
            .get("language_hint")
            .and_then(|v| v.as_str())
            .and_then(|l| l.parse().ok());

        let channel_mapping: Option<callmind_core::ChannelMapping> = ctx
            .job
            .payload
            .get("channel_mapping")
            .and_then(|v| serde_json::from_value(v.clone()).ok());

        let transcript = self
            .transcriber
            .transcribe_conversation(
                call_id,
                &decoded_audio,
                explicit_lang,
                channel_mapping.as_ref(),
                &[],
            )
            .await
            .map_err(|e| {
                JobExecutionError::Retryable(format!("Audio transcription failed: {e}"))
            })?;

        if ctx.cancellation_token.is_cancelled() {
            return Err(JobExecutionError::Cancelled);
        }

        // 3. Run Conversation Intelligence Analysis
        let analysis = self
            .analyzer
            .analyze(&transcript, "Organization", &[])
            .await
            .map_err(|e| JobExecutionError::Retryable(format!("Analysis engine failed: {e}")))?;

        // 4. Execute Atomic Database Transaction for Transcripts, Analysis, and Status
        let mut tx = self.pool.begin().await.map_err(|e| {
            JobExecutionError::Retryable(format!("Failed to begin database transaction: {e}"))
        })?;

        let transcript_json = serde_json::to_string(&transcript).unwrap_or_default();
        sqlx::query("DELETE FROM call_transcripts WHERE call_id = ?")
            .bind(call_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(|e| {
                JobExecutionError::Failed(format!("Failed to delete old transcript: {e}"))
            })?;

        sqlx::query(
            "INSERT INTO call_transcripts (call_id, transcript_json, created_at) VALUES (?, ?, ?)",
        )
        .bind(call_id.to_string())
        .bind(&transcript_json)
        .bind(analysis.created_at.to_rfc3339())
        .execute(&mut *tx)
        .await
        .map_err(|e| JobExecutionError::Failed(format!("Failed to insert transcript: {e}")))?;

        let analysis_id = analysis.id.to_string();
        let title = &analysis.title;
        let summary = &analysis.summary;
        let reason = analysis.reason.as_deref();
        let resolution = analysis.resolution.as_deref();
        let resolved = i32::from(analysis.resolved);
        let customer_intent = analysis.customer_intent.as_deref();
        let sentiment_score = analysis.sentiment_score;
        let metrics_json = serde_json::to_string(&analysis.metrics).unwrap_or_default();
        let full_analysis_json = serde_json::to_string(&analysis).unwrap_or_default();
        let created_at = analysis.created_at.to_rfc3339();

        sqlx::query("DELETE FROM call_analyses WHERE call_id = ?")
            .bind(call_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(|e| {
                JobExecutionError::Failed(format!("Failed to delete old analysis: {e}"))
            })?;

        sqlx::query(
            r#"
            INSERT INTO call_analyses (
                id, call_id, title, summary, reason, resolution,
                resolved, customer_intent, sentiment_score,
                metrics_json, full_analysis_json, created_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&analysis_id)
        .bind(call_id.to_string())
        .bind(title)
        .bind(summary)
        .bind(reason)
        .bind(resolution)
        .bind(resolved)
        .bind(customer_intent)
        .bind(sentiment_score)
        .bind(&metrics_json)
        .bind(&full_analysis_json)
        .bind(&created_at)
        .execute(&mut *tx)
        .await
        .map_err(|e| JobExecutionError::Failed(format!("Failed to save call analysis: {e}")))?;

        // 11. Index in SQLite FTS5 Full-Text Search inside the atomic transaction
        let topic_names: Vec<String> = analysis.topics.iter().map(|t| t.name.clone()).collect();
        let entity_values: Vec<String> =
            analysis.entities.iter().map(|e| e.value.clone()).collect();
        let full_transcript_text = transcript.full_text();

        sqlx::query("DELETE FROM fts_calls WHERE call_id = ?")
            .bind(call_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(|e| {
                JobExecutionError::Failed(format!("Failed to clear old FTS index: {e}"))
            })?;

        sqlx::query(
            r#"
            INSERT INTO fts_calls (
                call_id, organization_id, title, summary, transcript,
                topics, entities, reason, resolution
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(call_id.to_string())
        .bind(call.organization_id.to_string())
        .bind(title)
        .bind(summary)
        .bind(&full_transcript_text)
        .bind(topic_names.join(" "))
        .bind(entity_values.join(" "))
        .bind(reason.unwrap_or_default())
        .bind(resolution.unwrap_or_default())
        .execute(&mut *tx)
        .await
        .map_err(|e| JobExecutionError::Failed(format!("Failed to write FTS index: {e}")))?;

        let now_str = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            "UPDATE calls SET processing_status = 'completed', updated_at = ? WHERE id = ?",
        )
        .bind(&now_str)
        .bind(call_id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(|e| JobExecutionError::Failed(format!("Failed to update call status: {e}")))?;

        tx.commit().await.map_err(|e| {
            JobExecutionError::Failed(format!("Failed to commit pipeline transaction: {e}"))
        })?;

        info!(
            "Successfully finished intelligence pipeline for Call {}",
            call_id
        );
        Ok(())
    }
}
