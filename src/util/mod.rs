//! Shared utilities: fatal errors, filesystem helpers, monitored spawning, tracing setup,
//! structured log formatting, and XML escaping.

mod error;
pub(crate) mod fs;
pub mod log_format;
mod spawn;
pub mod telemetry;
pub mod tracing_init;
mod xml;

pub use error::FatalError;
pub use spawn::{spawn_monitored, spawn_supervised};
pub use xml::xml_escape;
