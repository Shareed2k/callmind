use callmind_core::{
    Call, CallDirection, CallFilter, EnqueueJob, JobKind, JobStatus, OrgId, ProcessingStatus,
    Recording,
};
use callmind_db::{
    CallRepository, JobRepository, SqlCallRepository, SqlJobRepository, create_sqlite_pool,
    orm_connection, run_migrations,
};

#[tokio::test]
async fn test_sqlite_call_and_job_repositories() {
    let pool = create_sqlite_pool(":memory:", 5, std::time::Duration::from_secs(5))
        .await
        .unwrap();
    run_migrations(&pool).await.unwrap();

    let call_repo = SqlCallRepository::new(orm_connection(&pool));
    let job_repo = SqlJobRepository::new(orm_connection(&pool));

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

/// The pipeline feeds the organization name into the LLM prompt. It used to
/// pass a literal "Organization", which is not even the seeded default name.
#[tokio::test]
async fn test_get_organization_name() {
    let pool = create_sqlite_pool(":memory:", 5, std::time::Duration::from_secs(5))
        .await
        .unwrap();
    run_migrations(&pool).await.unwrap();
    let repo = SqlCallRepository::new(orm_connection(&pool));

    // The default organization the initial migration seeds.
    let default_org = OrgId(
        uuid::Uuid::parse_str("00000000-0000-0000-0000-000000000001")
            .expect("valid default org id"),
    );
    assert_eq!(
        repo.get_organization_name(default_org).await.unwrap(),
        Some("Default Organization".to_string())
    );

    let org_id = OrgId::generate();
    sqlx::query("INSERT INTO organizations (id, name, created_at) VALUES (?, ?, ?)")
        .bind(org_id.to_string())
        .bind("Acme Logistics")
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        repo.get_organization_name(org_id).await.unwrap(),
        Some("Acme Logistics".to_string())
    );

    // Unknown org is None, not an error, so the caller can fall back.
    assert_eq!(
        repo.get_organization_name(OrgId::generate()).await.unwrap(),
        None
    );
}

/// Startup reconciliation and shutdown requeue used to be raw SQL inlined in
/// the worker, duplicated five times and bypassing these repositories.
#[tokio::test]
async fn test_orphan_reconciliation_and_requeue() {
    let pool = create_sqlite_pool(":memory:", 5, std::time::Duration::from_secs(5))
        .await
        .unwrap();
    run_migrations(&pool).await.unwrap();
    let call_repo = SqlCallRepository::new(orm_connection(&pool));
    let job_repo = SqlJobRepository::new(orm_connection(&pool));

    let org_id = OrgId::DEFAULT;
    let make_call = |repo: &SqlCallRepository| {
        let call = Call::new(org_id, None, CallDirection::Incoming, None, None, None);
        let repo = repo.clone();
        async move {
            repo.create(&call).await.unwrap();
            repo.update_status(call.id, ProcessingStatus::Processing)
                .await
                .unwrap();
            call.id
        }
    };

    // One orphan (no job at all) and one with a live job.
    let orphan = make_call(&call_repo).await;
    let attended = make_call(&call_repo).await;

    let job_id = job_repo
        .enqueue(&EnqueueJob {
            kind: JobKind::IngestRecording,
            call_id: Some(attended),
            payload: serde_json::json!({}),
            priority: 0,
            max_attempts: 3,
            run_after: None,
        })
        .await
        .unwrap();

    assert_eq!(call_repo.fail_orphaned_processing().await.unwrap(), 1);
    assert_eq!(
        call_repo
            .get_by_id(orphan)
            .await
            .unwrap()
            .unwrap()
            .processing_status,
        ProcessingStatus::Failed
    );
    assert_eq!(
        call_repo
            .get_by_id(attended)
            .await
            .unwrap()
            .unwrap()
            .processing_status,
        ProcessingStatus::Processing,
        "a call with a live job must not be reconciled"
    );

    // Requeue must not consume a retry attempt.
    let locked = job_repo
        .fetch_and_lock("worker-1", &[JobKind::IngestRecording])
        .await
        .unwrap()
        .expect("job should be lockable");
    assert_eq!(locked.status, JobStatus::Running);
    let attempt_while_running = locked.attempt;

    job_repo
        .requeue_interrupted(job_id, "Interrupted by server shutdown")
        .await
        .unwrap();

    let requeued = job_repo.get_by_id(job_id).await.unwrap().unwrap();
    assert_eq!(requeued.status, JobStatus::Pending);
    assert!(
        requeued.attempt < attempt_while_running,
        "shutdown must not burn an attempt: {} -> {}",
        attempt_while_running,
        requeued.attempt
    );
}

/// Transcription is the expensive stage, so it is committed on its own and
/// reused on a retry instead of being redone.
#[tokio::test]
async fn test_transcript_is_stored_and_replaceable() {
    let pool = create_sqlite_pool(":memory:", 5, std::time::Duration::from_secs(5))
        .await
        .unwrap();
    run_migrations(&pool).await.unwrap();
    let repo = SqlCallRepository::new(orm_connection(&pool));

    let call = Call::new(
        OrgId::DEFAULT,
        None,
        CallDirection::Incoming,
        None,
        None,
        None,
    );
    repo.create(&call).await.unwrap();

    // Nothing stored yet: the pipeline must transcribe.
    assert_eq!(repo.get_transcript_json(call.id).await.unwrap(), None);

    let first = r#"{"call_id":"x","languages":[],"speakers":[],"segments":[]}"#;
    repo.save_transcript(call.id, first).await.unwrap();
    assert_eq!(
        repo.get_transcript_json(call.id).await.unwrap().as_deref(),
        Some(first)
    );

    // Saving again replaces rather than accumulating, so a forced
    // re-transcription does not leave two rows behind.
    let second = r#"{"call_id":"y","languages":[],"speakers":[],"segments":[]}"#;
    repo.save_transcript(call.id, second).await.unwrap();
    assert_eq!(
        repo.get_transcript_json(call.id).await.unwrap().as_deref(),
        Some(second)
    );
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM call_transcripts WHERE call_id = ?")
        .bind(call.id.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 1, "save_transcript must replace, not append");
}
