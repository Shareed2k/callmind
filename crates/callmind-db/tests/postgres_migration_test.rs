//! The schema, applied to Postgres by the same migration code SQLite uses.
//!
//! Set `CALLMIND_TEST_POSTGRES_URL` to run it. The CI job and
//! `docker-compose.test.yml` provide a container; without the variable the test
//! skips, because a missing database is a missing environment rather than a
//! failing schema.
//!
//! What it is really guarding: the two pieces of DDL that have no portable form
//! — the full-text index and the generated `primary_language` column. Everything
//! else is produced by sea-orm's schema builder and would fail loudly.

use callmind_db::migration::Migrator;
use sea_orm_migration::MigratorTrait;
use sea_orm_migration::sea_orm::{ConnectionTrait, Database, DatabaseConnection, Statement};

/// Connect with a schema of this test's own.
///
/// The tests run in parallel against one database, so they cannot share
/// `public` — each resetting it would stomp on the others.
async fn connect(schema: &str) -> Option<DatabaseConnection> {
    // An empty value counts as unset: `export VAR=` is how a shell disables one.
    let url = std::env::var("CALLMIND_TEST_POSTGRES_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())?;

    let admin = Database::connect(&url)
        .await
        .unwrap_or_else(|e| panic!("CALLMIND_TEST_POSTGRES_URL is set but unreachable: {e}"));
    admin
        .execute_unprepared(&format!(
            "DROP SCHEMA IF EXISTS {schema} CASCADE; CREATE SCHEMA {schema};"
        ))
        .await
        .expect("create a private schema");

    // Set on the pool rather than per connection: sea-orm pools, so a bare
    // `SET search_path` would only affect whichever connection ran it.
    let mut options = sea_orm_migration::sea_orm::ConnectOptions::new(url);
    options.set_schema_search_path(schema);
    Some(
        Database::connect(options)
            .await
            .expect("connect with the private schema"),
    )
}

async fn scalar(conn: &DatabaseConnection, sql: &str) -> String {
    let row = conn
        .query_one_raw(Statement::from_string(conn.get_database_backend(), sql))
        .await
        .expect("query")
        .expect("a row");
    row.try_get_by_index::<String>(0).expect("a text column")
}

#[tokio::test]
async fn schema_applies_to_postgres() {
    const SCHEMA: &str = "t_schema";
    let Some(conn) = connect(SCHEMA).await else {
        eprintln!("skipping: CALLMIND_TEST_POSTGRES_URL is not set");
        return;
    };

    Migrator::up(&conn, None).await.expect("migrations apply");

    // Every table the repositories touch.
    for table in [
        "organizations",
        "calls",
        "call_recordings",
        "jobs",
        "call_transcripts",
        "call_analyses",
        "call_plugin_results",
        "fts_calls",
    ] {
        let found = scalar(
            &conn,
            &format!(
                "SELECT count(*)::text FROM information_schema.tables \
                 WHERE table_schema='{SCHEMA}' AND table_name='{table}'"
            ),
        )
        .await;
        assert_eq!(found, "1", "table {table} was not created");
    }

    // Re-running must be a no-op, not an error.
    Migrator::up(&conn, None)
        .await
        .expect("migrations idempotent");

    // The default organization the whole app assumes.
    let org = scalar(
        &conn,
        "SELECT name FROM organizations WHERE id='00000000-0000-0000-0000-000000000001'",
    )
    .await;
    assert_eq!(org, "Default Organization");
}

/// SQLite gets a VIRTUAL generated column, Postgres only allows STORED. Both
/// must extract the same value from the same transcript JSON.
#[tokio::test]
async fn generated_language_column_works_on_postgres() {
    const SCHEMA: &str = "t_generated";
    let Some(conn) = connect(SCHEMA).await else {
        eprintln!("skipping: CALLMIND_TEST_POSTGRES_URL is not set");
        return;
    };
    Migrator::up(&conn, None).await.expect("migrations apply");

    conn.execute_unprepared(
        r#"
        INSERT INTO calls (id, organization_id, direction, processing_status, created_at, updated_at)
        VALUES ('11111111-1111-1111-1111-111111111111',
                '00000000-0000-0000-0000-000000000001',
                'incoming', 'completed', 'now', 'now');
        INSERT INTO call_transcripts (call_id, transcript_json, created_at)
        VALUES ('11111111-1111-1111-1111-111111111111',
                '{"call_id":"x","languages":[{"language":"Hebrew","probability":0.9}],"speakers":[],"segments":[]}',
                'now');
        "#,
    )
    .await
    .expect("seed");

    // Lower-cased, matching what the SQLite expression produces.
    let lang = scalar(
        &conn,
        "SELECT primary_language FROM call_transcripts \
         WHERE call_id='11111111-1111-1111-1111-111111111111'",
    )
    .await;
    assert_eq!(lang, "hebrew");
}

/// Postgres replaces FTS5 with a generated `tsvector` and a GIN index. The
/// search semantics established for SQLite have to hold here too, including the
/// Hebrew proclitic expansion, which is query-side and therefore shared.
#[tokio::test]
async fn full_text_search_works_on_postgres() {
    const SCHEMA: &str = "t_fts";
    let Some(conn) = connect(SCHEMA).await else {
        eprintln!("skipping: CALLMIND_TEST_POSTGRES_URL is not set");
        return;
    };
    Migrator::up(&conn, None).await.expect("migrations apply");

    conn.execute_unprepared(
        r#"
        INSERT INTO fts_calls (call_id, organization_id, title, summary, transcript)
        VALUES ('c1', 'o1', 'השיחה', 'הלקוח ביקש בהזמנה', 'שלום'),
               ('c2', 'o1', 'Телефонный разговор', 'обсуждение счетов', 'привет');
        "#,
    )
    .await
    .expect("seed");

    async fn matches(conn: &DatabaseConnection, q: &str) -> String {
        let sql = format!(
            "SELECT count(*)::text FROM fts_calls WHERE document @@ to_tsquery('simple', '{q}')"
        );
        scalar(conn, &sql).await
    }

    // Russian inflects by suffix, so a prefix term finds it.
    assert_eq!(matches(&conn, "разгов:*").await, "1", "russian stem");
    // Hebrew attaches its article to the front, so the bare stem needs the
    // proclitic form OR'd in — exactly as `sanitize_fts5_query` emits.
    assert_eq!(
        matches(&conn, "שיחה:*").await,
        "0",
        "bare hebrew stem alone"
    );
    assert_eq!(
        matches(&conn, "שיחה:* | השיחה:*").await,
        "1",
        "proclitic expansion is what makes the hebrew stem work"
    );
    assert_eq!(matches(&conn, "zzznope:*").await, "0");
}
