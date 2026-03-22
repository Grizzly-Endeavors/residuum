//! Shared utilities: fatal errors, filesystem helpers, monitored spawning, and tracing setup.

mod error;
pub(crate) mod fs;
mod spawn;
pub mod tracing_init;

pub use error::FatalError;
pub use spawn::{spawn_monitored, spawn_supervised};
