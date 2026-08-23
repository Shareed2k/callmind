use crate::errors::DbError;
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;
use tracing::info;

/// Create and configure a SQLite connection pool with production-ready pragmas.
pub async fn create_sqlite_pool(
    database_url: &str,
    max_connections: u32,
) -> Result<SqlitePool, DbError> {
    // If it's a file path, ensure the parent directory exists
    if database_url != ":memory:" && !database_url.starts_with("sqlite::memory:") {
        let clean_path = database_url
            .trim_start_matches("sqlite://")
            .trim_start_matches("sqlite:");
        if let Some(parent) = Path::new(clean_path).parent()
            && !parent.as_os_str().is_empty()
        {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                DbError::NotFound(format!(
                    "Failed to create DB directory {}: {e}",
                    parent.display()
                ))
            })?;
        }
    }

    let mut connect_options = if database_url == ":memory:" {
        SqliteConnectOptions::from_str("sqlite::memory:")?
    } else {
        SqliteConnectOptions::from_str(database_url)?.create_if_missing(true)
    };

    connect_options = connect_options
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));

    let pool = SqlitePoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(Duration::from_secs(10))
        .connect_with(connect_options)
        .await?;

    info!("SQLite connection pool initialized successfully");
    Ok(pool)
}

/// Open the database named by the config, whichever backend it is.
///
/// The repositories take a `DatabaseConnection` and build their SQL with
/// `sea-query`, so this is the only place that has to know which backend is in
/// use. SQLite still goes through [`create_sqlite_pool`] because its pragmas
/// matter -- WAL, a busy timeout, create-if-missing -- and Postgres needs none
/// of that.
pub async fn connect(
    driver: &str,
    url: &str,
    max_connections: u32,
) -> Result<sea_orm_migration::sea_orm::DatabaseConnection, DbError> {
    match driver.to_lowercase().as_str() {
        "sqlite" => Ok(orm_connection(
            &create_sqlite_pool(url, max_connections).await?,
        )),
        "postgres" | "postgresql" => {
            let mut options = sea_orm_migration::sea_orm::ConnectOptions::new(url.to_string());
            options
                .max_connections(max_connections)
                .acquire_timeout(Duration::from_secs(10));
            let conn = sea_orm_migration::sea_orm::Database::connect(options)
                .await
                .map_err(|e| DbError::Query(format!("Postgres connection failed: {e}")))?;
            info!("Postgres connection pool initialized successfully");
            Ok(conn)
        }
        other => Err(DbError::NotFound(format!(
            "Unsupported database driver: '{other}'. Supported: 'sqlite', 'postgres'."
        ))),
    }
}

/// Adopt an existing sqlx pool as a sea-orm connection.
///
/// The backend-agnostic repositories take a `DatabaseConnection`, but the
/// process already owns a configured sqlx pool (WAL, busy timeout, connection
/// cap). Wrapping it keeps that to one pool per process instead of opening a
/// second one alongside it.
#[must_use]
pub fn orm_connection(pool: &SqlitePool) -> sea_orm_migration::sea_orm::DatabaseConnection {
    sea_orm_migration::sea_orm::SqlxSqliteConnector::from_sqlx_sqlite_pool(pool.clone())
}

/// Apply the schema, driven by one migration definition for every backend.
///
/// Adopts the caller's existing sqlx pool rather than opening a second
/// connection, so there is one pool per process.
pub async fn run_migrations(pool: &SqlitePool) -> Result<(), DbError> {
    info!("Running database migrations...");
    run_migrations_on(&orm_connection(pool)).await
}

/// Apply the schema to any backend sea-orm can reach.
///
/// Separate from [`run_migrations`] so a Postgres deployment, and the test that
/// exercises one, share exactly the same migration code path.
pub async fn run_migrations_on(
    conn: &sea_orm_migration::sea_orm::DatabaseConnection,
) -> Result<(), DbError> {
    use sea_orm_migration::MigratorTrait;
    crate::migration::Migrator::up(conn, None)
        .await
        .map_err(|e| DbError::MigrationFailed(e.to_string()))?;
    info!("Database migrations applied successfully");
    Ok(())
}
