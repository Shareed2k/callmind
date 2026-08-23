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
use std::str::FromStr;
use std::sync::Arc;
use tracing::{info, warn};

/// Complete multi-stage call intelligence processing pipeline handler.
pub struct CallPipelineHandler {
    pub call_repo: Arc<dyn CallRepository>,
    pub storage: Arc<dyn RecordingStorage>,
    pub transcriber: Arc<AudioTranscriber>,
    pub analyzer: Arc<AnalysisEngine>,
    pub search: Arc<SearchEngine>,
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

        // 1. Reuse an already-stored transcript when there is one.
        //
        // Transcription is by far the most expensive stage — measured at ~520s
        // for a 43-minute call — and it used to be redone on every attempt.
        // A `docker stop` or a retryable failure after transcription therefore
        // threw away minutes of GPU work, and `reanalyze-all` re-ran
        // speech-to-text for the whole archive when all it needed was the LLM.
        //
        // `force_retranscribe` in the payload opts out, for when the audio or the
        // STT configuration is what changed.
        let force_retranscribe = ctx
            .job
            .payload
            .get("force_retranscribe")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);

        let stored_transcript = if force_retranscribe {
            None
        } else {
            self.call_repo
                .get_transcript_json(call_id)
                .await
                .map_err(|e| JobExecutionError::Retryable(e.to_string()))?
        };

        let transcript = match stored_transcript
            .as_deref()
            .map(serde_json::from_str::<callmind_transcript::Transcript>)
        {
            Some(Ok(existing)) => {
                info!("Reusing stored transcript for Call {call_id}; skipping transcription");
                existing
            }
            other => {
                if let Some(Err(e)) = other {
                    warn!(
                        "Stored transcript for Call {call_id} is unreadable ({e}); re-transcribing"
                    );
                }

                let local_path = self
                    .storage
                    .get_local_path(&recording.storage_key)
                    .await
                    .map_err(|e| {
                        JobExecutionError::Retryable(format!(
                            "Failed to locate audio recording: {e}"
                        ))
                    })?;

                let decoded_audio = AudioDecoder::decode_file(&local_path).map_err(|e| {
                    JobExecutionError::Failed(format!("Audio decoding failed: {e}"))
                })?;

                // Persist the real duration and format now that the audio has
                // been decoded. Only the batch importer set these before, so
                // anything arriving through the upload endpoint or a bot showed a
                // dash for its duration and contributed a zero to the analytics
                // averages. A failure here is logged, not fatal: it is metadata,
                // and losing the transcript over it would be a poor trade.
                if let Err(e) = self
                    .call_repo
                    .set_audio_metadata(
                        call_id,
                        decoded_audio.duration_ms(),
                        decoded_audio.channels,
                        decoded_audio.sample_rate,
                    )
                    .await
                {
                    tracing::warn!("Failed to record audio metadata for call {call_id}: {e}");
                }

                if ctx.cancellation_token.is_cancelled() {
                    return Err(JobExecutionError::Cancelled);
                }

                // 2. VAD, language ID, STT and diarization, aligned into segments.
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

                // How many people are on the recording, when the ingesting
                // channel knows. A voice note is one person by construction; a
                // phone recording is two. Absent the hint the diarizer keeps its
                // own default rather than guessing from the audio, which is not
                // something the embeddings support -- see
                // `callmind-diarization/tests/onnx_centroid_probe.rs`.
                let expected_speakers = ctx
                    .job
                    .payload
                    .get("expected_speakers")
                    .and_then(serde_json::Value::as_u64)
                    .map(|n| n.clamp(1, 10) as usize);

                let transcript = self
                    .transcriber
                    .transcribe_conversation(
                        call_id,
                        &decoded_audio,
                        explicit_lang,
                        channel_mapping.as_ref(),
                        &[],
                        expected_speakers,
                    )
                    .await
                    .map_err(|e| {
                        JobExecutionError::Retryable(format!("Audio transcription failed: {e}"))
                    })?;

                // Committed on its own, before analysis runs, so a crash or a
                // later-stage failure cannot discard it.
                let transcript_json = serde_json::to_string(&transcript).map_err(|e| {
                    JobExecutionError::Retryable(format!("Failed to serialize transcript: {e}"))
                })?;
                self.call_repo
                    .save_transcript(call_id, &transcript_json)
                    .await
                    .map_err(|e| {
                        JobExecutionError::Retryable(format!("Failed to store transcript: {e}"))
                    })?;

                transcript
            }
        };

        if ctx.cancellation_token.is_cancelled() {
            return Err(JobExecutionError::Cancelled);
        }

        // 3. Run Conversation Intelligence Analysis
        //
        // The organization name is fed straight into the LLM prompt, so passing
        // a literal "Organization" told the model the company was actually
        // called that. Fall back to the id only if the row is missing.
        let organization_name = self
            .call_repo
            .get_organization_name(call.organization_id)
            .await
            .map_err(|e| JobExecutionError::Retryable(e.to_string()))?
            .unwrap_or_else(|| call.organization_id.to_string());

        let analysis = self
            .analyzer
            .analyze(&transcript, &organization_name, &[])
            .await
            .map_err(|e| JobExecutionError::Retryable(format!("Analysis engine failed: {e}")))?;

        // 4. Analysis and the call status commit together; the repository owns
        // that transaction so the trait stays free of any one database's
        // connection type.
        let metrics_json = serde_json::to_string(&analysis.metrics).map_err(|e| {
            JobExecutionError::Retryable(format!("Failed to serialize analysis metrics: {e}"))
        })?;
        let full_analysis_json = serde_json::to_string(&analysis).map_err(|e| {
            JobExecutionError::Retryable(format!("Failed to serialize analysis: {e}"))
        })?;

        self.call_repo
            .commit_analysis(
                &callmind_db::AnalysisRow {
                    id: analysis.id,
                    call_id,
                    title: &analysis.title,
                    summary: &analysis.summary,
                    reason: analysis.reason.as_deref(),
                    resolution: analysis.resolution.as_deref(),
                    resolved: analysis.resolved,
                    customer_intent: analysis.customer_intent.as_deref(),
                    sentiment_score: analysis.sentiment_score,
                    metrics_json: &metrics_json,
                    full_analysis_json: &full_analysis_json,
                    created_at: analysis.created_at,
                },
                ProcessingStatus::Completed,
            )
            .await
            .map_err(|e| JobExecutionError::Retryable(format!("Failed to commit analysis: {e}")))?;

        // 5. Index for full-text search. Derived data, so it is written after the
        // commit rather than inside it: a failure here is retryable, and the
        // retry reuses the transcript and rewrites both idempotently.
        let topic_names: Vec<String> = analysis.topics.iter().map(|t| t.name.clone()).collect();
        let entity_values: Vec<String> =
            analysis.entities.iter().map(|e| e.value.clone()).collect();
        let full_transcript_text = transcript.full_text();

        self.search
            .index_call(callmind_search::IndexCallParams {
                call_id,
                org_id: call.organization_id,
                title: &analysis.title,
                summary: &analysis.summary,
                transcript: &full_transcript_text,
                topics: &topic_names,
                entities: &entity_values,
                reason: analysis.reason.as_deref(),
                resolution: analysis.resolution.as_deref(),
            })
            .await
            .map_err(|e| JobExecutionError::Retryable(format!("Failed to write FTS index: {e}")))?;

        info!(
            "Successfully finished intelligence pipeline for Call {}",
            call_id
        );
        Ok(())
    }
}
