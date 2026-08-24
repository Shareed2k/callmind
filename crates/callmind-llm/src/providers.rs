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
        "ollama" => Arc::new(OllamaEngine::new(
            &config.endpoint,
            &config.model,
            config.context_tokens,
        )),
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
/// Body of an Ollama `/api/generate` call.
///
/// Extracted so the request can be asserted on. `num_ctx` is the field that
/// matters: Ollama defaults to a couple of thousand tokens and drops whatever
/// does not fit, without an error, which cut roughly half of a 13-minute call on
/// this archive.
/// Upper bound on generated tokens.
///
/// Without one, Ollama shifts its context window and keeps going, so a model in
/// a repetition loop generates until the request deadline: llama3.2:3b did that
/// on a real Hebrew call, filling the summary with one repeated token.
///
/// 4096 is roughly nine times what a real analysis needs -- measured across
/// eight runs of the same call, every one stopped on its own between 60 and 459
/// tokens -- so it bounds a runaway without truncating legitimate output.
const MAX_RESPONSE_TOKENS: u32 = 4096;

/// How many times to ask Ollama for JSON before giving up.
///
/// Local models drift out of the requested format: qwen2.5:7b switched to
/// Chinese mid-object in 2 of 5 identical requests on a real Hebrew call. Three
/// attempts takes that from roughly one call in four to one in sixty, and the
/// requests are seconds each.
const MAX_JSON_ATTEMPTS: usize = 3;

fn ollama_generate_body(
    model: &str,
    prompt: &str,
    system: Option<&str>,
    context_tokens: usize,
) -> serde_json::Value {
    json!({
        "model": model,
        "prompt": prompt,
        "system": system.unwrap_or_default(),
        "stream": false,
        "format": "json",
        "options": {
            "temperature": 0.2,
            "num_predict": MAX_RESPONSE_TOKENS,
            "num_ctx": context_tokens
        }
    })
}

pub struct OllamaEngine {
    client: reqwest::Client,
    endpoint: String,
    model: String,
    context_tokens: usize,
    semaphore: Arc<tokio::sync::Semaphore>,
}

impl OllamaEngine {
    pub fn new(endpoint: &str, model: &str, context_tokens: usize) -> Self {
        let base = endpoint.trim_end_matches('/');
        Self {
            context_tokens,
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
        let make_body = |m: &str| ollama_generate_body(m, prompt, system, self.context_tokens);

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

        // Local models drift: measured on a real Hebrew call, qwen2.5:7b switched
        // to Chinese mid-object in 2 of 5 identical requests, leaving text that
        // is not JSON. Asking again is cheap next to the alternative, which is a
        // fallback that never read the call.
        let mut response = res;
        let mut attempt = 1_usize;
        loop {
            let envelope: serde_json::Value = response
                .json()
                .await
                .map_err(|e| LlmError::Provider(format!("Failed to parse Ollama envelope: {e}")))?;

            let Some(generated) = envelope.get("response").and_then(|r| r.as_str()) else {
                return Err(LlmError::Provider(
                    "Ollama response missing 'response' field".into(),
                ));
            };

            match serde_json::from_str(generated) {
                Ok(value) => return Ok(value),
                Err(parse_error) => {
                    if attempt >= MAX_JSON_ATTEMPTS {
                        return Err(LlmError::JsonParse(parse_error));
                    }
                    warn!(
                        attempt,
                        "Ollama answered with text that is not JSON; asking again"
                    );
                    attempt += 1;
                    response = self
                        .client
                        .post(&self.endpoint)
                        .json(&make_body(&model_name))
                        .send()
                        .await
                        .map_err(|e| LlmError::Provider(format!("Ollama connection error: {e}")))?;
                    if !response.status().is_success() {
                        let err_text = response.text().await.unwrap_or_default();
                        return Err(LlmError::Provider(format!("Ollama API error: {err_text}")));
                    }
                }
            }
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

#[cfg(test)]
mod ollama_body_tests {
    use super::*;

    /// Ollama defaults to a small context and silently drops the rest of the
    /// prompt, with nothing in the logs, so the field has to be on the wire.
    #[test]
    fn the_request_carries_the_configured_context_size() {
        let body = ollama_generate_body("model-x", "prompt text", Some("system text"), 12_345);

        assert_eq!(body["model"], "model-x");
        assert_eq!(body["prompt"], "prompt text");
        assert_eq!(body["system"], "system text");
        assert_eq!(body["format"], "json");
        assert_eq!(
            body["options"]["num_ctx"], 12_345,
            "num_ctx must reach Ollama: {body}"
        );
    }

    /// A model that degenerates into a loop generates until something stops it,
    /// and nothing did: measured on a real Hebrew call, llama3.2:3b ran past a
    /// 300 s deadline emitting one repeated token. The cap bounds the waste; it
    /// is generous enough that a legitimate analysis is never truncated.
    #[test]
    fn the_request_bounds_how_much_the_model_may_generate() {
        let body = ollama_generate_body("m", "p", None, 8192);
        assert_eq!(body["options"]["num_predict"], MAX_RESPONSE_TOKENS);
    }

    #[test]
    fn a_missing_system_prompt_is_an_empty_string_not_null() {
        let body = ollama_generate_body("m", "p", None, 2048);
        assert_eq!(body["system"], "");
    }
}
