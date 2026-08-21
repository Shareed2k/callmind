use callmind_core::{
    Call, CallDirection, CallFilter, EnqueueJob, JobKind, JobStatus, OrgId, ProcessingStatus,
    Recording,
};
use callmind_db::{
    CallRepository, JobRepository, SqliteCallRepository, SqliteJobRepository, create_sqlite_pool,
    run_migrations,
};

#[tokio::test]
async fn test_sqlite_call_and_job_repositories() {
    let pool = create_sqlite_pool(":memory:", 5).await.unwrap();
    run_migrations(&pool).await.unwrap();

    let call_repo = SqliteCallRepository::new(pool.clone());
    let job_repo = SqliteJobRepository::new(pool.clone());

    let org_id = OrgId::generate();
    // Insert organization first
    sqlx::query("INSERT INTO organizations (id, name, created_at) VALUES (?, ?, ?)")
        .bind(org_id.to_string())
        .bind("Test Org")
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&pool)
        .await
        .unwrap();

    let call = Call::new(
        org_id,
        Some("ext-123".to_string()),
        CallDirection::Incoming,
        Some("+972501234567".to_string()),
        Some("+97235551234".to_string()),
        Some(chrono::Utc::now()),
    );

    call_repo.create(&call).await.unwrap();

    let fetched = call_repo
        .get_by_id(call.id)
        .await
        .unwrap()
        .expect("Call should exist");
    assert_eq!(fetched.external_id.as_deref(), Some("ext-123"));
    assert_eq!(fetched.processing_status, ProcessingStatus::Pending);

    // Add Recording
    let recording = Recording::new(
        call.id,
        "recordings/org/call1.wav".to_string(),
        "audio/wav".to_string(),
        1024,
        "sha256dummy".to_string(),
    );
    call_repo.add_recording(&recording).await.unwrap();

    let fetched_rec = call_repo
        .get_recording_by_call_id(call.id)
        .await
        .unwrap()
        .expect("Recording should exist");
    assert_eq!(fetched_rec.storage_key, "recordings/org/call1.wav");

    // List calls
    let list = call_repo
        .list(&CallFilter {
            organization_id: Some(org_id),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(list.len(), 1);

    // Job Enqueue & Fetch & Lock
    let enqueue_req = EnqueueJob::new(
        JobKind::IngestRecording,
        serde_json::json!({ "recording_id": recording.id.to_string() }),
    )
    .with_call_id(call.id);

    let job_id = job_repo.enqueue(&enqueue_req).await.unwrap();

    let locked_job = job_repo
        .fetch_and_lock("worker-1", &[JobKind::IngestRecording])
        .await
        .unwrap();
    assert!(locked_job.is_some());
    let j = locked_job.unwrap();
    assert_eq!(j.id, job_id);
    assert_eq!(j.status, JobStatus::Running);
    assert_eq!(j.locked_by.as_deref(), Some("worker-1"));

    // Complete Job
    job_repo.mark_completed(job_id).await.unwrap();
    let completed_job = job_repo.get_by_id(job_id).await.unwrap().unwrap();
    assert_eq!(completed_job.status, JobStatus::Completed);

    // Test duplicate external_id uniqueness rejection
    let duplicate_call = Call::new(
        org_id,
        Some("ext-123".to_string()),
        CallDirection::Outgoing,
        None,
        None,
        None,
    );
    let dup_err = call_repo.create(&duplicate_call).await;
    assert!(dup_err.is_err(), "Duplicate external_id must fail");
    match dup_err {
        Err(callmind_db::DbError::DuplicateKey(_)) => (),
        other => panic!("Expected DuplicateKey error, got {other:?}"),
    }
}
