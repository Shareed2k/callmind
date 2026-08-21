use crate::errors::LlmError;
use crate::traits::LlmEngine;
use async_trait::async_trait;

/// Mock LLM engine for testing conversation intelligence analysis.
#[derive(Default)]
pub struct MockLlmEngine {
    pub json_response: Option<serde_json::Value>,
    pub text_response: Option<String>,
}

impl MockLlmEngine {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_json(mut self, val: serde_json::Value) -> Self {
        self.json_response = Some(val);
        self
    }

    #[must_use]
    pub fn with_text(mut self, text: String) -> Self {
        self.text_response = Some(text);
        self
    }
}

#[async_trait]
impl LlmEngine for MockLlmEngine {
    async fn generate_json(
        &self,
        _prompt: &str,
        _system: Option<&str>,
    ) -> Result<serde_json::Value, LlmError> {
        if let Some(ref val) = self.json_response {
            Ok(val.clone())
        } else {
            Err(LlmError::Inference(
                "Mock LLM json response not configured".into(),
            ))
        }
    }

    async fn generate_text(
        &self,
        _prompt: &str,
        _system: Option<&str>,
    ) -> Result<String, LlmError> {
        Ok(self
            .text_response
            .clone()
            .unwrap_or_else(|| "Mock LLM text output".into()))
    }
}
