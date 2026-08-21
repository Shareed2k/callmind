use thiserror::Error;

#[derive(Debug, Error)]
pub enum SearchError {
    #[error("Database query failed during search: {0}")]
    Database(#[from] sqlx::Error),

    #[error("LLM inference error during Ask Calls: {0}")]
    Llm(#[from] callmind_llm::LlmError),

    #[error("Serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Invalid search query: {0}")]
    InvalidQuery(String),
}
