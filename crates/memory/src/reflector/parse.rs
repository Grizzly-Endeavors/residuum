use chrono_tz::Tz;
use serde::Deserialize;

use crate::types::{Observation, ObservationLog, Visibility};
use residuum_core::time::now_local;

#[derive(Deserialize)]
struct ReflectorJsonResponse {
    observations: Vec<ReflectorItem>,
}

#[derive(Deserialize)]
struct ReflectorItem {
    content: String,
    timestamp: String,
    visibility: Visibility,
}

pub(super) fn parse_reflection_response(content: &str, tz: Tz) -> anyhow::Result<ObservationLog> {
    let trimmed = content.trim();
    let json_str = residuum_inference::strip_code_fences(trimmed);

    if let Ok(typed) = serde_json::from_str::<ReflectorJsonResponse>(json_str) {
        let mut log = ObservationLog::new();
        for item in &typed.observations {
            if item.content.is_empty() {
                continue;
            }
            let timestamp = crate::parse_minute_timestamp(&item.timestamp, tz);
            log.observations.push(Observation {
                timestamp,
                source_episodes: None,
                visibility: item.visibility.clone(),
                content: item.content.clone(),
                source: crate::types::SourceTag::main(),
            });
        }
        return Ok(log);
    }

    tracing::debug!("reflector structured output failed, falling back to value-based parsing");
    let value: serde_json::Value = serde_json::from_str(json_str).map_err(|e| {
        anyhow::anyhow!(
            "malformed reflector LLM response (not valid JSON): {e}\nraw response: {trimmed}"
        )
    })?;

    let items = value.as_array().ok_or_else(|| {
        anyhow::anyhow!(
            "malformed reflector LLM response: expected a JSON array or \
             {{\"observations\": [...]}}, got neither\nraw response: {trimmed}"
        )
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
            .map_or_else(|| now_local(tz), |ts| crate::parse_minute_timestamp(ts, tz));

        let visibility = item
            .get("visibility")
            .and_then(|v| serde_json::from_value::<Visibility>(v.clone()).ok())
            .unwrap_or_default();

        log.observations.push(Observation {
            timestamp,
            source_episodes: None,
            visibility,
            content: obs_content.to_string(),
            source: crate::types::SourceTag::main(),
        });
    }

    Ok(log)
}
