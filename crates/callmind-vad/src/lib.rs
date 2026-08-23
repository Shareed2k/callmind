//! Voice activity detection, used to find speech regions before STT and
//! diarization.
//!
//! Only the energy-based engine ships today, and not for want of trying: Silero
//! is the obvious upgrade but `tract` cannot load any of its ONNX exports (v5,
//! `16k_op15` and v4 all contain an `If` node that `tract` 0.23 does not
//! translate), and running it needs either `ort` -- an ONNX Runtime C++
//! dependency -- or a pure-Rust port. `silero-vad-pure` 0.1.1 was tried and
//! returns near-zero probabilities on real speech.
//!
//! Worth knowing before reaching for a better detector: measured against
//! speaker-count accuracy on labelled recordings, `earshot` (a pure-Rust
//! WebRTC-derived detector) scored *identically* to this energy engine. Speech
//! detection was not the limiting factor there -- the speaker embeddings were.

pub mod energy;
pub mod errors;
pub mod region;
pub mod traits;

pub use energy::EnergyVadEngine;
pub use errors::VadError;
pub use region::SpeechRegion;
pub use traits::VadEngine;
