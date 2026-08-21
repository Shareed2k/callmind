use crate::errors::StorageError;
use crate::traits::{ByteStream, RecordingStorage, StoragePutResult};
use async_trait::async_trait;
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tokio::fs::{self, File};
use tokio::io::AsyncWriteExt;

/// Local filesystem implementation of `RecordingStorage`.
#[derive(Debug, Clone)]
pub struct FilesystemStorage {
    base_dir: PathBuf,
}

impl FilesystemStorage {
    /// Create a new `FilesystemStorage` instance at the given base directory.
    pub async fn new<P: AsRef<Path>>(base_dir: P) -> Result<Self, StorageError> {
        let base_path = base_dir.as_ref().to_path_buf();
        fs::create_dir_all(&base_path)
            .await
            .map_err(|e| StorageError::Io {
                path: base_path.clone(),
                source: e,
            })?;

        let tmp_dir = base_path.join("tmp");
        fs::create_dir_all(&tmp_dir)
            .await
            .map_err(|e| StorageError::Io {
                path: tmp_dir,
                source: e,
            })?;

        Ok(Self {
            base_dir: base_path,
        })
    }

    /// Resolve a sanitized storage key to an absolute filesystem path.
    fn resolve_path(&self, key: &str) -> Result<PathBuf, StorageError> {
        if key.contains("..") || key.starts_with('/') || key.starts_with('\\') {
            return Err(StorageError::InvalidKey(format!(
                "Key '{key}' contains illegal path traversal components"
            )));
        }
        Ok(self.base_dir.join(key))
    }
}

#[async_trait]
impl RecordingStorage for FilesystemStorage {
    async fn put(
        &self,
        key: &str,
        mut stream: ByteStream,
    ) -> Result<StoragePutResult, StorageError> {
        let dest_path = self.resolve_path(key)?;

        if let Some(parent) = dest_path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| StorageError::Io {
                    path: parent.to_path_buf(),
                    source: e,
                })?;
        }

        let tmp_path = self
            .base_dir
            .join("tmp")
            .join(format!("{}.tmp", uuid::Uuid::new_v4()));

        let mut file = File::create(&tmp_path)
            .await
            .map_err(|e| StorageError::Io {
                path: tmp_path.clone(),
                source: e,
            })?;

        let mut hasher = Sha256::new();
        let mut total_bytes: u64 = 0;

        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result.map_err(|e| StorageError::Io {
                path: tmp_path.clone(),
                source: e,
            })?;

            hasher.update(&chunk);
            total_bytes += chunk.len() as u64;

            file.write_all(&chunk).await.map_err(|e| StorageError::Io {
                path: tmp_path.clone(),
                source: e,
            })?;
        }

        file.flush().await.map_err(|e| StorageError::Io {
            path: tmp_path.clone(),
            source: e,
        })?;

        // Atomic rename from temp file to final destination
        fs::rename(&tmp_path, &dest_path)
            .await
            .map_err(|e| StorageError::Io {
                path: dest_path.clone(),
                source: e,
            })?;

        let sha256_hex = hex::encode(hasher.finalize());

        Ok(StoragePutResult {
            size_bytes: total_bytes,
            sha256: sha256_hex,
        })
    }

    async fn get(&self, key: &str) -> Result<File, StorageError> {
        let path = self.resolve_path(key)?;
        if !path.exists() {
            return Err(StorageError::NotFound(format!("File at {key} not found")));
        }

        File::open(&path)
            .await
            .map_err(|e| StorageError::Io { path, source: e })
    }

    async fn get_local_path(&self, key: &str) -> Result<PathBuf, StorageError> {
        let path = self.resolve_path(key)?;
        if !path.exists() {
            return Err(StorageError::NotFound(format!("File at {key} not found")));
        }
        Ok(path)
    }

    async fn delete(&self, key: &str) -> Result<(), StorageError> {
        let path = self.resolve_path(key)?;
        if path.exists() {
            fs::remove_file(&path)
                .await
                .map_err(|e| StorageError::Io { path, source: e })?;
        }
        Ok(())
    }

    async fn exists(&self, key: &str) -> Result<bool, StorageError> {
        let path = self.resolve_path(key)?;
        Ok(path.exists())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use futures_util::stream;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_filesystem_storage_lifecycle() {
        let dir = tempdir().unwrap();
        let storage = FilesystemStorage::new(dir.path()).await.unwrap();

        let data = b"RIFF....WAVEfmt ....test audio data";
        let chunks: Vec<Result<Bytes, std::io::Error>> = vec![
            Ok(Bytes::from_static(&data[..10])),
            Ok(Bytes::from_static(&data[10..])),
        ];
        let stream = Box::pin(stream::iter(chunks));

        let put_res = storage.put("org_1/call_1.wav", stream).await.unwrap();
        assert_eq!(put_res.size_bytes, data.len() as u64);
        assert!(!put_res.sha256.is_empty());

        assert!(storage.exists("org_1/call_1.wav").await.unwrap());

        let mut file = storage.get("org_1/call_1.wav").await.unwrap();
        use tokio::io::AsyncReadExt;
        let mut read_buf = Vec::new();
        file.read_to_end(&mut read_buf).await.unwrap();
        assert_eq!(read_buf, data);

        storage.delete("org_1/call_1.wav").await.unwrap();
        assert!(!storage.exists("org_1/call_1.wav").await.unwrap());
    }
}
