use thiserror::Error;

/// Core domain error types.
#[derive(Debug, Error)]
pub enum DomainError {
    #[error("Resource not found: {0}")]
    NotFound(String),

    #[error("Resource already exists: {0}")]
    AlreadyExists(String),

    #[error("Validation failed: {0}")]
    Validation(String),

    #[error("Invalid state transition: {0}")]
    InvalidState(String),

    #[error("Internal domain error: {0}")]
    Internal(String),
}
