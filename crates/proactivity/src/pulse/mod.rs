pub mod cadence;
pub mod edit;
pub mod executor;
pub mod next_run;
pub mod scheduler;
pub mod script;
pub mod types;

#[cfg(any(test, feature = "test-support"))]
pub mod testing;

pub(crate) const MIGRATION_GUIDE_URL: &str = "https://github.com/Grizzly-Endeavors/residuum/blob/main/docs/guides/migrating-to-agent-sessions.md";

pub(crate) const HEARTBEATS_REFERENCE_URL: &str =
    "https://github.com/Grizzly-Endeavors/residuum/blob/main/docs/systems-usage/heartbeats.md";
