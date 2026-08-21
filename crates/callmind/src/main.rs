pub mod models_cli;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

use callmind_api::{AppState, create_router};
use callmind_config::AppConfig;
use callmind_core::JobKind;
use callmind_db::{
    CallRepository, JobRepository, SqliteCallRepository, SqliteJobRepository, create_sqlite_pool,
    run_migrations,
};
use callmind_jobs::{CallPipelineHandler, JobRegistry, WorkerPool};
use callmind_storage::FilesystemStorage;
use callmind_vad::VadEngine;

#[derive(Parser, Debug)]
#[command(
    name = "callmind",
    author,
    version,
    about = "CallMind Conversation Intelligence Server"
)]
struct Cli {
    /// Path to YAML configuration file
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Start the unified server (Axum REST API + Tokio Background Job Workers)
    Serve,
    /// Apply pending database migrations
    Migrate,
    /// Run diagnostic environment and health checks
    Doctor,
    /// Manage AI model weights (list, download, verify)
    Models {
        #[command(subcommand)]
        command: models_cli::ModelCommands,
    },
    /// Import audio call recordings from a directory
    Import {
        /// Path to directory containing call recordings
        path: PathBuf,
        /// Maximum number of files to import
        #[arg(short, long)]
        limit: Option<usize>,
    },
    /// Run performance benchmarks (decoding, VAD, resampling) on a directory of calls
    Benchmark {
        /// Path to directory containing audio benchmark files
        path: PathBuf,
        /// Maximum number of files to benchmark
        #[arg(short, long)]
        limit: Option<usize>,
    },
    /// Re-run the full intelligence processing pipeline on an existing call
    Reprocess {
        /// Call UUID identifier to reprocess
        call_id: String,
    },
    /// Display version and build information
    Version,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Serve => run_serve(cli.config).await,
        Commands::Migrate => run_migrate(cli.config).await,
        Commands::Doctor => run_doctor(cli.config).await,
        Commands::Models { command } => {
            let config = AppConfig::load_from_file_or_default(cli.config)?;
            models_cli::run_models_command(&config.models.models_dir, command).await
        }
        Commands::Import { path, limit } => run_import(cli.config, path, limit).await,
        Commands::Benchmark { path, limit } => run_benchmark(path, limit).await,
        Commands::Reprocess { call_id } => run_reprocess(cli.config, call_id).await,
        Commands::Version => run_version(),
    }
}

async fn run_serve(config_path: Option<PathBuf>) -> Result<()> {
    init_tracing();
    info!("Starting CallMind Server v{}", env!("CARGO_PKG_VERSION"));

    let config = Arc::new(
        AppConfig::load_from_file_or_default(config_path)
            .context("Failed to load configuration")?,
    );

    // Validate and Initialize Database
    let pool = match config.database.driver.to_lowercase().as_str() {
        "sqlite" => create_sqlite_pool(&config.database.url, config.database.max_connections)
            .await
            .context("Failed to connect to SQLite database")?,
        other => anyhow::bail!(
            "Unsupported database driver: '{other}'. Only 'sqlite' is currently supported."
        ),
    };

    run_migrations(&pool)
        .await
        .context("Failed to apply database migrations")?;

    let call_repo = Arc::new(SqliteCallRepository::new(pool.clone()));
    let job_repo = Arc::new(SqliteJobRepository::new(pool.clone()));

    // Validate and Initialize Recording Storage
    let storage: Arc<dyn callmind_storage::RecordingStorage> =
        match config.storage.driver.to_lowercase().as_str() {
            "filesystem" | "local" => Arc::new(
                FilesystemStorage::new(&config.storage.path)
                    .await
                    .context("Failed to initialize filesystem recording storage")?,
            ),
            other => anyhow::bail!(
                "Unsupported storage driver: '{other}'. Only 'filesystem' is currently supported."
            ),
        };

    // Initialize AI and Processing Subsystems
    let vad = Arc::new(callmind_vad::EnergyVadEngine::default());
    let language_engine = Arc::new(callmind_language::SamplingLanguageEngine::default());

    let hebrew_model_path = config
        .models
        .models_dir
        .join("stt/ivrit-ai-large-v3-turbo.bin");
    let multi_model_path = config.models.models_dir.join("stt/whisper-large-v3.bin");

    let hebrew_stt = Arc::new(callmind_stt::WhisperCppEngine::new(
        hebrew_model_path,
        "ivrit-ai-turbo",
        "1.0",
    ));
    let multi_stt = Arc::new(callmind_stt::WhisperCppEngine::new(
        multi_model_path,
        "whisper-large-v3",
        "1.0",
    ));
    let stt_router = Arc::new(callmind_stt::SttRouter::new(hebrew_stt, multi_stt, 0.90));

    let stereo_diarizer = Arc::new(callmind_diarization::StereoChannelDiarizer::new(
        vad.clone(),
    ));
    let onnx_diarization_model = config
        .models
        .models_dir
        .join("diarization/speaker_embedding.onnx");
    let clustering_diarizer = Arc::new(callmind_diarization::NeuralDiarizer::new_with_fallback(
        Some(onnx_diarization_model),
        vad.clone(),
    ));

    let gpu_semaphore = Arc::new(tokio::sync::Semaphore::new(1));
    let transcriber = Arc::new(callmind_transcript::AudioTranscriber::new(
        vad,
        language_engine,
        stt_router,
        stereo_diarizer,
        clustering_diarizer,
        gpu_semaphore,
    ));

    let search_engine = Arc::new(callmind_search::SearchEngine::new(pool.clone()));
    let llm_engine = callmind_llm::create_llm_engine(&config.llm);
    let ask_engine = Arc::new(callmind_search::AskEngine::new(
        (*search_engine).clone(),
        llm_engine.clone(),
    ));
    let analysis_engine = Arc::new(callmind_analysis::AnalysisEngine::new(llm_engine));

    // Initialize Complete End-to-End Pipeline Handler
    let pipeline_handler = CallPipelineHandler {
        call_repo: call_repo.clone(),
        storage: storage.clone(),
        transcriber,
        analyzer: analysis_engine.clone(),
        search_engine: search_engine.clone(),
        pool: pool.clone(),
    };

    // Initialize Job Registry & Worker Pool
    let cancellation_token = CancellationToken::new();
    let registry = JobRegistry::builder()
        .register(JobKind::IngestRecording, pipeline_handler)
        .build();

    let mut worker_pool = WorkerPool::new(
        job_repo.clone(),
        registry,
        config.jobs.clone(),
        cancellation_token.clone(),
    )
    .with_pool(pool.clone());

    worker_pool.start();

    let default_org_id = callmind_core::OrgId(
        uuid::Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap(),
    );

    // Start background directory watcher if enabled in config
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    if config.watcher.enabled {
        let watcher = callmind_audio::DirectoryWatcher::new(
            config.watcher.watch_dir.clone(),
            config.watcher.poll_secs,
            call_repo.clone(),
            job_repo.clone(),
            storage.clone(),
            default_org_id,
        );
        watcher.spawn(shutdown_rx);
    }

    // Initialize REST API State and Router
    let app_state = AppState::new(
        config.clone(),
        call_repo,
        job_repo,
        storage,
        search_engine,
        ask_engine,
        analysis_engine,
        pool,
    );
    let app = create_router(app_state);

    let listener = tokio::net::TcpListener::bind(&config.server.bind)
        .await
        .context(format!("Failed to bind server to {}", config.server.bind))?;

    info!(
        "CallMind HTTP server listening on http://{}",
        config.server.bind
    );
    info!(
        "Swagger UI available at http://{}/swagger-ui",
        config.server.bind
    );

    let server_handle = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            error!("HTTP server encountered fatal error: {e}");
        }
    });

    // Wait for shutdown signal (Ctrl+C / SIGTERM)
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("Received SIGINT shutdown signal");
        }
    }

    info!("Initiating graceful shutdown...");
    let _ = shutdown_tx.send(true);
    cancellation_token.cancel();
    server_handle.abort();
    worker_pool.wait().await;
    info!("CallMind Server stopped cleanly.");

    Ok(())
}

async fn run_migrate(config_path: Option<PathBuf>) -> Result<()> {
    init_tracing();
    info!("Running CallMind database migrations...");

    let config = AppConfig::load_from_file_or_default(config_path)?;
    let pool = create_sqlite_pool(&config.database.url, 2).await?;
    run_migrations(&pool).await?;

    println!("All database migrations applied successfully.");
    Ok(())
}

async fn run_doctor(config_path: Option<PathBuf>) -> Result<()> {
    println!("=== CallMind System Diagnostic (Doctor) ===");

    let config = match AppConfig::load_from_file_or_default(config_path) {
        Ok(c) => {
            println!("[✓] Configuration loaded successfully");
            c
        }
        Err(e) => {
            println!("[✗] Configuration error: {e}");
            return Ok(());
        }
    };

    // Check Database
    match create_sqlite_pool(&config.database.url, 2).await {
        Ok(pool) => {
            println!("[✓] SQLite database connected ({})", config.database.url);
            match run_migrations(&pool).await {
                Ok(()) => println!("[✓] Database migrations up to date"),
                Err(e) => println!("[✗] Migration error: {e}"),
            }
        }
        Err(e) => println!("[✗] Database connection failed: {e}"),
    }

    // Check Storage
    match FilesystemStorage::new(&config.storage.path).await {
        Ok(_) => println!(
            "[✓] Recording storage directory writable ({:?})",
            config.storage.path
        ),
        Err(e) => println!("[✗] Storage error: {e}"),
    }

    // Check Tokio runtime
    println!(
        "[✓] Tokio async runtime operational (configured for {} background workers)",
        config.jobs.workers
    );

    // Check Models directory
    if config.models.models_dir.exists() {
        println!(
            "[✓] Models directory exists ({:?})",
            config.models.models_dir
        );
    } else {
        println!(
            "[!] Models directory not yet created ({:?}) - will be required for STT/LLM in Phase 3+",
            config.models.models_dir
        );
    }

    println!("\nDiagnostics complete: Environment is ready for CallMind operations.");
    Ok(())
}

async fn run_import(
    config_path: Option<PathBuf>,
    import_path: PathBuf,
    limit: Option<usize>,
) -> Result<()> {
    init_tracing();
    println!("=== CallMind Batch Importer ===");
    println!("Scanning directory: {:?}", import_path);

    let config = AppConfig::load_from_file_or_default(config_path)?;
    let pool = create_sqlite_pool(&config.database.url, 4).await?;
    run_migrations(&pool).await?;

    let call_repo = Arc::new(SqliteCallRepository::new(pool.clone()));
    let job_repo = Arc::new(SqliteJobRepository::new(pool));
    let storage = Arc::new(FilesystemStorage::new(&config.storage.path).await?);

    let org_id = callmind_core::OrgId(
        uuid::Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap(),
    );

    let start_time = std::time::Instant::now();
    let summary = callmind_audio::BatchImporter::import_directory(
        import_path,
        call_repo,
        Some(job_repo),
        storage,
        org_id,
        limit,
    )
    .await
    .map_err(|e| anyhow::anyhow!("Import failed: {e}"))?;

    let elapsed = start_time.elapsed();

    println!("\n=== Batch Import Summary ===");
    println!("Files Scanned:        {}", summary.scanned_files);
    println!("Successfully Imported: {}", summary.imported_calls);
    println!("Skipped (Existing):    {}", summary.skipped_existing);
    println!("Failed / Unreadable:  {}", summary.failed_files);
    println!(
        "Channel Breakdown:     {} Mono, {} Stereo",
        summary.mono_calls, summary.stereo_calls
    );
    println!(
        "Total Audio Time:      {:.2} hours",
        summary.total_duration_secs / 3600.0
    );
    println!(
        "Import Time Elapsed:   {:.2} seconds",
        elapsed.as_secs_f64()
    );
    println!(
        "Throughput:            {:.1} audio hours/sec",
        (summary.total_duration_secs / 3600.0) / elapsed.as_secs_f64().max(0.001)
    );

    Ok(())
}

async fn run_benchmark(benchmark_path: PathBuf, limit: Option<usize>) -> Result<()> {
    println!("=== CallMind Audio Performance Benchmark ===");
    println!("Benchmark Directory: {:?}", benchmark_path);

    if !benchmark_path.is_dir() {
        anyhow::bail!("Benchmark directory {:?} does not exist", benchmark_path);
    }

    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&benchmark_path)?.flatten() {
        let p = entry.path();
        if p.is_file() {
            if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                if matches!(
                    ext.to_lowercase().as_str(),
                    "m4a" | "wav" | "mp3" | "ogg" | "flac"
                ) {
                    entries.push(p);
                }
            }
        }
    }

    entries.sort();
    let count = limit.unwrap_or(entries.len()).min(entries.len());
    println!("Benchmarking {} audio files...", count);

    let vad = callmind_vad::EnergyVadEngine::default();

    let mut total_audio_secs = 0.0;
    let mut total_decode_secs = 0.0;
    let mut total_resample_secs = 0.0;
    let mut total_vad_secs = 0.0;
    let mut total_speech_regions = 0;

    for file in entries.into_iter().take(count) {
        // 1. Decode benchmark
        let t0 = std::time::Instant::now();
        let decoded = match callmind_audio::AudioDecoder::decode_file(&file) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let decode_elapsed = t0.elapsed().as_secs_f64();
        total_decode_secs += decode_elapsed;

        let audio_dur = decoded.duration_ms() as f64 / 1000.0;
        total_audio_secs += audio_dur;

        // 2. Resample benchmark
        let t1 = std::time::Instant::now();
        let resampled = match callmind_audio::AudioResampler::resample_to_16k_mono(&decoded) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let resample_elapsed = t1.elapsed().as_secs_f64();
        total_resample_secs += resample_elapsed;

        // 3. VAD benchmark
        let t2 = std::time::Instant::now();
        let regions = vad.detect(&resampled).await.unwrap_or_default();
        let vad_elapsed = t2.elapsed().as_secs_f64();
        total_vad_secs += vad_elapsed;
        total_speech_regions += regions.len();
    }

    let total_proc_secs = total_decode_secs + total_resample_secs + total_vad_secs;
    let rtf = total_proc_secs / total_audio_secs.max(0.001);

    println!("\n=== Benchmark Results ===");
    println!(
        "Total Audio Processed: {:.2} minutes ({:.1} seconds)",
        total_audio_secs / 60.0,
        total_audio_secs
    );
    println!("Total Processing Time: {:.2} seconds", total_proc_secs);
    println!(
        "  - Decoding:          {:.3}s (RTF: {:.4})",
        total_decode_secs,
        total_decode_secs / total_audio_secs.max(0.001)
    );
    println!(
        "  - Resampling (16k):  {:.3}s (RTF: {:.4})",
        total_resample_secs,
        total_resample_secs / total_audio_secs.max(0.001)
    );
    println!(
        "  - VAD Segmentation:  {:.3}s (RTF: {:.4})",
        total_vad_secs,
        total_vad_secs / total_audio_secs.max(0.001)
    );
    println!("Speech Regions Found:  {}", total_speech_regions);
    println!(
        "Overall Real-Time Factor (RTF): {:.4} (Speedup: {:.1}x realtime)",
        rtf,
        1.0 / rtf.max(0.0001)
    );

    Ok(())
}

async fn run_reprocess(config_path: Option<PathBuf>, call_id_str: String) -> Result<()> {
    init_tracing();
    let call_id = callmind_core::CallId::from_str(&call_id_str)
        .context(format!("Invalid Call UUID: {call_id_str}"))?;

    println!("=== CallMind Call Reprocessor ===");
    println!("Target Call ID: {}", call_id);

    let config = AppConfig::load_from_file_or_default(config_path)?;
    let pool = create_sqlite_pool(&config.database.url, 4).await?;
    run_migrations(&pool).await?;

    let call_repo = Arc::new(SqliteCallRepository::new(pool.clone()));
    let job_repo = Arc::new(SqliteJobRepository::new(pool));

    let _ = call_repo
        .get_by_id(call_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Call {call_id} not found in database"))?;

    call_repo
        .update_status(call_id, callmind_core::ProcessingStatus::Pending)
        .await?;

    let req = callmind_core::EnqueueJob::new(
        callmind_core::JobKind::IngestRecording,
        serde_json::json!({ "call_id": call_id.to_string() }),
    )
    .with_call_id(call_id);

    job_repo.enqueue(&req).await?;
    println!(
        "[✓] Reprocessing job enqueued for Call {call_id}. Background workers will process it."
    );

    Ok(())
}

fn run_version() -> Result<()> {
    println!("callmind {}", env!("CARGO_PKG_VERSION"));
    println!("Rust Edition: 2024");
    println!("Target: {}", std::env::consts::ARCH);
    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,callmind=debug,tower_http=debug"));

    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .try_init();
}
