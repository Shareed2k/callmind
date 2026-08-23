//! A migrated database per backend, shared by the contract tests.
//!
//! Postgres joins in only when `CALLMIND_TEST_POSTGRES_URL` is set, and each
//! test gets a schema of its own so they can run in parallel.

use callmind_db::migration::Migrator;
use sea_orm::{ConnectionTrait, Database, DatabaseConnection};
use sea_orm_migration::MigratorTrait;

pub async fn all(schema: &str) -> Vec<(&'static str, DatabaseConnection)> {
    let mut out = Vec::new();

    let sqlite = Database::connect("sqlite::memory:")
        .await
        .expect("in-memory sqlite");
    Migrator::up(&sqlite, None)
        .await
        .expect("sqlite migrations");
    out.push(("sqlite", sqlite));

    // An empty value counts as unset: `export VAR=` is how a shell disables one,
    // and treating it as a connection string turns a skip into six failures.
    if let Some(url) = std::env::var("CALLMIND_TEST_POSTGRES_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())
    {
        let admin = Database::connect(&url).await.expect("postgres reachable");
        admin
            .execute_unprepared(&format!(
                "DROP SCHEMA IF EXISTS {schema} CASCADE; CREATE SCHEMA {schema};"
            ))
            .await
            .expect("private schema");

        let mut options = sea_orm::ConnectOptions::new(url);
        options.set_schema_search_path(schema);
        let pg = Database::connect(options).await.expect("postgres connect");
        Migrator::up(&pg, None).await.expect("postgres migrations");
        out.push(("postgres", pg));
    }

    out
}

/// Insert a call directly, for tests that only care about aggregates.
///
/// This module is compiled into each integration test binary separately, so a
/// helper only some of them use reads as dead code in the rest.
#[allow(dead_code)]
pub async fn seed_call(
    conn: &DatabaseConnection,
    status: &str,
    duration_ms: Option<i64>,
) -> uuid::Uuid {
    let id = uuid::Uuid::new_v4();
    let duration = duration_ms.map_or_else(|| "NULL".to_string(), |v| v.to_string());
    conn.execute_unprepared(&format!(
        "INSERT INTO calls (id, organization_id, direction, processing_status, duration_ms, \
         created_at, updated_at) VALUES ('{id}', '00000000-0000-0000-0000-000000000001', \
         'incoming', '{status}', {duration}, '2026-08-23T10:00:00Z', '2026-08-23T10:00:00Z')"
    ))
    .await
    .expect("seed call");
    id
}
