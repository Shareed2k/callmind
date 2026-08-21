use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AudioError {
    #[error("Audio file I/O error at '{path}': {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Audio decode error: {0}")]
    Decode(String),

    #[error("Unsupported audio codec or format: {0}")]
    UnsupportedFormat(String),

    #[error("Resampling failed: {0}")]
    Resample(String),

    #[error("Channel operation error: {0}")]
    Channel(String),

    #[error("Empty audio stream or buffer")]
    EmptyAudio,
}
