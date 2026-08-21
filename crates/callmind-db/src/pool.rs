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

/// Run all embedded migrations against the database.
pub async fn run_migrations(pool: &SqlitePool) -> Result<(), DbError> {
    info!("Running database migrations...");
    sqlx::migrate!("./migrations")
        .run(pool)
        .await
        .map_err(DbError::Migration)?;
    info!("Database migrations applied successfully");
    Ok(())
}
