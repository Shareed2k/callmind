//! Speech-to-text over `whisper.cpp`, plus the router that picks a model.
//!
//! [`SttRouter`] sends confidently-Hebrew audio to a fine-tuned ivrit-ai model
//! and everything else to multilingual Whisper. Model weights are loaded lazily
//! and cached; the load happens inside a blocking task because it reads
//! multi-gigabyte files and initialises the GPU backend.

pub mod errors;
pub mod mock;
pub mod models;
pub mod router;
pub mod traits;
pub mod whisper;

pub use errors::SttError;
pub use mock::MockSttEngine;
pub use models::{ModelInfo, SttRequest, SttResult, SttWord};
pub use router::{SttProfile, SttRouter};
pub use traits::SttEngine;
pub use whisper::WhisperCppEngine;
