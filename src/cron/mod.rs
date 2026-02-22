//! Cron system: agent-managed scheduled jobs.
//!
//! Jobs are persisted in `cron/jobs.json` and survive across restarts.
//! The agent can create, list, update, and remove jobs via cron tools.
//!
//! Supported schedule types:
//! - `At`: fire once at a specific UTC datetime
//! - `Every`: repeat on a fixed interval anchored to an epoch
//! - `Cron`: standard cron expression (UTC only in Phase 3)
//!
//! Each job has a `Delivery` mode:
//! - `UserVisible`: print to CLI and queue for the next user turn
//! - `Background`: run silently, feeding results to the memory pipeline

pub mod executor;
pub mod scheduler;
pub mod store;
pub mod types;
