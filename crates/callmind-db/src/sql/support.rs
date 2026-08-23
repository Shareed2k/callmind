//! Pieces every backend-agnostic repository needs.
//!
//! Also the one honest place for the two things that have **no** portable form:
//! full-text matching and JSON path extraction. Keeping them here means the
//! branch is a named function with a comment, not a surprise in the middle of a
//! query.

use crate::errors::DbError;
use chrono::{DateTime, Utc};
use sea_orm::sea_query::{Alias, BinOper, Expr, ExprTrait, Func, Order, SelectStatement};
use sea_orm::{DbBackend, QueryResult, TryGetable};

/// Read one column, mapping sea-orm's error into ours.
pub fn get<T: TryGetable>(row: &QueryResult, col: &str) -> Result<T, DbError> {
    row.try_get("", col)
        .map_err(|e| DbError::Query(e.to_string()))
}

/// Timestamps are stored as RFC 3339 text on every backend, so parsing is shared.
///
/// Text rather than a native timestamp type is deliberate: it keeps ordering,
/// comparison and the generated columns identical everywhere, at the cost of a
/// few bytes per row.
pub fn parse_ts(raw: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(raw)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

/// `WHERE` predicate restricting `id_expr` to the calls a free-text query matches.
///
/// This is the one query shape with no portable spelling: SQLite matches an FTS5
/// virtual table with `MATCH`, Postgres matches a `tsvector` column with `@@`
/// through its GIN index. Both take the *same* expanded term list from
/// [`callmind_core::search_query`], rendered into each dialect -- which is what
/// makes Hebrew proclitic search behave identically on the two.
///
/// Returns `None` for a query that expands to nothing, so callers can skip the
/// predicate rather than match everything or nothing by accident.
///
/// Built from real `sea-query` expressions rather than `Expr::cust_with_values`
/// on purpose. That function's placeholder marker is **dialect-specific** -- the
/// SQLite builder substitutes `?` and the Postgres builder substitutes `$1` --
/// and the mismatched one is not an error: the marker stays in the SQL verbatim
/// and its value is dropped from the bind list. On SQLite that then reads as
/// named parameter 1 and the *previous* bind lands in the `MATCH`.
pub fn fts_matches(backend: DbBackend, id_expr: Expr, query: &str) -> Option<Expr> {
    let mut sub = SelectStatement::new();
    sub.column(Alias::new("call_id"))
        .from(Alias::new("fts_calls"));

    match backend {
        DbBackend::Sqlite => {
            let rendered = callmind_core::search_query::to_fts5(query);
            if rendered.is_empty() {
                return None;
            }
            // `<table> MATCH <query>` is FTS5's own operator, so it has to go in
            // as a custom binary op; the query itself is still a bound value.
            sub.and_where(
                Expr::cust("fts_calls").binary(BinOper::Custom("MATCH"), Expr::val(rendered)),
            );
        }
        _ => {
            let rendered = callmind_core::search_query::to_tsquery(query);
            if rendered.is_empty() {
                return None;
            }
            sub.and_where(
                Expr::col(Alias::new("document")).binary(
                    BinOper::Custom("@@"),
                    Expr::FunctionCall(
                        Func::cust(Alias::new("to_tsquery"))
                            // The dictionary must be a literal `regconfig`: the
                            // two-argument `to_tsquery` has no `(text, text)`
                            // overload, so a bound parameter does not resolve to
                            // any function at all. It is a constant in this file,
                            // never user input.
                            .arg(Expr::cust("'simple'::regconfig"))
                            .arg(rendered),
                    ),
                ),
            );
        }
    }

    Some(id_expr.in_subquery(sub))
}

/// The match predicate for a query whose `FROM` clause is `fts_calls` itself.
///
/// Distinct from [`fts_matches`] and not interchangeable with it. FTS5's
/// auxiliary functions -- `bm25()` and `snippet()` -- only have match state to
/// report when the query *itself* constrains the FTS table. Reaching the same
/// rows through `id IN (SELECT ... MATCH ...)` leaves the outer query with none:
/// `snippet()` then returns the column verbatim with no markup and `bm25()`
/// returns nothing useful, so results come back correct but unranked and
/// unhighlighted, with no error anywhere.
///
/// Returns `None` when the query expands to no terms.
pub fn fts_direct_match(backend: DbBackend, query: &str) -> Option<Expr> {
    match backend {
        DbBackend::Sqlite => {
            let rendered = callmind_core::search_query::to_fts5(query);
            if rendered.is_empty() {
                return None;
            }
            Some(Expr::cust("fts_calls").binary(BinOper::Custom("MATCH"), Expr::val(rendered)))
        }
        _ => {
            let rendered = callmind_core::search_query::to_tsquery(query);
            if rendered.is_empty() {
                return None;
            }
            Some(
                Expr::col(Alias::new("document")).binary(
                    BinOper::Custom("@@"),
                    Expr::FunctionCall(
                        Func::cust(Alias::new("to_tsquery"))
                            .arg(Expr::cust("'simple'::regconfig"))
                            .arg(rendered),
                    ),
                ),
            )
        }
    }
}

/// How a backend ranks a full-text match, and which way the score sorts.
pub struct FtsRanking {
    pub expr: Expr,
    pub order: Order,
}

/// The relevance score for a full-text match.
///
/// The two backends disagree about the *direction* as well as the function:
/// FTS5's `bm25()` returns a negative score where more negative is better, so it
/// sorts ascending, while Postgres's `ts_rank` sorts descending. Getting the
/// direction wrong returns the worst matches first and looks like a broken
/// index rather than a bug.
pub fn fts_ranking(backend: DbBackend, query: &str) -> FtsRanking {
    match backend {
        DbBackend::Sqlite => FtsRanking {
            expr: Expr::cust("cast(bm25(fts_calls) as double precision)"),
            order: Order::Asc,
        },
        _ => FtsRanking {
            // Cast for the same reason as the aggregates: `ts_rank` returns
            // `real`, which will not decode as `f64`.
            // `$1` is right here and `?` would be wrong -- the opposite of the
            // `fts_matches` case -- because this branch is only ever rendered by
            // the Postgres builder. See the note there: the marker belongs to
            // the dialect, and the mismatched one fails silently.
            expr: Expr::cust_with_values(
                "cast(ts_rank(document, to_tsquery('simple'::regconfig, $1)) as double precision)",
                [callmind_core::search_query::to_tsquery(query)],
            ),
            order: Order::Desc,
        },
    }
}

/// A snippet of the matched transcript with the hit marked up.
///
/// `snippet()` is FTS5's, `ts_headline()` is Postgres's; column 4 of the FTS5
/// table is `transcript`, which is what both are asked to excerpt.
pub fn fts_highlight(backend: DbBackend, query: &str) -> Expr {
    match backend {
        DbBackend::Sqlite => Expr::cust("snippet(fts_calls, 4, '<b>', '</b>', '...', 15)"),
        _ => Expr::cust_with_values(
            "ts_headline('simple'::regconfig, transcript, \
             to_tsquery('simple'::regconfig, $1), \
             'StartSel=<b>, StopSel=</b>, MaxWords=18, MinWords=1')",
            [callmind_core::search_query::to_tsquery(query)],
        ),
    }
}

/// Extract a string from a JSON text column at a fixed path.
///
/// `path` is a `$.`-style JSON path, and it is a **literal from this codebase**,
/// never user input — it is interpolated, so it must stay that way.
pub fn json_text(backend: DbBackend, column: &str, path: &[&str]) -> Expr {
    match backend {
        DbBackend::Sqlite => {
            let joined = path
                .iter()
                .map(|seg| {
                    if seg.chars().all(|c| c.is_ascii_digit()) {
                        format!("[{seg}]")
                    } else {
                        format!(".{seg}")
                    }
                })
                .collect::<String>();
            Expr::cust(format!("json_extract({column}, '${joined}')"))
        }
        _ => {
            // Postgres needs the last step to be `->>` to get text rather than
            // a JSON scalar, which would arrive quoted.
            use std::fmt::Write as _;
            let mut expr = format!("{column}::jsonb");
            for (i, seg) in path.iter().enumerate() {
                let arrow = if i + 1 == path.len() { "->>" } else { "->" };
                // An array index is bare; an object key is quoted.
                if seg.chars().all(|c| c.is_ascii_digit()) {
                    let _ = write!(expr, " {arrow} {seg}");
                } else {
                    let _ = write!(expr, " {arrow} '{seg}'");
                }
            }
            Expr::cust(expr)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::sea_query::{PostgresQueryBuilder, Query, SqliteQueryBuilder};

    #[test]
    fn json_path_renders_per_dialect() {
        let render = |backend| {
            let e = json_text(backend, "t.transcript_json", &["segments", "0", "text"]);
            Query::select()
                .expr(e)
                .to_owned()
                .to_string(SqliteQueryBuilder)
        };
        assert!(
            render(DbBackend::Sqlite)
                .contains("json_extract(t.transcript_json, '$.segments[0].text')"),
            "{}",
            render(DbBackend::Sqlite)
        );
        assert!(
            render(DbBackend::Postgres)
                .contains("t.transcript_json::jsonb -> 'segments' -> 0 ->> 'text'"),
            "{}",
            render(DbBackend::Postgres)
        );
    }

    /// The query text must travel as a bound parameter on both backends, not
    /// baked into the SQL.
    #[test]
    fn fts_query_is_a_bound_parameter() {
        for backend in [DbBackend::Sqlite, DbBackend::Postgres] {
            let expr = fts_matches(backend, Expr::col(Alias::new("id")), "שיחה").expect("terms");
            // Built the way it is used, and rendered by the builder that matches
            // the backend -- which is the whole point of the exercise.
            let mut stmt = Query::select();
            stmt.expr(Expr::cust("count(*)"))
                .from(Alias::new("calls"))
                .and_where(expr);
            let (sql, values) = if backend == DbBackend::Sqlite {
                stmt.clone().build(SqliteQueryBuilder)
            } else {
                stmt.clone().build(PostgresQueryBuilder)
            };
            assert!(
                !sql.contains("שיחה"),
                "{backend:?} interpolated the query: {sql}"
            );
            // The expanded term list must arrive as a bound value. Postgres
            // binds the `'simple'` configuration name alongside it, so the
            // assertion is on presence rather than on a count.
            assert!(
                values.0.iter().any(|v| matches!(
                    v,
                    sea_orm::Value::String(Some(s)) if s.contains("שיחה")
                )),
                "{backend:?} did not bind the query: {:?}",
                values.0
            );
        }
        let id = || Expr::col(Alias::new("id"));
        assert!(fts_matches(DbBackend::Sqlite, id(), "  ").is_none());
        assert!(fts_matches(DbBackend::Postgres, id(), "  ").is_none());
    }

    /// A regression test for a placeholder that looked right and was not.
    ///
    /// `cust_with_values` substitutes `?`. Writing `$1` leaves the text as-is,
    /// drops the value, and SQLite then treats `$1` as named parameter 1 -- so
    /// the *preceding* bind lands in the `MATCH`, and the failure surfaces as an
    /// FTS5 syntax error about a character the user never typed.
    #[test]
    fn fts_placeholder_is_substituted_alongside_other_binds() {
        let mut any = Expr::col(("c", Alias::new("external_id"))).like("%ext-1%");
        any = any.or(fts_matches(
            DbBackend::Sqlite,
            Expr::col(("c", Alias::new("id"))),
            "hello",
        )
        .expect("terms"));

        let (sql, values) = Query::select()
            .expr(Expr::cust("count(*)"))
            .from_as(Alias::new("calls"), Alias::new("c"))
            .and_where(any)
            .to_owned()
            .build(SqliteQueryBuilder);

        assert!(
            !sql.contains('$'),
            "unsubstituted placeholder left in {sql}"
        );
        assert_eq!(
            values.0.len(),
            2,
            "both the LIKE pattern and the query must be bound: {:?}",
            values.0
        );
    }
}
