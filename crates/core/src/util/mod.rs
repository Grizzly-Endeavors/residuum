//! Shared utilities: fatal errors, filesystem helpers, monitored spawning, env-var
//! expansion, frontmatter parsing, and XML escaping.

pub mod backoff_tracker;
mod env_expand;
mod error;
pub mod frontmatter;
pub mod fs;
pub mod secret_compare;
mod spawn;
pub mod telemetry;
mod xml;

pub use backoff_tracker::{BackoffTracker, NoticeAction, RetryGate};
pub use env_expand::expand_env_vars;
pub use error::FatalError;
pub use frontmatter::{parse_frontmatter_md, validate_kebab_name};
pub use secret_compare::secrets_match;
pub use spawn::{panic_message, spawn_blocking_in_span, spawn_in_span, spawn_monitored};
pub use xml::xml_escape;
