//! Shared utilities: fatal errors, filesystem helpers, monitored spawning, tracing setup,
//! and structured log formatting.

mod error;
pub(crate) mod fs;
pub mod log_format;
mod spawn;
pub mod telemetry;
pub mod tracing_init;

pub use error::FatalError;
pub use spawn::{spawn_monitored, spawn_supervised};
