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
    pub llm: LlmConfig,

    #[serde(default)]
    pub watcher: WatcherConfig,

    #[serde(default)]
    pub auth: AuthConfig,
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
                Self::default()
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
        if let Ok(val) = std::env::var("CALLMIND_SERVER_BIND") {
            self.server.bind = val;
        }
        if let Ok(val) = std::env::var("CALLMIND_DATABASE_URL") {
            self.database.url = val;
        }
        if let Ok(val) = std::env::var("CALLMIND_STORAGE_PATH") {
            self.storage.path = PathBuf::from(val);
        }
        if let Ok(Ok(parsed)) = std::env::var("CALLMIND_JOBS_WORKERS").map(|v| v.parse::<usize>()) {
            self.jobs.workers = parsed;
        }
        if let Ok(val) = std::env::var("CALLMIND_AUTH_API_KEY") {
            if !val.trim().is_empty() {
                self.auth.api_key = Some(val);
                self.auth.enabled = true;
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
}

fn default_llm_provider() -> String {
    "ollama".to_string()
}

fn default_llm_endpoint() -> String {
    "http://localhost:11434".to_string()
}

fn default_llm_model() -> String {
    "llama3.2".to_string()
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: default_llm_provider(),
            endpoint: default_llm_endpoint(),
            model: default_llm_model(),
            api_key: None,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelsConfig {
    #[serde(default = "default_models_dir")]
    pub models_dir: PathBuf,
}

fn default_models_dir() -> PathBuf {
    PathBuf::from("./models")
}

impl Default for ModelsConfig {
    fn default() -> Self {
        Self {
            models_dir: default_models_dir(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
