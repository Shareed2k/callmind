use anyhow::{Context, Result, bail};
use clap::Subcommand;
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Write;
use std::path::Path;

#[derive(Subcommand, Debug)]
pub enum ModelCommands {
    /// List all registered and available AI models
    List,
    /// Download model weights (Whisper GGML, ivrit-ai, LLM GGUF)
    Download {
        /// Model identifier to download (e.g. 'whisper-large-v3', 'ivrit-ai-v3', 'qwen-7b', or 'all')
        #[arg(default_value = "all")]
        model: String,
    },
    /// Verify SHA-256 checksums and presence of downloaded models
    Verify {
        /// Model identifier to verify
        #[arg(default_value = "all")]
        model: String,
    },
}

#[derive(Debug, Clone)]
pub struct ModelSpec {
    pub id: &'static str,
    pub kind: &'static str,
    pub filename: &'static str,
    pub url: &'static str,
    pub size_mb: u64,
    pub sha256: &'static str,
}

pub const REGISTERED_MODELS: &[ModelSpec] = &[
    ModelSpec {
        id: "whisper-large-v3",
        kind: "STT Multilingual",
        filename: "stt/whisper-large-v3.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3.bin",
        size_mb: 2952,
        sha256: "64d182b440b98d5203c4f9bd541544d84c605196c4f7b845dfa11fb23594d1e2",
    },
    ModelSpec {
        id: "whisper-turbo",
        kind: "STT Multilingual Fast",
        filename: "stt/whisper-large-v3-turbo.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin",
        // Roughly half the weights of large-v3. Point `models.stt_multilingual`
        // at it to trade some accuracy for speed on the stage that costs 87% of
        // processing time -- and measure on your own recordings, because which
        // way that trade falls depends on the language and the audio.
        size_mb: 1564,
        sha256: "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
    },
    ModelSpec {
        id: "whisper-tiny",
        kind: "STT Test Fixture",
        filename: "stt/whisper-tiny.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin",
        // Not for transcribing anything you care about. It is here because
        // `crates/callmind-stt/tests/whisper_engine_test.rs` needs a real
        // whisper model to run against, and it is the only multilingual one
        // small enough for CI to download on every push.
        size_mb: 74,
        sha256: "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21",
    },
    ModelSpec {
        id: "ivrit-ai-v3",
        kind: "STT Hebrew Fine-Tuned",
        filename: "stt/ivrit-ai-large-v3.bin",
        url: "https://huggingface.co/ivrit-ai/whisper-large-v3-ggml/resolve/main/ggml-model.bin",
        size_mb: 2952,
        sha256: "09e66ec67b2e00c6933afab6684cbf78fe023e8ad153c1848f62000e4335a07f",
    },
    ModelSpec {
        id: "ivrit-ai-turbo",
        kind: "STT Hebrew Turbo",
        filename: "stt/ivrit-ai-large-v3-turbo.bin",
        url: "https://huggingface.co/ivrit-ai/whisper-large-v3-turbo-ggml/resolve/main/ggml-model.bin",
        size_mb: 1549,
        sha256: "c8090411113357097bfafc2b8e228ec1639fa7f5fe4ecb5d054ac0ccef8641b1",
    },
    ModelSpec {
        id: "qwen-7b",
        kind: "LLM Conversation Intelligence",
        filename: "llm/qwen2.5-7b-instruct-q3_k_m.gguf",
        url: "https://huggingface.co/Qwen/Qwen2.5-7B-Instruct-GGUF/resolve/main/qwen2.5-7b-instruct-q3_k_m.gguf",
        size_mb: 3632,
        sha256: "a96b16179dc6cc9afdf0cf7a96a80c199cbd00b9be207c3465be21cb721cca5e",
    },
    ModelSpec {
        id: "qwen-3b",
        kind: "LLM Fast / Low-VRAM",
        filename: "llm/qwen2.5-3b-instruct-q4_k_m.gguf",
        url: "https://huggingface.co/Qwen/Qwen2.5-3B-Instruct-GGUF/resolve/main/qwen2.5-3b-instruct-q4_k_m.gguf",
        size_mb: 2007,
        sha256: "626b4a6678b86442240e33df819e00132d3ba7dddfe1cdc4fbb18e0a9615c62d",
    },
    ModelSpec {
        id: "diarization-segmentation",
        kind: "Speaker Segmentation ONNX",
        filename: "diarization/segmentation.onnx",
        // The one entry this project hosts rather than mirrors: the published
        // pyannote export contains a shape-conditional `If` node that a pure-Rust
        // ONNX runtime cannot translate, so this is a re-export with the input
        // fixed to one 10-second chunk. Reproduce it -- and re-verify that the
        // transformation leaves the output unchanged -- with
        // scripts/export_pyannote_segmentation.py. Upstream is MIT,
        // Copyright (c) 2022 CNRS.
        url: "https://huggingface.co/shareed2k/callmind-pyannote-segmentation/resolve/main/segmentation.onnx",
        size_mb: 6,
        sha256: "91dbcbee0c2564395ffb1e7cd78935cd764706530f0f336671edc6a534c2bb17",
    },
    ModelSpec {
        id: "diarization-embedding",
        kind: "Speaker Embedding ONNX",
        filename: "diarization/speaker_embedding.onnx",
        url: "https://huggingface.co/wespeaker/wespeaker-voxceleb-resnet34-LM/resolve/main/voxceleb_resnet34_LM.onnx",
        size_mb: 26,
        sha256: "7bb2f06e9df17cdf1ef14ee8a15ab08ed28e8d0ef5054ee135741560df2ec068",
    },
];

pub async fn run_models_command(models_dir: &Path, cmd: ModelCommands) -> Result<()> {
    tokio::fs::create_dir_all(models_dir).await?;

    match cmd {
        ModelCommands::List => {
            println!("=== CallMind Model Registry ===");
            println!(
                "{:<22} {:<24} {:<10} {:<15}",
                "MODEL ID", "KIND", "SIZE", "STATUS"
            );
            println!("{:-<75}", "");

            for spec in REGISTERED_MODELS {
                let target_path = models_dir.join(spec.filename);
                let status = if target_path.exists() {
                    let sz = target_path.metadata().map_or(0, |m| m.len()) / (1024 * 1024);
                    if sz >= spec.size_mb.saturating_sub(100) {
                        format!("✓ Downloaded ({sz} MB)")
                    } else {
                        format!("⚠ Partial ({sz}/{} MB)", spec.size_mb)
                    }
                } else {
                    "✗ Missing".to_string()
                };

                println!(
                    "{:<22} {:<24} {:<10} {:<15}",
                    spec.id,
                    spec.kind,
                    format!("~{} MB", spec.size_mb),
                    status
                );
            }
            Ok(())
        }
        ModelCommands::Download { model } => {
            println!("=== CallMind Model Downloader (with Resume Support) ===");
            let specs_to_download: Vec<&ModelSpec> = if model == "all" {
                REGISTERED_MODELS.iter().collect()
            } else {
                REGISTERED_MODELS.iter().filter(|s| s.id == model).collect()
            };

            if specs_to_download.is_empty() {
                bail!(
                    "Model '{model}' not found in model registry. Run 'callmind models list' to view available models."
                );
            }

            for spec in specs_to_download {
                let target_path = models_dir.join(spec.filename);

                if let Some(parent) = target_path.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }

                println!("Processing model '{}' (~{} MB)...", spec.id, spec.size_mb);
                download_file_resumable(spec.url, &target_path).await?;
                println!("[✓] Model '{}' ready at {:?}", spec.id, target_path);
            }

            Ok(())
        }
        ModelCommands::Verify { model } => {
            println!("=== CallMind Model Verification ===");
            let specs_to_verify: Vec<&ModelSpec> = if model == "all" {
                REGISTERED_MODELS.iter().collect()
            } else {
                REGISTERED_MODELS.iter().filter(|s| s.id == model).collect()
            };

            let mut failed_checks = 0;

            for spec in specs_to_verify {
                let target_path = models_dir.join(spec.filename);
                if !target_path.exists() {
                    println!("[✗] Model '{}' is MISSING at {:?}", spec.id, target_path);
                    failed_checks += 1;
                    continue;
                }

                let hash = sha256_file(&target_path)?;

                if spec.sha256.is_empty() {
                    println!(
                        "[✓] Model '{}' present (Calculated SHA256: {})",
                        spec.id, hash
                    );
                } else if hash == spec.sha256 {
                    println!(
                        "[✓] Model '{}' checksum MATCHED (SHA256: {})",
                        spec.id, hash
                    );
                } else {
                    println!(
                        "[✗] Model '{}' checksum MISMATCH!\n    Expected: {}\n    Got:      {}",
                        spec.id, spec.sha256, hash
                    );
                    failed_checks += 1;
                }
            }

            if failed_checks > 0 {
                bail!("{failed_checks} model verification check(s) failed.");
            }

            Ok(())
        }
    }
}

/// Downloads a file with HTTP Range resume support and live terminal progress.
async fn download_file_resumable(url: &str, dest_path: &Path) -> Result<()> {
    let client = reqwest::Client::new();

    // Check if partial download exists
    let initial_offset = if dest_path.exists() {
        std::fs::metadata(dest_path)?.len()
    } else {
        0
    };

    let mut req = client.get(url);
    if initial_offset > 0 {
        println!(
            "  -> Found partial download of {} MB. Requesting resume...",
            initial_offset / (1024 * 1024)
        );
        req = req.header(reqwest::header::RANGE, format!("bytes={initial_offset}-"));
    }

    let response = req
        .send()
        .await
        .context(format!("Failed to initiate download from {url}"))?;

    let status = response.status();

    if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
        println!("  -> Download is already complete.");
        return Ok(());
    }

    let is_resuming = status == reqwest::StatusCode::PARTIAL_CONTENT;

    if !status.is_success() {
        bail!("Download failed with HTTP status: {status}");
    }

    let total_len = if is_resuming {
        response.content_length().map(|len| len + initial_offset)
    } else {
        response.content_length()
    };

    if !is_resuming && initial_offset > 0 {
        if let Some(total) = total_len {
            if total == initial_offset {
                println!(
                    "  -> Model file is already completely downloaded ({} MB).",
                    total / (1024 * 1024)
                );
                return Ok(());
            }
        }
    }

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(is_resuming)
        .truncate(!is_resuming)
        .open(dest_path)
        .context(format!("Failed to open destination file at {dest_path:?}"))?;

    let mut stream = response.bytes_stream();
    let mut downloaded = if is_resuming { initial_offset } else { 0 };
    let mut last_log_time = std::time::Instant::now();
    let start_time = std::time::Instant::now();

    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result.context("Error while streaming download")?;
        file.write_all(&chunk)?;
        downloaded += chunk.len() as u64;

        if last_log_time.elapsed().as_millis() >= 1000 {
            let total_mb_str =
                total_len.map_or("?".to_string(), |t| format!("{} MB", t / (1024 * 1024)));
            let pct_str = total_len.map_or(String::new(), |t| {
                if t > 0 {
                    format!(" ({:.1}%)", (downloaded as f64 / t as f64) * 100.0)
                } else {
                    String::new()
                }
            });

            let elapsed_secs = start_time.elapsed().as_secs_f64().max(0.001);
            let bytes_since_start =
                downloaded.saturating_sub(if is_resuming { initial_offset } else { 0 });
            let speed_mb_s = (bytes_since_start as f64 / (1024.0 * 1024.0)) / elapsed_secs;

            print!(
                "\r  -> Progress: {} MB / {}{}, speed: {:.2} MB/s...      ",
                downloaded / (1024 * 1024),
                total_mb_str,
                pct_str,
                speed_mb_s
            );
            let _ = std::io::stdout().flush();
            last_log_time = std::time::Instant::now();
        }
    }

    file.flush()?;
    println!(
        "\r  -> Progress: 100% completed ({} MB).                              ",
        downloaded / (1024 * 1024)
    );
    Ok(())
}

/// SHA-256 of a file, read in blocks.
///
/// sha2 0.11 dropped the `io::Write` implementation its hashers used to carry,
/// so `io::copy` no longer feeds one. Reading in blocks is what the file needs
/// anyway: model weights run to several gigabytes and must not be loaded whole.
fn sha256_file(path: &std::path::Path) -> std::io::Result<String> {
    use std::io::Read;

    const BLOCK: usize = 1 << 20;

    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; BLOCK];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod checksum_tests {
    use super::*;

    /// sha2 0.11 dropped the `io::Write` implementation its hashers used to
    /// carry, so `io::copy` no longer feeds one. Model files run to gigabytes,
    /// so they are read in fixed-size blocks rather than loaded whole.
    #[test]
    fn it_hashes_a_file_in_blocks_like_sha256sum_does() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("payload.bin");
        std::fs::write(&path, b"abc").expect("write");

        assert_eq!(
            sha256_file(&path).expect("hash"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            "the published vector for \"abc\""
        );
    }

    /// A model that downloaded as an empty file must not look like a match for
    /// something; the empty digest is a real, recognisable value.
    #[test]
    fn an_empty_file_hashes_to_the_empty_digest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("empty.bin");
        std::fs::write(&path, b"").expect("write");

        assert_eq!(
            sha256_file(&path).expect("hash"),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
