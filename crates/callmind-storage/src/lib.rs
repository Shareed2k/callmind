//! Recording storage behind the [`RecordingStorage`] trait.
//!
//! Only a filesystem backend ships today. Objects are addressed by an opaque
//! key, and the trait exposes `get_local_path` so callers that need a real file
//! (audio decoding, model loading) can avoid buffering.

pub mod errors;
pub mod filesystem;
pub mod traits;

pub use errors::*;
pub use filesystem::*;
pub use traits::*;
