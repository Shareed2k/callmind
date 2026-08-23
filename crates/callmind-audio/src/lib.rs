//! Audio decoding, resampling, channel analysis and batch import.
//!
//! Decoding is Symphonia, with one exception: Symphonia demuxes the OGG
//! container but has no Opus decoder (there is no `symphonia-codec-opus`), and
//! WhatsApp and Telegram voice notes are OGG/Opus. Those streams are detected by
//! container magic and decoded through `libopus` instead — see [`opus`].
//!
//! Resampling uses `rubato`, which band-limits before decimating. Its filter
//! latency is compensated so word-level timestamps stay aligned with the source.

pub mod buffer;
pub mod channel;
pub mod decoder;
pub mod errors;
pub mod importer;
pub mod opus;
pub mod resampler;
pub mod watcher;

pub use buffer::AudioBuffer;
pub use channel::{ChannelAnalyzer, ChannelMode};
pub use decoder::{AudioDecoder, AudioMetadata};
pub use errors::AudioError;
pub use importer::{BatchImportSummary, BatchImporter};
pub use resampler::{AudioResampler, STANDARD_STT_SAMPLE_RATE};
pub use watcher::DirectoryWatcher;
