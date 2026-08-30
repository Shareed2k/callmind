//! Job queue repository, one implementation for every backend.
//!
//! Queries are built with `sea-query` rather than written as SQL strings, so the
//! placeholder dialect (`?` on SQLite, `$1` on Postgres) is produced by the
//! builder instead of being hand-maintained twice. That is the whole reason this
//! is not two files.
//!
//! The SQL shapes themselves are portable: `UPDATE ... RETURNING` works on
//! SQLite 3.35+ and on Postgres, which is what lets the lease be taken in a
//! single statement.

use crate::errors::DbError;
use crate::traits::JobRepository;
use async_trait::async_trait;
use callmind_core::{CallId, EnqueueJob, Job, JobId, JobKind, JobStatus};
use chrono::{DateTime, Utc};
use sea_orm::sea_query::{Expr, ExprTrait, Order, Query};
use sea_orm::{ConnectionTrait, DatabaseConnection, DeriveIden, QueryResult, TryGetable};
use std::str::FromStr;
use std::time::Duration;

#[derive(DeriveIden)]
enum Jobs {
    Table,
    Id,
    CallId,
    Kind,
    Payload,
    Status,
    Priority,
    Attempt,
    MaxAttempts,
    RunAfter,
    LockedAt,
    LockedBy,
    LastError,
    CreatedAt,
    CompletedAt,
}

/// Every column the row mapper reads, in a fixed order.
fn job_columns() -> [Jobs; 14] {
    [
        Jobs::Id,
        Jobs::CallId,
        Jobs::Kind,
        Jobs::Payload,
        Jobs::Status,
        Jobs::Priority,
        Jobs::Attempt,
        Jobs::MaxAttempts,
        Jobs::RunAfter,
        Jobs::LockedAt,
        Jobs::LockedBy,
        Jobs::LastError,
        Jobs::CreatedAt,
        Jobs::CompletedAt,
    ]
}

pub struct SqlJobRepository {
    conn: DatabaseConnection,
}

impl SqlJobRepository {
    pub fn new(conn: DatabaseConnection) -> Self {
        Self { conn }
    }
}

fn get<T: TryGetable>(row: &QueryResult, col: &str) -> Result<T, DbError> {
    row.try_get("", col)
        .map_err(|e| DbError::Query(e.to_string()))
}

/// Timestamps are stored as RFC 3339 text on both backends, so parsing is shared.
fn parse_ts(raw: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(raw)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

fn map_job(row: &QueryResult) -> Result<Job, DbError> {
    let id_str: String = get(row, "id")?;
    let call_id_str: Option<String> = get(row, "call_id")?;
    let kind_str: String = get(row, "kind")?;
    let payload_str: String = get(row, "payload")?;
    let status_str: String = get(row, "status")?;
    let run_after_str: String = get(row, "run_after")?;
    let locked_at_str: Option<String> = get(row, "locked_at")?;
    let created_at_str: String = get(row, "created_at")?;
    let completed_at_str: Option<String> = get(row, "completed_at")?;

    Ok(Job {
        id: JobId::from_str(&id_str).map_err(|e| DbError::NotFound(e.to_string()))?,
        call_id: call_id_str.and_then(|s| CallId::from_str(&s).ok()),
        kind: JobKind::from_str(&kind_str).map_err(DbError::NotFound)?,
        payload: serde_json::from_str(&payload_str)?,
        status: JobStatus::from_str(&status_str).unwrap_or(JobStatus::Pending),
        priority: get(row, "priority")?,
        attempt: get(row, "attempt")?,
        max_attempts: get(row, "max_attempts")?,
        run_after: parse_ts(&run_after_str),
        locked_at: locked_at_str.as_deref().map(parse_ts),
        locked_by: get(row, "locked_by")?,
        last_error: get(row, "last_error")?,
        created_at: parse_ts(&created_at_str),
        completed_at: completed_at_str.as_deref().map(parse_ts),
    })
}

/// `NULL`-safe string value for sea-query.
fn opt(value: Option<String>) -> Expr {
    Expr::val(value)
}

#[async_trait]
impl JobRepository for SqlJobRepository {
    async fn enqueue(&self, job: &EnqueueJob) -> Result<JobId, DbError> {
        let id = JobId::generate();
        let now = Utc::now();
        let run_after = job.run_after.unwrap_or(now);

        let stmt = Query::insert()
            .into_table(Jobs::Table)
            .columns([
                Jobs::Id,
                Jobs::CallId,
                Jobs::Kind,
                Jobs::Payload,
                Jobs::Status,
                Jobs::Priority,
                Jobs::Attempt,
                Jobs::MaxAttempts,
                Jobs::RunAfter,
                Jobs::CreatedAt,
            ])
            .values_panic([
                id.to_string().into(),
                opt(job.call_id.map(|c| c.to_string())),
                job.kind.as_str().into(),
                job.payload.to_string().into(),
                JobStatus::Pending.as_str().into(),
                job.priority.into(),
                0.into(),
                job.max_attempts.into(),
                run_after.to_rfc3339().into(),
                now.to_rfc3339().into(),
            ])
            .to_owned();

        self.conn
            .execute(&stmt)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        Ok(id)
    }

    async fn fetch_and_lock(
        &self,
        worker_id: &str,
        kinds: &[JobKind],
    ) -> Result<Option<Job>, DbError> {
        let now = Utc::now().to_rfc3339();

        // Pick the single most deserving pending job, then claim it in the same
        // statement so two workers cannot take the same one.
        let mut candidate = Query::select();
        candidate
            .column(Jobs::Id)
            .from(Jobs::Table)
            .and_where(Expr::col(Jobs::Status).eq(JobStatus::Pending.as_str()))
            .and_where(Expr::col(Jobs::RunAfter).lte(now.clone()))
            // Exhaustion is enforced where work is handed out, not where failure
            // is reported -- the same place graphile-worker puts it, and for the
            // same reason: `release_stale_locks` returns a job to the queue
            // without consulting anything, so a worker that leases a job and
            // dies would otherwise have it handed out forever.
            .and_where(Expr::col(Jobs::Attempt).lt(Expr::col(Jobs::MaxAttempts)))
            .order_by(Jobs::Priority, Order::Desc)
            // Then by when the job became due, not when it was created, which is
            // what makes the reclaim above mean anything and what graphile-worker
            // orders on. For a job enqueued normally the two are the same
            // instant, so ordinary work keeps its existing order; a job deferred
            // by a retry, or pushed back after a stalled lease, correctly waits
            // behind work that has been due for longer.
            .order_by(Jobs::RunAfter, Order::Asc)
            .order_by(Jobs::CreatedAt, Order::Asc)
            .limit(1);
        if !kinds.is_empty() {
            candidate.and_where(
                Expr::col(Jobs::Kind).is_in(kinds.iter().map(JobKind::as_str).collect::<Vec<_>>()),
            );
        }

        let stmt = Query::update()
            .table(Jobs::Table)
            .values([
                (Jobs::Status, JobStatus::Running.as_str().into()),
                (Jobs::LockedAt, now.clone().into()),
                (Jobs::LockedBy, worker_id.into()),
                (Jobs::Attempt, Expr::col(Jobs::Attempt).add(1)),
            ])
            .and_where(Expr::col(Jobs::Id).in_subquery(candidate))
            .returning(Query::returning().columns(job_columns()))
            .to_owned();

        let row = self
            .conn
            .query_one(&stmt)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        row.as_ref().map(map_job).transpose()
    }

    async fn renew_lock(&self, id: JobId, worker_id: &str) -> Result<bool, DbError> {
        let stmt = Query::update()
            .table(Jobs::Table)
            .value(Jobs::LockedAt, Utc::now().to_rfc3339())
            .and_where(Expr::col(Jobs::Id).eq(id.to_string()))
            .and_where(Expr::col(Jobs::LockedBy).eq(worker_id))
            .and_where(Expr::col(Jobs::Status).eq(JobStatus::Running.as_str()))
            .to_owned();

        let res = self
            .conn
            .execute(&stmt)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        Ok(res.rows_affected() > 0)
    }

    async fn mark_completed(&self, id: JobId) -> Result<(), DbError> {
        let now = Utc::now().to_rfc3339();
        let stmt = Query::update()
            .table(Jobs::Table)
            .values([
                (Jobs::Status, JobStatus::Completed.as_str().into()),
                (Jobs::CompletedAt, now.into()),
                (Jobs::LockedAt, opt(None)),
                (Jobs::LockedBy, opt(None)),
            ])
            .and_where(Expr::col(Jobs::Id).eq(id.to_string()))
            .to_owned();

        self.conn
            .execute(&stmt)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        Ok(())
    }

    async fn mark_failed(
        &self,
        id: JobId,
        error: &str,
        retry_delay: Option<Duration>,
    ) -> Result<(), DbError> {
        let now = Utc::now();
        // A delay means "try again later"; its absence means the job is done for.
        let (status, run_after) = match retry_delay {
            Some(delay) => {
                let when = now
                    + chrono::Duration::from_std(delay)
                        .unwrap_or_else(|_| chrono::Duration::seconds(30));
                (JobStatus::Pending, Some(when.to_rfc3339()))
            }
            None => (JobStatus::Failed, None),
        };

        let mut stmt = Query::update();
        stmt.table(Jobs::Table)
            .values([
                (Jobs::Status, status.as_str().into()),
                (Jobs::LastError, error.into()),
                (Jobs::LockedAt, opt(None)),
                (Jobs::LockedBy, opt(None)),
            ])
            .and_where(Expr::col(Jobs::Id).eq(id.to_string()));
        if let Some(when) = run_after {
            stmt.value(Jobs::RunAfter, when);
        } else {
            stmt.value(Jobs::CompletedAt, now.to_rfc3339());
        }

        self.conn
            .execute(&stmt)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        Ok(())
    }

    async fn release_stale_locks(&self, older_than: Duration) -> Result<u64, DbError> {
        let threshold = Utc::now()
            - chrono::Duration::from_std(older_than)
                .unwrap_or_else(|_| chrono::Duration::seconds(600));

        let stmt = Query::update()
            .table(Jobs::Table)
            .values([
                (Jobs::Status, JobStatus::Pending.as_str().into()),
                (Jobs::LockedAt, opt(None)),
                (Jobs::LockedBy, opt(None)),
                // Reclaimed work goes behind whatever arrived while it hung.
                // graphile-worker moves `run_at` for the same reason: a job that
                // keeps stalling would otherwise compete on equal terms with
                // fresh calls every time it is swept up, and starve them.
                (Jobs::RunAfter, Utc::now().to_rfc3339().into()),
            ])
            .and_where(Expr::col(Jobs::Status).eq(JobStatus::Running.as_str()))
            .and_where(Expr::col(Jobs::LockedAt).lte(threshold.to_rfc3339()))
            .to_owned();

        let res = self
            .conn
            .execute(&stmt)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        Ok(res.rows_affected())
    }

    async fn requeue_interrupted(&self, id: JobId, reason: &str) -> Result<(), DbError> {
        self.requeue(id, None, reason).await
    }

    async fn requeue_as(&self, id: JobId, kind: &JobKind, reason: &str) -> Result<(), DbError> {
        self.requeue(id, Some(kind), reason).await
    }

    async fn requeue_all_running(&self, reason: &str) -> Result<u64, DbError> {
        let stmt = Query::update()
            .table(Jobs::Table)
            .values([
                (Jobs::Status, JobStatus::Pending.as_str().into()),
                (
                    Jobs::Attempt,
                    Expr::cust("CASE WHEN attempt > 0 THEN attempt - 1 ELSE 0 END"),
                ),
                (Jobs::RunAfter, Utc::now().to_rfc3339().into()),
                (Jobs::LockedAt, opt(None)),
                (Jobs::LockedBy, opt(None)),
                (Jobs::LastError, reason.into()),
                (Jobs::CompletedAt, opt(None)),
            ])
            .and_where(Expr::col(Jobs::Status).eq(JobStatus::Running.as_str()))
            .to_owned();

        let res = self
            .conn
            .execute(&stmt)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        Ok(res.rows_affected())
    }

    async fn get_by_id(&self, id: JobId) -> Result<Option<Job>, DbError> {
        let stmt = Query::select()
            .columns(job_columns())
            .from(Jobs::Table)
            .and_where(Expr::col(Jobs::Id).eq(id.to_string()))
            .to_owned();

        let row = self
            .conn
            .query_one(&stmt)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        row.as_ref().map(map_job).transpose()
    }

    async fn list_by_call_id(&self, call_id: CallId) -> Result<Vec<Job>, DbError> {
        let stmt = Query::select()
            .columns(job_columns())
            .from(Jobs::Table)
            .and_where(Expr::col(Jobs::CallId).eq(call_id.to_string()))
            .order_by(Jobs::CreatedAt, Order::Desc)
            .to_owned();

        let rows = self
            .conn
            .query_all(&stmt)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        rows.iter().map(map_job).collect()
    }
}

impl SqlJobRepository {
    /// Return a job to the queue, optionally as a different kind.
    async fn requeue(
        &self,
        id: JobId,
        kind: Option<&JobKind>,
        reason: &str,
    ) -> Result<(), DbError> {
        let mut stmt = Query::update();
        stmt.table(Jobs::Table)
            .values([
                (Jobs::Status, JobStatus::Pending.as_str().into()),
                // Interrupted work should not cost the job an attempt.
                (
                    Jobs::Attempt,
                    Expr::cust("CASE WHEN attempt > 0 THEN attempt - 1 ELSE 0 END"),
                ),
                (Jobs::RunAfter, Utc::now().to_rfc3339().into()),
                (Jobs::LockedAt, opt(None)),
                (Jobs::LockedBy, opt(None)),
                (Jobs::LastError, reason.into()),
                (Jobs::CompletedAt, opt(None)),
            ])
            .and_where(Expr::col(Jobs::Id).eq(id.to_string()));
        if let Some(kind) = kind {
            stmt.value(Jobs::Kind, kind.as_str().to_string());
        }

        self.conn
            .execute(&stmt)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        Ok(())
    }
}
