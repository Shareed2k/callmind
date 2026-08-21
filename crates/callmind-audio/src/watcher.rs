use crate::importer::BatchImporter;
use callmind_core::OrgId;
use callmind_db::{CallRepository, JobRepository};
use callmind_storage::RecordingStorage;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tracing::{error, info};

/// Background service watching a directory for new audio recordings to ingest.
pub struct DirectoryWatcher {
    watch_dir: PathBuf,
    poll_interval: Duration,
    call_repo: Arc<dyn CallRepository>,
    job_repo: Arc<dyn JobRepository>,
    storage: Arc<dyn RecordingStorage>,
    org_id: OrgId,
}

impl DirectoryWatcher {
    pub fn new(
        watch_dir: PathBuf,
        poll_secs: u64,
        call_repo: Arc<dyn CallRepository>,
        job_repo: Arc<dyn JobRepository>,
        storage: Arc<dyn RecordingStorage>,
        org_id: OrgId,
    ) -> Self {
        Self {
            watch_dir,
            poll_interval: Duration::from_secs(poll_secs.max(1)),
            call_repo,
            job_repo,
            storage,
            org_id,
        }
    }

    /// Spawn the directory watcher background loop.
    pub fn spawn(self, mut shutdown_rx: watch::Receiver<bool>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            info!("Directory watcher started for path: {:?}", self.watch_dir);

            if !self.watch_dir.exists() {
                let _ = std::fs::create_dir_all(&self.watch_dir);
            }

            loop {
                tokio::select! {
                    () = tokio::time::sleep(self.poll_interval) => {
                        if self.watch_dir.exists() {
                            match BatchImporter::import_directory(
                                &self.watch_dir,
                                self.call_repo.clone(),
                                Some(self.job_repo.clone()),
                                self.storage.clone(),
                                self.org_id,
                                None,
                            ).await {
                                Ok(summary) => {
                                    if summary.imported_calls > 0 {
                                        info!("Directory watcher auto-imported {} new recordings", summary.imported_calls);
                                    }
                                }
                                Err(e) => {
                                    error!("Directory watcher import error: {e}");
                                }
                            }
                        }
                    }
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            info!("Directory watcher shutting down");
                            break;
                        }
                    }
                }
            }
        })
    }
}
