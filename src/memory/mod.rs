//! Memory subsystem: observation, reflection, and search.
//!
//! Provides persistence across restarts through structured episodes,
//! observation logs, and full-text search.

pub(crate) mod chunk_extractor;
pub mod episode_store;
pub mod log_store;
pub mod observer;
pub mod recent_context;
pub mod recent_messages;
pub mod reflector;
pub mod search;
pub mod tokens;
pub mod types;

#[cfg(test)]
pub(crate) mod test_helpers;

/// Strip markdown code fences (```` ```json ... ``` ````) from LLM responses.
///
/// Returns the inner content trimmed, or the original string if no fences found.
pub(crate) fn strip_code_fences(s: &str) -> &str {
    s.strip_prefix("```json")
        .or_else(|| s.strip_prefix("```"))
        .and_then(|inner| inner.strip_suffix("```"))
        .map_or(s, str::trim)
}

/// Parse a timestamp string into a local `NaiveDateTime`.
///
/// Accepts `YYYY-MM-DDTHH:MM` format (plain local time, no offset/Z).
/// Falls back to `now_local(tz)` with a warning if parsing fails.
pub(crate) fn parse_minute_timestamp(ts: &str, tz: chrono_tz::Tz) -> chrono::NaiveDateTime {
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%dT%H:%M") {
        return naive;
    }
    tracing::warn!(
        timestamp = ts,
        "failed to parse timestamp, using current time"
    );
    crate::time::now_local(tz)
}
