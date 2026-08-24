//! The job queue contract, run against every backend by the same test.
//!
//! `SqlJobRepository` is one implementation whose SQL is produced by `sea-query`,
//! so this is what proves the claim: identical behaviour on SQLite and Postgres
//! from a single definition. Postgres participates only when
//! `CALLMIND_TEST_POSTGRES_URL` is set.

use callmind_core::{EnqueueJob, JobKind, JobStatus};
use callmind_db::JobRepository;
mod backend;

use callmind_db::sql::SqlJobRepository;
use std::time::Duration;

fn job(kind: JobKind) -> EnqueueJob {
    EnqueueJob::new(kind, serde_json::json!({ "hello": "world" }))
}

#[tokio::test]
async fn leasing_behaves_the_same_on_every_backend() {
    for (name, conn) in backend::all("t_job_lease").await {
        let repo = SqlJobRepository::new(conn);

        let id = repo.enqueue(&job(JobKind::IngestRecording)).await.unwrap();
        let stored = repo.get_by_id(id).await.unwrap().expect("stored");
        assert_eq!(stored.status, JobStatus::Pending, "{name}");
        assert_eq!(stored.attempt, 0, "{name}");
        assert_eq!(
            stored.payload["hello"], "world",
            "{name}: payload round-trip"
        );

        // Leasing claims the job and counts the attempt.
        let leased = repo
            .fetch_and_lock("worker-1", &[JobKind::IngestRecording])
            .await
            .unwrap()
            .expect("a job to lease");
        assert_eq!(leased.id, id, "{name}");
        assert_eq!(leased.status, JobStatus::Running, "{name}");
        assert_eq!(leased.attempt, 1, "{name}");
        assert_eq!(leased.locked_by.as_deref(), Some("worker-1"), "{name}");

        // A second worker must find nothing.
        assert!(
            repo.fetch_and_lock("worker-2", &[JobKind::IngestRecording])
                .await
                .unwrap()
                .is_none(),
            "{name}: job leased twice"
        );

        // Only the holder can renew.
        assert!(repo.renew_lock(id, "worker-1").await.unwrap(), "{name}");
        assert!(!repo.renew_lock(id, "worker-2").await.unwrap(), "{name}");

        repo.mark_completed(id).await.unwrap();
        let done = repo.get_by_id(id).await.unwrap().unwrap();
        assert_eq!(done.status, JobStatus::Completed, "{name}");
        assert!(done.completed_at.is_some(), "{name}");
        assert!(done.locked_by.is_none(), "{name}: lock not released");
    }
}

#[tokio::test]
async fn kind_filtering_and_retries_match() {
    for (name, conn) in backend::all("t_job_kinds").await {
        let repo = SqlJobRepository::new(conn);
        let ingest = repo.enqueue(&job(JobKind::IngestRecording)).await.unwrap();
        let emotions = repo.enqueue(&job(JobKind::DeliverWebhook)).await.unwrap();

        // A worker only receives kinds it asked for.
        let leased = repo
            .fetch_and_lock("w", &[JobKind::DeliverWebhook])
            .await
            .unwrap()
            .expect("emotion job");
        assert_eq!(leased.id, emotions, "{name}: wrong kind leased");

        // A retryable failure returns it to the queue with a future run_after.
        repo.mark_failed(emotions, "temporary", Some(Duration::from_secs(3600)))
            .await
            .unwrap();
        let retried = repo.get_by_id(emotions).await.unwrap().unwrap();
        assert_eq!(retried.status, JobStatus::Pending, "{name}");
        assert!(
            retried.run_after > chrono::Utc::now(),
            "{name}: not delayed"
        );
        // Not due yet, so it must not be leasable.
        assert!(
            repo.fetch_and_lock("w", &[JobKind::DeliverWebhook])
                .await
                .unwrap()
                .is_none(),
            "{name}: leased a job that is not due"
        );

        // A terminal failure stays failed.
        repo.mark_failed(ingest, "permanent", None).await.unwrap();
        let dead = repo.get_by_id(ingest).await.unwrap().unwrap();
        assert_eq!(dead.status, JobStatus::Failed, "{name}");
        assert_eq!(dead.last_error.as_deref(), Some("permanent"), "{name}");
    }
}

#[tokio::test]
async fn requeue_does_not_burn_an_attempt() {
    for (name, conn) in backend::all("t_job_requeue").await {
        let repo = SqlJobRepository::new(conn);
        let id = repo.enqueue(&job(JobKind::IngestRecording)).await.unwrap();

        let leased = repo.fetch_and_lock("w", &[]).await.unwrap().unwrap();
        assert_eq!(leased.attempt, 1, "{name}");

        repo.requeue_interrupted(id, "shutdown").await.unwrap();
        let back = repo.get_by_id(id).await.unwrap().unwrap();
        assert_eq!(back.status, JobStatus::Pending, "{name}");
        // The decrement is expressed as a portable CASE: SQLite's two-argument
        // MAX() does not exist in Postgres, where MAX is an aggregate.
        assert_eq!(back.attempt, 0, "{name}: shutdown cost an attempt");
        assert!(back.locked_by.is_none(), "{name}");

        // And the bulk form used at shutdown.
        repo.fetch_and_lock("w", &[]).await.unwrap().unwrap();
        assert_eq!(repo.requeue_all_running("bye").await.unwrap(), 1, "{name}");
        assert_eq!(
            repo.get_by_id(id).await.unwrap().unwrap().attempt,
            0,
            "{name}"
        );
    }
}

#[tokio::test]
async fn stale_locks_are_reclaimed() {
    for (name, conn) in backend::all("t_job_stale").await {
        let repo = SqlJobRepository::new(conn);
        let id = repo.enqueue(&job(JobKind::IngestRecording)).await.unwrap();
        repo.fetch_and_lock("dead-worker", &[])
            .await
            .unwrap()
            .unwrap();

        // Nothing is stale yet.
        assert_eq!(
            repo.release_stale_locks(Duration::from_secs(3600))
                .await
                .unwrap(),
            0,
            "{name}: released a fresh lease"
        );

        // A zero threshold makes every held lease stale.
        assert_eq!(
            repo.release_stale_locks(Duration::from_secs(0))
                .await
                .unwrap(),
            1,
            "{name}"
        );
        let freed = repo.get_by_id(id).await.unwrap().unwrap();
        assert_eq!(freed.status, JobStatus::Pending, "{name}");
        assert!(freed.locked_by.is_none(), "{name}");
    }
}
