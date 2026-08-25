use callmind_analysis::AnalysisEngine;
use callmind_config::AppConfig;
use callmind_db::{CallRepository, JobRepository, StatsRepository};
use callmind_search::{AskEngine, SearchEngine};
use callmind_storage::RecordingStorage;
use std::collections::HashMap;
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
    /// SHA-256 of each pinned worker certificate, to the worker's name.
    ///
    /// Empty when the worker listener runs without TLS, where there is no
    /// certificate to look up. Empty *with* TLS means nobody is recognised and
    /// every worker RPC is refused, which is the safe way to fail.
    pub worker_names: Arc<HashMap<String, String>>,
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
            worker_names: Arc::new(HashMap::new()),
        }
    }

    /// Pin the worker certificates the listener will accept.
    ///
    /// Separate from `new` because only the worker listener needs it and only
    /// when it runs TLS; every other caller would pass an empty map.
    #[must_use]
    pub fn with_worker_names(mut self, worker_names: HashMap<String, String>) -> Self {
        self.worker_names = Arc::new(worker_names);
        self
    }
}
