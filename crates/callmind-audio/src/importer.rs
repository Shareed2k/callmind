use crate::decoder::AudioDecoder;
use crate::errors::AudioError;
use callmind_core::{
    Call, CallDirection, CallFilenameParser, EnqueueJob, JobKind, OrgId, Recording,
};
use callmind_db::{CallRepository, JobRepository};
use callmind_storage::RecordingStorage;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::path::Path;
use std::sync::Arc;
use tokio_util::io::ReaderStream;

/// Summary statistics of a batch import operation.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BatchImportSummary {
    pub scanned_files: usize,
    pub imported_calls: usize,
    pub skipped_existing: usize,
    pub failed_files: usize,
    pub total_duration_secs: f64,
    pub mono_calls: usize,
    pub stereo_calls: usize,
}

/// Utility for importing directories of call recordings into CallMind.
pub struct BatchImporter;

impl BatchImporter {
    /// Scan a directory and import all supported audio recordings.
    pub async fn import_directory<P: AsRef<Path>>(
        dir_path: P,
        call_repo: Arc<dyn CallRepository>,
        job_repo: Option<Arc<dyn JobRepository>>,
        storage: Arc<dyn RecordingStorage>,
        org_id: OrgId,
        limit: Option<usize>,
    ) -> Result<BatchImportSummary, AudioError> {
        let path = dir_path.as_ref();
        if !path.is_dir() {
            return Err(AudioError::Io {
                path: path.to_path_buf(),
                source: std::io::Error::new(std::io::ErrorKind::NotFound, "Not a directory"),
            });
        }

        let mut entries = Vec::new();
        let read_dir = std::fs::read_dir(path).map_err(|e| AudioError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;

        for entry in read_dir.flatten() {
            let p = entry.path();
            if p.is_file() {
                if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                    let ext_lower = ext.to_lowercase();
                    if matches!(
                        ext_lower.as_str(),
                        "m4a" | "wav" | "mp3" | "ogg" | "flac" | "aac"
                    ) {
                        entries.push(p);
                    }
                }
            }
        }

        // Sort alphabetically
        entries.sort();

        let max_files = limit.unwrap_or(entries.len()).min(entries.len());
        let mut summary = BatchImportSummary {
            scanned_files: max_files,
            ..Default::default()
        };

        for file_path in entries.into_iter().take(max_files) {
            let filename = file_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown_call");

            // Check if call with external_id == filename already exists
            if let Ok(Some(_)) = call_repo.get_by_external_id(org_id, filename).await {
                summary.skipped_existing += 1;
                continue;
            }

            // Decode metadata using Symphonia
            let decoded = match AudioDecoder::decode_file(&file_path) {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!(
                        "Skipping corrupt or unreadable audio file {:?}: {e}",
                        file_path
                    );
                    summary.failed_files += 1;
                    continue;
                }
            };

            let duration_ms = decoded.duration_ms();
            let channels = decoded.channels;
            let sample_rate = decoded.sample_rate;

            if channels == 1 {
                summary.mono_calls += 1;
            } else {
                summary.stereo_calls += 1;
            }
            summary.total_duration_secs += duration_ms as f64 / 1000.0;

            // Parse metadata from filename
            let parsed_meta = CallFilenameParser::parse(filename);

            let phone_from = parsed_meta
                .contact_name
                .or_else(|| parsed_meta.phone_number.clone());
            let phone_to = None;

            let mut call = Call::new(
                org_id,
                Some(filename.to_string()),
                CallDirection::Incoming,
                phone_from,
                phone_to,
                parsed_meta.started_at,
            );
            call.duration_ms = Some(duration_ms);

            // Stream file into storage first
            let ext = file_path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("m4a");
            let storage_key = format!("{}/{}.{}", org_id, call.id, ext);

            let file = match File::open(&file_path) {
                Ok(f) => f,
                Err(e) => {
                    tracing::warn!("Failed to open file for streaming {:?}: {e}", file_path);
                    summary.failed_files += 1;
                    continue;
                }
            };

            let tokio_file = tokio::fs::File::from_std(file);
            let byte_stream = Box::pin(ReaderStream::new(tokio_file));

            let put_res = match storage.put(&storage_key, byte_stream).await {
                Ok(res) => res,
                Err(e) => {
                    tracing::error!("Failed to store recording audio for {}: {e}", call.id);
                    summary.failed_files += 1;
                    continue;
                }
            };

            // Insert Call record in DB
            if let Err(e) = call_repo.create(&call).await {
                tracing::error!(
                    "Failed to insert call record {}: {e}. Compensating storage.",
                    call.id
                );
                let _ = storage.delete(&storage_key).await;
                summary.failed_files += 1;
                continue;
            }

            let mime_type = match ext {
                "m4a" | "mp4" | "aac" => "audio/mp4".to_string(),
                "mp3" => "audio/mpeg".to_string(),
                "ogg" | "opus" => "audio/ogg".to_string(),
                "flac" => "audio/flac".to_string(),
                _ => "audio/wav".to_string(),
            };
            let mut recording = Recording::new(
                call.id,
                storage_key.clone(),
                mime_type,
                put_res.size_bytes,
                put_res.sha256,
            );
            recording.duration_ms = Some(duration_ms);
            recording.channels = Some(channels);
            recording.sample_rate = Some(sample_rate);

            if let Err(e) = call_repo.add_recording(&recording).await {
                tracing::error!(
                    "Failed to add recording metadata for {}: {e}. Compensating.",
                    call.id
                );
                let _ = call_repo.delete(call.id).await;
                let _ = storage.delete(&storage_key).await;
                summary.failed_files += 1;
                continue;
            }

            if let Some(ref jr) = job_repo {
                let enqueue_req = EnqueueJob::new(
                    JobKind::IngestRecording,
                    serde_json::json!({ "call_id": call.id.to_string() }),
                )
                .with_call_id(call.id);

                if let Err(e) = jr.enqueue(&enqueue_req).await {
                    tracing::error!(
                        "Failed to enqueue ingest job for {}: {e}. Compensating.",
                        call.id
                    );
                    let _ = call_repo.delete(call.id).await;
                    let _ = storage.delete(&storage_key).await;
                    summary.failed_files += 1;
                    continue;
                }
            }

            summary.imported_calls += 1;
        }

        Ok(summary)
    }
}
