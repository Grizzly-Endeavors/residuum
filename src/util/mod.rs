//! Shared utilities: fatal errors, filesystem helpers, monitored spawning, and tracing setup.

mod error;
pub(crate) mod fs;
mod spawn;

pub use error::FatalError;
pub use spawn::spawn_monitored;
