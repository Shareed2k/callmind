//! Call, recording, transcript and analysis storage — one implementation for
//! every backend.
//!
//! Built with `sea-query` for the same reason as the job repository: the
//! placeholder dialect and quoting come from the builder, so there is no second
//! copy of these twenty-odd queries to keep in step.
//!
//! Three things needed care to stay portable, and each is a named helper in
//! [`super::support`] rather than an inline branch:
//!
//! - **Full-text search** — an FTS5 virtual table on SQLite, a `tsvector` column
//!   on Postgres. Genuinely different; both driven from the same term expansion.
//! - **JSON extraction** — `json_extract` versus `->>`.
//! - **Relative dates** — computed in Rust and bound, instead of `date('now',
//!   '-7 days')` (SQLite) or `now() - interval '7 days'` (Postgres). One fewer
//!   dialect difference, and the truncation semantics match the old SQL exactly.

use super::support::{fts_matches, get, json_text, parse_ts};
use crate::errors::DbError;
use crate::traits::{AnalysisRow, CallListFilter, CallListRow, CallRepository};
use async_trait::async_trait;
use callmind_core::{
    Call, CallDirection, CallFilter, CallId, OrgId, ProcessingStatus, Recording, RecordingId,
};
use chrono::{Duration, Utc};
use sea_orm::sea_query::{
    Alias, Asterisk, Expr, ExprTrait, JoinType, OnConflict, Order, Query, SelectStatement,
};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbBackend, DeriveIden, QueryResult, TransactionTrait,
};
use std::str::FromStr;

#[derive(DeriveIden)]
enum Calls {
    Table,
    Id,
    OrganizationId,
    ExternalId,
    Direction,
    PhoneFrom,
    PhoneTo,
    StartedAt,
    EndedAt,
    DurationMs,
    ProcessingStatus,
    IsFavorite,
    Tags,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum Organizations {
    Table,
    Id,
    Name,
}

#[derive(DeriveIden)]
enum CallRecordings {
    Table,
    Id,
    CallId,
    StorageKey,
    MimeType,
    FileSizeBytes,
    Sha256,
    DurationMs,
    Channels,
    SampleRate,
    CreatedAt,
}

#[derive(DeriveIden)]
enum CallTranscripts {
    Table,
    CallId,
    TranscriptJson,
    CreatedAt,
    PrimaryLanguage,
}

#[derive(DeriveIden)]
enum CallAnalyses {
    Table,
    Id,
    CallId,
    Title,
    Summary,
    Reason,
    Resolution,
    Resolved,
    CustomerIntent,
    SentimentScore,
    MetricsJson,
    FullAnalysisJson,
    CreatedAt,
}

#[derive(DeriveIden)]
enum CallPluginResults {
    Table,
    CallId,
    Plugin,
    PayloadJson,
    CreatedAt,
}

#[derive(DeriveIden)]
enum Jobs {
    Table,
    CallId,
    Status,
}

#[derive(Debug, Clone)]
pub struct SqlCallRepository {
    conn: DatabaseConnection,
}

impl SqlCallRepository {
    pub fn new(conn: DatabaseConnection) -> Self {
        Self { conn }
    }

    fn backend(&self) -> DbBackend {
        self.conn.get_database_backend()
    }
}

/// Every column the call mapper reads, in a fixed order.
fn call_columns() -> [Calls; 14] {
    [
        Calls::Id,
        Calls::OrganizationId,
        Calls::ExternalId,
        Calls::Direction,
        Calls::PhoneFrom,
        Calls::PhoneTo,
        Calls::StartedAt,
        Calls::EndedAt,
        Calls::DurationMs,
        Calls::ProcessingStatus,
        Calls::IsFavorite,
        Calls::Tags,
        Calls::CreatedAt,
        Calls::UpdatedAt,
    ]
}

fn map_call(row: &QueryResult) -> Result<Call, DbError> {
    let id_str: String = get(row, "id")?;
    let org_id_str: String = get(row, "organization_id")?;
    let direction_str: String = get(row, "direction")?;
    let status_str: String = get(row, "processing_status")?;
    let started_at: Option<String> = get(row, "started_at")?;
    let ended_at: Option<String> = get(row, "ended_at")?;
    let created_at: String = get(row, "created_at")?;
    let updated_at: String = get(row, "updated_at")?;
    let duration_ms: Option<i64> = get(row, "duration_ms")?;
    // Stored as an integer rather than a boolean so the column type is identical
    // on both backends.
    let is_favorite: i32 = get(row, "is_favorite").unwrap_or(0);
    let tags_json: String = get(row, "tags").unwrap_or_else(|_| "[]".to_string());

    Ok(Call {
        id: CallId::from_str(&id_str).map_err(|e| DbError::NotFound(e.to_string()))?,
        organization_id: OrgId::from_str(&org_id_str)
            .map_err(|e| DbError::NotFound(e.to_string()))?,
        external_id: get(row, "external_id")?,
        direction: CallDirection::from_str(&direction_str).unwrap_or(CallDirection::Unknown),
        phone_from: get(row, "phone_from")?,
        phone_to: get(row, "phone_to")?,
        started_at: started_at.as_deref().map(parse_ts),
        ended_at: ended_at.as_deref().map(parse_ts),
        duration_ms: duration_ms.map(|d| d as u64),
        processing_status: ProcessingStatus::from_str(&status_str)
            .unwrap_or(ProcessingStatus::Pending),
        is_favorite: is_favorite != 0,
        tags: serde_json::from_str(&tags_json).unwrap_or_default(),
        created_at: parse_ts(&created_at),
        updated_at: parse_ts(&updated_at),
    })
}

fn map_recording(row: &QueryResult) -> Result<Recording, DbError> {
    let id_str: String = get(row, "id")?;
    let call_id_str: String = get(row, "call_id")?;
    let created_at: String = get(row, "created_at")?;
    let file_size_bytes: i64 = get(row, "file_size_bytes")?;
    let duration_ms: Option<i64> = get(row, "duration_ms")?;
    let channels: Option<i32> = get(row, "channels")?;
    let sample_rate: Option<i32> = get(row, "sample_rate")?;

    Ok(Recording {
        id: RecordingId::from_str(&id_str).map_err(|e| DbError::NotFound(e.to_string()))?,
        call_id: CallId::from_str(&call_id_str).map_err(|e| DbError::NotFound(e.to_string()))?,
        storage_key: get(row, "storage_key")?,
        mime_type: get(row, "mime_type")?,
        file_size_bytes: file_size_bytes as u64,
        sha256: get(row, "sha256")?,
        duration_ms: duration_ms.map(|d| d as u64),
        channels: channels.map(|c| c as u16),
        sample_rate: sample_rate.map(|r| r as u32),
        created_at: parse_ts(&created_at),
    })
}

/// The cutoff for a `today` / `7d` / `30d` filter.
///
/// Computed here instead of in SQL: `date('now', '-7 days')` and
/// `now() - interval '7 days'` are spelled differently, and both truncate to
/// midnight, which is what the UI's "last 7 days" is understood to mean.
fn date_cutoff(key: &str) -> Option<String> {
    let days = match key {
        "today" => 0,
        "7d" => 7,
        "30d" => 30,
        _ => return None,
    };
    let day = (Utc::now() - Duration::days(days)).date_naive();
    Some(day.and_hms_opt(0, 0, 0)?.and_utc().to_rfc3339())
}

/// The language codes the UI filters by, mapped to what the transcript stores.
///
/// The column holds whatever Whisper reported (`hebrew`), while the UI speaks in
/// codes (`he`). Both spellings are accepted so a re-transcription under a
/// different naming does not silently drop out of the filter.
fn language_aliases(code: &str) -> Option<[&'static str; 2]> {
    match code {
        "he" => Some(["hebrew", "he"]),
        "ru" => Some(["russian", "ru"]),
        "en" => Some(["english", "en"]),
        _ => None,
    }
}

#[async_trait]
impl CallRepository for SqlCallRepository {
    async fn create(&self, call: &Call) -> Result<(), DbError> {
        let stmt = Query::insert()
            .into_table(Calls::Table)
            .columns(call_columns())
            .values([
                call.id.to_string().into(),
                call.organization_id.to_string().into(),
                call.external_id.clone().into(),
                call.direction.as_str().into(),
                call.phone_from.clone().into(),
                call.phone_to.clone().into(),
                call.started_at.map(|dt| dt.to_rfc3339()).into(),
                call.ended_at.map(|dt| dt.to_rfc3339()).into(),
                call.duration_ms.map(|d| d as i64).into(),
                call.processing_status.as_str().into(),
                i32::from(call.is_favorite).into(),
                serde_json::to_string(&call.tags)?.into(),
                call.created_at.to_rfc3339().into(),
                call.updated_at.to_rfc3339().into(),
            ])
            .map_err(|e| DbError::Query(e.to_string()))?
            .to_owned();

        self.conn.execute(&stmt).await.map_err(|e| {
            // Both backends report a unique violation, with different codes and
            // wording, so the message is what there is to go on.
            let msg = e.to_string();
            let lowered = msg.to_lowercase();
            if lowered.contains("unique") || lowered.contains("duplicate key") {
                DbError::DuplicateKey(msg)
            } else {
                DbError::Query(msg)
            }
        })?;
        Ok(())
    }

    async fn get_by_id(&self, id: CallId) -> Result<Option<Call>, DbError> {
        let stmt = Query::select()
            .columns(call_columns())
            .from(Calls::Table)
            .and_where(Expr::col(Calls::Id).eq(id.to_string()))
            .to_owned();
        let row = self
            .conn
            .query_one(&stmt)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        row.as_ref().map(map_call).transpose()
    }

    async fn get_by_external_id(
        &self,
        org_id: OrgId,
        ext_id: &str,
    ) -> Result<Option<Call>, DbError> {
        let stmt = Query::select()
            .columns(call_columns())
            .from(Calls::Table)
            .and_where(Expr::col(Calls::OrganizationId).eq(org_id.to_string()))
            .and_where(Expr::col(Calls::ExternalId).eq(ext_id))
            .to_owned();
        let row = self
            .conn
            .query_one(&stmt)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        row.as_ref().map(map_call).transpose()
    }

    async fn list(&self, filter: &CallFilter) -> Result<Vec<Call>, DbError> {
        let mut stmt = Query::select();
        stmt.columns(call_columns()).from(Calls::Table);

        if let Some(org_id) = filter.organization_id {
            stmt.and_where(Expr::col(Calls::OrganizationId).eq(org_id.to_string()));
        }
        if let Some(ref ext_id) = filter.external_id {
            stmt.and_where(Expr::col(Calls::ExternalId).eq(ext_id.as_str()));
        }
        if let Some(status) = filter.status {
            stmt.and_where(Expr::col(Calls::ProcessingStatus).eq(status.as_str()));
        }
        if let Some(dir) = filter.direction {
            stmt.and_where(Expr::col(Calls::Direction).eq(dir.as_str()));
        }
        if let Some(from) = filter.from_date {
            stmt.and_where(Expr::col(Calls::CreatedAt).gte(from.to_rfc3339()));
        }
        if let Some(to) = filter.to_date {
            stmt.and_where(Expr::col(Calls::CreatedAt).lte(to.to_rfc3339()));
        }

        let stmt = stmt
            .order_by(Calls::IsFavorite, Order::Desc)
            .order_by(Calls::CreatedAt, Order::Desc)
            .limit(filter.limit.unwrap_or(50) as u64)
            .offset(filter.offset.unwrap_or(0) as u64)
            .to_owned();

        let rows = self
            .conn
            .query_all(&stmt)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        rows.iter().map(map_call).collect()
    }

    async fn list_for_display(
        &self,
        filter: &CallListFilter<'_>,
    ) -> Result<(Vec<CallListRow>, i64), DbError> {
        // The predicates are built once and handed to both the page and its
        // count. Two hand-maintained copies had to agree about the filter *and*
        // the bind order, which is a standing invitation to drift.
        let mut conditions: Vec<Expr> = Vec::new();

        if let Some(status) = filter.status {
            conditions.push(Expr::col(("c", Calls::ProcessingStatus)).eq(status));
        }
        if let Some(codes) = filter.language.and_then(language_aliases) {
            conditions.push(Expr::col(("t", CallTranscripts::PrimaryLanguage)).is_in(codes));
        }
        if let Some(cutoff) = filter.date.and_then(date_cutoff) {
            conditions.push(Expr::col(("c", Calls::CreatedAt)).gte(cutoff));
        }
        if let Some(search) = filter.search.map(str::trim).filter(|s| !s.is_empty()) {
            // The external id is matched literally as well as through the index:
            // call identifiers are not natural language and the tokenizer would
            // split them apart.
            let mut any = Expr::col(("c", Calls::ExternalId)).like(format!("%{search}%"));
            if let Some(fts) = fts_matches(self.backend(), Expr::col(("c", Calls::Id)), search) {
                any = any.or(fts);
            }
            conditions.push(any);
        }

        let apply = |stmt: &mut SelectStatement| {
            stmt.from_as(Calls::Table, Alias::new("c")).join_as(
                JoinType::LeftJoin,
                CallTranscripts::Table,
                Alias::new("t"),
                Expr::col(("c", Calls::Id)).equals(("t", CallTranscripts::CallId)),
            );
            for condition in &conditions {
                stmt.and_where(condition.clone());
            }
        };

        let mut count_stmt = Query::select();
        count_stmt.expr_as(Expr::cust("count(*)"), Alias::new("total"));
        apply(&mut count_stmt);
        let total: i64 = self
            .conn
            .query_one(&count_stmt.clone())
            .await
            .map_err(|e| DbError::Query(e.to_string()))?
            .map(|row| get::<i64>(&row, "total"))
            .transpose()?
            .unwrap_or(0);

        let mut list_stmt = Query::select();
        list_stmt
            .expr(Expr::col(("c", Asterisk)))
            .expr_as(
                Expr::col(("t", CallTranscripts::PrimaryLanguage)),
                Alias::new("primary_language"),
            )
            .expr_as(
                json_text(
                    self.backend(),
                    "t.transcript_json",
                    &["segments", "0", "normalized_text"],
                ),
                Alias::new("sample_text"),
            );
        apply(&mut list_stmt);
        let list_stmt = list_stmt
            .order_by(("c", Calls::IsFavorite), Order::Desc)
            .order_by(("c", Calls::CreatedAt), Order::Desc)
            .limit(u64::from(filter.limit))
            .offset(u64::from(filter.offset))
            .to_owned();

        let rows = self
            .conn
            .query_all(&list_stmt)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;

        let items = rows
            .iter()
            .map(|row| {
                Ok(CallListRow {
                    // The same row mapping as every other call query.
                    call: map_call(row)?,
                    sample_text: get(row, "sample_text").unwrap_or(None),
                    primary_language: get(row, "primary_language").unwrap_or(None),
                })
            })
            .collect::<Result<Vec<_>, DbError>>()?;

        Ok((items, total))
    }

    async fn update_status(&self, id: CallId, status: ProcessingStatus) -> Result<(), DbError> {
        let stmt = Query::update()
            .table(Calls::Table)
            .value(Calls::ProcessingStatus, status.as_str())
            .value(Calls::UpdatedAt, Utc::now().to_rfc3339())
            .and_where(Expr::col(Calls::Id).eq(id.to_string()))
            .to_owned();
        self.conn
            .execute(&stmt)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        Ok(())
    }

    async fn delete(&self, id: CallId) -> Result<bool, DbError> {
        let stmt = Query::delete()
            .from_table(Calls::Table)
            .and_where(Expr::col(Calls::Id).eq(id.to_string()))
            .to_owned();
        let res = self
            .conn
            .execute(&stmt)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        Ok(res.rows_affected() > 0)
    }

    async fn toggle_favorite(&self, id: CallId) -> Result<bool, DbError> {
        // Flipped in SQL rather than read-then-write, so two concurrent toggles
        // cannot both read the same value and land on the same result.
        let stmt = Query::update()
            .table(Calls::Table)
            .value(
                Calls::IsFavorite,
                Expr::cust("case when is_favorite = 0 then 1 else 0 end"),
            )
            .value(Calls::UpdatedAt, Utc::now().to_rfc3339())
            .and_where(Expr::col(Calls::Id).eq(id.to_string()))
            .returning(Query::returning().column(Calls::IsFavorite))
            .to_owned();

        let row = self
            .conn
            .query_one(&stmt)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?
            .ok_or_else(|| DbError::NotFound(format!("Call {id} not found")))?;
        Ok(get::<i32>(&row, "is_favorite")? != 0)
    }

    async fn update_tags(&self, id: CallId, tags: &[String]) -> Result<(), DbError> {
        let stmt = Query::update()
            .table(Calls::Table)
            .value(Calls::Tags, serde_json::to_string(tags)?)
            .value(Calls::UpdatedAt, Utc::now().to_rfc3339())
            .and_where(Expr::col(Calls::Id).eq(id.to_string()))
            .to_owned();
        let res = self
            .conn
            .execute(&stmt)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        if res.rows_affected() == 0 {
            return Err(DbError::NotFound(format!("Call {id} not found")));
        }
        Ok(())
    }

    async fn set_audio_metadata(
        &self,
        call_id: CallId,
        duration_ms: u64,
        channels: u16,
        sample_rate: u32,
    ) -> Result<(), DbError> {
        let id = call_id.to_string();
        let tx = self
            .conn
            .begin()
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;

        let call = Query::update()
            .table(Calls::Table)
            .value(Calls::DurationMs, duration_ms as i64)
            .value(Calls::UpdatedAt, Utc::now().to_rfc3339())
            .and_where(Expr::col(Calls::Id).eq(id.clone()))
            .to_owned();
        tx.execute(&call)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;

        // The recording row may not exist yet on some paths; updating nothing is
        // fine, the call still carries the duration.
        let recording = Query::update()
            .table(CallRecordings::Table)
            .value(CallRecordings::DurationMs, duration_ms as i64)
            .value(CallRecordings::Channels, i32::from(channels))
            .value(CallRecordings::SampleRate, sample_rate as i32)
            .and_where(Expr::col(CallRecordings::CallId).eq(id))
            .to_owned();
        tx.execute(&recording)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;

        tx.commit().await.map_err(|e| DbError::Query(e.to_string()))
    }

    async fn get_organization_name(&self, org_id: OrgId) -> Result<Option<String>, DbError> {
        let stmt = Query::select()
            .column(Organizations::Name)
            .from(Organizations::Table)
            .and_where(Expr::col(Organizations::Id).eq(org_id.to_string()))
            .to_owned();
        let row = self
            .conn
            .query_one(&stmt)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        row.as_ref().map(|r| get(r, "name")).transpose()
    }

    async fn commit_analysis(
        &self,
        analysis: &AnalysisRow<'_>,
        status: ProcessingStatus,
    ) -> Result<(), DbError> {
        let call_id = analysis.call_id.to_string();
        // One transaction: an analysis row and a status that disagree is worse
        // than neither being written.
        let tx = self
            .conn
            .begin()
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;

        let delete = Query::delete()
            .from_table(CallAnalyses::Table)
            .and_where(Expr::col(CallAnalyses::CallId).eq(call_id.clone()))
            .to_owned();
        tx.execute(&delete)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;

        let insert = Query::insert()
            .into_table(CallAnalyses::Table)
            .columns([
                CallAnalyses::Id,
                CallAnalyses::CallId,
                CallAnalyses::Title,
                CallAnalyses::Summary,
                CallAnalyses::Reason,
                CallAnalyses::Resolution,
                CallAnalyses::Resolved,
                CallAnalyses::CustomerIntent,
                CallAnalyses::SentimentScore,
                CallAnalyses::MetricsJson,
                CallAnalyses::FullAnalysisJson,
                CallAnalyses::CreatedAt,
            ])
            .values([
                analysis.id.to_string().into(),
                call_id.clone().into(),
                analysis.title.into(),
                analysis.summary.into(),
                analysis.reason.into(),
                analysis.resolution.into(),
                i32::from(analysis.resolved).into(),
                analysis.customer_intent.into(),
                f64::from(analysis.sentiment_score).into(),
                analysis.metrics_json.into(),
                analysis.full_analysis_json.into(),
                analysis.created_at.to_rfc3339().into(),
            ])
            .map_err(|e| DbError::Query(e.to_string()))?
            .to_owned();
        tx.execute(&insert)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;

        let update = Query::update()
            .table(Calls::Table)
            .value(Calls::ProcessingStatus, status.as_str())
            .value(Calls::UpdatedAt, Utc::now().to_rfc3339())
            .and_where(Expr::col(Calls::Id).eq(call_id))
            .to_owned();
        tx.execute(&update)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;

        tx.commit().await.map_err(|e| DbError::Query(e.to_string()))
    }

    async fn save_plugin_result(
        &self,
        call_id: CallId,
        plugin: &str,
        payload_json: &str,
    ) -> Result<(), DbError> {
        let stmt = Query::insert()
            .into_table(CallPluginResults::Table)
            .columns([
                CallPluginResults::CallId,
                CallPluginResults::Plugin,
                CallPluginResults::PayloadJson,
                CallPluginResults::CreatedAt,
            ])
            .values([
                call_id.to_string().into(),
                plugin.into(),
                payload_json.into(),
                Utc::now().to_rfc3339().into(),
            ])
            .map_err(|e| DbError::Query(e.to_string()))?
            // Upsert is spelled the same on SQLite 3.24+ and Postgres 9.5+.
            .on_conflict(
                OnConflict::columns([CallPluginResults::CallId, CallPluginResults::Plugin])
                    .update_columns([CallPluginResults::PayloadJson, CallPluginResults::CreatedAt])
                    .to_owned(),
            )
            .to_owned();
        self.conn
            .execute(&stmt)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        Ok(())
    }

    async fn list_plugin_results(&self, call_id: CallId) -> Result<Vec<(String, String)>, DbError> {
        let stmt = Query::select()
            .columns([CallPluginResults::Plugin, CallPluginResults::PayloadJson])
            .from(CallPluginResults::Table)
            .and_where(Expr::col(CallPluginResults::CallId).eq(call_id.to_string()))
            .order_by(CallPluginResults::Plugin, Order::Asc)
            .to_owned();
        let rows = self
            .conn
            .query_all(&stmt)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        rows.iter()
            .map(|row| Ok((get(row, "plugin")?, get(row, "payload_json")?)))
            .collect()
    }

    async fn get_transcript_json(&self, call_id: CallId) -> Result<Option<String>, DbError> {
        let stmt = Query::select()
            .column(CallTranscripts::TranscriptJson)
            .from(CallTranscripts::Table)
            .and_where(Expr::col(CallTranscripts::CallId).eq(call_id.to_string()))
            .to_owned();
        let row = self
            .conn
            .query_one(&stmt)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        row.as_ref().map(|r| get(r, "transcript_json")).transpose()
    }

    async fn save_transcript(&self, call_id: CallId, transcript_json: &str) -> Result<(), DbError> {
        let stmt = Query::insert()
            .into_table(CallTranscripts::Table)
            .columns([
                CallTranscripts::CallId,
                CallTranscripts::TranscriptJson,
                CallTranscripts::CreatedAt,
            ])
            .values([
                call_id.to_string().into(),
                transcript_json.into(),
                Utc::now().to_rfc3339().into(),
            ])
            .map_err(|e| DbError::Query(e.to_string()))?
            // An upsert instead of delete-then-insert, so re-transcribing needs
            // no transaction to stay consistent.
            .on_conflict(
                OnConflict::column(CallTranscripts::CallId)
                    .update_columns([CallTranscripts::TranscriptJson, CallTranscripts::CreatedAt])
                    .to_owned(),
            )
            .to_owned();
        self.conn
            .execute(&stmt)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        Ok(())
    }

    async fn update_analysis_title(&self, call_id: CallId, title: &str) -> Result<(), DbError> {
        let stmt = Query::update()
            .table(CallAnalyses::Table)
            .value(CallAnalyses::Title, title)
            .and_where(Expr::col(CallAnalyses::CallId).eq(call_id.to_string()))
            .to_owned();
        self.conn
            .execute(&stmt)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        Ok(())
    }

    async fn update_transcript_json(
        &self,
        call_id: CallId,
        transcript_json: &str,
    ) -> Result<(), DbError> {
        // `created_at` is deliberately left alone: renaming a speaker must not
        // look like a re-transcription.
        let stmt = Query::update()
            .table(CallTranscripts::Table)
            .value(CallTranscripts::TranscriptJson, transcript_json)
            .and_where(Expr::col(CallTranscripts::CallId).eq(call_id.to_string()))
            .to_owned();
        self.conn
            .execute(&stmt)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        Ok(())
    }

    async fn get_analysis_json(
        &self,
        call_id: CallId,
    ) -> Result<Option<(String, String)>, DbError> {
        let stmt = Query::select()
            .columns([CallAnalyses::Title, CallAnalyses::FullAnalysisJson])
            .from(CallAnalyses::Table)
            .and_where(Expr::col(CallAnalyses::CallId).eq(call_id.to_string()))
            .to_owned();
        let row = self
            .conn
            .query_one(&stmt)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        match row {
            Some(row) => Ok(Some((
                get(&row, "title")?,
                get(&row, "full_analysis_json")?,
            ))),
            None => Ok(None),
        }
    }

    async fn fail_orphaned_processing(&self) -> Result<u64, DbError> {
        self.sweep_processing(
            ProcessingStatus::Failed,
            false,
            &[JobStatusText::Pending, JobStatusText::Running],
        )
        .await
    }

    async fn reset_processing_to_pending(&self) -> Result<u64, DbError> {
        self.sweep_processing(ProcessingStatus::Pending, true, &[JobStatusText::Pending])
            .await
    }

    async fn add_recording(&self, recording: &Recording) -> Result<(), DbError> {
        let stmt = Query::insert()
            .into_table(CallRecordings::Table)
            .columns([
                CallRecordings::Id,
                CallRecordings::CallId,
                CallRecordings::StorageKey,
                CallRecordings::MimeType,
                CallRecordings::FileSizeBytes,
                CallRecordings::Sha256,
                CallRecordings::DurationMs,
                CallRecordings::Channels,
                CallRecordings::SampleRate,
                CallRecordings::CreatedAt,
            ])
            .values([
                recording.id.to_string().into(),
                recording.call_id.to_string().into(),
                recording.storage_key.clone().into(),
                recording.mime_type.clone().into(),
                (recording.file_size_bytes as i64).into(),
                recording.sha256.clone().into(),
                recording.duration_ms.map(|d| d as i64).into(),
                recording.channels.map(i32::from).into(),
                recording.sample_rate.map(|r| r as i32).into(),
                recording.created_at.to_rfc3339().into(),
            ])
            .map_err(|e| DbError::Query(e.to_string()))?
            .to_owned();
        self.conn
            .execute(&stmt)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        Ok(())
    }

    async fn delete_recording(&self, id: RecordingId) -> Result<bool, DbError> {
        let stmt = Query::delete()
            .from_table(CallRecordings::Table)
            .and_where(Expr::col(CallRecordings::Id).eq(id.to_string()))
            .to_owned();
        let res = self
            .conn
            .execute(&stmt)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        Ok(res.rows_affected() > 0)
    }

    async fn get_recording_by_call_id(
        &self,
        call_id: CallId,
    ) -> Result<Option<Recording>, DbError> {
        let stmt = Query::select()
            .columns([
                CallRecordings::Id,
                CallRecordings::CallId,
                CallRecordings::StorageKey,
                CallRecordings::MimeType,
                CallRecordings::FileSizeBytes,
                CallRecordings::Sha256,
                CallRecordings::DurationMs,
                CallRecordings::Channels,
                CallRecordings::SampleRate,
                CallRecordings::CreatedAt,
            ])
            .from(CallRecordings::Table)
            .and_where(Expr::col(CallRecordings::CallId).eq(call_id.to_string()))
            .to_owned();
        let row = self
            .conn
            .query_one(&stmt)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        row.as_ref().map(map_recording).transpose()
    }
}

/// Job states, as stored.
#[derive(Clone, Copy)]
enum JobStatusText {
    Pending,
    Running,
}

impl JobStatusText {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
        }
    }
}

impl SqlCallRepository {
    /// Reconcile calls stuck in `processing`, by whether a job still covers them.
    ///
    /// The two callers differ only in the new status, the direction of the
    /// membership test and which job states count, so they share the statement
    /// rather than keeping two near-identical `UPDATE`s in step.
    async fn sweep_processing(
        &self,
        new_status: ProcessingStatus,
        covered: bool,
        job_states: &[JobStatusText],
    ) -> Result<u64, DbError> {
        let states: Vec<&str> = job_states.iter().map(|s| s.as_str()).collect();
        let live_jobs = Query::select()
            .column(Jobs::CallId)
            .from(Jobs::Table)
            .and_where(Expr::col(Jobs::Status).is_in(states))
            .to_owned();

        let membership = if covered {
            Expr::col(Calls::Id).in_subquery(live_jobs)
        } else {
            Expr::col(Calls::Id).not_in_subquery(live_jobs)
        };

        let stmt = Query::update()
            .table(Calls::Table)
            .value(Calls::ProcessingStatus, new_status.as_str())
            .value(Calls::UpdatedAt, Utc::now().to_rfc3339())
            .and_where(Expr::col(Calls::ProcessingStatus).eq(ProcessingStatus::Processing.as_str()))
            .and_where(membership)
            .to_owned();

        let res = self
            .conn
            .execute(&stmt)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        Ok(res.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_cutoffs_truncate_to_midnight() {
        for key in ["today", "7d", "30d"] {
            let cutoff = date_cutoff(key).expect("known key");
            assert!(cutoff.contains("T00:00:00"), "{key} -> {cutoff}");
        }
        assert!(date_cutoff("all").is_none());
        assert!(date_cutoff("").is_none());

        // Ordering must be strict, or "last 7 days" and "last 30 days" would
        // select the same rows.
        let today = date_cutoff("today").unwrap();
        let week = date_cutoff("7d").unwrap();
        let month = date_cutoff("30d").unwrap();
        assert!(month < week && week < today);
    }

    /// The column stores what the transcript reported; the UI asks in codes.
    #[test]
    fn language_filter_accepts_both_spellings() {
        assert_eq!(language_aliases("he"), Some(["hebrew", "he"]));
        assert_eq!(language_aliases("ru"), Some(["russian", "ru"]));
        assert!(language_aliases("all").is_none());
    }
}
