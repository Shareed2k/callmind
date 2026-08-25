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
    /// Plugins that want the audio and transcript together. Empty by default;
    /// nothing here names any of them.
    pub plugins: Vec<Arc<dyn crate::plugin::Plugin>>,
    /// Voice prints. Its own trait because these are biometric data and the
    /// surface that touches them is worth keeping small.
    pub speaker_repo: Arc<dyn callmind_db::SpeakerRepository>,
    /// Where to queue the outbound delivery of a finished call and the jobs
    /// dispatched to remote plugins. `None` when neither is configured, which
    /// is the default: nothing leaves the machine unless somebody asked for it.
    pub job_queue: Option<Arc<dyn callmind_db::JobRepository>>,
    /// Plugin kinds handed to remote workers after transcription.
    ///
    /// Names only: the core dispatches a job per kind and stores whatever comes
    /// back. What a plugin does is its own business, and deliberately not
    /// modelled here.
    pub remote_plugin_kinds: Vec<String>,
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

                let decode_started = std::time::Instant::now();
                let decoded_audio = AudioDecoder::decode_file(&local_path).map_err(|e| {
                    JobExecutionError::Failed(format!("Audio decoding failed: {e}"))
                })?;
                // Per-stage timing, as fields rather than prose: this is what
                // answers "which stage ate the forty minutes" without anybody
                // grepping a log.
                tracing::info!(
                    call_id = %call_id,
                    stage = "decode",
                    ms = decode_started.elapsed().as_millis() as u64,
                    audio_ms = decoded_audio.duration_ms(),
                    "pipeline stage finished"
                );

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

                let transcribe_started = std::time::Instant::now();
                let outcome = self
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

                tracing::info!(
                    call_id = %call_id,
                    stage = "transcribe",
                    ms = transcribe_started.elapsed().as_millis() as u64,
                    segments = outcome.transcript.segments.len(),
                    speakers = outcome.speaker_embeddings.len(),
                    "pipeline stage finished"
                );

                // Voice prints, so this speaker can be recognised in a later
                // call. Stored before the transcript because a name recognised
                // from them is written into that transcript below.
                let mut recognised: std::collections::HashMap<u16, String> =
                    std::collections::HashMap::new();
                if !outcome.speaker_embeddings.is_empty() {
                    let known = self
                        .speaker_repo
                        .list_named_speakers(call.organization_id)
                        .await
                        .unwrap_or_default();
                    let exemplars: Vec<callmind_diarization::identity::KnownSpeaker> = known
                        .into_iter()
                        .map(
                            |(name, embedding)| callmind_diarization::identity::KnownSpeaker {
                                name,
                                embedding,
                            },
                        )
                        .collect();

                    for (speaker, embedding) in &outcome.speaker_embeddings {
                        if let Err(e) = self
                            .speaker_repo
                            .save_speaker_embedding(call_id, *speaker, embedding)
                            .await
                        {
                            // Metadata, not the transcript: log and carry on.
                            tracing::warn!("Failed to store a voice print: {e}");
                        }
                        if let Some(hit) = callmind_diarization::identity::identify(
                            embedding,
                            &exemplars,
                            callmind_diarization::identity::SAME_SPEAKER_DISTANCE,
                        ) {
                            tracing::debug!(
                                speaker = speaker.as_u16(),
                                name = %hit.name,
                                distance = hit.distance,
                                "recognised a speaker from an earlier call"
                            );
                            recognised.insert(speaker.as_u16(), hit.name);
                        }
                    }
                }

                let transcript = outcome.transcript;

                // Committed on its own, before analysis runs, so a crash or a
                // later-stage failure cannot discard it.
                let transcript_json = serde_json::to_string(&transcript).map_err(|e| {
                    JobExecutionError::Retryable(format!("Failed to serialize transcript: {e}"))
                })?;
                // A recognised name goes in through the same helper the rename
                // endpoint uses, so there is one implementation of what
                // `speaker_label` means.
                let transcript_json = if recognised.is_empty() {
                    transcript_json
                } else {
                    serde_json::from_str::<serde_json::Value>(&transcript_json)
                        .ok()
                        .and_then(|mut value| {
                            let changed = callmind_transcript::labels::apply_speaker_labels(
                                &mut value,
                                &recognised,
                            );
                            (changed > 0)
                                .then(|| serde_json::to_string(&value).ok())
                                .flatten()
                        })
                        .unwrap_or(transcript_json)
                };

                self.call_repo
                    .save_transcript(call_id, &transcript_json)
                    .await
                    .map_err(|e| {
                        JobExecutionError::Retryable(format!("Failed to store transcript: {e}"))
                    })?;

                // Plugins see the audio and the transcript together -- the one
                // point in the pipeline where both exist. Run after the
                // transcript is committed, so a plugin cannot cost the call its
                // transcription.
                if !self.plugins.is_empty() {
                    let produced = crate::plugin::run_transcript_plugins(
                        &self.plugins,
                        &crate::plugin::CallAnalysisContext {
                            call_id,
                            audio: &decoded_audio,
                            transcript: &transcript,
                        },
                    )
                    .await;
                    for (plugin, value) in produced {
                        let payload = serde_json::to_string(&value).unwrap_or_else(|_| "{}".into());
                        if let Err(e) = self
                            .call_repo
                            .save_plugin_result(call_id, &plugin, &payload)
                            .await
                        {
                            tracing::warn!("Failed to store the '{plugin}' result: {e}");
                        }
                    }
                }

                // Remote plugins get a job each, leased by a worker that declares the
                // kind. A kind nobody serves leaves a job pending -- visible in the
                // queue rather than silently skipped.
                //
                // A failed enqueue is logged, not propagated: the transcript is
                // already committed, and this whole branch is skipped on a retry
                // once the transcript is stored (see the `Some(Ok(existing))` arm
                // above). Failing the job here would only pay for another LLM
                // analysis pass without ever retrying the enqueue itself.
                for kind in &self.remote_plugin_kinds {
                    let request = callmind_core::EnqueueJob::new(
                        callmind_core::JobKind::Custom(kind.clone()),
                        serde_json::json!({ "call_id": call_id.to_string() }),
                    )
                    .with_call_id(call_id);

                    if let Some(queue) = &self.job_queue {
                        if let Err(e) = queue.enqueue(&request).await {
                            tracing::warn!(
                                "Failed to dispatch the '{kind}' plugin job for call {call_id}: {e}"
                            );
                        }
                    }
                }

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

        let analysis_started = std::time::Instant::now();
        let analysis = self
            .analyzer
            .analyze(&transcript, &organization_name, &[])
            .await
            .map_err(|e| JobExecutionError::Retryable(format!("Analysis engine failed: {e}")))?;
        tracing::info!(
            call_id = %call_id,
            stage = "analyze",
            ms = analysis_started.elapsed().as_millis() as u64,
            "pipeline stage finished"
        );

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

        // 6. Hand the finished call to the outbound webhook, if one is configured.
        //
        // Deliberately carries no phone numbers: this payload leaves the machine,
        // and a receiver that wants them can ask for the call by id.
        if let Some(queue) = &self.job_queue {
            let payload = serde_json::json!({
                "event": "call.completed",
                "call_id": call_id.to_string(),
                "organization_id": call.organization_id.to_string(),
                "title": analysis.title,
                "summary": analysis.summary,
                "resolved": analysis.resolved,
                "sentiment_score": analysis.sentiment_score,
                "action_items": analysis.action_items,
                "key_facts": analysis.key_facts,
                "topics": topic_names,
                "language": transcript.languages.first().map(|l| l.language.clone()),
                "duration_ms": call.duration_ms,
                "completed_at": analysis.created_at,
            });

            let request =
                callmind_core::EnqueueJob::new(callmind_core::JobKind::DeliverWebhook, payload)
                    .with_call_id(call_id);

            // Retryable rather than swallowed: a delivery dropped on the floor is
            // invisible to both ends. The retry reuses the stored transcript.
            queue.enqueue(&request).await.map_err(|e| {
                JobExecutionError::Retryable(format!("Failed to queue webhook delivery: {e}"))
            })?;
        }

        info!(
            "Successfully finished intelligence pipeline for Call {}",
            call_id
        );
        Ok(())
    }
}
