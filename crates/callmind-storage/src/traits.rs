use crate::errors::StorageError;
use async_trait::async_trait;
use bytes::Bytes;
use futures_util::Stream;
use std::path::PathBuf;
use std::pin::Pin;
use tokio::fs::File;

/// Result of storing an audio stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoragePutResult {
    pub size_bytes: u64,
    pub sha256: String,
}

pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static>>;

/// High-level trait for audio recording storage backend (Filesystem, S3, etc.).
#[async_trait]
pub trait RecordingStorage: Send + Sync {
    /// Streams audio data into storage, computing SHA-256 and total bytes on the fly.
    async fn put(&self, key: &str, stream: ByteStream) -> Result<StoragePutResult, StorageError>;

    /// Opens an audio file for reading (supports seek / range requests).
    async fn get(&self, key: &str) -> Result<File, StorageError>;

    /// Returns the absolute filesystem path if available locally.
    async fn get_local_path(&self, key: &str) -> Result<PathBuf, StorageError>;

    /// Deletes a recording by key.
    async fn delete(&self, key: &str) -> Result<(), StorageError>;

    /// Checks if a recording exists.
    async fn exists(&self, key: &str) -> Result<bool, StorageError>;
}
