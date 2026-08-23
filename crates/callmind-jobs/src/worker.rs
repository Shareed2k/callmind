use crate::errors::JobExecutionError;
use crate::handler::{JobContext, JobRegistry};
use callmind_config::JobsConfig;
use callmind_core::{CallId, ProcessingStatus};
use callmind_db::{CallRepository, JobRepository};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, trace, warn};

/// Tokio-based worker pool managing background job execution.
pub struct WorkerPool {
    job_repo: Arc<dyn JobRepository>,
    call_repo: Arc<dyn CallRepository>,
    registry: JobRegistry,
    config: JobsConfig,
    cancellation_token: CancellationToken,
    worker_handles: Vec<JoinHandle<()>>,
    cleanup_handle: Option<JoinHandle<()>>,
}

/// Mark a call failed, logging rather than swallowing the error.
///
/// Replaces five hand-written copies of the same `UPDATE calls ...` statement
/// that bypassed `CallRepository::update_status`.
async fn fail_call(call_repo: &Arc<dyn CallRepository>, worker_id: &str, call_id: CallId) {
    if let Err(e) = call_repo
        .update_status(call_id, ProcessingStatus::Failed)
        .await
    {
        error!("[{worker_id}] Failed to mark call {call_id} as failed: {e}");
    }
}

impl WorkerPool {
    pub fn new(
        job_repo: Arc<dyn JobRepository>,
        call_repo: Arc<dyn CallRepository>,
        registry: JobRegistry,
        config: JobsConfig,
        cancellation_token: CancellationToken,
    ) -> Self {
        Self {
            job_repo,
            call_repo,
            registry,
            config,
            cancellation_token,
            worker_handles: Vec::new(),
            cleanup_handle: None,
        }
    }

    /// Start worker tasks in the background.
    pub fn start(&mut self) {
        info!(
            "Starting Job Worker Pool with {} workers",
            self.config.workers
        );

        // Startup reconciliation for orphaned processing calls
        let reconcile_repo = self.call_repo.clone();
        tokio::spawn(async move {
            match reconcile_repo.fail_orphaned_processing().await {
                Ok(count) if count > 0 => {
                    info!(
                        "Startup reconciliation: marked {count} orphaned processing calls as failed"
                    );
                }
                Ok(_) => {}
                Err(e) => {
                    error!("Startup reconciliation of orphaned calls failed: {e}");
                }
            }
        });

        for worker_idx in 0..self.config.workers {
            let worker_id = format!("worker-{}", worker_idx + 1);
            let job_repo = self.job_repo.clone();
            let registry = self.registry.clone();
            let token = self.cancellation_token.clone();
            let call_repo = self.call_repo.clone();
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

                                    // Run the handler in its own task so a panic
                                    // is contained. Previously a panicking
                                    // handler killed this worker permanently:
                                    // the pool never noticed, never restarted
                                    // it, and capacity silently decayed toward
                                    // zero while each victim job sat `running`
                                    // until stale-lock cleanup.
                                    let exec_res = match tokio::spawn(async move {
                                        h.execute(ctx).await
                                    })
                                    .await
                                    {
                                        Ok(res) => res,
                                        Err(join_err) if join_err.is_panic() => {
                                            error!(
                                                "[{worker_id}] Job {job_id} handler panicked: {join_err}"
                                            );
                                            Err(JobExecutionError::Failed(format!(
                                                "handler panicked: {join_err}"
                                            )))
                                        }
                                        Err(join_err) => {
                                            warn!(
                                                "[{worker_id}] Job {job_id} handler task aborted: {join_err}"
                                            );
                                            Err(JobExecutionError::Cancelled)
                                        }
                                    };
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
                                                if let Some(cid) = job_call_id {
                                                    fail_call(&call_repo, &worker_id, cid).await;
                                                }
                                            }
                                        }
                                        Err(JobExecutionError::Cancelled) => {
                                            warn!(
                                                "[{worker_id}] Job {job_id} was cancelled during execution"
                                            );
                                            if let Err(e) = job_repo
                                                .requeue_interrupted(
                                                    job_id,
                                                    "Interrupted by server shutdown",
                                                )
                                                .await
                                            {
                                                error!(
                                                    "[{worker_id}] Failed to requeue cancelled job {job_id}: {e}"
                                                );
                                            }
                                            if let Some(cid) = job_call_id {
                                                if let Err(e) = call_repo
                                                    .update_status(cid, ProcessingStatus::Pending)
                                                    .await
                                                {
                                                    error!(
                                                        "[{worker_id}] Failed to reset call {cid} to pending: {e}"
                                                    );
                                                }
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
                                            if let Some(cid) = job_call_id {
                                                fail_call(&call_repo, &worker_id, cid).await;
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
                                    if let Some(cid) = job_call_id {
                                        fail_call(&call_repo, &worker_id, cid).await;
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
