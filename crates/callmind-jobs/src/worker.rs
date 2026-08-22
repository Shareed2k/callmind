use crate::errors::JobExecutionError;
use crate::handler::{JobContext, JobRegistry};
use callmind_config::JobsConfig;
use callmind_db::JobRepository;
use sqlx::SqlitePool;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, trace, warn};

/// Tokio-based worker pool managing background job execution.
pub struct WorkerPool {
    job_repo: Arc<dyn JobRepository>,
    registry: JobRegistry,
    config: JobsConfig,
    cancellation_token: CancellationToken,
    pool: Option<SqlitePool>,
    worker_handles: Vec<JoinHandle<()>>,
    cleanup_handle: Option<JoinHandle<()>>,
}

impl WorkerPool {
    pub fn new(
        job_repo: Arc<dyn JobRepository>,
        registry: JobRegistry,
        config: JobsConfig,
        cancellation_token: CancellationToken,
    ) -> Self {
        Self {
            job_repo,
            registry,
            config,
            cancellation_token,
            pool: None,
            worker_handles: Vec::new(),
            cleanup_handle: None,
        }
    }

    #[must_use]
    pub fn with_pool(mut self, pool: SqlitePool) -> Self {
        self.pool = Some(pool);
        self
    }

    /// Start worker tasks in the background.
    pub fn start(&mut self) {
        info!(
            "Starting Job Worker Pool with {} workers",
            self.config.workers
        );

        // Startup reconciliation for orphaned processing calls
        if let Some(ref pool) = self.pool {
            let p = pool.clone();
            tokio::spawn(async move {
                let now_str = chrono::Utc::now().to_rfc3339();
                let res = sqlx::query(
                    r#"
                    UPDATE calls
                    SET processing_status = 'failed', updated_at = ?
                    WHERE processing_status = 'processing'
                      AND id NOT IN (SELECT call_id FROM jobs WHERE status IN ('pending', 'running'))
                    "#,
                )
                .bind(&now_str)
                .execute(&p)
                .await;

                if let Ok(r) = res {
                    if r.rows_affected() > 0 {
                        info!(
                            "Startup reconciliation: marked {} orphaned processing calls as failed",
                            r.rows_affected()
                        );
                    }
                }
            });
        }

        for worker_idx in 0..self.config.workers {
            let worker_id = format!("worker-{}", worker_idx + 1);
            let job_repo = self.job_repo.clone();
            let registry = self.registry.clone();
            let token = self.cancellation_token.clone();
            let pool_opt = self.pool.clone();
            let poll_interval = Duration::from_millis(self.config.poll_interval_ms);
            let lock_timeout_secs = self.config.lock_timeout_secs;

            let handle = tokio::spawn(async move {
                trace!("[{worker_id}] Worker task started");

                while !token.is_cancelled() {
                    let kinds = registry.registered_kinds();
                    if kinds.is_empty() {
                        tokio::select! {
                            () = token.cancelled() => break,
                            () = tokio::time::sleep(poll_interval) => continue,
                        }
                    }

                    match job_repo.fetch_and_lock(&worker_id, &kinds).await {
                        Ok(Some(job)) => {
                            info!("[{worker_id}] Locked job {} (kind: {})", job.id, job.kind);

                            let handler = registry.get(&job.kind);
                            let job_id = job.id;
                            let attempt = job.attempt;
                            let max_attempts = job.max_attempts;
                            let job_call_id = job.call_id;

                            match handler {
                                Some(h) => {
                                    let ctx = JobContext {
                                        job,
                                        cancellation_token: token.clone(),
                                    };

                                    // Spawn background heartbeat lease extension task
                                    let hb_repo = job_repo.clone();
                                    let hb_job_id = job_id;
                                    let hb_worker_id = worker_id.clone();
                                    let hb_token = token.clone();
                                    let hb_interval_dur =
                                        Duration::from_secs(lock_timeout_secs / 3)
                                            .max(Duration::from_secs(5));
                                    let (hb_tx, mut hb_rx) = tokio::sync::oneshot::channel::<()>();

                                    let hb_handle = tokio::spawn(async move {
                                        let mut interval = tokio::time::interval(hb_interval_dur);
                                        interval.tick().await; // skip initial immediate tick
                                        loop {
                                            tokio::select! {
                                                _ = &mut hb_rx => break,
                                                () = hb_token.cancelled() => break,
                                                _ = interval.tick() => {
                                                    match hb_repo.renew_lock(hb_job_id, &hb_worker_id).await {
                                                        Ok(true) => {
                                                            trace!("[{hb_worker_id}] Extended lease for job {hb_job_id}");
                                                        }
                                                        Ok(false) => {
                                                            warn!("[{hb_worker_id}] Lost lock lease for job {hb_job_id}");
                                                            break;
                                                        }
                                                        Err(e) => {
                                                            warn!("[{hb_worker_id}] Error renewing lock for job {hb_job_id}: {e}");
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    });

                                    let exec_res = h.execute(ctx).await;
                                    let _ = hb_tx.send(());
                                    let _ = hb_handle.await;

                                    match exec_res {
                                        Ok(()) => {
                                            info!(
                                                "[{worker_id}] Job {job_id} completed successfully"
                                            );
                                            if let Err(e) = job_repo.mark_completed(job_id).await {
                                                error!(
                                                    "[{worker_id}] Failed to mark job {job_id} completed: {e}"
                                                );
                                            }
                                        }
                                        Err(JobExecutionError::Retryable(err)) => {
                                            warn!(
                                                "[{worker_id}] Job {job_id} failed with retryable error: {err} (attempt {attempt}/{max_attempts})"
                                            );
                                            if attempt < max_attempts {
                                                let backoff_secs =
                                                    2_u64.pow(attempt.clamp(1, 6) as u32) * 5;
                                                let delay = Duration::from_secs(backoff_secs);
                                                if let Err(e) = job_repo
                                                    .mark_failed(job_id, &err, Some(delay))
                                                    .await
                                                {
                                                    error!(
                                                        "[{worker_id}] Failed to reschedule job {job_id}: {e}"
                                                    );
                                                }
                                            } else {
                                                error!(
                                                    "[{worker_id}] Job {job_id} exceeded max attempts ({max_attempts})"
                                                );
                                                if let Err(e) =
                                                    job_repo.mark_failed(job_id, &err, None).await
                                                {
                                                    error!(
                                                        "[{worker_id}] Failed to mark job {job_id} failed: {e}"
                                                    );
                                                }
                                                if let (Some(cid), Some(p)) =
                                                    (job_call_id, pool_opt.as_ref())
                                                {
                                                    let now_str = chrono::Utc::now().to_rfc3339();
                                                    let _ = sqlx::query("UPDATE calls SET processing_status = 'failed', updated_at = ? WHERE id = ?")
                                                        .bind(&now_str)
                                                        .bind(cid.to_string())
                                                        .execute(p)
                                                        .await;
                                                }
                                            }
                                        }
                                        Err(JobExecutionError::Cancelled) => {
                                            warn!(
                                                "[{worker_id}] Job {job_id} was cancelled during execution"
                                            );
                                            if let Some(ref p) = pool_opt {
                                                let now = chrono::Utc::now().to_rfc3339();
                                                if let Err(e) = sqlx::query(
                                                    r#"
                                                    UPDATE jobs
                                                    SET status = 'pending',
                                                        attempt = MAX(attempt - 1, 0),
                                                        run_after = ?,
                                                        locked_at = NULL,
                                                        locked_by = NULL,
                                                        last_error = 'Interrupted by server shutdown',
                                                        completed_at = NULL
                                                    WHERE id = ?
                                                    "#,
                                                )
                                                .bind(&now)
                                                .bind(job_id.to_string())
                                                .execute(p)
                                                .await
                                                {
                                                    error!(
                                                        "[{worker_id}] Failed to requeue cancelled job {job_id}: {e}"
                                                    );
                                                }

                                                if let Some(call_id) = job_call_id {
                                                    let _ = sqlx::query(
                                                        "UPDATE calls SET processing_status = 'pending', updated_at = ? WHERE id = ?",
                                                    )
                                                    .bind(&now)
                                                    .bind(call_id.to_string())
                                                    .execute(p)
                                                    .await;
                                                }
                                            } else {
                                                let _ = job_repo
                                                    .mark_failed(
                                                        job_id,
                                                        "Interrupted by server shutdown",
                                                        Some(Duration::ZERO),
                                                    )
                                                    .await;
                                            }
                                        }
                                        Err(JobExecutionError::Failed(err))
                                        | Err(JobExecutionError::HandlerNotFound(err)) => {
                                            error!(
                                                "[{worker_id}] Job {job_id} failed non-retryable: {err}"
                                            );
                                            if let Err(e) =
                                                job_repo.mark_failed(job_id, &err, None).await
                                            {
                                                error!(
                                                    "[{worker_id}] Failed to mark job {job_id} failed: {e}"
                                                );
                                            }
                                            if let (Some(cid), Some(p)) =
                                                (job_call_id, pool_opt.as_ref())
                                            {
                                                let now_str = chrono::Utc::now().to_rfc3339();
                                                let _ = sqlx::query("UPDATE calls SET processing_status = 'failed', updated_at = ? WHERE id = ?")
                                                    .bind(&now_str)
                                                    .bind(cid.to_string())
                                                    .execute(p)
                                                    .await;
                                            }
                                        }
                                    }
                                }
                                None => {
                                    error!(
                                        "[{worker_id}] No handler registered for job kind {}",
                                        job.kind
                                    );
                                    let _ = job_repo
                                        .mark_failed(job_id, "Handler not registered", None)
                                        .await;
                                    if let (Some(cid), Some(p)) = (job_call_id, pool_opt.as_ref()) {
                                        let now_str = chrono::Utc::now().to_rfc3339();
                                        let _ = sqlx::query("UPDATE calls SET processing_status = 'failed', updated_at = ? WHERE id = ?")
                                            .bind(&now_str)
                                            .bind(cid.to_string())
                                            .execute(p)
                                            .await;
                                    }
                                }
                            }
                        }
                        Ok(None) => {
                            // No jobs available, wait for poll interval or shutdown
                            tokio::select! {
                                () = token.cancelled() => break,
                                () = tokio::time::sleep(poll_interval) => {},
                            }
                        }
                        Err(e) => {
                            error!("[{worker_id}] Error fetching job from repository: {e}");
                            tokio::select! {
                                () = token.cancelled() => break,
                                () = tokio::time::sleep(poll_interval) => {},
                            }
                        }
                    }
                }

                trace!("[{worker_id}] Worker task stopped gracefully");
            });

            self.worker_handles.push(handle);
        }

        // Stale locks periodic cleanup task
        let job_repo = self.job_repo.clone();
        let token = self.cancellation_token.clone();
        let lock_timeout = Duration::from_secs(self.config.lock_timeout_secs);

        let cleanup_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            loop {
                tokio::select! {
                    () = token.cancelled() => break,
                    _ = interval.tick() => {
                        match job_repo.release_stale_locks(lock_timeout).await {
                            Ok(count) if count > 0 => {
                                warn!("Released {count} stale job locks");
                            }
                            Ok(_) => {},
                            Err(e) => {
                                error!("Failed to release stale job locks: {e}");
                            }
                        }
                    }
                }
            }
        });

        self.cleanup_handle = Some(cleanup_handle);
    }

    /// Wait for all worker tasks to finish after cancellation.
    pub async fn wait(self) {
        for handle in self.worker_handles {
            let _ = handle.await;
        }
        if let Some(handle) = self.cleanup_handle {
            let _ = handle.await;
        }
    }
}
