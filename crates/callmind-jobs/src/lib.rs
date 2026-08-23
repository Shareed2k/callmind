//! Background job queue: worker pool and the call-processing pipeline handler.
//!
//! Jobs are leased rather than locked outright: a worker takes a lease, renews it
//! with a heartbeat while it works, and a reaper releases leases that go stale so
//! a crashed worker cannot strand a job. Handlers run in their own task, so a
//! panicking handler fails that job instead of killing the worker.

pub mod errors;
pub mod handler;
pub mod pipeline;
pub mod worker;

pub use errors::*;
pub use handler::*;
pub use pipeline::*;
pub use worker::*;
