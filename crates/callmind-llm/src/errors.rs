use thiserror::Error;

#[derive(Debug, Error)]
pub enum LlmError {
    #[error("LLM inference failed: {0}")]
    Inference(String),

    #[error("LLM provider error: {0}")]
    Provider(String),

    #[error("Structured JSON parsing failed: {0}")]
    JsonParse(#[from] serde_json::Error),

    #[error("Model context length exceeded")]
    ContextLengthExceeded,

    #[error("LLM backend timeout")]
    Timeout,
}
