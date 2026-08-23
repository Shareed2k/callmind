//! Conversation intelligence: turns a transcript into a structured analysis.
//!
//! Objective metrics (talk ratio, silence, interruptions) are computed directly
//! from timestamps and never involve the LLM. The generative part — title,
//! summary, intent, action items — asks the LLM for structured JSON and degrades
//! to a heuristic summarizer if that fails, so a provider outage costs quality
//! rather than the whole job.

pub mod analyzer;
pub mod classifiers;
pub mod emotions;
pub mod lenient;
pub mod metrics;
pub mod models;
pub mod summarizer;

pub use analyzer::{AnalysisEngine, AnalysisError};
pub use classifiers::{Classifier, ClassifierOutputType};
pub use emotions::{EmotionClassifier, EmotionDistribution, EmotionType};
pub use metrics::ConversationMetricsCalculator;
pub use models::*;
pub use summarizer::ConversationSummarizer;
