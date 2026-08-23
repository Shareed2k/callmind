use crate::errors::LlmError;
use crate::local::LocalLlmEngine;
use crate::traits::LlmEngine;
use async_trait::async_trait;
use callmind_config::{LlmConfig, SUPPORTED_LLM_PROVIDERS};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, warn};

/// Build an HTTP client with the given timeout.
///
/// `build().unwrap_or_default()` silently handed back a client with *no*
/// timeout, so a hung provider would hang the job forever.
fn http_client(timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .unwrap_or_else(|e| {
            warn!("Falling back to default HTTP client ({e}); request timeout not applied");
            reqwest::Client::new()
        })
}

/// Factory function creating the appropriate `LlmEngine` from configuration.
#[must_use]
pub fn create_llm_engine(config: &LlmConfig) -> Arc<dyn LlmEngine> {
    match config.provider.to_lowercase().as_str() {
        "ollama" => Arc::new(OllamaEngine::new(&config.endpoint, &config.model)),
        "openai" | "groq" | "vllm" => {
            let api_key = config.api_key.clone().unwrap_or_default();
            Arc::new(OpenAiEngine::new(&config.endpoint, &config.model, api_key))
        }
        "anthropic" | "claude" => {
            let api_key = config.api_key.clone().unwrap_or_default();
            Arc::new(AnthropicEngine::new(
                &config.endpoint,
                &config.model,
                api_key,
            ))
        }
        "heuristic" | "local" => Arc::new(LocalLlmEngine::new(None, None)),
        other => {
            // Reachable only if validation was bypassed; a typo used to
            // silently degrade "AI analysis" to keyword heuristics forever.
            warn!(
                "Unknown llm.provider {other:?}; falling back to local heuristics. \
                 Supported providers: {SUPPORTED_LLM_PROVIDERS:?}"
            );
            Arc::new(LocalLlmEngine::new(None, None))
        }
    }
}

/// Ollama local LLM adapter.
pub struct OllamaEngine {
    client: reqwest::Client,
    endpoint: String,
    model: String,
    semaphore: Arc<tokio::sync::Semaphore>,
}

impl OllamaEngine {
    pub fn new(endpoint: &str, model: &str) -> Self {
        let base = endpoint.trim_end_matches('/');
        Self {
            client: http_client(Duration::from_secs(300)),
            endpoint: format!("{base}/api/generate"),
            model: model.to_string(),
            semaphore: Arc::new(tokio::sync::Semaphore::new(1)),
        }
    }
}

#[async_trait]
impl LlmEngine for OllamaEngine {
    async fn generate_json(
        &self,
        prompt: &str,
        system: Option<&str>,
    ) -> Result<serde_json::Value, LlmError> {
        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|e| LlmError::Provider(format!("LLM semaphore acquire error: {e}")))?;

        let mut model_name = self.model.clone();
        let make_body = |m: &str| {
            json!({
                "model": m,
                "prompt": prompt,
                "system": system.unwrap_or_default(),
                "stream": false,
                "format": "json",
                "options": {
                    "temperature": 0.2
                }
            })
        };

        debug!(
            "Sending Ollama JSON generation request to {}",
            self.endpoint
        );
        let mut res = self
            .client
            .post(&self.endpoint)
            .json(&make_body(&model_name))
            .send()
            .await
            .map_err(|e| LlmError::Provider(format!("Ollama connection error: {e}")))?;

        if !res.status().is_success() {
            // Auto-resolve installed tag from /api/tags if model name differed (e.g. llama3.2 -> llama3.2:3b)
            let tags_url = self.endpoint.replace("/api/generate", "/api/tags");
            if let Ok(tags_res) = self.client.get(&tags_url).send().await {
                if let Ok(tags_json) = tags_res.json::<serde_json::Value>().await {
                    if let Some(models_arr) = tags_json.get("models").and_then(|m| m.as_array()) {
                        for m in models_arr {
                            if let Some(name) = m.get("name").and_then(|n| n.as_str()) {
                                if name.starts_with(&self.model)
                                    || self.model.starts_with(name.split(':').next().unwrap_or(""))
                                {
                                    model_name = name.to_string();
                                    break;
                                }
                            }
                        }
                    }
                }
            }

            res = self
                .client
                .post(&self.endpoint)
                .json(&make_body(&model_name))
                .send()
                .await
                .map_err(|e| LlmError::Provider(format!("Ollama connection error: {e}")))?;

            if !res.status().is_success() {
                let err_text = res.text().await.unwrap_or_default();
                warn!("Ollama returned non-success: {err_text}");
                return Err(LlmError::Provider(format!("Ollama API error: {err_text}")));
            }
        }

        let resp_json: serde_json::Value = res
            .json()
            .await
            .map_err(|e| LlmError::Provider(format!("Failed to parse Ollama envelope: {e}")))?;

        if let Some(resp_str) = resp_json.get("response").and_then(|r| r.as_str()) {
            serde_json::from_str(resp_str).map_err(LlmError::JsonParse)
        } else {
            Err(LlmError::Provider(
                "Ollama response missing 'response' field".into(),
            ))
        }
    }

    async fn generate_text(&self, prompt: &str, system: Option<&str>) -> Result<String, LlmError> {
        // Same single-flight permit `generate_json` takes. Without it this path
        // bypassed the serialisation entirely and could pile concurrent
        // requests onto one Ollama instance.
        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|e| LlmError::Provider(format!("LLM semaphore acquire error: {e}")))?;

        let body = json!({
            "model": self.model,
            "prompt": prompt,
            "system": system.unwrap_or_default(),
            "stream": false,
        });

        let res = self
            .client
            .post(&self.endpoint)
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::Provider(format!("Ollama connection error: {e}")))?;

        let resp_json: serde_json::Value = res.json().await.map_err(|e| {
            LlmError::Provider(format!("Failed to parse Ollama text response: {e}"))
        })?;

        Ok(resp_json
            .get("response")
            .and_then(|r| r.as_str())
            .unwrap_or_default()
            .to_string())
    }
}

/// OpenAI / Groq / vLLM chat completions adapter.
pub struct OpenAiEngine {
    client: reqwest::Client,
    endpoint: String,
    model: String,
    api_key: String,
}

impl OpenAiEngine {
    pub fn new(endpoint: &str, model: &str, api_key: String) -> Self {
        let base = endpoint.trim_end_matches('/');
        let url = if base.ends_with("/chat/completions") {
            base.to_string()
        } else {
            format!("{base}/chat/completions")
        };

        Self {
            client: http_client(Duration::from_secs(60)),
            endpoint: url,
            model: model.to_string(),
            api_key,
        }
    }
}

#[async_trait]
impl LlmEngine for OpenAiEngine {
    async fn generate_json(
        &self,
        prompt: &str,
        system: Option<&str>,
    ) -> Result<serde_json::Value, LlmError> {
        let mut messages = Vec::new();
        if let Some(sys) = system {
            messages.push(json!({ "role": "system", "content": sys }));
        }
        messages.push(json!({ "role": "user", "content": prompt }));

        let body = json!({
            "model": self.model,
            "messages": messages,
            "response_format": { "type": "json_object" },
            "temperature": 0.2
        });

        let mut req = self.client.post(&self.endpoint).json(&body);
        if !self.api_key.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", self.api_key));
        }

        let res = req
            .send()
            .await
            .map_err(|e| LlmError::Provider(format!("OpenAI request failed: {e}")))?;

        if !res.status().is_success() {
            let err_text = res.text().await.unwrap_or_default();
            warn!("OpenAI returned error {err_text}");
            return Err(LlmError::Provider(format!("OpenAI API error: {err_text}")));
        }

        let resp_json: serde_json::Value = res
            .json()
            .await
            .map_err(|e| LlmError::Provider(format!("Failed to parse OpenAI envelope: {e}")))?;

        let content = resp_json["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or_default();
        serde_json::from_str(content).map_err(LlmError::JsonParse)
    }

    async fn generate_text(&self, prompt: &str, system: Option<&str>) -> Result<String, LlmError> {
        let mut messages = Vec::new();
        if let Some(sys) = system {
            messages.push(json!({ "role": "system", "content": sys }));
        }
        messages.push(json!({ "role": "user", "content": prompt }));

        let body = json!({
            "model": self.model,
            "messages": messages,
            "temperature": 0.2
        });

        let mut req = self.client.post(&self.endpoint).json(&body);
        if !self.api_key.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", self.api_key));
        }

        let res = req
            .send()
            .await
            .map_err(|e| LlmError::Provider(format!("OpenAI request failed: {e}")))?;

        let resp_json: serde_json::Value = res
            .json()
            .await
            .map_err(|e| LlmError::Provider(format!("Failed to parse OpenAI text: {e}")))?;

        Ok(resp_json["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or_default()
            .to_string())
    }
}

/// Anthropic Claude messages adapter.
pub struct AnthropicEngine {
    client: reqwest::Client,
    endpoint: String,
    model: String,
    api_key: String,
}

impl AnthropicEngine {
    pub fn new(endpoint: &str, model: &str, api_key: String) -> Self {
        let base = endpoint.trim_end_matches('/');
        let url = if base.ends_with("/messages") {
            base.to_string()
        } else if base.is_empty()
            || base == "https://api.anthropic.com"
            || base == "https://api.anthropic.com/v1"
        {
            "https://api.anthropic.com/v1/messages".to_string()
        } else {
            format!("{base}/v1/messages")
        };

        Self {
            client: http_client(Duration::from_secs(60)),
            endpoint: url,
            model: model.to_string(),
            api_key,
        }
    }
}

#[async_trait]
impl LlmEngine for AnthropicEngine {
    async fn generate_json(
        &self,
        prompt: &str,
        system: Option<&str>,
    ) -> Result<serde_json::Value, LlmError> {
        let body = json!({
            "model": self.model,
            "max_tokens": 4096,
            "system": system.unwrap_or_default(),
            "messages": [{ "role": "user", "content": prompt }],
            "temperature": 0.2
        });

        let res = self
            .client
            .post(&self.endpoint)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::Provider(format!("Anthropic request failed: {e}")))?;

        if !res.status().is_success() {
            let err_text = res.text().await.unwrap_or_default();
            warn!("Anthropic returned error {err_text}");
            return Err(LlmError::Provider(format!(
                "Anthropic API error: {err_text}"
            )));
        }

        let resp_json: serde_json::Value = res
            .json()
            .await
            .map_err(|e| LlmError::Provider(format!("Failed to parse Anthropic envelope: {e}")))?;

        let content = resp_json["content"][0]["text"].as_str().unwrap_or_default();
        serde_json::from_str(content).map_err(LlmError::JsonParse)
    }

    async fn generate_text(&self, prompt: &str, system: Option<&str>) -> Result<String, LlmError> {
        let body = json!({
            "model": self.model,
            "max_tokens": 4096,
            "system": system.unwrap_or_default(),
            "messages": [{ "role": "user", "content": prompt }],
            "temperature": 0.2
        });

        let res = self
            .client
            .post(&self.endpoint)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::Provider(format!("Anthropic request failed: {e}")))?;

        let resp_json: serde_json::Value = res
            .json()
            .await
            .map_err(|e| LlmError::Provider(format!("Failed to parse Anthropic text: {e}")))?;

        Ok(resp_json["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string())
    }
}
