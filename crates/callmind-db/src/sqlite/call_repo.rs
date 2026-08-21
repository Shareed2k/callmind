use crate::errors::DbError;
use crate::traits::CallRepository;
use async_trait::async_trait;
use callmind_core::{
    Call, CallDirection, CallFilter, CallId, OrgId, ProcessingStatus, Recording, RecordingId,
};
use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};
use std::str::FromStr;

#[derive(Debug, Clone)]
pub struct SqliteCallRepository {
    pool: SqlitePool,
}

impl SqliteCallRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl CallRepository for SqliteCallRepository {
    async fn create(&self, call: &Call) -> Result<(), DbError> {
        let id_str = call.id.to_string();
        let org_id_str = call.organization_id.to_string();
        let direction_str = call.direction.as_str();
        let status_str = call.processing_status.as_str();
        let started_at_str = call.started_at.map(|dt| dt.to_rfc3339());
        let ended_at_str = call.ended_at.map(|dt| dt.to_rfc3339());
        let created_at_str = call.created_at.to_rfc3339();
        let updated_at_str = call.updated_at.to_rfc3339();
        let duration_ms = call.duration_ms.map(|d| d as i64);
        let is_favorite = i32::from(call.is_favorite);
        let tags_json = serde_json::to_string(&call.tags).unwrap_or_else(|_| "[]".to_string());

        sqlx::query(
            r#"
            INSERT INTO calls (
                id, organization_id, external_id, direction, phone_from, phone_to,
                started_at, ended_at, duration_ms, processing_status, is_favorite, tags, created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(id_str)
        .bind(org_id_str)
        .bind(&call.external_id)
        .bind(direction_str)
        .bind(&call.phone_from)
        .bind(&call.phone_to)
        .bind(started_at_str)
        .bind(ended_at_str)
        .bind(duration_ms)
        .bind(status_str)
        .bind(is_favorite)
        .bind(tags_json)
        .bind(created_at_str)
        .bind(updated_at_str)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            if let sqlx::Error::Database(ref db_err) = e {
                if db_err.is_unique_violation() {
                    return DbError::DuplicateKey(db_err.message().to_string());
                }
            }
            DbError::Sqlx(e)
        })?;

        Ok(())
    }

    async fn get_by_id(&self, id: CallId) -> Result<Option<Call>, DbError> {
        let id_str = id.to_string();
        let row = sqlx::query(
            r#"
            SELECT id, organization_id, external_id, direction, phone_from, phone_to,
                   started_at, ended_at, duration_ms, processing_status, is_favorite, tags, created_at, updated_at
            FROM calls
            WHERE id = ?
            "#,
        )
        .bind(id_str)
        .fetch_optional(&self.pool)
        .await?;

        row.map(map_call_row).transpose()
    }

    async fn get_by_external_id(
        &self,
        org_id: OrgId,
        ext_id: &str,
    ) -> Result<Option<Call>, DbError> {
        let org_id_str = org_id.to_string();
        let row = sqlx::query(
            r#"
            SELECT id, organization_id, external_id, direction, phone_from, phone_to,
                   started_at, ended_at, duration_ms, processing_status, is_favorite, tags, created_at, updated_at
            FROM calls
            WHERE organization_id = ? AND external_id = ?
            "#,
        )
        .bind(org_id_str)
        .bind(ext_id)
        .fetch_optional(&self.pool)
        .await?;

        row.map(map_call_row).transpose()
    }

    async fn list(&self, filter: &CallFilter) -> Result<Vec<Call>, DbError> {
        let mut query = String::from(
            r#"
            SELECT id, organization_id, external_id, direction, phone_from, phone_to,
                   started_at, ended_at, duration_ms, processing_status, is_favorite, tags, created_at, updated_at
            FROM calls
            WHERE 1=1
            "#,
        );

        if filter.organization_id.is_some() {
            query.push_str(" AND organization_id = ?");
        }
        if filter.external_id.is_some() {
            query.push_str(" AND external_id = ?");
        }
        if filter.status.is_some() {
            query.push_str(" AND processing_status = ?");
        }
        if filter.direction.is_some() {
            query.push_str(" AND direction = ?");
        }
        if filter.from_date.is_some() {
            query.push_str(" AND created_at >= ?");
        }
        if filter.to_date.is_some() {
            query.push_str(" AND created_at <= ?");
        }

        query.push_str(" ORDER BY is_favorite DESC, created_at DESC LIMIT ? OFFSET ?");

        let mut q = sqlx::query(&query);

        if let Some(org_id) = filter.organization_id {
            q = q.bind(org_id.to_string());
        }
        if let Some(ref ext_id) = filter.external_id {
            q = q.bind(ext_id);
        }
        if let Some(status) = filter.status {
            q = q.bind(status.as_str());
        }
        if let Some(dir) = filter.direction {
            q = q.bind(dir.as_str());
        }
        if let Some(from) = filter.from_date {
            q = q.bind(from.to_rfc3339());
        }
        if let Some(to) = filter.to_date {
            q = q.bind(to.to_rfc3339());
        }

        let limit = filter.limit.unwrap_or(50) as i64;
        let offset = filter.offset.unwrap_or(0) as i64;
        q = q.bind(limit).bind(offset);

        let rows = q.fetch_all(&self.pool).await?;
        rows.into_iter().map(map_call_row).collect()
    }

    async fn toggle_favorite(&self, id: CallId) -> Result<bool, DbError> {
        let id_str = id.to_string();
        let now = Utc::now().to_rfc3339();
        let row: Option<(i32,)> = sqlx::query_as("SELECT is_favorite FROM calls WHERE id = ?")
            .bind(&id_str)
            .fetch_optional(&self.pool)
            .await?;

        let Some((current,)) = row else {
            return Err(DbError::NotFound(format!("Call {id} not found")));
        };

        let new_val = i32::from(current == 0);

        sqlx::query("UPDATE calls SET is_favorite = ?, updated_at = ? WHERE id = ?")
            .bind(new_val)
            .bind(&now)
            .bind(&id_str)
            .execute(&self.pool)
            .await?;

        Ok(new_val != 0)
    }

    async fn update_tags(&self, id: CallId, tags: &[String]) -> Result<(), DbError> {
        let id_str = id.to_string();
        let tags_json = serde_json::to_string(tags).unwrap_or_else(|_| "[]".to_string());
        let now = Utc::now().to_rfc3339();

        let res = sqlx::query("UPDATE calls SET tags = ?, updated_at = ? WHERE id = ?")
            .bind(&tags_json)
            .bind(&now)
            .bind(&id_str)
            .execute(&self.pool)
            .await?;

        if res.rows_affected() == 0 {
            return Err(DbError::NotFound(format!("Call {id} not found")));
        }

        Ok(())
    }

    async fn update_status(&self, id: CallId, status: ProcessingStatus) -> Result<(), DbError> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"
            UPDATE calls
            SET processing_status = ?, updated_at = ?
            WHERE id = ?
            "#,
        )
        .bind(status.as_str())
        .bind(now)
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn delete(&self, id: CallId) -> Result<bool, DbError> {
        let res = sqlx::query("DELETE FROM calls WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;

        Ok(res.rows_affected() > 0)
    }

    async fn add_recording(&self, recording: &Recording) -> Result<(), DbError> {
        let id_str = recording.id.to_string();
        let call_id_str = recording.call_id.to_string();
        let created_at_str = recording.created_at.to_rfc3339();

        sqlx::query(
            r#"
            INSERT INTO call_recordings (
                id, call_id, storage_key, mime_type, file_size_bytes,
                sha256, duration_ms, channels, sample_rate, created_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(id_str)
        .bind(call_id_str)
        .bind(&recording.storage_key)
        .bind(&recording.mime_type)
        .bind(recording.file_size_bytes as i64)
        .bind(&recording.sha256)
        .bind(recording.duration_ms.map(|d| d as i64))
        .bind(recording.channels.map(|c| c as i32))
        .bind(recording.sample_rate.map(|r| r as i64))
        .bind(created_at_str)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn get_recording_by_call_id(
        &self,
        call_id: CallId,
    ) -> Result<Option<Recording>, DbError> {
        let call_id_str = call_id.to_string();
        let row = sqlx::query(
            r#"
            SELECT id, call_id, storage_key, mime_type, file_size_bytes,
                   sha256, duration_ms, channels, sample_rate, created_at
            FROM call_recordings
            WHERE call_id = ?
            "#,
        )
        .bind(call_id_str)
        .fetch_optional(&self.pool)
        .await?;

        row.map(map_recording_row).transpose()
    }
}

fn map_call_row(row: sqlx::sqlite::SqliteRow) -> Result<Call, DbError> {
    let id_str: String = row.get("id");
    let org_id_str: String = row.get("organization_id");
    let external_id: Option<String> = row.get("external_id");
    let direction_str: String = row.get("direction");
    let phone_from: Option<String> = row.get("phone_from");
    let phone_to: Option<String> = row.get("phone_to");
    let started_at_str: Option<String> = row.get("started_at");
    let ended_at_str: Option<String> = row.get("ended_at");
    let duration_ms: Option<i64> = row.get("duration_ms");
    let status_str: String = row.get("processing_status");
    let is_favorite: bool = row
        .try_get::<i32, _>("is_favorite")
        .map(|v| v != 0)
        .unwrap_or(false);
    let tags_json: String = row
        .try_get::<String, _>("tags")
        .unwrap_or_else(|_| "[]".to_string());
    let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();
    let created_at_str: String = row.get("created_at");
    let updated_at_str: String = row.get("updated_at");

    let id = CallId::from_str(&id_str).map_err(|e| DbError::NotFound(e.to_string()))?;
    let organization_id =
        OrgId::from_str(&org_id_str).map_err(|e| DbError::NotFound(e.to_string()))?;
    let direction = CallDirection::from_str(&direction_str).unwrap_or(CallDirection::Unknown);
    let processing_status =
        ProcessingStatus::from_str(&status_str).unwrap_or(ProcessingStatus::Pending);

    let started_at = started_at_str.and_then(|s| {
        DateTime::parse_from_rfc3339(&s)
            .ok()
            .map(|dt| dt.with_timezone(&Utc))
    });
    let ended_at = ended_at_str.and_then(|s| {
        DateTime::parse_from_rfc3339(&s)
            .ok()
            .map(|dt| dt.with_timezone(&Utc))
    });
    let created_at = DateTime::parse_from_rfc3339(&created_at_str)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    let updated_at = DateTime::parse_from_rfc3339(&updated_at_str)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());

    Ok(Call {
        id,
        organization_id,
        external_id,
        direction,
        phone_from,
        phone_to,
        started_at,
        ended_at,
        duration_ms: duration_ms.map(|d| d as u64),
        processing_status,
        is_favorite,
        tags,
        created_at,
        updated_at,
    })
}

fn map_recording_row(row: sqlx::sqlite::SqliteRow) -> Result<Recording, DbError> {
    let id_str: String = row.get("id");
    let call_id_str: String = row.get("call_id");
    let storage_key: String = row.get("storage_key");
    let mime_type: String = row.get("mime_type");
    let file_size_bytes: i64 = row.get("file_size_bytes");
    let sha256: String = row.get("sha256");
    let duration_ms: Option<i64> = row.get("duration_ms");
    let channels: Option<i32> = row.get("channels");
    let sample_rate: Option<i64> = row.get("sample_rate");
    let created_at_str: String = row.get("created_at");

    let id = RecordingId::from_str(&id_str).map_err(|e| DbError::NotFound(e.to_string()))?;
    let call_id = CallId::from_str(&call_id_str).map_err(|e| DbError::NotFound(e.to_string()))?;
    let created_at = DateTime::parse_from_rfc3339(&created_at_str)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());

    Ok(Recording {
        id,
        call_id,
        storage_key,
        mime_type,
        file_size_bytes: file_size_bytes as u64,
        sha256,
        duration_ms: duration_ms.map(|d| d as u64),
        channels: channels.map(|c| c as u16),
        sample_rate: sample_rate.map(|r| r as u32),
        created_at,
    })
}
