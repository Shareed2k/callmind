//! Full-text index, one implementation over two genuinely different engines.
//!
//! This is the only part of the schema where the backends do not merely spell
//! things differently — they use different machinery: an FTS5 virtual table with
//! `MATCH`, `bm25()` and `snippet()`, versus a `tsvector` column with `@@`,
//! `ts_rank()` and `ts_headline()`. Even the sort direction differs.
//!
//! It is still one implementation rather than two, because the *structure* is
//! identical: the same nine-column row, the same filters, the same join to
//! `calls`. Only the match predicate, the score and the snippet vary, and each of
//! those is one named helper in [`super::support`].

use super::support::{fts_direct_match, fts_highlight, fts_ranking, get, parse_ts};
use crate::errors::DbError;
use crate::traits::{IndexDocument, SearchHit, SearchIndex, SearchQuery};
use async_trait::async_trait;
use callmind_core::CallId;
use sea_orm::sea_query::{Alias, Expr, ExprTrait, JoinType, Order, Query};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, DeriveIden};
use std::str::FromStr;

#[derive(DeriveIden)]
enum FtsCalls {
    Table,
    CallId,
    OrganizationId,
    Title,
    Summary,
    Transcript,
    Topics,
    Entities,
    Reason,
    Resolution,
}

#[derive(DeriveIden)]
enum Calls {
    Table,
    Id,
    Direction,
    ProcessingStatus,
    CreatedAt,
}

#[derive(Debug, Clone)]
pub struct SqlSearchIndex {
    conn: DatabaseConnection,
}

impl SqlSearchIndex {
    pub fn new(conn: DatabaseConnection) -> Self {
        Self { conn }
    }

    fn backend(&self) -> DbBackend {
        self.conn.get_database_backend()
    }
}

#[async_trait]
impl SearchIndex for SqlSearchIndex {
    async fn index(&self, doc: &IndexDocument<'_>) -> Result<(), DbError> {
        // Delete then insert rather than upsert: an FTS5 virtual table has no
        // `ON CONFLICT` to hang one on, and this keeps both backends on the same
        // path.
        self.delete(doc.call_id).await?;

        let stmt = Query::insert()
            .into_table(FtsCalls::Table)
            .columns([
                FtsCalls::CallId,
                FtsCalls::OrganizationId,
                FtsCalls::Title,
                FtsCalls::Summary,
                FtsCalls::Transcript,
                FtsCalls::Topics,
                FtsCalls::Entities,
                FtsCalls::Reason,
                FtsCalls::Resolution,
            ])
            .values([
                doc.call_id.to_string().into(),
                doc.org_id.to_string().into(),
                doc.title.into(),
                doc.summary.into(),
                doc.transcript.into(),
                doc.topics.join(" ").into(),
                doc.entities.join(" ").into(),
                doc.reason.unwrap_or("").into(),
                doc.resolution.unwrap_or("").into(),
            ])
            .map_err(|e| DbError::Query(e.to_string()))?
            .to_owned();

        self.conn
            .execute(&stmt)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        Ok(())
    }

    async fn update_title(&self, call_id: CallId, title: &str) -> Result<(), DbError> {
        // On Postgres the `tsvector` is a generated column, so it re-derives
        // itself; on SQLite the FTS5 row is re-indexed by the update.
        let stmt = Query::update()
            .table(FtsCalls::Table)
            .value(FtsCalls::Title, title)
            .and_where(Expr::col(FtsCalls::CallId).eq(call_id.to_string()))
            .to_owned();
        self.conn
            .execute(&stmt)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        Ok(())
    }

    async fn delete(&self, call_id: CallId) -> Result<(), DbError> {
        let stmt = Query::delete()
            .from_table(FtsCalls::Table)
            .and_where(Expr::col(FtsCalls::CallId).eq(call_id.to_string()))
            .to_owned();
        self.conn
            .execute(&stmt)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        Ok(())
    }

    async fn search(&self, query: &SearchQuery<'_>) -> Result<Vec<SearchHit>, DbError> {
        let text = query.text.trim();
        let backend = self.backend();
        // A query that expands to no terms matches nothing; running it would
        // either error or return the whole table depending on the backend.
        let Some(matches) = fts_direct_match(backend, text) else {
            return Ok(Vec::new());
        };

        let ranking = fts_ranking(backend, text);
        let mut stmt = Query::select();
        // `fts_calls` is deliberately not aliased: FTS5's `bm25()` and
        // `snippet()` take the table's own name, and an alias changes what they
        // resolve to.
        stmt.expr_as(
            Expr::col((FtsCalls::Table, FtsCalls::CallId)),
            Alias::new("hit_call_id"),
        )
        .expr_as(
            Expr::col((FtsCalls::Table, FtsCalls::Title)),
            Alias::new("title"),
        )
        .expr_as(
            Expr::col((FtsCalls::Table, FtsCalls::Summary)),
            Alias::new("summary"),
        )
        .expr_as(fts_highlight(backend, text), Alias::new("match_highlight"))
        .expr_as(ranking.expr.clone(), Alias::new("rank"))
        .expr_as(Expr::col(("c", Calls::CreatedAt)), Alias::new("created_at"))
        .from(FtsCalls::Table)
        .join_as(
            JoinType::InnerJoin,
            Calls::Table,
            Alias::new("c"),
            Expr::col(("c", Calls::Id)).equals((FtsCalls::Table, FtsCalls::CallId)),
        )
        .and_where(matches);

        if let Some(org_id) = query.organization_id {
            stmt.and_where(
                Expr::col((FtsCalls::Table, FtsCalls::OrganizationId)).eq(org_id.to_string()),
            );
        }
        if let Some(from) = query.from_date {
            stmt.and_where(Expr::col(("c", Calls::CreatedAt)).gte(from.to_rfc3339()));
        }
        if let Some(to) = query.to_date {
            stmt.and_where(Expr::col(("c", Calls::CreatedAt)).lte(to.to_rfc3339()));
        }
        if let Some(dir) = query.direction {
            stmt.and_where(Expr::col(("c", Calls::Direction)).eq(dir));
        }
        if let Some(status) = query.status {
            stmt.and_where(Expr::col(("c", Calls::ProcessingStatus)).eq(status));
        }

        let stmt = stmt
            .order_by_expr(ranking.expr, ranking.order)
            .order_by(("c", Calls::CreatedAt), Order::Desc)
            .limit(u64::from(query.limit))
            .offset(u64::from(query.offset))
            .to_owned();

        let rows = self
            .conn
            .query_all(&stmt)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;

        rows.iter()
            .map(|row| {
                let id_str: String = get(row, "hit_call_id")?;
                let created_at: String = get(row, "created_at")?;
                Ok(SearchHit {
                    call_id: CallId::from_str(&id_str)
                        .map_err(|e| DbError::NotFound(e.to_string()))?,
                    title: get(row, "title")?,
                    summary: get(row, "summary")?,
                    match_highlight: get(row, "match_highlight").unwrap_or_default(),
                    // `bm25()` is NULL for a non-matching row, which a plain
                    // decode would refuse.
                    rank: get::<Option<f64>>(row, "rank")?.unwrap_or(0.0),
                    created_at: parse_ts(&created_at),
                })
            })
            .collect()
    }
}
