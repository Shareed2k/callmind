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
//! History is deliberately collapsed into one migration. The previous seven
//! replayed a development history — create a table, then add a column, then an
//! index — which is noise once the shape has settled.

use sea_orm_migration::prelude::*;

mod m0001_initial_schema;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(m0001_initial_schema::Migration)]
    }
}
