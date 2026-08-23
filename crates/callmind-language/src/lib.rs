//! Language identification by sampling several windows of a recording and
//! aggregating the per-window distributions.
//!
//! The acoustic detector is injected as a closure — in production a Whisper
//! forward pass — and is run on a blocking thread, since it is CPU/GPU-bound and
//! would otherwise starve the async runtime.

pub mod detector;
pub mod errors;
pub mod models;
pub mod traits;

pub use detector::SamplingLanguageEngine;
pub use errors::LanguageError;
pub use models::{LanguageDetection, LanguageProbability};
pub use traits::LanguageEngine;
