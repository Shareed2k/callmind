use async_trait::async_trait;
use callmind_config::JobsConfig;
use callmind_core::{EnqueueJob, JobKind};
use callmind_db::{
    JobRepository, SqlCallRepository, SqlJobRepository, create_sqlite_pool, orm_connection,
    run_migrations,
};
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
    let pool = create_sqlite_pool(":memory:", 5, std::time::Duration::from_secs(5))
        .await
        .unwrap();
    run_migrations(&pool).await.unwrap();

    let job_repo: Arc<dyn JobRepository> = Arc::new(SqlJobRepository::new(orm_connection(&pool)));

    let executed_count = Arc::new(AtomicU32::new(0));
    let handler = MockIngestHandler {
        executed_count: executed_count.clone(),
    };

    let registry = JobRegistry::builder()
        .register(JobKind::IngestRecording, handler)
        .build();

    let config = JobsConfig {
        kinds: Vec::new(),
        workers: 2,
        poll_interval_ms: 50,
        lock_timeout_secs: 60,
        max_attempts: 3,
    };

    let cancellation_token = CancellationToken::new();
    let call_repo = Arc::new(SqlCallRepository::new(orm_connection(&pool)));
    let mut worker_pool = WorkerPool::new(
        job_repo.clone(),
        call_repo,
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

    wait_until("all three jobs to run", || async {
        executed_count.load(Ordering::SeqCst) == 3
    })
    .await;

    // Test graceful shutdown
    cancellation_token.cancel();
    worker_pool.wait().await;
}

/// Panics on the first invocation, succeeds afterwards.
struct PanicOnceHandler {
    calls: Arc<AtomicU32>,
    completed: Arc<AtomicU32>,
}

#[async_trait]
impl JobHandler for PanicOnceHandler {
    async fn execute(&self, _ctx: JobContext) -> Result<(), JobExecutionError> {
        assert!(
            self.calls.fetch_add(1, Ordering::SeqCst) != 0,
            "simulated handler panic"
        );
        self.completed.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// A panicking handler used to kill its worker task outright. The pool never
/// noticed and never restarted it, so capacity decayed silently.
#[tokio::test]
async fn test_handler_panic_does_not_kill_the_worker() {
    let pool = create_sqlite_pool(":memory:", 5, std::time::Duration::from_secs(5))
        .await
        .unwrap();
    run_migrations(&pool).await.unwrap();

    let job_repo: Arc<dyn JobRepository> = Arc::new(SqlJobRepository::new(orm_connection(&pool)));
    let call_repo = Arc::new(SqlCallRepository::new(orm_connection(&pool)));

    let calls = Arc::new(AtomicU32::new(0));
    let completed = Arc::new(AtomicU32::new(0));
    let registry = JobRegistry::builder()
        .register(
            JobKind::IngestRecording,
            PanicOnceHandler {
                calls: calls.clone(),
                completed: completed.clone(),
            },
        )
        .build();

    // One worker, so a dead worker means nothing else can ever be processed.
    let config = JobsConfig {
        kinds: Vec::new(),
        workers: 1,
        poll_interval_ms: 20,
        lock_timeout_secs: 60,
        max_attempts: 1,
    };

    let cancellation_token = CancellationToken::new();
    let mut worker_pool = WorkerPool::new(
        job_repo.clone(),
        call_repo,
        registry,
        config,
        cancellation_token.clone(),
    );

    for i in 0..3 {
        let req = EnqueueJob::new(JobKind::IngestRecording, serde_json::json!({ "index": i }));
        job_repo.enqueue(&req).await.unwrap();
    }

    worker_pool.start();
    wait_until("the worker to pick up all three jobs", || async {
        calls.load(Ordering::SeqCst) == 3
    })
    .await;
    assert_eq!(
        completed.load(Ordering::SeqCst),
        2,
        "the two non-panicking jobs should have completed"
    );

    cancellation_token.cancel();
    worker_pool.wait().await;
}

/// Poll until `condition` holds instead of assuming a fixed sleep is enough.
///
/// A loaded CI runner is slower than a laptop: the 500 ms this replaced was
/// enough on a pull request and not enough on the release build minutes later,
/// where the assertion read `Pending` and blocked the tag from publishing.
async fn wait_until<F, Fut>(what: &str, mut condition: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    const DEADLINE: Duration = Duration::from_secs(60);

    let start = std::time::Instant::now();
    loop {
        if condition().await {
            return;
        }
        assert!(
            start.elapsed() < DEADLINE,
            "timed out after {DEADLINE:?} waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
