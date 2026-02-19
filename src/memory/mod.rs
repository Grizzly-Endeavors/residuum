//! Memory subsystem: observation, reflection, and search.
//!
//! Provides persistence across sessions through structured episodes,
//! observation logs, daily notes, and full-text search.

pub mod daily_log;
pub mod episode_store;
pub mod log_store;
pub mod observer;
pub mod reflector;
pub mod search;
pub mod tokens;
pub mod types;
