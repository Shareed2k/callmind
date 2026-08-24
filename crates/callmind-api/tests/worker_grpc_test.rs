//! The worker gRPC contract, exercised over a real socket.
//!
//! This is the plugin boundary, so it is tested through the wire rather than by
//! calling the service methods directly: a third party's worker sees exactly
//! this, including the status codes it has to react to.

use callmind_analysis::AnalysisEngine;
use callmind_api::grpc::WorkerService;
use callmind_api::state::AppState;
use callmind_config::AppConfig;
use callmind_core::{
    Call, CallDirection, CallId, EnqueueJob, JobKind, JobStatus, OrgId, ProcessingStatus, Recording,
};
use callmind_db::{
    CallRepository, JobRepository, SqlCallRepository, SqlJobRepository, SqlSearchIndex,
    SqlStatsRepository, create_sqlite_pool, orm_connection, run_migrations,
};
use callmind_llm::MockLlmEngine;
use callmind_search::{AskEngine, SearchEngine};
use callmind_storage::{FilesystemStorage, RecordingStorage};
use callmind_worker_proto::v1::worker_client::WorkerClient;
use callmind_worker_proto::v1::worker_server::WorkerServer;
use callmind_worker_proto::{convert, v1};
use std::sync::Arc;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::Code;
use tonic::transport::Channel;

/// The audio a worker will stream back.
const AUDIO: &[u8] = b"not really audio, but the bytes have to arrive intact";

struct Fixture {
    client: WorkerClient<Channel>,
    call_repo: Arc<SqlCallRepository>,
    job_repo: Arc<SqlJobRepository>,
    call_id: CallId,
    _dir: tempfile::TempDir,
}

async fn start() -> Fixture {
    let pool = create_sqlite_pool(":memory:", 5).await.unwrap();
    run_migrations(&pool).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(FilesystemStorage::new(dir.path()).await.unwrap());

    let call_repo = Arc::new(SqlCallRepository::new(orm_connection(&pool)));
    let job_repo = Arc::new(SqlJobRepository::new(orm_connection(&pool)));
    let stats_repo = Arc::new(SqlStatsRepository::new(orm_connection(&pool)));

    let call = Call::new(
        OrgId::DEFAULT,
        Some("grpc-test".into()),
        CallDirection::Incoming,
        None,
        None,
        None,
    );
    call_repo.create(&call).await.unwrap();

    let storage_key = format!("{}/{}.m4a", OrgId::DEFAULT, call.id);
    let stream = Box::pin(futures_util::stream::once(async move {
        Ok::<bytes::Bytes, std::io::Error>(bytes::Bytes::from_static(AUDIO))
    }));
    let put = storage.put(&storage_key, stream).await.unwrap();
    call_repo
        .add_recording(&Recording::new(
            call.id,
            storage_key,
            "audio/mp4".into(),
            put.size_bytes,
            put.sha256,
        ))
        .await
        .unwrap();

    job_repo
        .enqueue(
            &EnqueueJob::new(
                JobKind::IngestRecording,
                serde_json::json!({ "call_id": call.id.to_string(), "language_hint": "he" }),
            )
            .with_call_id(call.id),
        )
        .await
        .unwrap();

    let search = Arc::new(SearchEngine::new(Arc::new(SqlSearchIndex::new(
        orm_connection(&pool),
    ))));
    let llm = Arc::new(MockLlmEngine::default());
    let state = AppState::new(
        Arc::new(AppConfig::default()),
        call_repo.clone(),
        call_repo.clone(),
        job_repo.clone(),
        stats_repo.clone(),
        storage,
        search.clone(),
        Arc::new(AskEngine::new((*search).clone(), llm.clone())),
        Arc::new(AnalysisEngine::new(llm)),
        Arc::new(callmind_ui::templates::TemplateRegistry::new()),
    );

    // Ephemeral port so tests can run in parallel.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(WorkerServer::new(WorkerService::new(state)))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });

    let client = loop {
        match WorkerClient::connect(format!("http://{addr}")).await {
            Ok(c) => break c,
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
        }
    };

    Fixture {
        client,
        call_repo,
        job_repo,
        call_id: call.id,
        _dir: dir,
    }
}

fn sample_transcript(call_id: CallId) -> v1::Transcript {
    convert::transcript_to_proto(&callmind_transcript::Transcript {
        call_id,
        languages: vec![],
        speakers: vec![],
        segments: vec![callmind_transcript::TranscriptSegment {
            id: uuid::Uuid::new_v4(),
            call_id,
            sequence: 0,
            speaker_id: callmind_core::SpeakerId(1),
            speaker_role: callmind_core::SpeakerRole::Customer,
            language: callmind_core::Language::Hebrew,
            text_direction: callmind_transcript::TextDirection::Rtl,
            start_ms: 0,
            end_ms: 900,
            raw_text: "שלום".into(),
            normalized_text: "שלום".into(),
            words: vec![],
        }],
    })
}

#[tokio::test]
async fn a_worker_can_lease_stream_and_submit() {
    let mut f = start().await;

    // 1. Lease.
    let lease = f
        .client
        .lease(v1::LeaseRequest {
            worker_id: "gpu-box-1".into(),
            kinds: vec!["ingest_recording".into()],
        })
        .await
        .unwrap()
        .into_inner();
    let job = lease.job.expect("a job should be available");
    assert_eq!(job.call_id, f.call_id.to_string());
    assert_eq!(job.mime_type, "audio/mp4");
    assert_eq!(job.file_size_bytes, AUDIO.len() as u64);
    // A pinned language is surfaced as its own field, not left in the payload.
    assert_eq!(job.language_hint, "he");
    assert!(lease.lease_timeout_secs > 0);

    // 2. A second worker must not get the same job.
    let second = f
        .client
        .lease(v1::LeaseRequest {
            worker_id: "gpu-box-2".into(),
            kinds: vec![],
        })
        .await
        .unwrap()
        .into_inner();
    assert!(second.job.is_none(), "job leased twice");

    // 3. Streamed audio must arrive byte-for-byte.
    let mut stream = f
        .client
        .stream_recording(v1::StreamRecordingRequest {
            worker_id: "gpu-box-1".into(),
            job_id: job.job_id.clone(),
        })
        .await
        .unwrap()
        .into_inner();

    let mut received = Vec::new();
    while let Some(chunk) = stream.message().await.unwrap() {
        received.extend_from_slice(&chunk.data);
    }
    assert_eq!(received, AUDIO);

    // 4. Heartbeat reports lease ownership rather than erroring.
    let held = f
        .client
        .heartbeat(v1::HeartbeatRequest {
            worker_id: "gpu-box-1".into(),
            job_id: job.job_id.clone(),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(held.still_leased);

    let not_held = f
        .client
        .heartbeat(v1::HeartbeatRequest {
            worker_id: "gpu-box-2".into(),
            job_id: job.job_id.clone(),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(
        !not_held.still_leased,
        "a non-holder must be told it lost the lease"
    );

    // 5. Only the lease holder may submit.
    let err = f
        .client
        .submit_transcript(v1::SubmitTranscriptRequest {
            worker_id: "gpu-box-2".into(),
            job_id: job.job_id.clone(),
            transcript: Some(sample_transcript(f.call_id)),
        })
        .await
        .expect_err("a non-holder must be refused");
    assert_eq!(err.code(), Code::Aborted);

    // 6. A malformed transcript is refused up front.
    let err = f
        .client
        .submit_transcript(v1::SubmitTranscriptRequest {
            worker_id: "gpu-box-1".into(),
            job_id: job.job_id.clone(),
            transcript: Some(v1::Transcript {
                call_id: f.call_id.to_string(),
                languages: vec![],
                speakers: vec![],
                segments: vec![],
            }),
        })
        .await
        .expect_err("an empty transcript must be refused");
    assert_eq!(err.code(), Code::InvalidArgument);

    // 7. The real submission stores the transcript and requeues for analysis.
    let stored = f
        .client
        .submit_transcript(v1::SubmitTranscriptRequest {
            worker_id: "gpu-box-1".into(),
            job_id: job.job_id.clone(),
            transcript: Some(sample_transcript(f.call_id)),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(stored.segments_stored, 1);

    let saved = f
        .call_repo
        .get_transcript_json(f.call_id)
        .await
        .unwrap()
        .expect("transcript stored");
    assert!(saved.contains("שלום"));

    let requeued = f
        .job_repo
        .get_by_id(job.job_id.parse().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        requeued.status,
        JobStatus::Pending,
        "job should return to the queue for the analysis stage"
    );
}

#[tokio::test]
async fn a_worker_can_submit_a_plugin_result() {
    let mut f = start().await;
    let job = f
        .client
        .lease(v1::LeaseRequest {
            worker_id: "gpu-box-1".into(),
            kinds: vec![],
        })
        .await
        .unwrap()
        .into_inner()
        .job
        .unwrap();

    f.client
        .submit_plugin_result(v1::SubmitPluginResultRequest {
            worker_id: "gpu-box-1".into(),
            job_id: job.job_id.clone(),
            plugin: "acoustic-emotions".into(),
            payload: Some(v1::submit_plugin_result_request::Payload::SpeakerEmotions(
                v1::SpeakerEmotions {
                    call_id: f.call_id.to_string(),
                    model: "wav2vec2-emotion".into(),
                    summaries: vec![v1::SpeakerEmotionSummary {
                        speaker_id: 1,
                        dominant: "joy".into(),
                        scores: vec![v1::EmotionScore {
                            emotion: "joy".into(),
                            score: 0.8,
                        }],
                    }],
                    spans: vec![],
                },
            )),
        })
        .await
        .unwrap();

    let results = f.call_repo.list_plugin_results(f.call_id).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0, "acoustic-emotions");
    assert!(results[0].1.contains("wav2vec2-emotion"));
    assert!(results[0].1.contains("\"dominant\":\"joy\""));

    // A plugin name that could escape a storage key or a template path.
    let err = f
        .client
        .submit_plugin_result(v1::SubmitPluginResultRequest {
            worker_id: "gpu-box-1".into(),
            job_id: job.job_id.clone(),
            plugin: "../escape".into(),
            payload: Some(v1::submit_plugin_result_request::Payload::Json("{}".into())),
        })
        .await
        .expect_err("plugin names must be restricted");
    assert_eq!(err.code(), Code::InvalidArgument);

    // The JSON escape hatch must actually be JSON.
    let err = f
        .client
        .submit_plugin_result(v1::SubmitPluginResultRequest {
            worker_id: "gpu-box-1".into(),
            job_id: job.job_id,
            plugin: "custom".into(),
            payload: Some(v1::submit_plugin_result_request::Payload::Json(
                "not json".into(),
            )),
        })
        .await
        .expect_err("non-JSON payloads must be refused");
    assert_eq!(err.code(), Code::InvalidArgument);
}

#[tokio::test]
async fn a_worker_can_give_a_job_back() {
    let mut f = start().await;
    let job = f
        .client
        .lease(v1::LeaseRequest {
            worker_id: "gpu-box-1".into(),
            kinds: vec![],
        })
        .await
        .unwrap()
        .into_inner()
        .job
        .unwrap();

    f.client
        .fail_job(v1::FailJobRequest {
            worker_id: "gpu-box-1".into(),
            job_id: job.job_id,
            error: "CUDA out of memory".into(),
            retryable: false,
        })
        .await
        .unwrap();

    // A non-retryable failure has to surface on the call, not just the job.
    let call = f.call_repo.get_by_id(f.call_id).await.unwrap().unwrap();
    assert_eq!(call.processing_status, ProcessingStatus::Failed);
}
