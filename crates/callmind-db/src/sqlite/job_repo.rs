use crate::errors::DbError;
use crate::traits::JobRepository;
use async_trait::async_trait;
use callmind_core::{CallId, EnqueueJob, Job, JobId, JobKind, JobStatus};
use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};
use std::str::FromStr;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct SqliteJobRepository {
    pool: SqlitePool,
}

impl SqliteJobRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl JobRepository for SqliteJobRepository {
    async fn enqueue(&self, job: &EnqueueJob) -> Result<JobId, DbError> {
        let job_id = JobId::generate();
        let id_str = job_id.to_string();
        let call_id_str = job.call_id.map(|id| id.to_string());
        let kind_str = job.kind.as_str();
        let payload_str = serde_json::to_string(&job.payload)?;
        let status_str = JobStatus::Pending.as_str();
        let now = Utc::now();
        let run_after = job.run_after.unwrap_or(now);
        let created_at_str = now.to_rfc3339();
        let run_after_str = run_after.to_rfc3339();

        sqlx::query(
            r#"
            INSERT INTO jobs (
                id, call_id, kind, payload, status, priority,
                attempt, max_attempts, run_after, created_at
            ) VALUES (?, ?, ?, ?, ?, ?, 0, ?, ?, ?)
            "#,
        )
        .bind(id_str)
        .bind(call_id_str)
        .bind(kind_str)
        .bind(payload_str)
        .bind(status_str)
        .bind(job.priority)
        .bind(job.max_attempts)
        .bind(run_after_str)
        .bind(created_at_str)
        .execute(&self.pool)
        .await?;

        Ok(job_id)
    }

    async fn fetch_and_lock(
        &self,
        worker_id: &str,
        kinds: &[JobKind],
    ) -> Result<Option<Job>, DbError> {
        let now = Utc::now();
        let now_str = now.to_rfc3339();

        let mut subquery = String::from(
            r#"
            SELECT id
            FROM jobs
            WHERE status = 'pending' AND run_after <= ?
            "#,
        );

        if !kinds.is_empty() {
            subquery.push_str(" AND kind IN (");
            for (i, _) in kinds.iter().enumerate() {
                if i > 0 {
                    subquery.push_str(", ");
                }
                subquery.push('?');
            }
            subquery.push(')');
        }

        subquery.push_str(" ORDER BY priority DESC, created_at ASC LIMIT 1");

        let update_query = format!(
            r#"
            UPDATE jobs
            SET status = 'running', locked_at = ?, locked_by = ?, attempt = attempt + 1
            WHERE id = ({subquery})
            RETURNING id, call_id, kind, payload, status, priority,
                      attempt, max_attempts, run_after, locked_at, locked_by,
                      last_error, created_at, completed_at
            "#
        );

        let mut q = sqlx::query(&update_query)
            .bind(&now_str)
            .bind(worker_id)
            .bind(&now_str);

        for kind in kinds {
            q = q.bind(kind.as_str());
        }

        let maybe_row = q.fetch_optional(&self.pool).await?;
        maybe_row.map(map_job_row).transpose()
    }

    async fn renew_lock(&self, id: JobId, worker_id: &str) -> Result<bool, DbError> {
        let now = Utc::now().to_rfc3339();
        let res = sqlx::query(
            r#"
            UPDATE jobs
            SET locked_at = ?
            WHERE id = ? AND status = 'running' AND locked_by = ?
            "#,
        )
        .bind(now)
        .bind(id.to_string())
        .bind(worker_id)
        .execute(&self.pool)
        .await?;

        Ok(res.rows_affected() > 0)
    }

    async fn mark_completed(&self, id: JobId) -> Result<(), DbError> {
        let now_str = Utc::now().to_rfc3339();
        sqlx::query(
            r#"
            UPDATE jobs
            SET status = 'completed', completed_at = ?, locked_at = NULL, locked_by = NULL
            WHERE id = ?
            "#,
        )
        .bind(now_str)
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn mark_failed(
        &self,
        id: JobId,
        error: &str,
        retry_delay: Option<Duration>,
    ) -> Result<(), DbError> {
        let id_str = id.to_string();
        let now = Utc::now();

        if let Some(delay) = retry_delay {
            let chrono_delay =
                chrono::Duration::from_std(delay).unwrap_or_else(|_| chrono::Duration::seconds(30));
            let next_run = now + chrono_delay;
            sqlx::query(
                r#"
                UPDATE jobs
                SET status = 'pending', run_after = ?, last_error = ?, locked_at = NULL, locked_by = NULL
                WHERE id = ?
                "#,
            )
            .bind(next_run.to_rfc3339())
            .bind(error)
            .bind(id_str)
            .execute(&self.pool)
            .await?;
        } else {
            sqlx::query(
                r#"
                UPDATE jobs
                SET status = 'failed', completed_at = ?, last_error = ?, locked_at = NULL, locked_by = NULL
                WHERE id = ?
                "#,
            )
            .bind(now.to_rfc3339())
            .bind(error)
            .bind(id_str)
            .execute(&self.pool)
            .await?;
        }

        Ok(())
    }

    async fn release_stale_locks(&self, older_than: Duration) -> Result<u64, DbError> {
        let chrono_duration = chrono::Duration::from_std(older_than)
            .unwrap_or_else(|_| chrono::Duration::seconds(600));
        let threshold = Utc::now() - chrono_duration;
        let threshold_str = threshold.to_rfc3339();

        let res = sqlx::query(
            r#"
            UPDATE jobs
            SET status = 'pending', locked_at = NULL, locked_by = NULL
            WHERE status = 'running' AND locked_at <= ?
            "#,
        )
        .bind(threshold_str)
        .execute(&self.pool)
        .await?;

        Ok(res.rows_affected())
    }

    async fn get_by_id(&self, id: JobId) -> Result<Option<Job>, DbError> {
        let row = sqlx::query(
            r#"
            SELECT id, call_id, kind, payload, status, priority,
                   attempt, max_attempts, run_after, locked_at, locked_by,
                   last_error, created_at, completed_at
            FROM jobs
            WHERE id = ?
            "#,
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;

        row.map(map_job_row).transpose()
    }

    async fn list_by_call_id(&self, call_id: CallId) -> Result<Vec<Job>, DbError> {
        let rows = sqlx::query(
            r#"
            SELECT id, call_id, kind, payload, status, priority,
                   attempt, max_attempts, run_after, locked_at, locked_by,
                   last_error, created_at, completed_at
            FROM jobs
            WHERE call_id = ?
            ORDER BY created_at ASC
            "#,
        )
        .bind(call_id.to_string())
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(map_job_row).collect()
    }
}

fn map_job_row(row: sqlx::sqlite::SqliteRow) -> Result<Job, DbError> {
    let id_str: String = row.get("id");
    let call_id_str: Option<String> = row.get("call_id");
    let kind_str: String = row.get("kind");
    let payload_str: String = row.get("payload");
    let status_str: String = row.get("status");
    let priority: i32 = row.get("priority");
    let attempt: i32 = row.get("attempt");
    let max_attempts: i32 = row.get("max_attempts");
    let run_after_str: String = row.get("run_after");
    let locked_at_str: Option<String> = row.get("locked_at");
    let locked_by: Option<String> = row.get("locked_by");
    let last_error: Option<String> = row.get("last_error");
    let created_at_str: String = row.get("created_at");
    let completed_at_str: Option<String> = row.get("completed_at");

    let id = JobId::from_str(&id_str).map_err(|e| DbError::NotFound(e.to_string()))?;
    let call_id = call_id_str.and_then(|s| CallId::from_str(&s).ok());
    let kind = JobKind::from_str(&kind_str).map_err(DbError::NotFound)?;
    let payload: serde_json::Value = serde_json::from_str(&payload_str)?;
    let status = JobStatus::from_str(&status_str).unwrap_or(JobStatus::Pending);

    let run_after = DateTime::parse_from_rfc3339(&run_after_str)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    let locked_at = locked_at_str.and_then(|s| {
        DateTime::parse_from_rfc3339(&s)
            .ok()
            .map(|dt| dt.with_timezone(&Utc))
    });
    let created_at = DateTime::parse_from_rfc3339(&created_at_str)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    let completed_at = completed_at_str.and_then(|s| {
        DateTime::parse_from_rfc3339(&s)
            .ok()
            .map(|dt| dt.with_timezone(&Utc))
    });

    Ok(Job {
        id,
        call_id,
        kind,
        payload,
        status,
        priority,
        attempt,
        max_attempts,
        run_after,
        locked_at,
        locked_by,
        last_error,
        created_at,
        completed_at,
    })
}
