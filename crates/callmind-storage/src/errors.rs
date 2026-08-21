use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("Storage I/O error at path '{path}': {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("File not found in storage: {0}")]
    NotFound(String),

    #[error("Invalid storage key: {0}")]
    InvalidKey(String),

    #[error("Storage quota or limit exceeded: {0}")]
    QuotaExceeded(String),
}
