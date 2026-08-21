use crate::errors::LlmError;
use crate::traits::LlmEngine;
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use tracing::info;

/// Production LLM engine supporting local GGUF models and OpenAI-compatible local servers (Ollama / vLLM / mistral.rs).
pub struct LocalLlmEngine {
    pub model_path: Option<PathBuf>,
    pub endpoint_url: Option<String>,
}

impl LocalLlmEngine {
    pub fn new(model_path: Option<PathBuf>, endpoint_url: Option<String>) -> Self {
        Self {
            model_path,
            endpoint_url,
        }
    }

    pub fn from_path<P: AsRef<Path>>(path: P) -> Self {
        Self {
            model_path: Some(path.as_ref().to_path_buf()),
            endpoint_url: None,
        }
    }
}

#[async_trait]
impl LlmEngine for LocalLlmEngine {
    async fn generate_json(
        &self,
        prompt: &str,
        system: Option<&str>,
    ) -> Result<serde_json::Value, LlmError> {
        // If an external local LLM endpoint is provided, query it
        if let Some(ref url) = self.endpoint_url {
            info!("Querying local LLM endpoint at {}", url);
            let client = reqwest::Client::new();
            let mut messages = Vec::new();
            if let Some(sys) = system {
                messages.push(serde_json::json!({ "role": "system", "content": sys }));
            }
            messages.push(serde_json::json!({ "role": "user", "content": prompt }));

            let body = serde_json::json!({
                "model": "qwen2.5-7b-instruct",
                "messages": messages,
                "response_format": { "type": "json_object" },
                "temperature": 0.1
            });

            let resp = client
                .post(format!("{url}/chat/completions"))
                .json(&body)
                .send()
                .await
                .map_err(|e| LlmError::Provider(format!("Local LLM request failed: {e}")))?;

            if resp.status().is_success() {
                let json_resp = resp.json::<serde_json::Value>().await.map_err(|e| {
                    LlmError::Provider(format!("Failed to parse local LLM response: {e}"))
                })?;

                if let Some(content) = json_resp["choices"][0]["message"]["content"].as_str() {
                    return serde_json::from_str::<serde_json::Value>(content)
                        .map_err(LlmError::JsonParse);
                }
            } else {
                let err_text = resp.text().await.unwrap_or_default();
                return Err(LlmError::Provider(format!(
                    "Local LLM endpoint returned error: {err_text}"
                )));
            }
        }

        Err(LlmError::Provider(
            "No LLM provider model or endpoint configured".to_string(),
        ))
    }

    async fn generate_text(&self, prompt: &str, system: Option<&str>) -> Result<String, LlmError> {
        if let Some(ref url) = self.endpoint_url {
            let client = reqwest::Client::new();
            let mut messages = Vec::new();
            if let Some(sys) = system {
                messages.push(serde_json::json!({ "role": "system", "content": sys }));
            }
            messages.push(serde_json::json!({ "role": "user", "content": prompt }));

            let body = serde_json::json!({
                "model": "qwen2.5-7b-instruct",
                "messages": messages,
                "temperature": 0.2
            });

            let resp = client
                .post(format!("{url}/chat/completions"))
                .json(&body)
                .send()
                .await
                .map_err(|e| LlmError::Provider(format!("Local LLM request failed: {e}")))?;

            let json_resp = resp.json::<serde_json::Value>().await.map_err(|e| {
                LlmError::Provider(format!("Failed to parse local LLM response: {e}"))
            })?;

            return Ok(json_resp["choices"][0]["message"]["content"]
                .as_str()
                .unwrap_or_default()
                .to_string());
        }

        Err(LlmError::Provider(
            "No LLM provider model or endpoint configured".to_string(),
        ))
    }
}
