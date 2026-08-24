//! Schema migrations, expressed once for every supported backend.
//!
//! Written with `sea-orm-migration`'s schema builder rather than raw SQL so
//! SQLite and Postgres are driven by the same code. Two things genuinely cannot
//! be expressed portably and are branched on explicitly:
//!
//! - Full-text search: SQLite has an FTS5 virtual table, Postgres has
//!   `tsvector` with a GIN index. No abstraction covers both.
//! - The generated `primary_language` column: SQLite's `ALTER TABLE` accepts
//!   only `VIRTUAL`, Postgres only `STORED`.
//!
//! The initial schema is deliberately one migration: the previous seven replayed
//! a development history — create a table, then add a column, then an index —
//! which is noise once the shape has settled. Anything added after databases
//! went live gets its own migration, because collapsing only stays honest while
//! nothing has shipped.

use sea_orm_migration::prelude::*;

mod m0001_initial_schema;
mod m0002_speaker_embeddings;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m0001_initial_schema::Migration),
            Box::new(m0002_speaker_embeddings::Migration),
        ]
    }
}
