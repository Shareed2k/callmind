//! Speaker diarization: who spoke when.
//!
//! Two paths. Stereo telephony is split by channel. Everything else extracts
//! neural speaker embeddings with `tract-onnx` and clusters them with complete-linkage
//! agglomerative clustering, falling back to acoustic DSP features when no ONNX
//! model is present.
//!
//! Clustering uses `kodama`'s nn-chain implementation: the naive
//! recompute-every-merge approach is cubic and did not finish a one-hour call.

pub mod ahc;
pub mod aligner;
pub mod clustering;
pub mod der;
pub mod errors;
pub mod features;
pub mod identity;
pub mod models;
pub mod neural;
pub mod onnx_extractor;
pub mod pyannote;
pub mod spectral;
pub mod stereo;
pub mod traits;

pub use ahc::AgglomerativeClustering;
pub use aligner::TranscriptAligner;
pub use clustering::ClusteringDiarizer;
pub use der::{DerCalculator, DerEvaluation, GroundTruthTurn};
pub use errors::DiarizationError;
pub use features::AcousticFeatureExtractor;
pub use models::{DiarizationRequest, DiarizationResult, SpeakerTurn};
pub use neural::NeuralDiarizer;
pub use onnx_extractor::OnnxSpeakerEmbeddingExtractor;
pub use stereo::StereoChannelDiarizer;
pub use traits::DiarizationEngine;
