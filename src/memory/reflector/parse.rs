//! Response parsing for the reflector LLM output.

use chrono_tz::Tz;

use crate::error::IronclawError;
use crate::memory::types::{Observation, ObservationLog, Visibility};
use crate::time::now_local;

/// Parse the model's reflection response into an `ObservationLog`.
///
/// Expects a JSON array of objects:
/// `[{"content": "obs 1", "timestamp": "2026-02-21T14:30", "project_context": "ironclaw/memory", "visibility": "user"}, ...]`
///
/// Each object's `timestamp`, `project_context`, and `visibility` are preserved.
/// Defaults to `now_local(tz)` / `"general"` / `Visibility::User` when fields are absent or invalid.
///
/// # Errors
/// Returns an error if the response cannot be parsed.
pub(super) fn parse_reflection_response(
    content: &str,
    tz: Tz,
) -> Result<ObservationLog, IronclawError> {
    let trimmed = content.trim();
    let json_str = crate::memory::strip_code_fences(trimmed);

    let value: serde_json::Value = serde_json::from_str(json_str).map_err(|e| {
        IronclawError::Memory(format!(
            "failed to parse reflector response as JSON: {e}\nresponse: {trimmed}"
        ))
    })?;

    let items = value.as_array().ok_or_else(|| {
        IronclawError::Memory(format!(
            "reflector response is not a JSON array\nresponse: {trimmed}"
        ))
    })?;

    let mut log = ObservationLog::new();

    for item in items {
        let Some(obs_content) = item.get("content").and_then(serde_json::Value::as_str) else {
            continue;
        };

        if obs_content.is_empty() {
            continue;
        }

        let timestamp = item
            .get("timestamp")
            .and_then(serde_json::Value::as_str)
            .map_or_else(
                || now_local(tz),
                |ts| crate::memory::parse_minute_timestamp(ts, tz),
            );

        let project_context = item
            .get("project_context")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("general")
            .to_string();

        let visibility = item
            .get("visibility")
            .and_then(|v| serde_json::from_value::<Visibility>(v.clone()).ok())
            .unwrap_or_default();

        log.push(Observation {
            timestamp,
            project_context,
            source_episodes: vec![],
            visibility,
            content: obs_content.to_string(),
        });
    }

    Ok(log)
}
