//! Transcript assembly: orchestrates VAD, language ID, STT and diarization, then
//! aligns the results into speaker-attributed segments.
//!
//! Also handles RTL/LTR normalisation for mixed Hebrew text and exports
//! calendar events as RFC 5545 `.ics`.

pub mod builder;
pub mod export;
pub mod labels;
pub mod models;
pub mod normalizer;
pub mod roles;
pub mod rtl;
pub mod transcriber;
pub mod vocabulary;

pub use builder::TranscriptBuilder;
pub use export::TranscriptExporter;
pub use models::{SpeakerMetadata, TextDirection, Transcript, TranscriptSegment, TranscriptWord};
pub use normalizer::TextNormalizer;
pub use roles::RoleIdentifier;
pub use rtl::RtlDetector;
pub use transcriber::{AudioTranscriber, TranscriberError};
pub use vocabulary::{VocabularyEntry, VocabularyProcessor};
