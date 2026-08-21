use async_trait::async_trait;
use callmind_config::JobsConfig;
use callmind_core::{EnqueueJob, JobKind};
use callmind_db::{JobRepository, SqliteJobRepository, create_sqlite_pool, run_migrations};
use callmind_jobs::{JobContext, JobExecutionError, JobHandler, JobRegistry, WorkerPool};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

struct MockIngestHandler {
    executed_count: Arc<AtomicU32>,
}

#[async_trait]
impl JobHandler for MockIngestHandler {
    async fn execute(&self, _ctx: JobContext) -> Result<(), JobExecutionError> {
        self.executed_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn test_worker_pool_executes_job_and_shuts_down() {
    let pool = create_sqlite_pool(":memory:", 5).await.unwrap();
    run_migrations(&pool).await.unwrap();

    let job_repo: Arc<dyn JobRepository> = Arc::new(SqliteJobRepository::new(pool));

    let executed_count = Arc::new(AtomicU32::new(0));
    let handler = MockIngestHandler {
        executed_count: executed_count.clone(),
    };

    let registry = JobRegistry::builder()
        .register(JobKind::IngestRecording, handler)
        .build();

    let config = JobsConfig {
        workers: 2,
        poll_interval_ms: 50,
        lock_timeout_secs: 60,
        max_attempts: 3,
    };

    let cancellation_token = CancellationToken::new();
    let mut worker_pool = WorkerPool::new(
        job_repo.clone(),
        registry,
        config,
        cancellation_token.clone(),
    );

    // Enqueue 3 jobs
    for i in 0..3 {
        let req = EnqueueJob::new(JobKind::IngestRecording, serde_json::json!({ "index": i }));
        job_repo.enqueue(&req).await.unwrap();
    }

    worker_pool.start();

    // Wait a bit for workers to process
    tokio::time::sleep(Duration::from_millis(300)).await;

    assert_eq!(executed_count.load(Ordering::SeqCst), 3);

    // Test graceful shutdown
    cancellation_token.cancel();
    worker_pool.wait().await;
}
