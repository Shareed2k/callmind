use callmind_analysis::AnalysisEngine;
use callmind_config::JobsConfig;
use callmind_core::{
    Call, CallDirection, CallFilenameParser, JobKind, OrgId, ProcessingStatus, Recording,
};
use callmind_db::{
    CallRepository, JobRepository, SqlCallRepository, SqlJobRepository, SqlSearchIndex,
    create_sqlite_pool, orm_connection, run_migrations,
};
use callmind_diarization::{NeuralDiarizer, StereoChannelDiarizer};
use callmind_jobs::{CallPipelineHandler, JobRegistry, WorkerPool};
use callmind_language::SamplingLanguageEngine;
use callmind_llm::MockLlmEngine;
use callmind_search::{AskCallsRequest, AskEngine, SearchEngine, SearchFilter};
use callmind_storage::{FilesystemStorage, RecordingStorage};
use callmind_stt::{MockSttEngine, SttRouter};
use callmind_vad::EnergyVadEngine;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Write 16-bit PCM mono WAV with alternating speech-like bursts and silence, so
/// the VAD finds real regions. Hand-rolled because a 44-byte header is cheaper
/// than a WAV dependency.
fn write_wav_fixture(path: &Path, sample_rate: u32, secs: u32) {
    let total = (sample_rate * secs) as usize;
    let mut pcm = Vec::with_capacity(total * 2);
    for i in 0..total {
        let t = i as f32 / sample_rate as f32;
        // 600ms of tone, 400ms of silence.
        let voiced = (t * 1000.0) as u32 % 1000 < 600;
        let value = if voiced {
            let f0 = 140.0 + 20.0 * (t * 0.7).sin();
            let s = (2.0 * std::f32::consts::PI * f0 * t).sin() * 0.5
                + (2.0 * std::f32::consts::PI * f0 * 2.0 * t).sin() * 0.2;
            (s * 12000.0) as i16
        } else {
            0
        };
        pcm.extend_from_slice(&value.to_le_bytes());
    }

    let data_len = pcm.len() as u32;
    let mut f = std::fs::File::create(path).expect("create wav fixture");
    f.write_all(b"RIFF").unwrap();
    f.write_all(&(36 + data_len).to_le_bytes()).unwrap();
    f.write_all(b"WAVEfmt ").unwrap();
    f.write_all(&16u32.to_le_bytes()).unwrap(); // fmt chunk size
    f.write_all(&1u16.to_le_bytes()).unwrap(); // PCM
    f.write_all(&1u16.to_le_bytes()).unwrap(); // mono
    f.write_all(&sample_rate.to_le_bytes()).unwrap();
    f.write_all(&(sample_rate * 2).to_le_bytes()).unwrap(); // byte rate
    f.write_all(&2u16.to_le_bytes()).unwrap(); // block align
    f.write_all(&16u16.to_le_bytes()).unwrap(); // bits per sample
    f.write_all(b"data").unwrap();
    f.write_all(&data_len.to_le_bytes()).unwrap();
    f.write_all(&pcm).unwrap();
}

/// Audio for the end-to-end run.
///
/// This used to hard-code `/Volumes/calls/...` and `return` early when absent,
/// so CI passed the test having executed no pipeline code at all. Now it always
/// runs against a generated fixture, and `CALLMIND_TEST_AUDIO` can point it at
/// a real recording instead.
fn resolve_test_audio(dir: &Path) -> PathBuf {
    if let Some(raw) = std::env::var_os("CALLMIND_TEST_AUDIO") {
        let path = PathBuf::from(raw);
        assert!(
            path.exists(),
            "CALLMIND_TEST_AUDIO points at a missing file: {path:?}"
        );
        return path;
    }
    // Keeps the `Call <id>_<date>_<time>` shape so CallFilenameParser is exercised.
    let path = dir.join("Call 0300000000_260621_150956.wav");
    write_wav_fixture(&path, 16_000, 6);
    path
}

#[tokio::test]
async fn test_full_pipeline_e2e_real_audio() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let real_audio_path = resolve_test_audio(fixture_dir.path());
    let real_audio_path = real_audio_path.as_path();

    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("e2e_pipeline.db");
    let pool = create_sqlite_pool(
        db_path.to_str().unwrap(),
        5,
        std::time::Duration::from_secs(5),
    )
    .await
    .unwrap();
    run_migrations(&pool).await.unwrap();

    let storage_dir = temp_dir.path().join("recordings");
    let storage = Arc::new(FilesystemStorage::new(&storage_dir).await.unwrap());

    let call_repo = Arc::new(SqlCallRepository::new(orm_connection(&pool)));
    let job_repo = Arc::new(SqlJobRepository::new(orm_connection(&pool)));

    // The migration seeds this organization, so the test writes no SQL of its
    // own -- the point of routing everything through the repositories.
    let org_id = OrgId::DEFAULT;

    // 1. Create Call and Copy real file to storage
    let filename = real_audio_path.file_name().unwrap().to_str().unwrap();
    let parsed_meta = CallFilenameParser::parse(filename);

    let call = Call::new(
        org_id,
        Some(filename.to_string()),
        CallDirection::Incoming,
        parsed_meta.contact_name.or(parsed_meta.phone_number),
        None,
        parsed_meta.started_at,
    );
    call_repo.create(&call).await.unwrap();

    // Copy audio to storage
    let extension = real_audio_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("m4a");
    let storage_key = format!("{}/{}.{}", org_id, call.id, extension);
    let tokio_file = tokio::fs::File::open(real_audio_path).await.unwrap();
    let stream = Box::pin(tokio_util::io::ReaderStream::new(tokio_file));
    let put_res = storage.put(&storage_key, stream).await.unwrap();

    let recording = Recording::new(
        call.id,
        storage_key,
        format!("audio/{extension}"),
        put_res.size_bytes,
        put_res.sha256,
    );
    call_repo.add_recording(&recording).await.unwrap();

    // 2. Setup AI Pipeline & Engines
    let vad = Arc::new(EnergyVadEngine::default());
    let language_engine = Arc::new(SamplingLanguageEngine::default());

    let hebrew_stt = Arc::new(MockSttEngine::new("ivrit-ai-v3", "1.0"));
    let multi_stt = Arc::new(MockSttEngine::new("whisper-large-v3", "1.0"));
    let stt_router = Arc::new(SttRouter::new(hebrew_stt, multi_stt, 0.90));

    let stereo_diarizer = Arc::new(StereoChannelDiarizer::new(vad.clone()));
    let clustering_diarizer = Arc::new(NeuralDiarizer::new_with_fallback(None, vad.clone()));

    let search_engine = Arc::new(SearchEngine::new(Arc::new(SqlSearchIndex::new(
        orm_connection(&pool),
    ))));

    let mock_llm_json = serde_json::json!({
        "title": "Customer Service Inquiry",
        "summary": "Customer called regarding account information and billing verification.",
        "reason": "Account balance inquiry",
        "resolution": "Account details verified and confirmed with customer",
        "resolved": true,
        "customer_intent": "inquiry",
        "topics": ["account", "billing", "inquiry"],
        "action_items": [
            {
                "text": "Send account statement via SMS",
                "owner": "agent",
                "deadline": "within 1 hour",
                "evidence_segments": [0]
            }
        ],
        "entities": [
            {
                "entity_type": "phone",
                "value": "0300000000",
                "evidence_segments": [0]
            }
        ],
        "sentiment_score": 0.5,
        "scorecard": {
            "total_score": 95,
            "rules": [
                {
                    "name": "Greeting",
                    "score": 20,
                    "max_score": 20,
                    "explanation": "Agent properly introduced company",
                    "evidence_segments": [0]
                }
            ]
        },
        "compliance": [
            {
                "name": "Identity Verification",
                "passed": true,
                "explanation": "Customer identity verified",
                "evidence_segments": [0]
            }
        ]
    });

    let llm_engine = Arc::new(MockLlmEngine::new().with_json(mock_llm_json));
    let ask_engine = Arc::new(AskEngine::new((*search_engine).clone(), llm_engine.clone()));
    let analysis_engine = Arc::new(AnalysisEngine::new(llm_engine));

    let gpu_semaphore = Arc::new(tokio::sync::Semaphore::new(1));
    let transcriber = Arc::new(callmind_transcript::AudioTranscriber::new(
        vad,
        language_engine,
        stt_router,
        stereo_diarizer,
        clustering_diarizer,
        gpu_semaphore,
    ));

    // Built through a closure so the reuse checks further down can spin up more
    // worker pools against the same wiring.
    let make_registry = {
        let call_repo = call_repo.clone();
        let webhook_queue: Arc<dyn JobRepository> = job_repo.clone();
        let storage = storage.clone();
        let transcriber = transcriber.clone();
        let analysis_engine = analysis_engine.clone();
        let search_engine = search_engine.clone();
        move || {
            JobRegistry::builder()
                .register(
                    JobKind::IngestRecording,
                    CallPipelineHandler {
                        call_repo: call_repo.clone(),
                        speaker_repo: call_repo.clone(),
                        webhook_queue: Some(webhook_queue.clone()),
                        plugins: Vec::new(),
                        storage: storage.clone(),
                        transcriber: transcriber.clone(),
                        analyzer: analysis_engine.clone(),
                        search: search_engine.clone(),
                    },
                )
                .build()
        }
    };

    let jobs_config = JobsConfig {
        workers: 1,
        poll_interval_ms: 20,
        lock_timeout_secs: 60,
        max_attempts: 3,
    };

    let cancellation_token = CancellationToken::new();
    let mut worker_pool = WorkerPool::new(
        job_repo.clone(),
        call_repo.clone(),
        make_registry(),
        jobs_config.clone(),
        cancellation_token.clone(),
    );

    // 3. Enqueue Job & Start Workers
    let enqueue_req = callmind_core::EnqueueJob::new(
        JobKind::IngestRecording,
        serde_json::json!({ "call_id": call.id.to_string() }),
    )
    .with_call_id(call.id);
    job_repo.enqueue(&enqueue_req).await.unwrap();

    worker_pool.start();

    // Wait for worker to finish processing
    tokio::time::sleep(Duration::from_millis(500)).await;

    cancellation_token.cancel();
    worker_pool.wait().await;

    // 4. Verify Call status is Completed
    let updated_call = call_repo.get_by_id(call.id).await.unwrap().unwrap();
    assert_eq!(updated_call.processing_status, ProcessingStatus::Completed);

    // 5. Verify FTS5 Search finds the processed call
    let search_results = search_engine
        .search(&SearchFilter {
            query: "billing verification".into(),
            organization_id: Some(org_id),
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(search_results.len(), 1);
    assert_eq!(search_results[0].call_id, call.id);
    assert_eq!(search_results[0].title, "Customer Service Inquiry");

    // 6. Verify AskEngine answers analytical queries with citations
    let ask_res = ask_engine
        .ask(AskCallsRequest {
            question: "What account information was discussed?".into(),
            organization_id: Some(org_id),
            max_sources: Some(5),
        })
        .await
        .unwrap();

    assert!(!ask_res.citations.is_empty());
    assert_eq!(ask_res.citations[0].call_id, call.id);

    // 7. A completed call is offered to the outbound webhook as its own job.
    //
    // Its own job, not a call at the end of the pipeline, so a receiver that is
    // down costs a retry rather than a re-run of transcription and analysis. The
    // payload is the request body, so every retry delivers identical bytes.
    let delivery = job_repo
        .fetch_and_lock("webhook-assert", &[JobKind::DeliverWebhook])
        .await
        .unwrap()
        .expect("a completed call must enqueue its delivery");
    assert_eq!(delivery.call_id, Some(call.id));
    assert_eq!(delivery.payload["event"], "call.completed");
    assert_eq!(delivery.payload["call_id"], call.id.to_string());
    assert_eq!(delivery.payload["title"], "Customer Service Inquiry");
    assert!(
        delivery.payload["summary"].is_string(),
        "a receiver wants the summary without calling back: {}",
        delivery.payload
    );

    // 7. A retry must reuse the stored transcript rather than transcribing again.
    //
    // Transcription is the expensive stage; a crash or retryable failure after it
    // used to throw the work away. Detected here by planting a marker in the
    // stored transcript: if the pipeline re-transcribes, the marker disappears.
    let marker = r#"{"call_id":"00000000-0000-0000-0000-0000000000ff","languages":[],"speakers":[],"segments":[]}"#;
    call_repo.save_transcript(call.id, marker).await.unwrap();

    let run_again = |payload: serde_json::Value| {
        let job_repo = job_repo.clone();
        let call_repo = call_repo.clone();
        let registry = make_registry();
        let jobs_config = jobs_config.clone();
        async move {
            let req = callmind_core::EnqueueJob::new(JobKind::IngestRecording, payload)
                .with_call_id(call.id);
            job_repo.enqueue(&req).await.unwrap();

            let token = CancellationToken::new();
            let mut pool_2 =
                WorkerPool::new(job_repo, call_repo, registry, jobs_config, token.clone());
            pool_2.start();
            tokio::time::sleep(Duration::from_millis(800)).await;
            token.cancel();
            pool_2.wait().await;
        }
    };

    run_again(serde_json::json!({ "call_id": call.id.to_string() })).await;
    assert_eq!(
        call_repo
            .get_transcript_json(call.id)
            .await
            .unwrap()
            .as_deref(),
        Some(marker),
        "a plain retry must reuse the stored transcript, not re-transcribe"
    );

    // 8. `force_retranscribe` opts out, for when the audio or STT setup changed.
    run_again(serde_json::json!({
        "call_id": call.id.to_string(),
        "force_retranscribe": true,
    }))
    .await;
    assert_ne!(
        call_repo
            .get_transcript_json(call.id)
            .await
            .unwrap()
            .as_deref(),
        Some(marker),
        "force_retranscribe must replace the stored transcript"
    );
}
