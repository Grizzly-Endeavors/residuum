//! Shared utilities: fatal errors, filesystem helpers, monitored spawning, tracing setup,
//! structured log formatting, frontmatter parsing, and XML escaping.

mod error;
pub mod frontmatter;
pub(crate) mod fs;
pub mod log_format;
mod spawn;
pub mod telemetry;
pub mod tracing_init;
mod xml;

pub use error::FatalError;
pub use frontmatter::{parse_frontmatter_md, validate_kebab_name};
pub use spawn::{panic_message, spawn_monitored, spawn_supervised};
pub use xml::xml_escape;
