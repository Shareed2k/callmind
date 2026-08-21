use thiserror::Error;

#[derive(Debug, Error)]
pub enum LanguageError {
    #[error("Language detection failed: {0}")]
    Detection(String),

    #[error("Empty audio supplied for language detection")]
    EmptyAudio,
}
