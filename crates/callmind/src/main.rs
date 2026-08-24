//! CallMind command-line entry point.
//!
//! `serve` runs the HTTP API, the server-rendered UI and the background worker
//! pool in one process. The other subcommands cover migrations, model weight
//! management, batch import, diagnostics, benchmarking and reprocessing a single
//! call.

pub mod models_cli;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use tracing_subscriber::{EnvFilter, Layer, layer::SubscriberExt, util::SubscriberInitExt};

use callmind_api::{AppState, create_router};
use callmind_config::AppConfig;
use callmind_core::JobKind;
use callmind_db::{
    CallRepository, JobRepository, SqlCallRepository, SqlJobRepository, SqlStatsRepository,
    connect, run_migrations_on,
};
use callmind_diarization::DiarizationEngine;
use callmind_jobs::{CallPipelineHandler, JobRegistry, WorkerPool};
use callmind_language::LanguageEngine;
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
    /// Run performance benchmarks on a directory of calls
    Benchmark {
        /// Path to directory containing audio benchmark files
        path: PathBuf,
        /// Maximum number of files to benchmark
        #[arg(short, long)]
        limit: Option<usize>,
        /// Also benchmark the expensive stages: LID, diarization and STT.
        /// Requires the model weights from `callmind models download all`.
        #[arg(long)]
        full: bool,
    },
    /// Re-run the full intelligence processing pipeline on an existing call
    Reprocess {
        /// Call UUID identifier to reprocess
        call_id: String,
    },
    /// Fill in missing audio duration and format for already-processed calls
    Backfill,
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
        Commands::Benchmark { path, limit, full } => {
            run_benchmark(cli.config, path, limit, full).await
        }
        Commands::Reprocess { call_id } => run_reprocess(cli.config, call_id).await,
        Commands::Backfill => run_backfill(cli.config).await,
        Commands::Version => run_version(),
    }
}

async fn run_serve(config_path: Option<PathBuf>) -> Result<()> {
    // The format comes from the config, so the subscriber is installed after it
    // is loaded rather than before.
    info!("Starting CallMind Server v{}", env!("CARGO_PKG_VERSION"));

    let config = Arc::new(
        AppConfig::load_from_file_or_default(config_path)
            .context("Failed to load configuration")?,
    );
    init_tracing_with(config.logging.format);

    // One connection for whichever backend the config names. Every repository
    // builds its SQL with sea-query, so nothing below this line knows or cares.
    let db = connect(
        &config.database.driver,
        &config.database.url,
        config.database.max_connections,
    )
    .await
    .context("Failed to connect to the database")?;

    run_migrations_on(&db)
        .await
        .context("Failed to apply database migrations")?;

    let call_repo = Arc::new(SqlCallRepository::new(db.clone()));
    let job_repo = Arc::new(SqlJobRepository::new(db.clone()));
    let stats_repo = Arc::new(SqlStatsRepository::new(db.clone()));

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

    // Initialize AI and Processing Subsystems.
    //
    // Speaker segmentation, when its model is present, is both a better speech
    // detector and the source of the speaker count. Loaded once and shared: the
    // diarizer uses it for both, and language identification -- which probes a
    // few windows of speech to choose a Whisper model -- gets regions that are
    // actually speech rather than possibly hold music. Speech-to-text is given
    // the whole recording either way.
    let segmentation_model = config
        .models
        .models_dir
        .join("diarization/segmentation.onnx");
    let segmenter = if segmentation_model.exists() {
        match callmind_diarization::pyannote::PyannoteSegmenter::load(&segmentation_model) {
            Ok(loaded) => {
                info!(
                    "Loaded pyannote speaker segmentation from {}; speech detection and the \
                     speaker count are measured rather than assumed",
                    segmentation_model.display()
                );
                Some(Arc::new(loaded))
            }
            Err(e) => {
                warn!(
                    "Failed to load speaker segmentation from {}: {e}. Falling back to the \
                     energy detector and the two-party prior.",
                    segmentation_model.display()
                );
                None
            }
        }
    } else {
        None
    };

    let vad: Arc<dyn callmind_vad::VadEngine> = match segmenter.clone() {
        Some(seg) => Arc::new(callmind_diarization::pyannote::PyannoteVad::new(seg)),
        None => Arc::new(callmind_vad::EnergyVadEngine::default()),
    };

    // From config, not compiled in: speech-to-text dominates processing time and
    // the model is the lever on it.
    let hebrew_model_path = config.models.models_dir.join(&config.models.stt_hebrew);
    let multi_model_path = config
        .models
        .models_dir
        .join(&config.models.stt_multilingual);

    ensure_model_present("hebrew speech-to-text", &hebrew_model_path)?;
    ensure_model_present("multilingual speech-to-text", &multi_model_path)?;

    let hebrew_label = model_label(&hebrew_model_path);
    let multi_label = model_label(&multi_model_path);
    let hebrew_stt = Arc::new(callmind_stt::WhisperCppEngine::new(
        hebrew_model_path,
        &hebrew_label,
        "1.0",
    ));
    let multi_stt = Arc::new(callmind_stt::WhisperCppEngine::new(
        multi_model_path,
        &multi_label,
        "1.0",
    ));
    let stt_router = Arc::new(callmind_stt::SttRouter::new(
        hebrew_stt,
        multi_stt.clone(),
        0.90,
    ));

    // Production acoustic LID backed by Whisper multi-model probe
    let multi_stt_for_lid = multi_stt.clone();
    let language_engine = Arc::new(
        callmind_language::SamplingLanguageEngine::new().with_detector(move |buf| {
            match multi_stt_for_lid.detect_language_probe(buf) {
                Ok(probs) => probs
                    .into_iter()
                    .map(
                        |(language, probability)| callmind_language::LanguageProbability {
                            language,
                            probability,
                        },
                    )
                    .collect(),
                Err(e) => {
                    // Swallowing this made a missing Whisper model look like
                    // "no languages detected", and the analyzer then defaults
                    // the language to Hebrew — silently mistranscribing.
                    warn!("Acoustic language probe failed: {e}");
                    Vec::new()
                }
            }
        }),
    );

    let stereo_diarizer = Arc::new(callmind_diarization::StereoChannelDiarizer::new(
        vad.clone(),
    ));
    let onnx_diarization_model = config
        .models
        .models_dir
        .join("diarization/speaker_embedding.onnx");
    let mut neural_diarizer = callmind_diarization::NeuralDiarizer::new_with_fallback(
        Some(onnx_diarization_model),
        vad.clone(),
    );

    // Measured on labelled recordings: with segmentation the speaker count comes
    // out 4/4 on monologues and 23/24 on two-party calls, against 0/4 and 24/24
    // for the two-party assumption it replaces.
    if let Some(seg) = segmenter {
        neural_diarizer = neural_diarizer.with_segmenter(seg);
    }
    let clustering_diarizer = Arc::new(neural_diarizer);

    let gpu_semaphore = Arc::new(tokio::sync::Semaphore::new(1));
    let transcriber = Arc::new(callmind_transcript::AudioTranscriber::new(
        vad,
        language_engine,
        stt_router,
        stereo_diarizer,
        clustering_diarizer,
        gpu_semaphore,
    ));

    let search_index = Arc::new(callmind_db::SqlSearchIndex::new(db.clone()));
    let search_engine = Arc::new(callmind_search::SearchEngine::new(search_index));
    let llm_engine = callmind_llm::create_llm_engine(&config.llm);
    let ask_engine = Arc::new(callmind_search::AskEngine::new(
        (*search_engine).clone(),
        llm_engine.clone(),
    ));
    // The analyser needs the same budget the model was given, or it cannot know
    // when a transcript will be cut.
    let analysis_engine = Arc::new(
        callmind_analysis::AnalysisEngine::new(llm_engine)
            .with_context_tokens(config.llm.context_tokens),
    );

    // Initialize Complete End-to-End Pipeline Handler
    // Plugins wire themselves in. The list is the only place a plugin is named:
    // closed-source ones are added here behind a Cargo feature, which is the
    // right linkage model given Rust has no stable ABI. Nothing below knows how
    // many there are.
    let plugins: Vec<Arc<dyn callmind_plugin_api::Plugin>> = Vec::new();

    let pipeline_handler = CallPipelineHandler {
        call_repo: call_repo.clone(),
        speaker_repo: call_repo.clone(),
        plugins: plugins.clone(),
        storage: storage.clone(),
        transcriber,
        analyzer: analysis_engine.clone(),
        search: search_engine.clone(),
    };

    // Initialize Job Registry & Worker Pool
    let cancellation_token = CancellationToken::new();
    let mut registry_builder =
        JobRegistry::builder().register(JobKind::IngestRecording, pipeline_handler);
    for plugin in &plugins {
        info!(
            "Loading plugin '{}' (job kind {})",
            plugin.name(),
            plugin.job_kind().as_str()
        );
        registry_builder = plugin.register_jobs(registry_builder);
    }
    let registry = registry_builder.build();

    let mut worker_pool = WorkerPool::new(
        job_repo.clone(),
        call_repo.clone(),
        registry,
        config.jobs.clone(),
        cancellation_token.clone(),
    );

    worker_pool.start();

    let default_org_id = callmind_core::OrgId::DEFAULT;

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

    // Start background Telegram Bot if enabled in config
    callmind_api::TelegramBotService::start(
        config.clone(),
        call_repo.clone(),
        job_repo.clone(),
        storage.clone(),
        cancellation_token.clone(),
    );

    // Initialize REST API State and Router
    // Runtime HTML templates. A plugin registers its own view here at startup;
    // built-in views are compiled in but still rendered at runtime.
    let mut template_registry = callmind_ui::templates::TemplateRegistry::new();
    for plugin in &plugins {
        // A plugin with no view is the common case, so a failure here is the
        // plugin's problem and not a reason to refuse to start.
        if let Err(e) = plugin.register_ui(&mut template_registry) {
            warn!(
                "Plugin '{}' failed to register its view: {e}",
                plugin.name()
            );
        }
    }
    let templates = Arc::new(template_registry);

    let app_state = AppState::new(
        config.clone(),
        call_repo.clone(),
        call_repo.clone(),
        job_repo.clone(),
        stats_repo,
        storage,
        search_engine,
        ask_engine,
        analysis_engine,
        templates,
    );
    // Remote worker gRPC listener, on its own port. The contract lives in
    // callmind-worker-proto; workers never touch the database.
    if config.workers.enabled {
        let grpc_addr: std::net::SocketAddr = config
            .workers
            .bind
            .parse()
            .context(format!("Invalid workers.bind: {}", config.workers.bind))?;
        let service = callmind_api::grpc::WorkerService::new(app_state.clone());
        let grpc_token = cancellation_token.clone();
        info!("CallMind worker gRPC listening on {grpc_addr}");
        tokio::spawn(async move {
            let server = tonic::transport::Server::builder()
                .add_service(callmind_worker_proto::WorkerServer::new(service))
                .serve_with_shutdown(grpc_addr, async move { grpc_token.cancelled().await });
            if let Err(e) = server.await {
                error!("Worker gRPC server stopped: {e}");
            }
        });
    }

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
    #[cfg(unix)]
    {
        // `docker stop` sends SIGTERM. Without this branch the default handler
        // killed the process instantly and the requeue block below — the whole
        // point of the job leasing design — never ran.
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .context("Failed to install SIGTERM handler")?;
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("Received SIGINT shutdown signal");
            }
            _ = sigterm.recv() => {
                info!("Received SIGTERM shutdown signal");
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
        info!("Received SIGINT shutdown signal");
    }

    info!("Initiating graceful shutdown...");
    let _ = shutdown_tx.send(true);
    cancellation_token.cancel();
    server_handle.abort();

    // Deliberately shorter than any sane container stop grace period. Workers
    // blocked inside `spawn_blocking` (STT, diarization) cannot be cancelled, so
    // waiting longer does not help them finish — it only risks SIGKILL arriving
    // before the requeue below runs, which is what happened against Docker's
    // 10s default.
    const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
    if tokio::time::timeout(SHUTDOWN_TIMEOUT, worker_pool.wait())
        .await
        .is_err()
    {
        warn!(
            "Workers did not stop within {} seconds; returning active jobs to the queue",
            SHUTDOWN_TIMEOUT.as_secs()
        );

        match job_repo
            .requeue_all_running("Interrupted by server shutdown")
            .await
        {
            Ok(0) => {}
            Ok(count) => info!("Requeued {count} interrupted job(s) for the next start"),
            Err(err) => error!("Failed to requeue interrupted jobs: {err}"),
        }

        match call_repo.reset_processing_to_pending().await {
            Ok(0) => {}
            Ok(count) => info!("Reset {count} interrupted call(s) to pending"),
            Err(err) => error!("Failed to reset interrupted calls: {err}"),
        }

        info!("Forcing process exit after returning interrupted work to the queue.");
        std::process::exit(0);
    }

    info!("CallMind Server stopped cleanly.");

    Ok(())
}

async fn run_migrate(config_path: Option<PathBuf>) -> Result<()> {
    init_tracing();
    info!("Running CallMind database migrations...");

    let config = AppConfig::load_from_file_or_default(config_path)?;
    let db = connect(&config.database.driver, &config.database.url, 2).await?;
    run_migrations_on(&db).await?;

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
    match connect(&config.database.driver, &config.database.url, 2).await {
        Ok(db) => {
            println!(
                "[✓] {} database connected ({})",
                config.database.driver, config.database.url
            );
            match run_migrations_on(&db).await {
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
    let db = connect(&config.database.driver, &config.database.url, 4).await?;
    run_migrations_on(&db).await?;

    let call_repo = Arc::new(SqlCallRepository::new(db.clone()));
    let job_repo = Arc::new(SqlJobRepository::new(db.clone()));
    let storage = Arc::new(FilesystemStorage::new(&config.storage.path).await?);

    let org_id = callmind_core::OrgId::DEFAULT;

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

async fn run_benchmark(
    config_path: Option<PathBuf>,
    benchmark_path: PathBuf,
    limit: Option<usize>,
    full: bool,
) -> Result<()> {
    init_tracing();
    println!("=== CallMind Audio Performance Benchmark ===");
    println!("Benchmark Directory: {:?}", benchmark_path);
    println!(
        "Mode: {}",
        if full {
            "full pipeline (decode, resample, VAD, LID, diarization, STT)"
        } else {
            "audio only (decode, resample, VAD)"
        }
    );

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

    // Heavy stages are opt-in: they need the model weights on disk.
    let heavy = if full {
        let config = AppConfig::load_from_file_or_default(config_path)
            .context("Failed to load configuration")?;
        let models_dir = &config.models.models_dir;

        let shared_vad: Arc<dyn VadEngine> = Arc::new(callmind_vad::EnergyVadEngine::default());
        let hebrew_stt = Arc::new(callmind_stt::WhisperCppEngine::new(
            models_dir.join("stt/ivrit-ai-large-v3-turbo.bin"),
            "ivrit-ai-turbo",
            "1.0",
        ));
        let multi_stt = Arc::new(callmind_stt::WhisperCppEngine::new(
            models_dir.join("stt/whisper-large-v3.bin"),
            "whisper-large-v3",
            "1.0",
        ));
        let stt_router = callmind_stt::SttRouter::new(hebrew_stt, multi_stt.clone(), 0.90);

        let multi_stt_for_lid = multi_stt.clone();
        let language_engine =
            callmind_language::SamplingLanguageEngine::new().with_detector(move |buf| {
                match multi_stt_for_lid.detect_language_probe(buf) {
                    Ok(probs) => probs
                        .into_iter()
                        .map(
                            |(language, probability)| callmind_language::LanguageProbability {
                                language,
                                probability,
                            },
                        )
                        .collect(),
                    Err(e) => {
                        warn!("Acoustic language probe failed: {e}");
                        Vec::new()
                    }
                }
            });

        let diarizer = callmind_diarization::NeuralDiarizer::new_with_fallback(
            Some(models_dir.join("diarization/speaker_embedding.onnx")),
            shared_vad,
        );
        Some((stt_router, language_engine, diarizer))
    } else {
        None
    };

    let mut total_lid_secs = 0.0;
    let mut total_diar_secs = 0.0;
    let mut total_stt_secs = 0.0;
    let mut total_speakers = 0usize;
    let mut total_words = 0usize;

    let mut total_audio_secs = 0.0;
    let mut total_decode_secs = 0.0;
    let mut total_resample_secs = 0.0;
    let mut total_vad_secs = 0.0;
    let mut total_speech_regions = 0;
    let mut total_speech_secs = 0.0;

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
        total_speech_secs += regions
            .iter()
            .map(|r| r.duration_ms() as f64 / 1000.0)
            .sum::<f64>();

        let Some((stt_router, language_engine, diarizer)) = heavy.as_ref() else {
            continue;
        };

        // 4. Language identification
        let t3 = std::time::Instant::now();
        let detection = match language_engine.detect(&resampled, &regions).await {
            Ok(d) => d,
            Err(e) => {
                warn!("LID failed for {:?}: {e}", file.file_name());
                continue;
            }
        };
        let lid_elapsed = t3.elapsed().as_secs_f64();
        total_lid_secs += lid_elapsed;

        // 5. Diarization (ONNX embeddings + agglomerative clustering)
        let t4 = std::time::Instant::now();
        let diar = diarizer
            .diarize(callmind_diarization::DiarizationRequest::new(&resampled))
            .await;
        let diar_elapsed = t4.elapsed().as_secs_f64();
        total_diar_secs += diar_elapsed;
        let speakers = diar.as_ref().map_or(0, |d| d.speakers);
        total_speakers += speakers;

        // 6. Speech-to-text
        let t5 = std::time::Instant::now();
        let stt = stt_router
            .transcribe_routed(&resampled, &detection, &[])
            .await;
        let stt_elapsed = t5.elapsed().as_secs_f64();
        total_stt_secs += stt_elapsed;
        let words = stt.as_ref().map_or(0, |(r, _)| r.words.len());
        total_words += words;

        println!(
            "  {:>6.0}s audio | decode {:>5.2} | vad {:>4.2} | lid {:>5.2} | diar {:>6.2} | stt {:>7.2} | {} spk, {} words | {}",
            audio_dur,
            decode_elapsed,
            vad_elapsed,
            lid_elapsed,
            diar_elapsed,
            stt_elapsed,
            speakers,
            words,
            file.file_name().and_then(|n| n.to_str()).unwrap_or("?")
        );
    }

    let total_proc_secs = total_decode_secs
        + total_resample_secs
        + total_vad_secs
        + total_lid_secs
        + total_diar_secs
        + total_stt_secs;
    let rtf = total_proc_secs / total_audio_secs.max(0.001);

    println!("\n=== Benchmark Results ===");
    if full {
        // Worth stating: `serve` runs diarization and STT concurrently via
        // `tokio::try_join!`, so the real pipeline is faster than the sum below.
        // Measured on a 42.9-minute call: 337s end to end against 526s of stages
        // added up.
        println!("(stages are timed sequentially; the pipeline overlaps diarization and STT)");
    }
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
    if full {
        let per = |secs: f64| secs / total_audio_secs.max(0.001);
        println!(
            "  - Language ID:       {:.3}s (RTF: {:.4})",
            total_lid_secs,
            per(total_lid_secs)
        );
        println!(
            "  - Diarization:       {:.3}s (RTF: {:.4})",
            total_diar_secs,
            per(total_diar_secs)
        );
        println!(
            "  - Speech-to-text:    {:.3}s (RTF: {:.4})",
            total_stt_secs,
            per(total_stt_secs)
        );
        println!("Speakers Detected:     {}", total_speakers);
        println!("Words Transcribed:     {}", total_words);
    }
    println!(
        "Speech Regions Found:  {} ({:.1}s speech, {:.0}% of audio)",
        total_speech_regions,
        total_speech_secs,
        100.0 * total_speech_secs / total_audio_secs.max(0.001)
    );
    // Diarization clusters one embedding per 400ms hop across speech.
    println!("Diarization Windows:   ~{:.0}", total_speech_secs / 0.4);
    println!(
        "Overall Real-Time Factor (RTF): {:.4} (Speedup: {:.1}x realtime)",
        rtf,
        1.0 / rtf.max(0.0001)
    );

    Ok(())
}

/// Fill in `duration_ms` for calls processed before it was recorded.
///
/// Reads the container header rather than decoding -- 183 ms and 7 MB against
/// 1.3 s and 504 MB for a 43-minute file -- so this is not a re-transcription.
/// Nothing else about the call is touched.
async fn run_backfill(config_path: Option<PathBuf>) -> Result<()> {
    init_tracing();
    println!("=== CallMind Metadata Backfill ===");

    let config = AppConfig::load_from_file_or_default(config_path)?;
    let db = connect(&config.database.driver, &config.database.url, 4).await?;
    run_migrations_on(&db).await?;
    let call_repo = SqlCallRepository::new(db);
    let storage: Arc<dyn callmind_storage::RecordingStorage> =
        Arc::new(FilesystemStorage::new(&config.storage.path).await?);

    let mut offset = 0u32;
    let (mut filled, mut skipped) = (0usize, 0usize);
    loop {
        let page = call_repo
            .list(&callmind_core::CallFilter {
                limit: Some(500),
                offset: Some(offset),
                ..Default::default()
            })
            .await?;
        if page.is_empty() {
            break;
        }
        offset += page.len() as u32;

        for call in page {
            if call.duration_ms.is_some() {
                continue;
            }
            let Some(recording) = call_repo.get_recording_by_call_id(call.id).await? else {
                skipped += 1;
                continue;
            };
            let path = match storage.get_local_path(&recording.storage_key).await {
                Ok(p) => p,
                Err(e) => {
                    warn!("Call {}: recording unavailable ({e})", call.id);
                    skipped += 1;
                    continue;
                }
            };
            match callmind_audio::AudioDecoder::read_metadata(&path) {
                Ok(Some(meta)) => {
                    call_repo
                        .set_audio_metadata(
                            call.id,
                            meta.duration_ms,
                            meta.channels,
                            meta.sample_rate,
                        )
                        .await?;
                    filled += 1;
                }
                Ok(None) | Err(_) => {
                    warn!("Call {}: could not read audio metadata", call.id);
                    skipped += 1;
                }
            }
        }
    }

    println!("Filled: {filled}   Skipped: {skipped}");
    Ok(())
}

async fn run_reprocess(config_path: Option<PathBuf>, call_id_str: String) -> Result<()> {
    init_tracing();
    let call_id = callmind_core::CallId::from_str(&call_id_str)
        .context(format!("Invalid Call UUID: {call_id_str}"))?;

    println!("=== CallMind Call Reprocessor ===");
    println!("Target Call ID: {}", call_id);

    let config = AppConfig::load_from_file_or_default(config_path)?;
    let db = connect(&config.database.driver, &config.database.url, 4).await?;
    run_migrations_on(&db).await?;

    let call_repo = Arc::new(SqlCallRepository::new(db.clone()));
    let job_repo = Arc::new(SqlJobRepository::new(db.clone()));

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
    init_tracing_with(callmind_config::LogFormat::default());
}

/// Install the log subscriber in the requested format.
///
/// JSON is what lets a log aggregator group by stage and call rather than
/// leaving a human to grep prose. The `json` feature was already a declared
/// dependency and simply unused.
fn init_tracing_with(format: callmind_config::LogFormat) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,callmind=debug,tower_http=debug"));

    let registry = tracing_subscriber::registry().with(filter);
    let _ = match format {
        callmind_config::LogFormat::Text => registry
            .with(tracing_subscriber::fmt::layer().boxed())
            .try_init(),
        callmind_config::LogFormat::Json => registry
            .with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_current_span(true)
                    .boxed(),
            )
            .try_init(),
    };
}

/// A missing weights file otherwise surfaces as a failed job on the first call,
/// long after startup, so refuse to start and name the command that fixes it.
fn ensure_model_present(kind: &str, path: &std::path::Path) -> Result<()> {
    anyhow::ensure!(
        path.exists(),
        "{kind} model not found at {}. Fetch it with `callmind models download <id>` \
         (`callmind models list` shows the ids), or point `models.stt_*` in the config \
         at a file you already have.",
        path.display()
    );
    Ok(())
}

/// The label is stored with every transcript, so it follows the configured file
/// instead of guessing which model is loaded.
fn model_label(path: &std::path::Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("unknown")
        .to_string()
}

#[cfg(test)]
mod stt_model_startup_tests {
    use super::*;

    /// A missing weights file otherwise surfaces as a failed job on the first
    /// call, long after startup -- and switching the default multilingual model
    /// to turbo makes that likely for anyone who downloaded the old default.
    #[test]
    fn a_missing_model_is_refused_at_startup_with_the_command_that_fixes_it() {
        let err = ensure_model_present("multilingual", &PathBuf::from("/nope/turbo.bin"))
            .expect_err("a missing model must not be accepted");
        let msg = err.to_string();
        assert!(msg.contains("/nope/turbo.bin"), "names the file: {msg}");
        assert!(msg.contains("models download"), "names the fix: {msg}");
    }

    #[test]
    fn a_present_model_is_accepted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("turbo.bin");
        std::fs::write(&path, b"weights").expect("write");
        ensure_model_present("multilingual", &path).expect("a present model is fine");
    }

    /// The label is stored with every transcript, so it has to follow the
    /// configured file rather than a compiled-in guess at which model is loaded.
    #[test]
    fn the_engine_label_follows_the_configured_filename() {
        assert_eq!(
            model_label(&PathBuf::from("models/stt/whisper-large-v3-turbo.bin")),
            "whisper-large-v3-turbo"
        );
        assert_eq!(
            model_label(&PathBuf::from("models/stt/whisper-large-v3.bin")),
            "whisper-large-v3"
        );
    }
}
