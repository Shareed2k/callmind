//! Axum HTTP surface: REST API, the server-rendered UI, and bot channels.
//!
//! Authentication, when enabled, covers the UI and the OpenAPI documents as well
//! as the API — they serve the same conversation data. CORS is deny-by-default.
//!
//! Bot channels live in [`bots`]: Telegram over long-polling, WhatsApp through a
//! self-hosted Evolution API instance, and a universal audio webhook for iOS
//! Shortcuts and similar.

pub mod bots;
pub mod errors;
pub mod grpc;
pub mod openapi;
pub mod routes;
pub mod state;

pub use bots::*;
pub use errors::*;
pub use openapi::*;
pub use routes::create_router;
pub use state::*;
