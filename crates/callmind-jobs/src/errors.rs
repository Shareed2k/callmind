use thiserror::Error;

#[derive(Debug, Error)]
pub enum JobExecutionError {
    #[error("Job processing failed: {0}")]
    Failed(String),

    #[error("Job retryable error: {0}")]
    Retryable(String),

    #[error("Job handler not registered for kind: {0}")]
    HandlerNotFound(String),

    #[error("Job cancelled")]
    Cancelled,
}
