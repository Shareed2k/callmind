//! Full-text search and question answering over indexed calls.
//!
//! Holds no SQL and no database driver: the index sits behind
//! [`callmind_db::SearchIndex`], which is what lets the same search run over
//! SQLite FTS5 and a Postgres `tsvector`. What lives here is the API-facing
//! shape — the request filter, the result item, and the LLM-backed
//! [`AskEngine`] built on top of retrieval.
//!
//! Query expansion is [`callmind_core::search_query`]: tokens become prefix
//! terms so a stem matches an inflected form, and Hebrew tokens are also emitted
//! with each proclitic attached, because no full-text engine's prefix term
//! matches `השיחה` from `שיחה`.

pub mod ask;
pub mod engine;
pub mod errors;
pub mod models;

pub use ask::AskEngine;
pub use engine::sanitize_fts5_query;
pub use engine::{IndexCallParams, SearchEngine};
pub use errors::SearchError;
pub use models::*;
