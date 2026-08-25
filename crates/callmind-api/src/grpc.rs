//! gRPC implementation of the remote-worker contract.
//!
//! See `callmind-worker-proto` for the contract itself. Served on its own port
//! so the worker interface can be firewalled independently of the browser-facing
//! HTTP surface.

use crate::state::AppState;
use callmind_core::{JobId, JobKind, ProcessingStatus};
use callmind_worker_proto::v1::worker_server::Worker;
use callmind_worker_proto::{convert, v1};
use futures_util::StreamExt;
use std::str::FromStr;
use std::time::Duration;
use tokio_util::io::ReaderStream;
use tonic::{Request, Response, Status};
use tracing::{info, warn};

/// Delay before a job a worker gave back becomes leasable again, so a worker in
/// a bad state cannot spin on the same job.
const RETRY_BACKOFF: Duration = Duration::from_secs(30);

pub struct WorkerService {
    state: AppState,
}

impl WorkerService {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }

    fn parse_job_id(raw: &str) -> Result<JobId, Status> {
        JobId::from_str(raw).map_err(|e| Status::invalid_argument(format!("invalid job_id: {e}")))
    }

    fn require_worker_id(worker_id: &str) -> Result<(), Status> {
        if worker_id.trim().is_empty() {
            return Err(Status::invalid_argument("worker_id must not be empty"));
        }
        Ok(())
    }

    /// Who is calling.
    ///
    /// From the client certificate when the listener runs TLS, which is what
    /// makes the answer unforgeable: a caller can put any string in
    /// `worker_id`, so that field is a label for logs. Without TLS -- loopback
    /// only, refused for any other address at startup -- the declared id is all
    /// there is.
    fn caller_identity<T>(&self, request: &Request<T>, declared: &str) -> Result<String, Status> {
        let Some(certs) = request.peer_certs() else {
            Self::require_worker_id(declared)?;
            return Ok(declared.to_string());
        };

        let leaf = certs
            .first()
            .ok_or_else(|| Status::unauthenticated("no client certificate presented"))?;

        self.state
            .worker_names
            .get(&crate::grpc_tls::fingerprint(leaf))
            .cloned()
            .ok_or_else(|| Status::unauthenticated("client certificate is not pinned"))
    }

    /// Confirm the caller still owns the lease.
    ///
    /// Checked before accepting any work so a worker whose lease was reaped
    /// cannot overwrite a result another worker is producing.
    async fn require_lease(&self, job_id: JobId, identity: &str) -> Result<(), Status> {
        let held = self
            .state
            .job_repo
            .renew_lock(job_id, identity)
            .await
            .map_err(|e| Status::internal(format!("lease check failed: {e}")))?;
        if held {
            Ok(())
        } else {
            Err(Status::aborted(format!(
                "lease for job {job_id} is not held by {identity}"
            )))
        }
    }
}

#[tonic::async_trait]
impl Worker for WorkerService {
    async fn lease(
        &self,
        request: Request<v1::LeaseRequest>,
    ) -> Result<Response<v1::LeaseResponse>, Status> {
        let identity = self.caller_identity(&request, &request.get_ref().worker_id)?;
        let req = request.into_inner();

        let kinds: Vec<JobKind> = if req.kinds.is_empty() {
            vec![JobKind::IngestRecording]
        } else {
            req.kinds
                .iter()
                .map(|k| {
                    serde_json::from_value::<JobKind>(serde_json::Value::String(k.clone()))
                        .map_err(|_| Status::invalid_argument(format!("unknown job kind {k:?}")))
                })
                .collect::<Result<_, _>>()?
        };

        let lease_timeout_secs = self.state.config.jobs.lock_timeout_secs;
        let leased = self
            .state
            .job_repo
            .fetch_and_lock(&identity, &kinds)
            .await
            .map_err(|e| Status::internal(format!("lease failed: {e}")))?;

        let Some(job) = leased else {
            return Ok(Response::new(v1::LeaseResponse {
                job: None,
                lease_timeout_secs,
            }));
        };

        // Recording metadata is informational; the audio itself comes over
        // StreamRecording so storage layout stays an internal detail.
        let (mime_type, file_size_bytes) = match job.call_id {
            Some(call_id) => self
                .state
                .call_repo
                .get_recording_by_call_id(call_id)
                .await
                .map_err(|e| Status::internal(format!("recording lookup failed: {e}")))?
                .map_or((String::new(), 0), |r| (r.mime_type, r.file_size_bytes)),
            None => (String::new(), 0),
        };

        let language_hint = job
            .payload
            .get("language_hint")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        info!("Leased job {} to worker {}", job.id, identity);
        Ok(Response::new(v1::LeaseResponse {
            job: Some(v1::Job {
                job_id: job.id.to_string(),
                kind: job.kind.as_str().to_string(),
                call_id: job.call_id.map(|c| c.to_string()).unwrap_or_default(),
                payload_json: job.payload.to_string(),
                attempt: job.attempt,
                max_attempts: job.max_attempts,
                language_hint,
                mime_type,
                file_size_bytes,
            }),
            lease_timeout_secs,
        }))
    }

    type StreamRecordingStream = std::pin::Pin<
        Box<dyn futures_util::Stream<Item = Result<v1::RecordingChunk, Status>> + Send>,
    >;

    async fn stream_recording(
        &self,
        request: Request<v1::StreamRecordingRequest>,
    ) -> Result<Response<Self::StreamRecordingStream>, Status> {
        let identity = self.caller_identity(&request, &request.get_ref().worker_id)?;
        let req = request.into_inner();
        let job_id = Self::parse_job_id(&req.job_id)?;
        self.require_lease(job_id, &identity).await?;

        let job = self
            .state
            .job_repo
            .get_by_id(job_id)
            .await
            .map_err(|e| Status::internal(format!("job lookup failed: {e}")))?
            .ok_or_else(|| Status::not_found(format!("job {job_id} not found")))?;
        let call_id = job
            .call_id
            .ok_or_else(|| Status::failed_precondition("job is not attached to a call"))?;

        let recording = self
            .state
            .call_repo
            .get_recording_by_call_id(call_id)
            .await
            .map_err(|e| Status::internal(format!("recording lookup failed: {e}")))?
            .ok_or_else(|| Status::not_found(format!("no recording for call {call_id}")))?;

        let file = self
            .state
            .storage
            .get(&recording.storage_key)
            .await
            .map_err(|e| Status::internal(format!("storage read failed: {e}")))?;

        // Streamed rather than buffered: a worker should not need to hold a
        // whole recording in memory before it can start.
        let stream = ReaderStream::new(file).map(|chunk| match chunk {
            Ok(data) => Ok(v1::RecordingChunk {
                data: data.to_vec(),
            }),
            Err(e) => Err(Status::internal(format!("recording read failed: {e}"))),
        });

        Ok(Response::new(Box::pin(stream)))
    }

    async fn heartbeat(
        &self,
        request: Request<v1::HeartbeatRequest>,
    ) -> Result<Response<v1::HeartbeatResponse>, Status> {
        let identity = self.caller_identity(&request, &request.get_ref().worker_id)?;
        let req = request.into_inner();
        let job_id = Self::parse_job_id(&req.job_id)?;

        let still_leased = self
            .state
            .job_repo
            .renew_lock(job_id, &identity)
            .await
            .map_err(|e| Status::internal(format!("heartbeat failed: {e}")))?;

        // Reported rather than raised: losing a lease is an expected outcome the
        // worker should react to by leasing again.
        Ok(Response::new(v1::HeartbeatResponse { still_leased }))
    }

    async fn submit_transcript(
        &self,
        request: Request<v1::SubmitTranscriptRequest>,
    ) -> Result<Response<v1::SubmitTranscriptResponse>, Status> {
        let identity = self.caller_identity(&request, &request.get_ref().worker_id)?;
        let req = request.into_inner();
        let job_id = Self::parse_job_id(&req.job_id)?;
        self.require_lease(job_id, &identity).await?;

        let job = self
            .state
            .job_repo
            .get_by_id(job_id)
            .await
            .map_err(|e| Status::internal(format!("job lookup failed: {e}")))?
            .ok_or_else(|| Status::not_found(format!("job {job_id} not found")))?;
        let call_id = job
            .call_id
            .ok_or_else(|| Status::failed_precondition("job is not attached to a call"))?;

        let proto = req
            .transcript
            .ok_or_else(|| Status::invalid_argument("transcript is required"))?;
        let transcript = convert::transcript_from_proto(proto)
            .map_err(|e| Status::invalid_argument(format!("invalid transcript: {e}")))?;
        let segments_stored = transcript.segments.len();

        let transcript_json = serde_json::to_string(&transcript)
            .map_err(|e| Status::internal(format!("failed to serialize transcript: {e}")))?;
        self.state
            .call_repo
            .save_transcript(call_id, &transcript_json)
            .await
            .map_err(|e| Status::internal(format!("failed to store transcript: {e}")))?;

        // Returned to the queue without consuming an attempt. The service picks
        // it up, finds the stored transcript and runs only the analysis stage.
        self.state
            .job_repo
            .requeue_interrupted(job_id, "Transcribed by remote worker; awaiting analysis")
            .await
            .map_err(|e| Status::internal(format!("failed to requeue job: {e}")))?;

        info!(
            "Worker {identity} submitted a transcript for call {call_id} ({segments_stored} segments)"
        );
        Ok(Response::new(v1::SubmitTranscriptResponse {
            segments_stored: u32::try_from(segments_stored).unwrap_or(u32::MAX),
        }))
    }

    async fn submit_plugin_result(
        &self,
        request: Request<v1::SubmitPluginResultRequest>,
    ) -> Result<Response<v1::SubmitPluginResultResponse>, Status> {
        let identity = self.caller_identity(&request, &request.get_ref().worker_id)?;
        let req = request.into_inner();
        let job_id = Self::parse_job_id(&req.job_id)?;
        self.require_lease(job_id, &identity).await?;

        let plugin = req.plugin.trim();
        if plugin.is_empty() {
            return Err(Status::invalid_argument("plugin must not be empty"));
        }
        // Keeps the identifier usable as a storage key and a template name.
        if !plugin
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(Status::invalid_argument(
                "plugin may only contain ASCII letters, digits, '-' and '_'",
            ));
        }

        let job = self
            .state
            .job_repo
            .get_by_id(job_id)
            .await
            .map_err(|e| Status::internal(format!("job lookup failed: {e}")))?
            .ok_or_else(|| Status::not_found(format!("job {job_id} not found")))?;
        let call_id = job
            .call_id
            .ok_or_else(|| Status::failed_precondition("job is not attached to a call"))?;

        // Typed payloads are re-serialized from the parsed message rather than
        // trusted as-is; the JSON escape hatch is validated as JSON.
        let payload_json = match req.payload {
            Some(v1::submit_plugin_result_request::Payload::SpeakerEmotions(emotions)) => {
                serde_json::to_string(&convert::speaker_emotions_to_json(&emotions))
                    .map_err(|e| Status::internal(format!("failed to encode payload: {e}")))?
            }
            Some(v1::submit_plugin_result_request::Payload::Json(raw)) => {
                serde_json::from_str::<serde_json::Value>(&raw)
                    .map_err(|e| Status::invalid_argument(format!("payload is not JSON: {e}")))?;
                raw
            }
            None => return Err(Status::invalid_argument("payload is required")),
        };

        self.state
            .call_repo
            .save_plugin_result(call_id, plugin, &payload_json)
            .await
            .map_err(|e| Status::internal(format!("failed to store plugin result: {e}")))?;

        info!("Worker {identity} stored {plugin} result for call {call_id}");
        Ok(Response::new(v1::SubmitPluginResultResponse {}))
    }

    async fn fail_job(
        &self,
        request: Request<v1::FailJobRequest>,
    ) -> Result<Response<v1::FailJobResponse>, Status> {
        let identity = self.caller_identity(&request, &request.get_ref().worker_id)?;
        let req = request.into_inner();
        let job_id = Self::parse_job_id(&req.job_id)?;

        warn!(
            "Worker {identity} reported job {job_id} failed (retryable={}): {}",
            req.retryable, req.error
        );

        let job = self
            .state
            .job_repo
            .get_by_id(job_id)
            .await
            .map_err(|e| Status::internal(format!("job lookup failed: {e}")))?;

        let retry_delay = if req.retryable {
            Some(RETRY_BACKOFF)
        } else {
            None
        };
        self.state
            .job_repo
            .mark_failed(job_id, &req.error, retry_delay)
            .await
            .map_err(|e| Status::internal(format!("failed to record failure: {e}")))?;

        if !req.retryable {
            if let Some(call_id) = job.and_then(|j| j.call_id) {
                self.state
                    .call_repo
                    .update_status(call_id, ProcessingStatus::Failed)
                    .await
                    .map_err(|e| Status::internal(format!("failed to update call: {e}")))?;
            }
        }

        Ok(Response::new(v1::FailJobResponse {}))
    }
}
