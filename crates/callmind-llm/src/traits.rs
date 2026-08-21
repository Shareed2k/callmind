use crate::errors::LlmError;
use async_trait::async_trait;
use serde::de::DeserializeOwned;

/// Interface for Local and Remote Large Language Model engines.
#[async_trait]
pub trait LlmEngine: Send + Sync {
    /// Generate a raw structured JSON response from the LLM.
    async fn generate_json(
        &self,
        prompt: &str,
        system: Option<&str>,
    ) -> Result<serde_json::Value, LlmError>;

    /// Generate freeform text response.
    async fn generate_text(&self, prompt: &str, system: Option<&str>) -> Result<String, LlmError>;
}

/// Extension methods for `LlmEngine` trait objects.
impl dyn LlmEngine {
    /// Generate a structured JSON response matching the target deserializable type `T`.
    pub async fn generate_structured<T: DeserializeOwned + Send>(
        &self,
        prompt: &str,
        system: Option<&str>,
    ) -> Result<T, LlmError> {
        let val = self.generate_json(prompt, system).await?;
        serde_json::from_value(val).map_err(LlmError::JsonParse)
    }
}
