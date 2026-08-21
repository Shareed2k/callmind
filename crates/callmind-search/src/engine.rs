use crate::errors::SearchError;
use crate::models::{SearchFilter, SearchResultItem};
use callmind_core::{CallId, OrgId};
use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};
use std::str::FromStr;

/// Input payload for indexing a conversation in the search index.
pub struct IndexCallParams<'a> {
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

/// Full-text search engine powered by SQLite FTS5.
#[derive(Debug, Clone)]
pub struct SearchEngine {
    pool: SqlitePool,
}

impl SearchEngine {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Index or update a call in the FTS5 search index.
    pub async fn index_call(&self, params: IndexCallParams<'_>) -> Result<(), SearchError> {
        let call_id_str = params.call_id.to_string();
        let org_id_str = params.org_id.to_string();
        let topics_str = params.topics.join(" ");
        let entities_str = params.entities.join(" ");

        // First remove existing entry if updating
        let _ = sqlx::query("DELETE FROM fts_calls WHERE call_id = ?")
            .bind(&call_id_str)
            .execute(&self.pool)
            .await;

        sqlx::query(
            r#"
            INSERT INTO fts_calls (
                call_id, organization_id, title, summary, transcript,
                topics, entities, reason, resolution
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&call_id_str)
        .bind(&org_id_str)
        .bind(params.title)
        .bind(params.summary)
        .bind(params.transcript)
        .bind(topics_str)
        .bind(entities_str)
        .bind(params.reason.unwrap_or(""))
        .bind(params.resolution.unwrap_or(""))
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Search indexed calls using FTS5 match query and metadata filters.
    pub async fn search(
        &self,
        filter: &SearchFilter,
    ) -> Result<Vec<SearchResultItem>, SearchError> {
        let raw_query = filter.query.trim();
        if raw_query.is_empty() {
            return Ok(Vec::new());
        }

        let sanitized_query = sanitize_fts5_query(raw_query);
        if sanitized_query.is_empty() {
            return Ok(Vec::new());
        }

        let mut query = String::from(
            r#"
            SELECT fts.call_id, fts.title, fts.summary,
                   snippet(fts_calls, 4, '<b>', '</b>', '...', 15) AS match_highlight,
                   bm25(fts_calls) AS rank,
                   c.created_at
            FROM fts_calls fts
            JOIN calls c ON c.id = fts.call_id
            WHERE fts_calls MATCH ?
            "#,
        );

        if filter.organization_id.is_some() {
            query.push_str(" AND fts.organization_id = ?");
        }
        if filter.from_date.is_some() {
            query.push_str(" AND c.created_at >= ?");
        }
        if filter.to_date.is_some() {
            query.push_str(" AND c.created_at <= ?");
        }
        if filter.direction.is_some() {
            query.push_str(" AND c.direction = ?");
        }
        if filter.status.is_some() {
            query.push_str(" AND c.processing_status = ?");
        }

        query.push_str(" ORDER BY rank ASC LIMIT ? OFFSET ?");

        let mut q = sqlx::query(&query).bind(&sanitized_query);

        if let Some(org_id) = filter.organization_id {
            q = q.bind(org_id.to_string());
        }
        if let Some(from) = filter.from_date {
            q = q.bind(from.to_rfc3339());
        }
        if let Some(to) = filter.to_date {
            q = q.bind(to.to_rfc3339());
        }
        if let Some(dir) = filter.direction {
            q = q.bind(dir.as_str());
        }
        if let Some(st) = filter.status {
            q = q.bind(st.as_str());
        }

        let limit = i64::from(filter.limit.unwrap_or(20));
        let offset = i64::from(filter.offset.unwrap_or(0));
        q = q.bind(limit).bind(offset);

        let rows = q.fetch_all(&self.pool).await?;
        let mut results = Vec::with_capacity(rows.len());

        for row in rows {
            let id_str: String = row.get("call_id");
            let title: String = row.get("title");
            let summary: String = row.get("summary");
            let match_highlight: String = row.get("match_highlight");
            let rank: f64 = row.get("rank");
            let created_at_str: String = row.get("created_at");

            let call_id =
                CallId::from_str(&id_str).map_err(|e| SearchError::InvalidQuery(e.to_string()))?;
            let created_at = DateTime::parse_from_rfc3339(&created_at_str)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());

            results.push(SearchResultItem {
                call_id,
                title,
                summary,
                match_highlight,
                rank,
                created_at,
            });
        }

        Ok(results)
    }

    /// Delete call from search index.
    pub async fn delete_index(&self, call_id: CallId) -> Result<(), SearchError> {
        sqlx::query("DELETE FROM fts_calls WHERE call_id = ?")
            .bind(call_id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

/// Sanitize user input query for SQLite FTS5 prefix and phrase matching.
fn sanitize_fts5_query(query: &str) -> String {
    let tokens: Vec<&str> = query
        .split(|c: char| {
            c.is_whitespace() || c == '"' || c == '*' || c == ':' || c == '(' || c == ')'
        })
        .filter(|s| !s.is_empty())
        .collect();

    if tokens.is_empty() {
        return String::new();
    }

    // Join with OR prefix search
    tokens
        .iter()
        .map(|t| format!("\"{t}\"*"))
        .collect::<Vec<_>>()
        .join(" OR ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_query() {
        assert_eq!(
            sanitize_fts5_query("הזמנה דחופה"),
            "\"הזמנה\"* OR \"דחופה\"*"
        );
        assert_eq!(
            sanitize_fts5_query("отмена заказа"),
            "\"отмена\"* OR \"заказа\"*"
        );
    }
}
