use crate::errors::DbError;
use async_trait::async_trait;
use callmind_core::{
    Call, CallFilter, CallId, EnqueueJob, Job, JobId, JobKind, OrgId, ProcessingStatus, Recording,
    RecordingId,
};
use std::time::Duration;

/// Columns of a call analysis, as the storage layer sees them.
///
/// A flat row rather than the `CallAnalysis` domain type: `callmind-db` sits
/// below `callmind-analysis` and must not depend upwards. Mirrors how
/// `IndexCallParams` keeps the search schema out of its callers.
#[derive(Debug, Clone)]
pub struct AnalysisRow<'a> {
    pub id: uuid::Uuid,
    pub call_id: CallId,
    pub title: &'a str,
    pub summary: &'a str,
    pub reason: Option<&'a str>,
    pub resolution: Option<&'a str>,
    pub resolved: bool,
    pub customer_intent: Option<&'a str>,
    pub sentiment_score: f32,
    pub metrics_json: &'a str,
    pub full_analysis_json: &'a str,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[async_trait]
pub trait CallRepository: Send + Sync {
    async fn create(&self, call: &Call) -> Result<(), DbError>;
    async fn get_by_id(&self, id: CallId) -> Result<Option<Call>, DbError>;
    async fn get_by_external_id(
        &self,
        org_id: OrgId,
        ext_id: &str,
    ) -> Result<Option<Call>, DbError>;
    async fn list(&self, filter: &CallFilter) -> Result<Vec<Call>, DbError>;

    /// One page of the calls list, plus the total matching count.
    ///
    /// Both come from one method so the filter is expressed once; the page and
    /// its count were previously two hand-written queries that had to agree.
    async fn list_for_display(
        &self,
        filter: &CallListFilter<'_>,
    ) -> Result<(Vec<CallListRow>, i64), DbError>;
    async fn update_status(&self, id: CallId, status: ProcessingStatus) -> Result<(), DbError>;
    async fn delete(&self, id: CallId) -> Result<bool, DbError>;
    async fn toggle_favorite(&self, id: CallId) -> Result<bool, DbError>;
    async fn update_tags(&self, id: CallId, tags: &[String]) -> Result<(), DbError>;

    /// Record the audio's real duration and format, once it has been decoded.
    ///
    /// Only the batch importer used to set these, and it took them from the
    /// container header. Every other way audio arrives -- the upload endpoint and
    /// all three bot channels -- left `calls.duration_ms` NULL, which showed as a
    /// dash in the calls list and as a flat zero in the analytics averages.
    ///
    /// Called after decoding, so the duration is what the samples actually say
    /// rather than what the container claims. Writes the call and its recording
    /// together: a duration on one and not the other is not a state worth having.
    async fn set_audio_metadata(
        &self,
        call_id: CallId,
        duration_ms: u64,
        channels: u16,
        sample_rate: u32,
    ) -> Result<(), DbError>;

    /// Display name of an organization, used to give the LLM real context
    /// instead of a placeholder.
    async fn get_organization_name(&self, org_id: OrgId) -> Result<Option<String>, DbError>;

    /// Store (replacing) a plugin's result for a call.
    ///
    /// One generic surface for every plugin, so adding one needs no migration.
    /// The payload is opaque here: the plugin and the renderer agree on its
    /// shape, not the storage layer.
    async fn save_plugin_result(
        &self,
        call_id: CallId,
        plugin: &str,
        payload_json: &str,
    ) -> Result<(), DbError>;

    /// All plugin results for a call, as `(plugin, payload_json)`.
    async fn list_plugin_results(&self, call_id: CallId) -> Result<Vec<(String, String)>, DbError>;

    /// Stored transcript JSON for a call, if transcription has already run.
    async fn get_transcript_json(&self, call_id: CallId) -> Result<Option<String>, DbError>;

    /// Persist (replacing any previous) the transcript for a call.
    ///
    /// Separate from the analysis write so the expensive transcription stage can
    /// be committed as soon as it finishes.
    async fn save_transcript(&self, call_id: CallId, transcript_json: &str) -> Result<(), DbError>;

    /// Write an analysis and move the call to `status`, atomically.
    ///
    /// The transaction is owned here rather than handed in, so the trait stays
    /// free of any one database's connection type. Replaces a previous analysis
    /// for the same call, making a retry idempotent.
    async fn commit_analysis(
        &self,
        analysis: &AnalysisRow<'_>,
        status: ProcessingStatus,
    ) -> Result<(), DbError>;

    /// Rename an analysis without touching the rest of it.
    async fn update_analysis_title(&self, call_id: CallId, title: &str) -> Result<(), DbError>;

    /// Replace a stored transcript in place, preserving its `created_at`.
    ///
    /// Distinct from [`CallRepository::save_transcript`], which stamps a new
    /// timestamp: editing speaker labels should not look like a
    /// re-transcription.
    async fn update_transcript_json(
        &self,
        call_id: CallId,
        transcript_json: &str,
    ) -> Result<(), DbError>;

    /// Fetch a completed analysis as `(title, full_analysis_json)`.
    ///
    /// Lives here for now so bot channels do not need raw SQL; a dedicated
    /// `AnalysisRepository` is the proper home once the pipeline's inline
    /// transaction moves out too.
    async fn get_analysis_json(&self, call_id: CallId)
    -> Result<Option<(String, String)>, DbError>;

    /// Mark calls left in `processing` with no pending or running job as failed.
    /// Used for startup reconciliation after an unclean shutdown. Returns the
    /// number of calls reconciled.
    async fn fail_orphaned_processing(&self) -> Result<u64, DbError>;

    async fn add_recording(&self, recording: &Recording) -> Result<(), DbError>;

    /// Remove a recording row, for rolling back a failed upload.
    async fn delete_recording(&self, id: RecordingId) -> Result<bool, DbError>;

    /// Return calls left in `processing` to `pending` when their job has been
    /// requeued, so a restart does not leave them stuck. Returns rows affected.
    async fn reset_processing_to_pending(&self) -> Result<u64, DbError>;
    async fn get_recording_by_call_id(&self, call_id: CallId)
    -> Result<Option<Recording>, DbError>;
}

#[async_trait]
pub trait JobRepository: Send + Sync {
    async fn enqueue(&self, job: &EnqueueJob) -> Result<JobId, DbError>;
    async fn fetch_and_lock(
        &self,
        worker_id: &str,
        kinds: &[JobKind],
    ) -> Result<Option<Job>, DbError>;
    async fn renew_lock(&self, id: JobId, worker_id: &str) -> Result<bool, DbError>;
    async fn mark_completed(&self, id: JobId) -> Result<(), DbError>;
    async fn mark_failed(
        &self,
        id: JobId,
        error: &str,
        retry_delay: Option<Duration>,
    ) -> Result<(), DbError>;
    async fn release_stale_locks(&self, older_than: Duration) -> Result<u64, DbError>;

    /// Return a locked job to the queue without consuming a retry attempt,
    /// for work interrupted by shutdown rather than by failure.
    async fn requeue_interrupted(&self, id: JobId, reason: &str) -> Result<(), DbError>;

    /// Return every running job to the queue without consuming an attempt, for
    /// graceful shutdown. Returns the number requeued.
    async fn requeue_all_running(&self, reason: &str) -> Result<u64, DbError>;
    async fn get_by_id(&self, id: JobId) -> Result<Option<Job>, DbError>;
    async fn list_by_call_id(&self, call_id: CallId) -> Result<Vec<Job>, DbError>;
}

/// Filters behind the calls list page.
///
/// Distinct from [`CallFilter`], which is the plain entity query: this one also
/// covers language, relative date ranges and full-text search, and it drives a
/// read model rather than returning bare entities.
#[derive(Debug, Clone, Default)]
pub struct CallListFilter<'a> {
    pub status: Option<&'a str>,
    /// "he", "ru" or "en".
    pub language: Option<&'a str>,
    /// "today", "7d" or "30d".
    pub date: Option<&'a str>,
    /// Raw user text. Matched against the external id and, through the
    /// full-text index, against the transcript and analysis. Escaping happens
    /// inside the repository, which is the only thing that knows whether the
    /// dialect is FTS5 or `tsquery`.
    pub search: Option<&'a str>,
    pub limit: u32,
    pub offset: u32,
}

/// A call plus the extras the list page shows alongside it.
#[derive(Debug, Clone)]
pub struct CallListRow {
    pub call: Call,
    /// Opening line of the transcript, for a preview.
    pub sample_text: Option<String>,
    /// Detected language, from the indexed transcript column rather than
    /// re-sniffed from characters: the filter and the badge must agree.
    pub primary_language: Option<String>,
}

/// Whole-table aggregates for the calls list.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CallStats {
    pub total: i64,
    pub completed: i64,
    /// Mean call duration in milliseconds, 0 when there are no calls.
    pub avg_duration_ms: f64,
    pub total_duration_ms: f64,
}

/// Read-model queries behind the dashboards.
///
/// Separate from [`CallRepository`], which is entity CRUD: these are aggregates
/// that exist only to be displayed, and keeping them apart stops the entity
/// trait growing a reporting surface. Also the place a second database backend
/// has to reimplement, so it is worth having them enumerated in one trait.
#[async_trait]
pub trait StatsRepository: Send + Sync {
    /// Job counts keyed by status string.
    async fn job_counts_by_status(&self) -> Result<Vec<(String, i64)>, DbError>;

    async fn call_stats(&self) -> Result<CallStats, DbError>;

    /// Most common customer intents, as `(intent, count)`.
    async fn top_intents(&self, limit: u32) -> Result<Vec<(String, i64)>, DbError>;

    /// Calls per day, most recent first, as `(YYYY-MM-DD, count)`.
    async fn daily_call_counts(&self, days: u32) -> Result<Vec<(String, i64)>, DbError>;

    /// Transcript counts per primary language, as `(language, count)`.
    async fn language_distribution(&self) -> Result<Vec<(String, i64)>, DbError>;

    /// The most recent failure message for a call, for the UI's error banner.
    async fn last_job_error(&self, call_id: CallId) -> Result<Option<String>, DbError>;
}

/// A conversation as the full-text index stores it.
///
/// The nine columns exist once, here: they used to be hand-written in the search
/// engine *and* inline in the processing pipeline, so the FTS schema had two
/// places that had to agree.
pub struct IndexDocument<'a> {
    pub call_id: CallId,
    pub org_id: OrgId,
    pub title: &'a str,
    pub summary: &'a str,
    pub transcript: &'a str,
    pub topics: &'a [String],
    pub entities: &'a [String],
    pub reason: Option<&'a str>,
    pub resolution: Option<&'a str>,
}

/// A full-text query with the metadata filters applied alongside it.
pub struct SearchQuery<'a> {
    /// Raw user text. Escaping and morphological expansion belong to the
    /// implementation, which is the only thing that knows its own dialect.
    pub text: &'a str,
    pub organization_id: Option<OrgId>,
    pub from_date: Option<chrono::DateTime<chrono::Utc>>,
    pub to_date: Option<chrono::DateTime<chrono::Utc>>,
    pub direction: Option<&'a str>,
    pub status: Option<&'a str>,
    pub limit: u32,
    pub offset: u32,
}

/// One search result, with the score the backend assigned it.
pub struct SearchHit {
    pub call_id: CallId,
    pub title: String,
    pub summary: String,
    /// The matched excerpt, with the hit wrapped in `<b>` tags.
    pub match_highlight: String,
    /// Relevance, already normalised so that a larger number is never assumed:
    /// the ordering is applied by the query, not by the caller.
    pub rank: f64,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Writing to and querying the full-text index.
///
/// Its own trait rather than part of [`CallRepository`] because it is the one
/// surface where the backends use genuinely different engines — FTS5 versus
/// `tsvector` — and because a deployment could plausibly point it at something
/// that is not the main database at all.
#[async_trait]
pub trait SearchIndex: Send + Sync {
    /// Index a conversation, replacing any existing entry for it.
    async fn index(&self, doc: &IndexDocument<'_>) -> Result<(), DbError>;
    /// Update just the title, for a rename that does not re-run analysis.
    async fn update_title(&self, call_id: CallId, title: &str) -> Result<(), DbError>;
    async fn search(&self, query: &SearchQuery<'_>) -> Result<Vec<SearchHit>, DbError>;
    async fn delete(&self, call_id: CallId) -> Result<(), DbError>;
}

/// One speaker of one call, with the voice print that identifies them.
pub struct StoredSpeaker {
    pub speaker_id: callmind_core::SpeakerId,
    pub embedding: Vec<f32>,
    /// Set once somebody names this voice; that is what makes it recognisable in
    /// a later call.
    pub name: Option<String>,
}

/// Voice prints, kept so a speaker can be recognised across calls.
///
/// Its own trait rather than more methods on [`CallRepository`] because it is a
/// different concern -- and because embeddings are biometric data, which is
/// easier to reason about when the surface that touches them is small.
///
/// There is deliberately no separate table of profiles: a profile is an
/// embedding with a name on it. Naming the same voice in several calls leaves
/// several exemplars, and the nearest one wins, which handles the same person on
/// a different handset better than one averaged vector.
#[async_trait]
pub trait SpeakerRepository: Send + Sync {
    /// Record a speaker's voice print, replacing any previous one for that
    /// speaker. A call is reprocessed often; one row per speaker.
    async fn save_speaker_embedding(
        &self,
        call_id: CallId,
        speaker_id: callmind_core::SpeakerId,
        embedding: &[f32],
    ) -> Result<(), DbError>;

    async fn speakers_for_call(&self, call_id: CallId) -> Result<Vec<StoredSpeaker>, DbError>;

    /// Give a voice a name, which turns it into an exemplar for later calls.
    async fn name_speaker(
        &self,
        call_id: CallId,
        speaker_id: callmind_core::SpeakerId,
        name: &str,
    ) -> Result<(), DbError>;

    /// Every named voice in an organization, as `(name, embedding)`.
    async fn list_named_speakers(&self, org_id: OrgId) -> Result<Vec<(String, Vec<f32>)>, DbError>;
}
