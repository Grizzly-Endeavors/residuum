//! Response parsing for the observer LLM output.

use chrono::NaiveDateTime;
use chrono_tz::Tz;

use crate::error::IronclawError;
use crate::memory::types::Visibility;
use crate::models::ModelResponse;
use crate::time::now_local;

/// Intermediate extraction result from the observer LLM response.
pub(super) struct ObserverExtraction {
    pub(super) content: String,
    pub(super) timestamp: NaiveDateTime,
    pub(super) visibility: Visibility,
}

/// Combined parse result: extractions plus optional narrative.
pub(super) struct ObserverParseResult {
    pub(super) extractions: Vec<ObserverExtraction>,
    pub(super) narrative: Option<String>,
}

/// Parse the model's JSON response into extractions and an optional narrative.
///
/// Accepts two formats:
/// - **New (object)**: `{"observations": [...], "narrative": "..."}`
/// - **Legacy (bare array)**: `[{"content": ..., ...}, ...]`
///
/// # Errors
/// Returns an error if the response cannot be parsed or the observations are empty.
pub(super) fn parse_observer_response(
    response: &ModelResponse,
    tz: Tz,
) -> Result<ObserverParseResult, IronclawError> {
    let content = response.content.trim();
    let json_str = crate::memory::strip_code_fences(content);

    let value: serde_json::Value = serde_json::from_str(json_str).map_err(|e| {
        IronclawError::Memory(format!(
            "failed to parse observer response as JSON: {e}\nresponse: {content}"
        ))
    })?;

    // Determine the items array and optional narrative
    let (items, narrative) = if let Some(arr) = value.as_array() {
        // Legacy bare-array format
        (arr.clone(), None)
    } else if let Some(obj) = value.as_object() {
        let obs_array = obj
            .get("observations")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| {
                IronclawError::Memory(format!(
                    "observer response object missing 'observations' array\nresponse: {content}"
                ))
            })?
            .clone();

        let narr = obj
            .get("narrative")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .map(String::from);

        (obs_array, narr)
    } else {
        return Err(IronclawError::Memory(format!(
            "observer response is not a JSON array or object\nresponse: {content}"
        )));
    };

    let extractions = parse_extraction_items(&items, tz);

    if extractions.is_empty() {
        return Err(IronclawError::Memory(
            "observer returned empty observations array".to_string(),
        ));
    }

    Ok(ObserverParseResult {
        extractions,
        narrative,
    })
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
            .and_then(serde_json::Value::as_str)
            .map_or(Visibility::User, |v| {
                if v == "background" {
                    Visibility::Background
                } else {
                    Visibility::User
                }
            });

        extractions.push(ObserverExtraction {
            content: obs_content.to_string(),
            timestamp,
            visibility,
        });
    }

    extractions
}
