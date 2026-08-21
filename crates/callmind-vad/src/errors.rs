use thiserror::Error;

#[derive(Debug, Error)]
pub enum VadError {
    #[error("VAD processing error: {0}")]
    Processing(String),

    #[error("Empty audio supplied to VAD")]
    EmptyAudio,
}
