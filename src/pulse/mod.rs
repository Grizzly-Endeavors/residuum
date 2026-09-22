//! Pulse system: ambient monitoring via HEARTBEAT.yml.
//!
//! The pulse system runs scheduled checks on a configurable interval. Each
//! pulse fires a `scheduled` session (see `crate::background`) — its result
//! flows through that session's normal completion pipeline into memory, the
//! same as any other session run.

pub mod executor;
pub mod scheduler;
pub mod types;
