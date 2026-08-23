//! Domain types shared by every other crate: identifiers, call and job records,
//! enums, and the recording-filename parser.
//!
//! Deliberately dependency-light — it pulls in no I/O, no database and no
//! inference — so the rest of the workspace can depend on it freely.

pub mod enums;
pub mod errors;
pub mod ids;
pub mod models;
pub mod parser;
pub mod search_query;

pub use enums::*;
pub use errors::*;
pub use ids::*;
pub use models::*;
pub use parser::*;
