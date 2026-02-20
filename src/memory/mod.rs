//! Memory subsystem: observation, reflection, and search.
//!
//! Provides persistence across sessions through structured episodes,
//! observation logs, daily notes, and full-text search.

pub mod daily_log;
pub mod episode_store;
pub mod log_store;
pub mod observer;
pub mod recent_store;
pub mod reflector;
pub mod search;
pub mod tokens;
pub mod types;

/// Strip markdown code fences (```` ```json ... ``` ````) from LLM responses.
///
/// Returns the inner content trimmed, or the original string if no fences found.
pub(crate) fn strip_code_fences(s: &str) -> &str {
    s.strip_prefix("```json")
        .or_else(|| s.strip_prefix("```"))
        .and_then(|inner| inner.strip_suffix("```"))
        .map_or(s, str::trim)
}
