//! Response parsing for the observer LLM output.

use chrono::NaiveDateTime;
use chrono_tz::Tz;
use serde::Deserialize;

use crate::memory::types::Visibility;
use crate::models::ModelResponse;
use crate::time::now_local;

/// Typed response from structured output mode.
#[derive(Deserialize)]
struct ObserverJsonResponse {
    observations: Vec<ObservationItem>,
    narrative: String,
}

/// Single observation item within the typed response.
#[derive(Deserialize)]
struct ObservationItem {
    content: String,
    timestamp: String,
    visibility: Visibility,
    project_context: String,
}

/// Intermediate extraction result from the observer LLM response.
pub(super) struct ObserverExtraction {
    pub(super) content: String,
    pub(super) timestamp: NaiveDateTime,
    pub(super) visibility: Visibility,
    pub(super) project_context: String,
}

/// Combined parse result: extractions plus optional narrative.
pub(super) struct ObserverParseResult {
    pub(super) extractions: Vec<ObserverExtraction>,
    pub(super) narrative: Option<String>,
}

/// Parse the model's JSON response into extractions and an optional narrative.
///
/// Tries typed deserialization first (structured output path), then falls back
/// to `Value`-based parsing for legacy bare arrays and Ollama fallback.
///
/// # Errors
/// Returns an error if the response cannot be parsed or the observations are empty.
pub(super) fn parse_observer_response(
    response: &ModelResponse,
    tz: Tz,
) -> anyhow::Result<ObserverParseResult> {
    let content = response.content.trim();
    let json_str = crate::memory::strip_code_fences(content);

    // Fast path: try typed deserialization (structured output)
    if let Ok(typed) = serde_json::from_str::<ObserverJsonResponse>(json_str) {
        let extractions = typed_items_to_extractions(&typed.observations, tz);

        if extractions.is_empty() {
            anyhow::bail!("observer returned empty observations array");
        }

        let narrative = if typed.narrative.is_empty() {
            None
        } else {
            Some(typed.narrative)
        };

        return Ok(ObserverParseResult {
            extractions,
            narrative,
        });
    }

    // Fallback: Value-based parsing for legacy bare arrays and malformed objects
    tracing::debug!("observer structured output failed, falling back to value-based parsing");
    let value: serde_json::Value = serde_json::from_str(json_str).map_err(|e| {
        anyhow::anyhow!("failed to parse observer response as JSON: {e}\nresponse: {content}")
    })?;

    let (items, narrative) = if let Some(arr) = value.as_array() {
        (arr.clone(), None)
    } else if let Some(obj) = value.as_object() {
        let obs_array = obj
            .get("observations")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "observer response object missing 'observations' array\nresponse: {content}"
                )
            })?
            .clone();

        let narr = obj
            .get("narrative")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .map(String::from);

        (obs_array, narr)
    } else {
        anyhow::bail!("observer response is not a JSON array or object\nresponse: {content}");
    };

    let extractions = parse_extraction_items(&items, tz);

    if extractions.is_empty() {
        anyhow::bail!("observer returned empty observations array");
    }

    Ok(ObserverParseResult {
        extractions,
        narrative,
    })
}

/// Convert typed `ObservationItem`s to `ObserverExtraction`s.
fn typed_items_to_extractions(items: &[ObservationItem], tz: Tz) -> Vec<ObserverExtraction> {
    items
        .iter()
        .filter(|item| {
            if item.content.is_empty() {
                tracing::debug!("observer typed item has empty content, skipping");
                false
            } else {
                true
            }
        })
        .map(|item| {
            let timestamp = crate::memory::parse_minute_timestamp(&item.timestamp, tz);
            ObserverExtraction {
                content: item.content.clone(),
                timestamp,
                visibility: item.visibility.clone(),
                project_context: item.project_context.clone(),
            }
        })
        .collect()
}

/// Parse individual observation items from a JSON array.
pub(super) fn parse_extraction_items(
    items: &[serde_json::Value],
    tz: Tz,
) -> Vec<ObserverExtraction> {
    let mut extractions = Vec::new();

    for item in items {
        let Some(obs_content) = item.get("content").and_then(serde_json::Value::as_str) else {
            tracing::warn!("observer response item missing 'content' field, skipping");
            continue;
        };

        if obs_content.is_empty() {
            continue;
        }

        let timestamp = item
            .get("timestamp")
            .and_then(serde_json::Value::as_str)
            .map_or_else(
                || {
                    tracing::warn!(
                        "observer response item missing 'timestamp', using current time"
                    );
                    now_local(tz)
                },
                |ts| crate::memory::parse_minute_timestamp(ts, tz),
            );

        let visibility = item
            .get("visibility")
            .and_then(|v| serde_json::from_value::<Visibility>(v.clone()).ok())
            .unwrap_or_default();

        let project_context = item
            .get("project_context")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or("general")
            .to_string();

        extractions.push(ObserverExtraction {
            content: obs_content.to_string(),
            timestamp,
            visibility,
            project_context,
        });
    }

    extractions
}
