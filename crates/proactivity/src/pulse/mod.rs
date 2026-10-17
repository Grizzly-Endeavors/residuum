//! Pulse system: ambient monitoring via HEARTBEAT.yml.
//!
//! The pulse system runs scheduled checks on a configurable interval. Each
//! pulse fires a `scheduled` session (see the background module) — its result
//! flows through that session's normal completion pipeline into memory, the
//! same as any other session run. A task can run a script first, whose
//! output joins its prompt or, for a `script_only` pulse, is the result
//! with no session at all (see `script`).

pub mod edit;
pub mod executor;
pub mod next_run;
pub mod scheduler;
pub mod script;
pub mod types;

#[cfg(any(test, feature = "test-support"))]
pub mod testing;

/// Guide linked from owner notices about `HEARTBEAT.yml` pulses or scheduled
/// actions rejected at load for using an option removed by the agent
/// sessions overhaul (`agent: "main"`, `include_identity`).
pub(crate) const MIGRATION_GUIDE_URL: &str = "https://github.com/Grizzly-Endeavors/residuum/blob/main/docs/guides/migrating-to-agent-sessions.md";

/// Reference doc linked from owner notices about a `HEARTBEAT.yml` problem
/// that isn't about a removed option (a duplicate pulse name, or an
/// unparseable `schedule`/`active_hours` string).
pub(crate) const HEARTBEATS_REFERENCE_URL: &str =
    "https://github.com/Grizzly-Endeavors/residuum/blob/main/docs/systems-usage/heartbeats.md";
