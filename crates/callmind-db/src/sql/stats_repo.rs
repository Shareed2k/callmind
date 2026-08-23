//! Dashboard aggregates, one implementation for every backend.
//!
//! These are read-model queries: they exist to be displayed, never to be written
//! through. Kept apart from [`crate::traits::CallRepository`] for that reason.

use crate::errors::DbError;
use crate::traits::{CallStats, StatsRepository};
use async_trait::async_trait;
use callmind_core::CallId;
use sea_orm::sea_query::{Alias, Expr, ExprTrait, Order, Query};
use sea_orm::{ConnectionTrait, DatabaseConnection, DeriveIden, QueryResult, TryGetable};

#[derive(DeriveIden)]
enum Jobs {
    Table,
    CallId,
    Status,
    LastError,
    CreatedAt,
}

#[derive(DeriveIden)]
enum Calls {
    Table,
}

#[derive(DeriveIden)]
enum CallTranscripts {
    Table,
    PrimaryLanguage,
}

pub struct SqlStatsRepository {
    conn: DatabaseConnection,
}

impl SqlStatsRepository {
    pub fn new(conn: DatabaseConnection) -> Self {
        Self { conn }
    }
}

fn get<T: TryGetable>(row: &QueryResult, col: &str) -> Result<T, DbError> {
    row.try_get("", col)
        .map_err(|e| DbError::Query(e.to_string()))
}

/// `(label, count)` pairs from a grouped query.
fn counts(rows: &[QueryResult]) -> Result<Vec<(String, i64)>, DbError> {
    rows.iter()
        .map(|row| Ok((get::<String>(row, "label")?, get::<i64>(row, "total")?)))
        .collect()
}

#[async_trait]
impl StatsRepository for SqlStatsRepository {
    async fn job_counts_by_status(&self) -> Result<Vec<(String, i64)>, DbError> {
        // One grouped scan rather than a `count(*)` per status.
        let stmt = Query::select()
            .expr_as(Expr::col(Jobs::Status), Alias::new("label"))
            .expr_as(Expr::col(Jobs::Status).count(), Alias::new("total"))
            .from(Jobs::Table)
            .group_by_col(Jobs::Status)
            .to_owned();

        let rows = self
            .conn
            .query_all(&stmt)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        counts(&rows)
    }

    async fn call_stats(&self) -> Result<CallStats, DbError> {
        // The aggregates are NULL on an empty table, hence the Options. Written
        // as one row so the dashboard needs a single round trip.
        let stmt = Query::select()
            .expr_as(Expr::cust("count(*)"), Alias::new("total"))
            .expr_as(
                Expr::cust("sum(case when processing_status = 'completed' then 1 else 0 end)"),
                Alias::new("completed"),
            )
            // Cast both: SQLite types `sum()` over an INTEGER column as INTEGER
            // and Postgres types it as `numeric`, so neither decodes as `f64`
            // without help. `double precision` is spelled the same on both --
            // SQLite gives any type name containing "DOUB" REAL affinity.
            .expr_as(
                Expr::cust("cast(avg(duration_ms) as double precision)"),
                Alias::new("avg_ms"),
            )
            .expr_as(
                Expr::cust("cast(sum(duration_ms) as double precision)"),
                Alias::new("total_ms"),
            )
            .from(Calls::Table)
            .to_owned();

        let row = self
            .conn
            .query_one(&stmt)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?
            .ok_or_else(|| DbError::Query("aggregate returned no row".into()))?;

        Ok(CallStats {
            total: get::<i64>(&row, "total")?,
            completed: get::<Option<i64>>(&row, "completed")?.unwrap_or(0),
            avg_duration_ms: get::<Option<f64>>(&row, "avg_ms")?.unwrap_or(0.0),
            total_duration_ms: get::<Option<f64>>(&row, "total_ms")?.unwrap_or(0.0),
        })
    }

    async fn top_intents(&self, limit: u32) -> Result<Vec<(String, i64)>, DbError> {
        let intent = Alias::new("customer_intent");
        let stmt = Query::select()
            .expr_as(Expr::col(intent.clone()), Alias::new("label"))
            .expr_as(Expr::col(intent.clone()).count(), Alias::new("total"))
            .from(Alias::new("call_analyses"))
            .and_where(Expr::col(intent.clone()).is_not_null())
            .and_where(Expr::col(intent.clone()).ne(""))
            .group_by_col(intent)
            .order_by_expr(Expr::cust("count(*)"), Order::Desc)
            .limit(u64::from(limit))
            .to_owned();

        let rows = self
            .conn
            .query_all(&stmt)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        counts(&rows)
    }

    async fn daily_call_counts(&self, days: u32) -> Result<Vec<(String, i64)>, DbError> {
        // `substr` over an RFC 3339 string is the portable way to get the date
        // part: timestamps are stored as text on both backends.
        let day = Expr::cust("substr(created_at, 1, 10)");
        let stmt = Query::select()
            .expr_as(day.clone(), Alias::new("label"))
            .expr_as(Expr::cust("count(*)"), Alias::new("total"))
            .from(Calls::Table)
            .add_group_by([day.clone()])
            .order_by_expr(day, Order::Desc)
            .limit(u64::from(days))
            .to_owned();

        let rows = self
            .conn
            .query_all(&stmt)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        counts(&rows)
    }

    async fn language_distribution(&self) -> Result<Vec<(String, i64)>, DbError> {
        // Reads the indexed generated column instead of parsing every blob.
        let label = Expr::col(CallTranscripts::PrimaryLanguage);
        let stmt = Query::select()
            .expr_as(
                Expr::cust("coalesce(primary_language, 'unknown')"),
                Alias::new("label"),
            )
            .expr_as(Expr::cust("count(*)"), Alias::new("total"))
            .from(CallTranscripts::Table)
            .add_group_by([label])
            .to_owned();

        let rows = self
            .conn
            .query_all(&stmt)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        counts(&rows)
    }

    async fn last_job_error(&self, call_id: CallId) -> Result<Option<String>, DbError> {
        let stmt = Query::select()
            .column(Jobs::LastError)
            .from(Jobs::Table)
            .and_where(Expr::col(Jobs::CallId).eq(call_id.to_string()))
            .and_where(Expr::col(Jobs::Status).eq("failed"))
            .order_by(Jobs::CreatedAt, Order::Desc)
            .limit(1)
            .to_owned();

        let row = self
            .conn
            .query_one(&stmt)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        match row {
            Some(row) => get::<Option<String>>(&row, "last_error"),
            None => Ok(None),
        }
    }
}
