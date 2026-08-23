//! Server-rendered HTML views: call list, call detail with a word-synced audio
//! player, analytics and the ask page.
//!
//! No JavaScript build step and no template engine — views are plain Rust
//! functions returning strings. Anything interpolated into markup or into a
//! JavaScript context must be escaped at the point of use.

pub mod styles;
pub mod templates;
pub mod views;

pub use styles::APP_CSS;
pub use views::*;
