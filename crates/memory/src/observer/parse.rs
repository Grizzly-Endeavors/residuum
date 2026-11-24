use chrono::NaiveDateTime;
use chrono_tz::Tz;
use serde::Deserialize;

use crate::types::Visibility;
use residuum_core::time::now_local;
use residuum_inference::InferenceResponse;

#[derive(Deserialize)]
struct ObserverJsonResponse {
    observations: Vec<ObservationItem>,
    narrative: String,
}

#[derive(Deserialize)]
struct ObservationItem {
    content: String,
    timestamp: String,
    visibility: Visibility,
}

pub(super) struct ObserverExtraction {
    pub(super) content: String,
    pub(super) timestamp: NaiveDateTime,
    pub(super) visibility: Visibility,
}

pub(super) struct ObserverParseResult {
    pub(super) extractions: Vec<ObserverExtraction>,
    pub(super) narrative: Option<String>,
}

pub(super) fn parse_observer_response(
    response: &InferenceResponse,
    tz: Tz,
) -> anyhow::Result<ObserverParseResult> {
    let content = response.content.trim();
    let json_str = residuum_inference::strip_code_fences(content);

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
            let timestamp = crate::parse_minute_timestamp(&item.timestamp, tz);
            ObserverExtraction {
                content: item.content.clone(),
                timestamp,
                visibility: item.visibility.clone(),
            }
        })
        .collect()
}

pub(super) fn parse_extraction_items(
    items: &[serde_json::Value],
    tz: Tz,
) -> Vec<ObserverExtraction> {
    let mut extractions = Vec::new();

    for (i, item) in items.iter().enumerate() {
        let Some(obs_content) = item.get("content").and_then(serde_json::Value::as_str) else {
            tracing::warn!(
                item_index = i,
                "observer response item missing 'content' field, skipping"
            );
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
                        item_index = i,
                        content = obs_content,
                        "observer response item missing 'timestamp', using current time"
                    );
                    now_local(tz)
                },
                |ts| crate::parse_minute_timestamp(ts, tz),
            );

        let visibility = item
            .get("visibility")
            .and_then(|v| serde_json::from_value::<Visibility>(v.clone()).ok())
            .unwrap_or_default();

        extractions.push(ObserverExtraction {
            content: obs_content.to_string(),
            timestamp,
            visibility,
        });
    }

    extractions
}
