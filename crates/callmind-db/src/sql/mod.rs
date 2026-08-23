//! Backend-agnostic repository implementations.
//!
//! Built with `sea-query`, so one definition produces correct SQL for every
//! backend sea-orm supports instead of a file per database.

pub mod call_repo;
pub mod job_repo;
pub mod search_index;
pub mod stats_repo;
pub mod support;

pub use call_repo::SqlCallRepository;
pub use job_repo::SqlJobRepository;
pub use search_index::SqlSearchIndex;
pub use stats_repo::SqlStatsRepository;
