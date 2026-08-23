//! Persistence: repository traits, their implementations, and the migrations.
//!
//! Callers work through [`CallRepository`], [`JobRepository`],
//! [`StatsRepository`] and [`SearchIndex`] rather than touching SQL, so every
//! statement in the project lives in this crate.
//!
//! There is **one** implementation of each, not one per database. Queries are
//! built with `sea-query`, so the placeholder dialect and quoting come from the
//! builder, and the migration is a single `sea-orm-migration` definition driven
//! by the schema builder. Both SQLite and Postgres are exercised by the same
//! contract tests in `tests/`; Postgres joins in when
//! `CALLMIND_TEST_POSTGRES_URL` is set.
//!
//! Only three things have no portable form, and each is a named helper in
//! [`sql::support`] rather than a branch buried in a query: full-text matching
//! (FTS5 versus `tsvector`), JSON path extraction, and the full-text score and
//! snippet. Everything else is genuinely shared code.

pub mod errors;
pub mod migration;
pub mod pool;
pub mod sql;
pub mod traits;

pub use errors::*;
pub use pool::*;
pub use traits::*;

pub use sql::{SqlCallRepository, SqlJobRepository, SqlSearchIndex, SqlStatsRepository};
