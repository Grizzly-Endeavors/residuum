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

/// Guide linked from owner notices about `HEARTBEAT.yml` pulses or scheduled
/// actions rejected at load for using an option removed by the agent
/// sessions overhaul (`agent: "main"`, `include_identity`).
pub(crate) const MIGRATION_GUIDE_URL: &str = "https://github.com/Grizzly-Endeavors/residuum/blob/main/docs/guides/migrating-to-agent-sessions.md";

/// Reference doc linked from owner notices about a `HEARTBEAT.yml` problem
/// that isn't about a removed option (a duplicate pulse name, or an
/// unparseable `schedule`/`active_hours` string).
pub(crate) const HEARTBEATS_REFERENCE_URL: &str =
    "https://github.com/Grizzly-Endeavors/residuum/blob/main/docs/systems-usage/heartbeats.md";
