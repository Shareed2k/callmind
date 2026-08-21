use thiserror::Error;

#[derive(Debug, Error)]
pub enum DbError {
    #[error("Database query failed: {0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("Database migration failed: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),

    #[error("Record not found: {0}")]
    NotFound(String),

    #[error("Duplicate key violation: {0}")]
    DuplicateKey(String),

    #[error("Constraint violation: {0}")]
    ConstraintViolation(String),

    #[error("Serialization / Deserialization error: {0}")]
    Serde(#[from] serde_json::Error),
}
