//! LLM adapters and localized prompt templates.
//!
//! Ollama, OpenAI-compatible and Anthropic backends sit behind [`LlmEngine`],
//! with a heuristic local engine as the offline fallback. Prompts are built in
//! the language of the conversation so summaries come back in that language.

pub mod errors;
pub mod local;
/// Test doubles. Behind a feature so they are not linked into the server
/// binary; integration tests in other crates turn it on via dev-dependencies.
#[cfg(any(test, feature = "mock"))]
pub mod mock;
pub mod prompts;
pub mod providers;
pub mod traits;

pub use errors::LlmError;
pub use local::LocalLlmEngine;
#[cfg(any(test, feature = "mock"))]
pub use mock::MockLlmEngine;
pub use prompts::*;
pub use providers::*;
pub use traits::LlmEngine;
