use callmind_analysis::AnalysisEngine;
use callmind_config::AppConfig;
use callmind_db::{CallRepository, JobRepository};
use callmind_search::{AskEngine, SearchEngine};
use callmind_storage::RecordingStorage;
use std::sync::Arc;

/// Shared application state injected into Axum route handlers.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<AppConfig>,
    pub call_repo: Arc<dyn CallRepository>,
    pub job_repo: Arc<dyn JobRepository>,
    pub storage: Arc<dyn RecordingStorage>,
    pub search: Arc<SearchEngine>,
    pub ask: Arc<AskEngine>,
    pub analyzer: Arc<AnalysisEngine>,
    pub pool: sqlx::SqlitePool,
}

impl AppState {
    pub fn new(
        config: Arc<AppConfig>,
        call_repo: Arc<dyn CallRepository>,
        job_repo: Arc<dyn JobRepository>,
        storage: Arc<dyn RecordingStorage>,
        search: Arc<SearchEngine>,
        ask: Arc<AskEngine>,
        analyzer: Arc<AnalysisEngine>,
        pool: sqlx::SqlitePool,
    ) -> Self {
        Self {
            config,
            call_repo,
            job_repo,
            storage,
            search,
            ask,
            analyzer,
            pool,
        }
    }
}
