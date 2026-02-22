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

/// Parse a timestamp string into a UTC `DateTime`.
///
/// Tries minute-precision UTC format (`YYYY-MM-DDTHH:MMZ`) first, then RFC3339.
/// Falls back to `Utc::now()` with a warning if both fail.
pub(crate) fn parse_minute_timestamp(ts: &str) -> chrono::DateTime<chrono::Utc> {
    // Try minute-precision UTC format: "2026-02-21T14:30Z"
    let without_z = ts.trim_end_matches('Z');
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(without_z, "%Y-%m-%dT%H:%M") {
        return naive.and_utc();
    }
    // Try RFC3339 fallback: "2026-02-21T14:30:00Z"
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts) {
        return dt.with_timezone(&chrono::Utc);
    }
    tracing::warn!(
        timestamp = ts,
        "failed to parse timestamp, using current time"
    );
    chrono::Utc::now()
}
