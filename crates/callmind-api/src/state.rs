use callmind_analysis::AnalysisEngine;
use callmind_config::AppConfig;
use callmind_db::{CallRepository, JobRepository, StatsRepository};
use callmind_search::{AskEngine, SearchEngine};
use callmind_storage::RecordingStorage;
use std::sync::Arc;

/// Shared application state injected into Axum route handlers.
///
/// Deliberately holds no database handle: every query goes through a repository
/// trait, so this crate compiles against any backend. Adding a concrete pool
/// back here would quietly re-couple the HTTP layer to SQLite.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<AppConfig>,
    pub call_repo: Arc<dyn CallRepository>,
    /// Voice prints, so naming a speaker here makes them recognisable later.
    pub speaker_repo: Arc<dyn callmind_db::SpeakerRepository>,
    pub job_repo: Arc<dyn JobRepository>,
    pub stats_repo: Arc<dyn StatsRepository>,
    pub storage: Arc<dyn RecordingStorage>,
    pub search: Arc<SearchEngine>,
    pub ask: Arc<AskEngine>,
    pub analyzer: Arc<AnalysisEngine>,
    /// Runtime HTML templates, including any a plugin has registered.
    pub templates: Arc<callmind_ui::templates::TemplateRegistry>,
}

impl AppState {
    pub fn new(
        config: Arc<AppConfig>,
        call_repo: Arc<dyn CallRepository>,
        speaker_repo: Arc<dyn callmind_db::SpeakerRepository>,
        job_repo: Arc<dyn JobRepository>,
        stats_repo: Arc<dyn StatsRepository>,
        storage: Arc<dyn RecordingStorage>,
        search: Arc<SearchEngine>,
        ask: Arc<AskEngine>,
        analyzer: Arc<AnalysisEngine>,
        templates: Arc<callmind_ui::templates::TemplateRegistry>,
    ) -> Self {
        Self {
            config,
            call_repo,
            speaker_repo,
            job_repo,
            stats_repo,
            storage,
            search,
            ask,
            analyzer,
            templates,
        }
    }
}
