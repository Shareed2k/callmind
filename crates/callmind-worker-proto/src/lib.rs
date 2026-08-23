//! gRPC contract between the CallMind service and remote processing workers.
//!
//! The `.proto` in this crate is the plugin boundary: a worker is a separate
//! program, possibly closed-source and possibly not written in Rust, that speaks
//! only this service. It never reaches the database and does not know which
//! backend the service uses.
//!
//! Rust workers can depend on this crate for generated client stubs and the
//! conversions below. Workers in other languages generate their own from
//! `proto/worker.proto`.

/// Generated messages and service stubs.
#[allow(clippy::all, clippy::pedantic, missing_docs)]
pub mod v1 {
    tonic::include_proto!("callmind.worker.v1");
}

pub mod convert;

pub use convert::TranscriptConversionError;
pub use v1::worker_client::WorkerClient;
pub use v1::worker_server::{Worker, WorkerServer};
