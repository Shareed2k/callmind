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
        if self.jobs.workers == 0 {
            return Err(ConfigError::Validation(
                "jobs.workers must be at least 1".into(),
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
            poll_interval_ms: default_poll_interval_ms(),
            lock_timeout_secs: default_lock_timeout_secs(),
            max_attempts: default_max_attempts(),
        }
    }
}

/// gRPC listener for remote processing workers.
///
/// A separate port from the HTTP surface so the worker interface can be
/// firewalled independently of anything a browser talks to.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkersConfig {
    #[serde(default)]
    pub enabled: bool,

    #[serde(default = "default_workers_bind")]
    pub bind: String,
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
