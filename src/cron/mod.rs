//! Cron system: agent-managed scheduled jobs.
//!
//! Jobs are persisted in `cron/jobs.json` and survive across restarts.
//! The agent can create, list, update, and remove jobs via cron tools.
//!
//! Supported schedule types:
//! - `At`: fire once at a specific datetime
//! - `Every`: repeat on a fixed interval anchored to an epoch
//! - `Cron`: standard cron expression

pub mod executor;
pub mod scheduler;
pub mod store;
pub mod types;
