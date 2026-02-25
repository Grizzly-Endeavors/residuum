//! Pulse system: ambient monitoring via HEARTBEAT.yml.
//!
//! The pulse system runs scheduled checks on a configurable interval.
//! Each pulse fires a background agent turn using `Agent::run_system_turn`,
//! and the resulting messages flow into the memory pipeline.

pub mod executor;
pub mod scheduler;
pub mod types;
