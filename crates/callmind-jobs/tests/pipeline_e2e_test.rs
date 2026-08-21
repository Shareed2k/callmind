use callmind_analysis::AnalysisEngine;
use callmind_config::JobsConfig;
use callmind_core::{
    Call, CallDirection, CallFilenameParser, JobKind, OrgId, ProcessingStatus, Recording,
};
use callmind_db::{
    CallRepository, JobRepository, SqliteCallRepository, SqliteJobRepository, create_sqlite_pool,
    run_migrations,
};
use callmind_diarization::{NeuralDiarizer, StereoChannelDiarizer};
use callmind_jobs::{CallPipelineHandler, JobRegistry, WorkerPool};
use callmind_language::SamplingLanguageEngine;
use callmind_llm::MockLlmEngine;
use callmind_search::{AskCallsRequest, AskEngine, SearchEngine, SearchFilter};
use callmind_storage::{FilesystemStorage, RecordingStorage};
use callmind_stt::{MockSttEngine, SttRouter};
use callmind_vad::EnergyVadEngine;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn test_full_pipeline_e2e_real_audio() {
    let real_audio_path = Path::new("/Volumes/calls/Call 033765660_260621_150956.m4a");
    if !real_audio_path.exists() {
        println!("Skipping real audio e2e test: /Volumes/calls not mounted");
        return;
    }

    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("e2e_pipeline.db");
    let pool = create_sqlite_pool(db_path.to_str().unwrap(), 5)
        .await
        .unwrap();
    run_migrations(&pool).await.unwrap();

    let storage_dir = temp_dir.path().join("recordings");
    let storage = Arc::new(FilesystemStorage::new(&storage_dir).await.unwrap());

    let call_repo = Arc::new(SqliteCallRepository::new(pool.clone()));
    let job_repo = Arc::new(SqliteJobRepository::new(pool.clone()));

    let org_id = OrgId::generate();
    sqlx::query("INSERT INTO organizations (id, name, created_at) VALUES (?, ?, ?)")
        .bind(org_id.to_string())
        .bind("E2E Test Org")
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&pool)
        .await
        .unwrap();

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
    let storage_key = format!("{}/{}.m4a", org_id, call.id);
    let tokio_file = tokio::fs::File::open(real_audio_path).await.unwrap();
    let stream = Box::pin(tokio_util::io::ReaderStream::new(tokio_file));
    let put_res = storage.put(&storage_key, stream).await.unwrap();

    let recording = Recording::new(
        call.id,
        storage_key,
        "audio/m4a".into(),
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

    let search_engine = Arc::new(SearchEngine::new(pool.clone()));

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
                "value": "033765660",
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

    let pipeline_handler = CallPipelineHandler {
        call_repo: call_repo.clone(),
        storage: storage.clone(),
        transcriber,
        analyzer: analysis_engine,
        search_engine: search_engine.clone(),
        pool: pool.clone(),
    };

    let registry = JobRegistry::builder()
        .register(JobKind::IngestRecording, pipeline_handler)
        .build();

    let cancellation_token = CancellationToken::new();
    let jobs_config = JobsConfig {
        workers: 1,
        poll_interval_ms: 20,
        lock_timeout_secs: 60,
        max_attempts: 3,
    };

    let mut worker_pool = WorkerPool::new(
        job_repo.clone(),
        registry,
        jobs_config,
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
}
