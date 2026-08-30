//! Configuration loading: YAML file, environment overrides, and validation.
//!
//! Precedence is file, then environment. Validation runs at startup and fails
//! loudly on a misconfiguration rather than degrading silently — an unknown
//! `llm.provider`, for instance, used to fall through to keyword heuristics with
//! no log line.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Failed to read configuration file at '{path}': {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to parse YAML configuration: {0}")]
    Yaml(#[from] serde_yaml::Error),

    #[error("Configuration validation failed: {0}")]
    Validation(String),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppConfig {
    #[serde(default)]
    pub server: ServerConfig,

    #[serde(default)]
    pub database: DatabaseConfig,

    #[serde(default)]
    pub storage: StorageConfig,

    #[serde(default)]
    pub jobs: JobsConfig,

    #[serde(default)]
    pub models: ModelsConfig,

    #[serde(default)]
    pub logging: LoggingConfig,

    #[serde(default)]
    pub llm: LlmConfig,

    #[serde(default)]
    pub watcher: WatcherConfig,

    #[serde(default)]
    pub auth: AuthConfig,

    #[serde(default)]
    pub bots: BotsConfig,

    #[serde(default)]
    pub workers: WorkersConfig,

    #[serde(default)]
    pub outbound_webhook: OutboundWebhookConfig,
}

impl AppConfig {
    /// Load configuration from an optional file path. If no path is provided or the file does not exist,
    /// checks for `callmind.yaml` or `callmind.yml`, then returns default configuration with environment variable overrides.
    pub fn load_from_file_or_default<P: AsRef<Path>>(path: Option<P>) -> Result<Self, ConfigError> {
        let mut config = if let Some(p) = path {
            let path_ref = p.as_ref();
            if path_ref.exists() {
                let content = fs::read_to_string(path_ref).map_err(|e| ConfigError::Io {
                    path: path_ref.to_path_buf(),
                    source: e,
                })?;
                serde_yaml::from_str::<Self>(&content)?
            } else {
                // An explicitly requested path is a user instruction. Silently
                // falling back to defaults here meant a typo'd `--config`
                // started the server with an entirely different configuration.
                return Err(ConfigError::Validation(format!(
                    "configuration file {} does not exist",
                    path_ref.display()
                )));
            }
        } else if Path::new("callmind.yaml").exists() {
            let content = fs::read_to_string("callmind.yaml").map_err(|e| ConfigError::Io {
                path: PathBuf::from("callmind.yaml"),
                source: e,
            })?;
            serde_yaml::from_str::<Self>(&content)?
        } else if Path::new("callmind.yml").exists() {
            let content = fs::read_to_string("callmind.yml").map_err(|e| ConfigError::Io {
                path: PathBuf::from("callmind.yml"),
                source: e,
            })?;
            serde_yaml::from_str::<Self>(&content)?
        } else {
            Self::default()
        };

        config.apply_env_overrides();
        config.validate()?;
        Ok(config)
    }

    /// Apply environment variable overrides (e.g., `CALLMIND_SERVER_BIND="0.0.0.0:8080"`).
    pub fn apply_env_overrides(&mut self) {
        self.apply_overrides_from(|key| std::env::var(key).ok());
    }

    /// Override body with an injectable lookup, so the mapping is testable
    /// without mutating process environment (`set_var` is unsafe in edition 2024).
    pub fn apply_overrides_from<F>(&mut self, lookup: F)
    where
        F: Fn(&str) -> Option<String>,
    {
        if let Some(val) = lookup("CALLMIND_SERVER_BIND") {
            self.server.bind = val;
        }
        if let Some(val) = lookup("CALLMIND_DATABASE_URL") {
            self.database.url = val;
        }
        if let Some(val) = lookup("CALLMIND_STORAGE_PATH") {
            self.storage.path = PathBuf::from(val);
        }
        if let Some(Ok(parsed)) = lookup("CALLMIND_JOBS_WORKERS").map(|v| v.parse::<usize>()) {
            self.jobs.workers = parsed;
        }
        if let Some(val) = lookup("CALLMIND_AUTH_API_KEY") {
            if !val.trim().is_empty() {
                self.auth.api_key = Some(val);
                self.auth.enabled = true;
            }
        }
        if let Some(val) = lookup("CALLMIND_OUTBOUND_WEBHOOK_URL") {
            if !val.trim().is_empty() {
                self.outbound_webhook.url = Some(val);
            }
        }
        if let Some(val) = lookup("CALLMIND_OUTBOUND_WEBHOOK_SECRET") {
            if !val.trim().is_empty() {
                self.outbound_webhook.secret = Some(val);
            }
        }
        if let Some(val) = lookup("CALLMIND_TELEGRAM_BOT_TOKEN") {
            if !val.trim().is_empty() {
                self.bots.telegram.bot_token = Some(val);
                self.bots.telegram.enabled = true;
            }
        }
        if let Some(val) = lookup("CALLMIND_EVOLUTION_API_KEY") {
            if !val.trim().is_empty() {
                self.bots.evolution.api_key = Some(val);
                self.bots.evolution.enabled = true;
            }
        }
        if let Some(val) = lookup("CALLMIND_EVOLUTION_BASE_URL") {
            if !val.trim().is_empty() {
                self.bots.evolution.base_url = Some(val);
            }
        }
        if let Some(val) = lookup("CALLMIND_EVOLUTION_INSTANCE") {
            if !val.trim().is_empty() {
                self.bots.evolution.instance = Some(val);
            }
        }
        if let Some(val) = lookup("CALLMIND_EVOLUTION_WEBHOOK_TOKEN") {
            if !val.trim().is_empty() {
                self.bots.evolution.webhook_token = Some(val);
            }
        }
        if let Some(val) = lookup("CALLMIND_WEBHOOK_SECRET_TOKEN") {
            if !val.trim().is_empty() {
                self.bots.webhook.secret_token = Some(val);
            }
        }
        // docker-compose.yml sets all three of these; without them the
        // container silently kept the baked-in `http://localhost:11434` and
        // could never reach the `ollama` service.
        if let Some(val) = lookup("CALLMIND_LLM_PROVIDER") {
            if !val.trim().is_empty() {
                self.llm.provider = val;
            }
        }
        if let Some(val) = lookup("CALLMIND_LLM_ENDPOINT") {
            if !val.trim().is_empty() {
                self.llm.endpoint = val;
            }
        }
        if let Some(val) = lookup("CALLMIND_LLM_MODEL") {
            if !val.trim().is_empty() {
                self.llm.model = val;
            }
        }
        if let Some(val) = lookup("CALLMIND_LLM_API_KEY") {
            if !val.trim().is_empty() {
                self.llm.api_key = Some(val);
            }
        }
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.server.bind.is_empty() {
            return Err(ConfigError::Validation(
                "server.bind cannot be empty".into(),
            ));
        }
        if self.database.url.is_empty() {
            return Err(ConfigError::Validation(
                "database.url cannot be empty".into(),
            ));
        }
        // Not even with remote workers configured. A remote worker offloads
        // transcription and hands the job back as `analyze_call`, which needs
        // the LLM and the database -- so the service's own pool has to run it.
        // Zero workers leaves every call transcribed and never analysed.
        for kind in &self.jobs.kinds {
            if kind.parse::<callmind_core::JobKind>().is_err() {
                return Err(ConfigError::Validation(format!(
                    "jobs.kinds entry {kind:?} is not a job kind. Use `ingest_recording`, \
                     `analyze_call`, `deliver_webhook`, or `plugin:<name>`."
                )));
            }
        }
        if !self.jobs.kinds.is_empty() {
            // Restricting the pool is how transcription is handed to a remote
            // worker; excluding analysis instead just stops calls being
            // analysed, silently, since the jobs queue up with nobody to take
            // them.
            if !self.jobs.kinds.iter().any(|k| k == "analyze_call") {
                return Err(ConfigError::Validation(
                    "jobs.kinds does not include `analyze_call`, so no call would ever be \
                     analysed. The analysis stage needs the LLM and the database, so it \
                     always runs in this process."
                        .into(),
                ));
            }
            if !self.jobs.kinds.iter().any(|k| k == "ingest_recording") && !self.workers.enabled {
                return Err(ConfigError::Validation(
                    "jobs.kinds excludes `ingest_recording` and workers.enabled is false, \
                     so nothing would ever transcribe. Enable the worker listener, or let \
                     this host take transcription jobs too."
                        .into(),
                ));
            }
        }
        if self.jobs.workers == 0 {
            return Err(ConfigError::Validation(
                "jobs.workers must be at least 1. Remote workers offload transcription, \
                 but the analysis stage runs in this process."
                    .into(),
            ));
        }
        if self.auth.enabled && self.auth.api_key.as_deref().unwrap_or("").trim().is_empty() {
            return Err(ConfigError::Validation(
                "auth.enabled is true, but auth.api_key is empty or unconfigured".into(),
            ));
        }
        if self.bots.evolution.enabled {
            for (field, value) in [
                ("base_url", &self.bots.evolution.base_url),
                ("instance", &self.bots.evolution.instance),
                ("api_key", &self.bots.evolution.api_key),
            ] {
                if value.as_deref().unwrap_or("").trim().is_empty() {
                    return Err(ConfigError::Validation(format!(
                        "bots.evolution.enabled is true, but bots.evolution.{field} is not set"
                    )));
                }
            }
        }
        let provider = self.llm.provider.trim().to_lowercase();
        if !SUPPORTED_LLM_PROVIDERS.contains(&provider.as_str()) {
            return Err(ConfigError::Validation(format!(
                "llm.provider {:?} is not supported; expected one of {:?}",
                self.llm.provider, SUPPORTED_LLM_PROVIDERS
            )));
        }
        // Here rather than at the call site that starts the listener: a refused
        // worker configuration must be impossible, not merely fatal once the
        // pool is already transcribing and holding jobs `Running`.
        self.workers.validate().map_err(ConfigError::Validation)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServerConfig {
    #[serde(default = "default_bind")]
    pub bind: String,

    #[serde(default = "default_body_limit_mb")]
    pub body_limit_mb: usize,
}

fn default_bind() -> String {
    "127.0.0.1:8080".to_string()
}

fn default_body_limit_mb() -> usize {
    500
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: default_bind(),
            body_limit_mb: default_body_limit_mb(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DatabaseConfig {
    #[serde(default = "default_db_driver")]
    pub driver: String,

    #[serde(default = "default_db_url")]
    pub url: String,

    #[serde(default = "default_max_connections")]
    pub max_connections: u32,

    #[serde(default = "default_busy_timeout_ms")]
    pub busy_timeout_ms: u64,
}

fn default_db_driver() -> String {
    "sqlite".to_string()
}

fn default_db_url() -> String {
    "./data/callmind.db".to_string()
}

fn default_max_connections() -> u32 {
    16
}

fn default_busy_timeout_ms() -> u64 {
    5000
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            driver: default_db_driver(),
            url: default_db_url(),
            max_connections: default_max_connections(),
            busy_timeout_ms: default_busy_timeout_ms(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StorageConfig {
    #[serde(default = "default_storage_driver")]
    pub driver: String,

    #[serde(default = "default_storage_path")]
    pub path: PathBuf,
}

fn default_storage_driver() -> String {
    "filesystem".to_string()
}

fn default_storage_path() -> PathBuf {
    PathBuf::from("./data/recordings")
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            driver: default_storage_driver(),
            path: default_storage_path(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobsConfig {
    #[serde(default = "default_workers")]
    pub workers: usize,

    /// Job kinds this host's own workers take. Empty means every kind that has
    /// a handler registered, which is the single-machine default.
    ///
    /// Naming them is how a host stops competing with a remote worker. The
    /// local pool polls every half second and a worker over a network cannot
    /// win that race, so without this a GPU box would only ever pick up the
    /// jobs the host happened to miss.
    #[serde(default)]
    pub kinds: Vec<String>,

    #[serde(default = "default_poll_interval_ms")]
    pub poll_interval_ms: u64,

    #[serde(default = "default_lock_timeout_secs")]
    pub lock_timeout_secs: u64,

    #[serde(default = "default_max_attempts")]
    pub max_attempts: i32,
}

fn default_workers() -> usize {
    4
}

fn default_poll_interval_ms() -> u64 {
    500
}

fn default_lock_timeout_secs() -> u64 {
    600
}

fn default_max_attempts() -> i32 {
    3
}

impl Default for JobsConfig {
    fn default() -> Self {
        Self {
            workers: default_workers(),
            kinds: Vec::new(),
            poll_interval_ms: default_poll_interval_ms(),
            lock_timeout_secs: default_lock_timeout_secs(),
            max_attempts: default_max_attempts(),
        }
    }
}

/// Remote worker listener.
///
/// A worker is a separate process — possibly closed-source, possibly not in
/// Rust — that leases jobs over gRPC. It never touches the database.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct WorkersConfig {
    #[serde(default)]
    pub enabled: bool,

    #[serde(default = "default_workers_bind")]
    pub bind: String,

    /// Server side of mutual TLS. Required once `bind` leaves loopback.
    #[serde(default)]
    pub tls: Option<WorkerTlsConfig>,

    /// Workers allowed to connect, each pinned to its certificate.
    ///
    /// Pinned rather than issued by a certificate authority, which is what
    /// HashiCorp's go-plugin does for the same reason: it needs no X.509
    /// parsing, leaves no question of `CN` versus `SAN`, and revoking a worker
    /// is deleting a line. A CA can replace this later without touching the
    /// protocol — how the server establishes identity is its own business.
    #[serde(default)]
    pub allowed: Vec<AllowedWorker>,

    /// Plugin kinds dispatched after every transcript.
    #[serde(default)]
    pub plugin_kinds: Vec<String>,
}

/// Server certificate and key for the worker listener.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerTlsConfig {
    pub server_cert: PathBuf,
    pub server_key: PathBuf,
}

/// One worker, pinned to the certificate it presents.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AllowedWorker {
    /// Identity used in logs and lease ownership.
    pub name: String,
    /// PEM file holding exactly the certificate this worker presents.
    pub certificate: PathBuf,
}

impl WorkersConfig {
    /// Reject configurations that would expose recordings to anyone who can
    /// reach the port.
    ///
    /// Returns the reason rather than logging it: the caller fails startup with
    /// it, the way a missing model file already does.
    pub fn validate(&self) -> Result<(), String> {
        // Ahead of the `enabled` check, because these are dispatched by the
        // pipeline whether or not anything is listening for them. Each travels
        // as the job kind `plugin:<name>` and is submitted back under the same
        // name: an empty or exotic name makes a job nothing can parse -- which
        // breaks the whole call's job list -- and a result the listener refuses.
        for kind in &self.plugin_kinds {
            if kind.is_empty()
                || !kind
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                return Err(format!(
                    "workers.plugin_kinds entry {kind:?} is not a usable plugin name; \
                     use ASCII letters, digits, '-' and '_'."
                ));
            }
        }

        if !self.enabled {
            return Ok(());
        }

        // Checked before the bind address: TLS with nobody pinned accepts no
        // one on loopback just as much as anywhere else, and only surfaces at
        // the first connection otherwise.
        if self.tls.is_some() && self.allowed.is_empty() {
            return Err(
                "workers.tls is set but workers.allowed is empty, so no worker could connect. \
                 Pin each worker to its certificate."
                    .to_string(),
            );
        }

        // Workers are keyed by their certificate's fingerprint, so two entries
        // sharing a certificate collapse into one and the other name is
        // silently dead.
        let mut seen = std::collections::HashSet::new();
        for worker in &self.allowed {
            if !seen.insert(&worker.certificate) {
                return Err(format!(
                    "workers.allowed pins {} more than once. A certificate identifies exactly one \
                     worker, so only the last name using it would ever be handed out.",
                    worker.certificate.display()
                ));
            }
        }

        let is_loopback = self
            .bind
            .parse::<std::net::SocketAddr>()
            .map(|addr| addr.ip().is_loopback())
            .unwrap_or(false);

        if is_loopback {
            return Ok(());
        }

        if self.tls.is_none() {
            return Err(format!(
                "workers.bind is {}, which is reachable beyond this machine, and workers.tls is not set. \
                 A worker leases jobs and downloads recordings, so the listener must require a client certificate.",
                self.bind
            ));
        }

        Ok(())
    }
}

fn default_workers_bind() -> String {
    "127.0.0.1:8081".to_string()
}

/// Provider strings the LLM factory understands. Validated at startup so a typo
/// fails loudly instead of silently degrading analysis to keyword heuristics.
pub const SUPPORTED_LLM_PROVIDERS: &[&str] = &[
    "ollama",
    "openai",
    "groq",
    "vllm",
    "anthropic",
    "claude",
    "heuristic",
    "local",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmConfig {
    #[serde(default = "default_llm_provider")]
    pub provider: String,

    #[serde(default = "default_llm_endpoint")]
    pub endpoint: String,

    #[serde(default = "default_llm_model")]
    pub model: String,

    #[serde(default)]
    pub api_key: Option<String>,

    /// Tokens the model is given to work with.
    ///
    /// Sent to Ollama as `num_ctx`. Ollama defaults to a couple of thousand and
    /// silently drops the rest of the prompt: measured on a real archive, a
    /// 13-minute call formats to ~4160 tokens, so roughly half of it never
    /// reached the model. The analyser compresses whatever still does not fit
    /// rather than letting it be cut.
    #[serde(default = "default_llm_context_tokens")]
    pub context_tokens: usize,
}

fn default_llm_context_tokens() -> usize {
    // Comfortable for a few minutes of speech without making the KV cache
    // expensive on a small local model.
    8192
}

fn default_llm_provider() -> String {
    "ollama".to_string()
}

fn default_llm_endpoint() -> String {
    "http://localhost:11434".to_string()
}

fn default_llm_model() -> String {
    "qwen2.5:7b".to_string()
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: default_llm_provider(),
            endpoint: default_llm_endpoint(),
            model: default_llm_model(),
            api_key: None,
            context_tokens: default_llm_context_tokens(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WatcherConfig {
    #[serde(default)]
    pub enabled: bool,

    #[serde(default = "default_watch_dir")]
    pub watch_dir: PathBuf,

    #[serde(default = "default_poll_secs")]
    pub poll_secs: u64,
}

fn default_watch_dir() -> PathBuf {
    PathBuf::from("./incoming")
}

fn default_poll_secs() -> u64 {
    5
}

impl Default for WatcherConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            watch_dir: default_watch_dir(),
            poll_secs: default_poll_secs(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthConfig {
    #[serde(default)]
    pub enabled: bool,

    #[serde(default)]
    pub api_key: Option<String>,

    #[serde(default)]
    pub allowed_origins: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct BotsConfig {
    #[serde(default)]
    pub telegram: TelegramBotConfig,

    #[serde(default)]
    pub evolution: EvolutionBotConfig,

    #[serde(default)]
    pub webhook: WebhookBotConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TelegramBotConfig {
    #[serde(default)]
    pub enabled: bool,

    #[serde(default)]
    pub bot_token: Option<String>,

    #[serde(default)]
    pub allowed_chat_ids: Vec<i64>,
}

/// Self-hosted [Evolution API](https://evolution-api.com) WhatsApp gateway.
///
/// Replaces the Meta Cloud API integration, which needed business verification
/// and a `phone_number_id` and never had a working message handler. Evolution
/// pairs with an ordinary WhatsApp account over QR.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvolutionBotConfig {
    #[serde(default)]
    pub enabled: bool,

    /// Base URL of the Evolution API deployment, e.g. `http://localhost:8080`.
    #[serde(default)]
    pub base_url: Option<String>,

    /// Instance name created in Evolution API.
    #[serde(default)]
    pub instance: Option<String>,

    /// Value for the `ApiKey` header (global or instance key).
    #[serde(default)]
    pub api_key: Option<String>,

    /// Shared secret required on the inbound webhook. Evolution does not sign
    /// its webhooks, so without this anyone who can reach the route could inject
    /// fabricated calls.
    #[serde(default)]
    pub webhook_token: Option<String>,

    /// Optional allowlist of sender numbers (digits only, no `@s.whatsapp.net`).
    /// Empty means allow everyone the instance can receive from.
    #[serde(default)]
    pub allowed_numbers: Vec<String>,

    /// How long to wait for analysis before giving up on replying with results.
    ///
    /// Generous because the first call after startup also pays for loading the
    /// Whisper weights: measured at ~305s for a 5-second voice note on a cold
    /// process, against ~20s once the model is resident.
    #[serde(default = "default_result_timeout_secs")]
    pub result_timeout_secs: u64,
}

fn default_result_timeout_secs() -> u64 {
    600
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebhookBotConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,

    #[serde(default)]
    pub secret_token: Option<String>,
}

/// Where to POST a call once it finishes, making CallMind a producer rather than
/// only a consumer of audio.
///
/// Off unless a URL is set: this is the one setting that sends call content off
/// the machine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OutboundWebhookConfig {
    /// Receiver URL. Absent or empty disables delivery.
    #[serde(default, deserialize_with = "empty_string_as_none")]
    pub url: Option<String>,

    /// Sent as `X-CallMind-Secret` so the receiver can tell the request is ours.
    #[serde(default, deserialize_with = "empty_string_as_none")]
    pub secret: Option<String>,

    #[serde(default = "default_webhook_timeout_seconds")]
    pub timeout_seconds: u64,
}

impl Default for OutboundWebhookConfig {
    fn default() -> Self {
        Self {
            url: None,
            secret: None,
            timeout_seconds: default_webhook_timeout_seconds(),
        }
    }
}

fn default_webhook_timeout_seconds() -> u64 {
    30
}

/// An unset environment variable expands to an empty string in a compose file or
/// a shell, and reading that as a value enables things nobody asked for.
fn empty_string_as_none<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = Option::<String>::deserialize(deserializer)?;
    Ok(raw.filter(|value| !value.trim().is_empty()))
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelsConfig {
    #[serde(default = "default_models_dir")]
    pub models_dir: PathBuf,

    /// Speech-to-text weights, relative to `models_dir`.
    ///
    /// Configurable because transcription is 88.8% of processing time -- 74.8 s of
    /// an 84.2 s total on a 13.8-minute call -- and the model is the only real
    /// lever on it. These filenames used to be compiled in, so trying a faster
    /// model meant rebuilding.
    ///
    /// The default is `whisper-large-v3-turbo` rather than the full
    /// `whisper-large-v3`, measured on two real Russian calls of about 14
    /// minutes each: turbo transcribed 2.24x and 2.93x faster, and was the
    /// better transcript on both -- which was not the expected direction.
    /// Full-v3 emitted more words, but they were repetition loops (words inside
    /// an immediately repeated 5-gram: 11 versus 0, and 14 versus 1). Where it
    /// looped, turbo had real speech: one 10-word loop stands where turbo
    /// transcribed 40 words of conversation. The loops are also why it was
    /// slower, since every repeat is decoded. Turbo is not strictly better --
    /// it dropped one 31-word run the full model kept -- but its failures are
    /// smaller and rarer.
    ///
    /// Point this at `stt/whisper-large-v3.bin` for a language where the full
    /// model earns its time. Both calls measured here were Russian; English,
    /// Arabic and the rest are untested, and Hebrew never reaches this model
    /// because language identification routes it to `stt_hebrew`.
    #[serde(default = "default_stt_multilingual")]
    pub stt_multilingual: String,

    /// Used when language identification is confident the call is Hebrew.
    #[serde(default = "default_stt_hebrew")]
    pub stt_hebrew: String,
}

fn default_stt_multilingual() -> String {
    "stt/whisper-large-v3-turbo.bin".to_string()
}

fn default_stt_hebrew() -> String {
    "stt/ivrit-ai-large-v3-turbo.bin".to_string()
}

fn default_models_dir() -> PathBuf {
    PathBuf::from("./models")
}

impl Default for ModelsConfig {
    fn default() -> Self {
        Self {
            models_dir: default_models_dir(),
            stt_multilingual: default_stt_multilingual(),
            stt_hebrew: default_stt_hebrew(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// docker-compose.yml sets the three LLM variables; none of them were
    /// wired, so the container silently kept `http://localhost:11434` and could
    /// never reach the `ollama` service.
    #[test]
    fn test_env_overrides_cover_compose_and_secret_vars() {
        let env = |key: &str| -> Option<String> {
            match key {
                "CALLMIND_LLM_ENDPOINT" => Some("http://ollama:11434".to_string()),
                "CALLMIND_LLM_PROVIDER" => Some("ollama".to_string()),
                "CALLMIND_LLM_MODEL" => Some("llama3.2:3b".to_string()),
                "CALLMIND_LLM_API_KEY" => Some("llm-key".to_string()),
                "CALLMIND_WEBHOOK_SECRET_TOKEN" => Some("hook-secret".to_string()),
                "CALLMIND_EVOLUTION_BASE_URL" => Some("http://evolution:8080".to_string()),
                "CALLMIND_EVOLUTION_INSTANCE" => Some("family".to_string()),
                "CALLMIND_EVOLUTION_API_KEY" => Some("evo-key".to_string()),
                "CALLMIND_EVOLUTION_WEBHOOK_TOKEN" => Some("hook-token".to_string()),
                "CALLMIND_JOBS_WORKERS" => Some("7".to_string()),
                _ => None,
            }
        };

        let mut config = AppConfig::default();
        config.apply_overrides_from(env);

        assert_eq!(config.llm.endpoint, "http://ollama:11434");
        assert_eq!(config.llm.provider, "ollama");
        assert_eq!(config.llm.model, "llama3.2:3b");
        assert_eq!(config.llm.api_key.as_deref(), Some("llm-key"));
        assert_eq!(
            config.bots.webhook.secret_token.as_deref(),
            Some("hook-secret")
        );
        assert_eq!(
            config.bots.evolution.base_url.as_deref(),
            Some("http://evolution:8080")
        );
        assert_eq!(config.bots.evolution.instance.as_deref(), Some("family"));
        assert_eq!(config.bots.evolution.api_key.as_deref(), Some("evo-key"));
        assert_eq!(
            config.bots.evolution.webhook_token.as_deref(),
            Some("hook-token")
        );
        assert!(
            config.bots.evolution.enabled,
            "supplying an API key should enable the channel"
        );
        assert_eq!(config.jobs.workers, 7);

        // Absent variables must leave the configured value untouched.
        let mut untouched = AppConfig::default();
        untouched.apply_overrides_from(|_| None);
        assert_eq!(untouched, AppConfig::default());
    }

    #[test]
    fn test_default_config_validates() {
        let config = AppConfig::default();
        assert!(config.validate().is_ok());
        assert_eq!(config.server.bind, "127.0.0.1:8080");
        assert_eq!(config.database.driver, "sqlite");
    }

    #[test]
    fn test_yaml_parsing() {
        let yaml_str = r#"
server:
  bind: "127.0.0.1:9090"
  body_limit_mb: 100

database:
  driver: "sqlite"
  url: ":memory:"

jobs:
  workers: 8
"#;
        let config: AppConfig = serde_yaml::from_str(yaml_str).unwrap();
        assert_eq!(config.server.bind, "127.0.0.1:9090");
        assert_eq!(config.server.body_limit_mb, 100);
        assert_eq!(config.database.url, ":memory:");
        assert_eq!(config.jobs.workers, 8);
    }

    /// A typo'd `llm.provider` used to fall through to keyword heuristics with
    /// no log line, so "AI analysis" silently became regex forever.
    #[test]
    fn test_unknown_llm_provider_is_rejected() {
        let mut config = AppConfig::default();
        assert!(config.validate().is_ok(), "default provider must be valid");

        config.llm.provider = "olama".to_string();
        let err = config.validate().expect_err("typo must not validate");
        assert!(
            err.to_string().contains("olama"),
            "error should name the bad value, got: {err}"
        );

        // Case and surrounding whitespace are tolerated.
        config.llm.provider = "  OpenAI ".to_string();
        assert!(config.validate().is_ok());
    }

    /// An explicit `--config /typo.yaml` used to silently start the server on
    /// built-in defaults instead of reporting the bad path.
    #[test]
    fn test_missing_explicit_config_path_is_an_error() {
        let missing = std::path::Path::new("definitely/not/here/callmind.yaml");
        assert!(!missing.exists());

        let err = AppConfig::load_from_file_or_default(Some(missing))
            .expect_err("a missing explicit path must fail");
        assert!(
            err.to_string().contains("does not exist"),
            "unexpected error: {err}"
        );
    }
}

/// How log lines are written.
///
/// The `json` feature of `tracing-subscriber` was already enabled and never
/// used, so every line went out as prose. Structured output is what makes a log
/// aggregator able to answer "which stage was slow" instead of a human grepping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    /// Human-readable, for a terminal.
    #[default]
    Text,
    /// One JSON object per line, for shipping somewhere.
    Json,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoggingConfig {
    #[serde(default)]
    pub format: LogFormat,
}

#[cfg(test)]
mod logging_config_tests {
    use super::*;

    #[test]
    fn the_format_is_text_unless_asked_otherwise() {
        let config: LoggingConfig = serde_yaml::from_str("{}").expect("empty section");
        assert_eq!(config.format, LogFormat::Text);
        assert_eq!(LoggingConfig::default().format, LogFormat::Text);
    }

    #[test]
    fn json_can_be_selected_by_name() {
        let config: LoggingConfig = serde_yaml::from_str("format: json").expect("json");
        assert_eq!(config.format, LogFormat::Json);
    }

    /// A typo must be loud. Silently falling back to text is how a deployment
    /// ends up shipping prose to a log aggregator for a year.
    #[test]
    fn an_unknown_format_is_rejected_rather_than_ignored() {
        assert!(serde_yaml::from_str::<LoggingConfig>("format: jsonl").is_err());
    }
}

#[cfg(test)]
mod stt_model_config_tests {
    use super::*;

    /// Turbo is the default because it was measured 2-3x faster and the better
    /// transcript on real recordings -- see [`ModelsConfig::stt_multilingual`].
    #[test]
    fn the_default_multilingual_model_is_the_turbo_one() {
        let config = ModelsConfig::default();
        assert_eq!(config.stt_multilingual, "stt/whisper-large-v3-turbo.bin");
        assert_eq!(config.stt_hebrew, "stt/ivrit-ai-large-v3-turbo.bin");
    }

    /// The knob has to work in both directions: a language where the full model
    /// is worth its time must be selectable without a recompile.
    #[test]
    fn the_slower_accurate_model_can_be_selected_without_rebuilding() {
        let config: ModelsConfig =
            serde_yaml::from_str("stt_multilingual: stt/whisper-large-v3.bin").expect("parses");
        assert_eq!(config.stt_multilingual, "stt/whisper-large-v3.bin");
        assert_eq!(
            config.stt_hebrew, "stt/ivrit-ai-large-v3-turbo.bin",
            "an unset field keeps its default"
        );
        assert_eq!(config.models_dir, default_models_dir());
    }
}

#[cfg(test)]
mod outbound_webhook_config_tests {
    use super::*;

    /// Off unless asked for: this is the one setting that sends call content off
    /// the machine, so an absent section must not enable it.
    #[test]
    fn it_is_disabled_by_default() {
        let config: AppConfig = serde_yaml::from_str("server: {}").expect("parses");
        assert_eq!(config.outbound_webhook.url, None);
        assert_eq!(config.outbound_webhook.secret, None);
        assert_eq!(config.outbound_webhook.timeout_seconds, 30);
    }

    #[test]
    fn a_configured_receiver_is_read_with_its_secret() {
        let config: AppConfig = serde_yaml::from_str(
            "outbound_webhook:\n  url: https://example.test/hook\n  secret: shh\n  timeout_seconds: 5\n",
        )
        .expect("parses");
        assert_eq!(
            config.outbound_webhook.url.as_deref(),
            Some("https://example.test/hook")
        );
        assert_eq!(config.outbound_webhook.secret.as_deref(), Some("shh"));
        assert_eq!(config.outbound_webhook.timeout_seconds, 5);
    }

    /// An empty string is what a shell writes for an unset variable, and reading
    /// it as a receiver URL would post every call to nowhere on every call.
    #[test]
    fn an_empty_url_counts_as_no_receiver() {
        let config: AppConfig =
            serde_yaml::from_str("outbound_webhook:\n  url: \"\"\n").expect("parses");
        assert_eq!(config.outbound_webhook.url, None);
    }
}

#[cfg(test)]
mod outbound_webhook_env_tests {
    use super::*;
    use std::collections::HashMap;

    /// The secret would otherwise only be settable by writing it into a YAML file
    /// on disk, which is the wrong place for it in a container.
    #[test]
    fn the_receiver_and_its_secret_come_from_the_environment() {
        let env: HashMap<&str, &str> = HashMap::from([
            ("CALLMIND_OUTBOUND_WEBHOOK_URL", "https://n8n.test/hook"),
            ("CALLMIND_OUTBOUND_WEBHOOK_SECRET", "shh"),
        ]);
        let mut config = AppConfig::default();
        config.apply_overrides_from(|key| env.get(key).map(|v| (*v).to_string()));

        assert_eq!(
            config.outbound_webhook.url.as_deref(),
            Some("https://n8n.test/hook")
        );
        assert_eq!(config.outbound_webhook.secret.as_deref(), Some("shh"));
    }

    #[test]
    fn an_empty_variable_does_not_enable_delivery() {
        let env: HashMap<&str, &str> = HashMap::from([("CALLMIND_OUTBOUND_WEBHOOK_URL", "")]);
        let mut config = AppConfig::default();
        config.apply_overrides_from(|key| env.get(key).map(|v| (*v).to_string()));
        assert_eq!(config.outbound_webhook.url, None);
    }
}

#[cfg(test)]
mod llm_model_default_tests {
    use super::*;

    /// llama3.2:3b degenerated into a repetition loop on a real Hebrew call --
    /// invalid JSON, generation capped at 4096 tokens, 43 s -- where qwen2.5:7b
    /// returned a correct Hebrew analysis in 14 s from the same prompt. In a
    /// Hebrew-first project the smaller model is the wrong default.
    #[test]
    fn the_default_model_is_one_that_can_write_hebrew() {
        assert_eq!(LlmConfig::default().model, "qwen2.5:7b");
    }
}

#[cfg(test)]
mod removed_slack_section_tests {
    use super::*;

    /// The `bots.slack` section had no handler behind it -- it invited putting a
    /// real `xoxb-` token in a file where nothing would ever read it. Removing a
    /// key must not break configs that still carry it.
    #[test]
    fn a_config_that_still_names_slack_still_loads() {
        let config: AppConfig = serde_yaml::from_str(
            "bots:\n  slack:\n    enabled: true\n    bot_token: xoxb-still-here\n  telegram:\n    enabled: true\n",
        )
        .expect("an unknown section is ignored, not rejected");

        assert!(config.bots.telegram.enabled, "the rest of the file is read");
    }
}

#[cfg(test)]
mod worker_tls_config_tests {
    use super::*;

    /// The worker port is where a remote process leases jobs and downloads
    /// recordings. Exposing it beyond loopback without TLS would let anyone who
    /// Naming the kinds is how a host stops competing with a GPU box for
    /// transcription -- the local pool polls every half second, so a worker over
    /// a network never wins that race otherwise.
    #[test]
    fn a_host_can_leave_transcription_to_a_remote_worker() {
        let mut config = AppConfig::default();
        config.workers.enabled = true;
        config.workers.bind = "127.0.0.1:8081".to_string();
        config.jobs.kinds = vec!["analyze_call".into(), "deliver_webhook".into()];
        config
            .validate()
            .expect("the host analyses, the worker transcribes");
    }

    #[test]
    fn a_kinds_list_that_would_strand_work_is_refused() {
        // Excluding analysis strands every call after transcription...
        let mut config = AppConfig::default();
        config.jobs.kinds = vec!["ingest_recording".into()];
        let err = config.validate().expect_err("nothing would analyse");
        assert!(err.to_string().contains("analyze_call"), "{err}");

        // ...and excluding transcription with no remote worker strands it
        // before transcription instead.
        config.jobs.kinds = vec!["analyze_call".into()];
        config.workers.enabled = false;
        let err = config.validate().expect_err("nothing would transcribe");
        assert!(err.to_string().contains("ingest_recording"), "{err}");

        // A name that is not a kind at all is a typo, not a restriction.
        config.jobs.kinds = vec!["analyse_call".into()];
        let err = config.validate().expect_err("a typo must not pass");
        assert!(err.to_string().contains("analyse_call"), "{err}");
    }

    /// Offloading transcription to a GPU box does not empty the local pool: the
    /// worker hands the job back as `analyze_call`, and that stage needs the LLM
    /// and the database. Zero workers would transcribe every call and analyse
    /// none of them, with no error anywhere.
    #[test]
    fn zero_local_workers_is_refused_even_with_remote_workers_configured() {
        let mut config = AppConfig::default();
        config.jobs.workers = 0;
        config.workers.enabled = true;
        config.workers.bind = "127.0.0.1:8081".to_string();

        let err = config
            .validate()
            .expect_err("the analysis stage would never run");
        let msg = err.to_string();
        assert!(msg.contains("jobs.workers"), "names the setting: {msg}");
        assert!(
            msg.contains("analysis"),
            "says why a remote worker does not cover it: {msg}"
        );
    }

    /// reaches it take a job, download the audio and submit a fabricated
    /// transcript, so the unsafe combination is refused rather than warned about.
    #[test]
    fn a_non_loopback_bind_without_tls_is_refused() {
        let config: WorkersConfig =
            serde_yaml::from_str("enabled: true\nbind: \"0.0.0.0:8081\"\n").expect("parses");

        let err = config.validate().expect_err("must be refused");

        assert!(err.contains("0.0.0.0:8081"), "names the address: {err}");
        assert!(err.contains("tls"), "names what is missing: {err}");
    }

    #[test]
    fn loopback_without_tls_is_allowed() {
        let config: WorkersConfig =
            serde_yaml::from_str("enabled: true\nbind: \"127.0.0.1:8081\"\n").expect("parses");
        config.validate().expect("loopback needs no certificate");
    }

    #[test]
    fn a_disabled_listener_is_not_validated() {
        let config: WorkersConfig =
            serde_yaml::from_str("enabled: false\nbind: \"0.0.0.0:8081\"\n").expect("parses");
        config.validate().expect("nothing is listening");
    }

    #[test]
    fn a_non_loopback_bind_with_tls_and_a_pinned_worker_is_allowed() {
        let config: WorkersConfig = serde_yaml::from_str(
            "enabled: true\nbind: \"0.0.0.0:8081\"\ntls:\n  server_cert: /etc/callmind/server.pem\n  server_key: /etc/callmind/server-key.pem\nallowed:\n  - name: gpu-1\n    certificate: /etc/callmind/gpu-1.pem\n",
        )
        .expect("parses");
        config.validate().expect("configured properly");
    }

    /// TLS with nobody pinned would accept no one, which is a configuration
    /// mistake worth naming at startup rather than at the first connection --
    /// on loopback as much as anywhere else, since the bind address has nothing
    /// to do with the mistake.
    #[test]
    fn tls_without_any_pinned_worker_is_refused() {
        for bind in ["0.0.0.0:8081", "127.0.0.1:8081"] {
            let config: WorkersConfig = serde_yaml::from_str(&format!(
                "enabled: true\nbind: \"{bind}\"\ntls:\n  server_cert: /a.pem\n  server_key: /b.pem\n",
            ))
            .expect("parses");

            let err = config.validate().expect_err("must be refused");
            assert!(err.contains("allowed"), "names the empty list: {err}");
        }
    }

    /// The listener keys workers by their certificate's fingerprint, so a
    /// certificate reused across two entries leaves one of the names dead with
    /// nothing to show for it.
    #[test]
    fn the_same_certificate_pinned_twice_is_refused() {
        let config: WorkersConfig = serde_yaml::from_str(
            "enabled: true\nbind: \"127.0.0.1:8081\"\ntls:\n  server_cert: /a.pem\n  server_key: /b.pem\nallowed:\n  - name: gpu-1\n    certificate: /etc/callmind/shared.pem\n  - name: gpu-2\n    certificate: /etc/callmind/shared.pem\n",
        )
        .expect("parses");

        let err = config.validate().expect_err("must be refused");
        assert!(err.contains("shared.pem"), "names the certificate: {err}");
    }

    /// A plugin kind travels as `plugin:<name>`: empty, and the job kind fails
    /// to parse for the whole call; exotic, and the listener refuses the result
    /// the worker sends back. Either way the job is dispatched and can never
    /// complete.
    #[test]
    fn a_plugin_kind_that_could_not_round_trip_is_refused() {
        for bad in ["", "acoustic emotions", "../escape", "emotions!"] {
            // Both listener states: the pipeline dispatches these jobs whether
            // or not a worker port is open.
            for enabled in [true, false] {
                let config = WorkersConfig {
                    enabled,
                    bind: default_workers_bind(),
                    plugin_kinds: vec![bad.to_string()],
                    ..WorkersConfig::default()
                };
                let err = config.validate().expect_err("must be refused");
                assert!(err.contains("plugin_kinds"), "names the setting: {err}");
            }
        }
    }

    #[test]
    fn an_ordinary_plugin_kind_is_allowed() {
        let config = WorkersConfig {
            enabled: true,
            bind: default_workers_bind(),
            plugin_kinds: vec!["acoustic-emotions".to_string(), "scorecard_v2".to_string()],
            ..WorkersConfig::default()
        };
        config.validate().expect("ordinary names are fine");
    }

    /// The gate is `AppConfig::validate`, which runs at load. Checked anywhere
    /// later, a refused configuration would have already started the worker
    /// pool and the watcher, leaving jobs locked `Running` when the process
    /// aborted.
    #[test]
    fn the_workers_section_is_checked_by_the_whole_config_gate() {
        let mut config = AppConfig::default();
        config.workers.enabled = true;
        config.workers.bind = "0.0.0.0:8081".to_string();

        let err = config
            .validate()
            .expect_err("an exposed listener without TLS must not load");
        assert!(
            format!("{err}").contains("workers.tls"),
            "names what is missing: {err}"
        );
    }
}
