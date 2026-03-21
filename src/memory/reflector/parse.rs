//! Response parsing for the reflector LLM output.

use chrono_tz::Tz;
use serde::Deserialize;

use crate::error::FatalError;
use crate::memory::types::{Observation, ObservationLog, Visibility};
use crate::time::now_local;

/// Typed response from structured output mode (object-wrapped).
#[derive(Deserialize)]
struct ReflectorJsonResponse {
    observations: Vec<ReflectorItem>,
}

/// Single observation item within the typed reflector response.
#[derive(Deserialize)]
struct ReflectorItem {
    content: String,
    timestamp: String,
    project_context: String,
    visibility: String,
}

/// Parse the model's reflection response into an `ObservationLog`.
///
/// Tries typed deserialization first (structured output object format), then
/// falls back to `Value`-based parsing for legacy bare arrays and Ollama fallback.
///
/// # Errors
/// Returns an error if the response cannot be parsed.
pub(super) fn parse_reflection_response(
    content: &str,
    tz: Tz,
) -> Result<ObservationLog, FatalError> {
    let trimmed = content.trim();
    let json_str = crate::memory::strip_code_fences(trimmed);

    // Fast path: try typed deserialization (structured output object format)
    if let Ok(typed) = serde_json::from_str::<ReflectorJsonResponse>(json_str) {
        let mut log = ObservationLog::new();
        for item in &typed.observations {
            if item.content.is_empty() {
                continue;
            }
            let timestamp = crate::memory::parse_minute_timestamp(&item.timestamp, tz);
            let visibility = if item.visibility == "background" {
                Visibility::Background
            } else {
                Visibility::User
            };
            log.push(Observation {
                timestamp,
                project_context: item.project_context.clone(),
                source_episodes: vec![],
                visibility,
                content: item.content.clone(),
            });
        }
        return Ok(log);
    }

    // Fallback: Value-based parsing for legacy bare arrays
    tracing::debug!("reflector structured output failed, falling back to value-based parsing");
    let value: serde_json::Value = serde_json::from_str(json_str).map_err(|e| {
        FatalError::Memory(format!(
            "failed to parse reflector response as JSON: {e}\nresponse: {trimmed}"
        ))
    })?;

    let items = value.as_array().ok_or_else(|| {
        FatalError::Memory(format!(
            "reflector response is not a JSON array or object\nresponse: {trimmed}"
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
