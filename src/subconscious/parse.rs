//! Response parsing for the subconscious classifier LLM output.

use serde::Deserialize;

use super::{EvalOutcome, Finding, FindingKind, LearnSignal, LearnSignalType, Severity};
use crate::models::ModelResponse;

/// Typed response from structured output mode.
#[derive(Deserialize)]
struct SubconsciousJsonResponse {
    findings: Vec<FindingItem>,
    /// Absent when the learning loop is off; defaults to empty.
    #[serde(default)]
    learnings: Vec<LearnItem>,
}

/// Single finding item within the typed response.
#[derive(Deserialize)]
struct FindingItem {
    kind: FindingKind,
    severity: Severity,
    instruction: String,
}

/// Single learn-signal item within the typed response.
#[derive(Deserialize)]
struct LearnItem {
    summary: String,
    signal_type: LearnSignalType,
}

/// Parse the model's JSON response into findings and learn signals.
///
/// An empty findings array is the normal "everything is fine" result, not an
/// error. Tries typed deserialization first (structured output path), then
/// falls back to `Value`-based parsing that skips malformed items. Learn
/// signals are only extracted when `include_learnings` is set (end-of-turn
/// triage with the learning loop enabled); otherwise they are dropped even if
/// the model returned them.
///
/// # Errors
/// Returns an error if the response is not parseable JSON or lacks a
/// `findings` array.
pub(super) fn parse_subconscious_response(
    response: &ModelResponse,
    include_learnings: bool,
) -> anyhow::Result<EvalOutcome> {
    let content = response.content.trim();
    let json_str = crate::memory::strip_code_fences(content);

    // Fast path: typed deserialization (structured output)
    if let Ok(typed) = serde_json::from_str::<SubconsciousJsonResponse>(json_str) {
        let findings = typed
            .findings
            .into_iter()
            .filter(|item| !item.instruction.trim().is_empty())
            .map(|item| Finding {
                kind: item.kind,
                severity: item.severity,
                instruction: item.instruction,
            })
            .collect();
        let learnings = if include_learnings {
            typed
                .learnings
                .into_iter()
                .filter(|item| !item.summary.trim().is_empty())
                .map(|item| LearnSignal {
                    summary: item.summary,
                    signal_type: item.signal_type,
                })
                .collect()
        } else {
            Vec::new()
        };
        return Ok(EvalOutcome {
            findings,
            learnings,
        });
    }

    // Fallback: Value-based parsing for malformed responses
    tracing::debug!("subconscious structured output failed, falling back to value-based parsing");
    let value: serde_json::Value = serde_json::from_str(json_str).map_err(|e| {
        anyhow::anyhow!("failed to parse subconscious response as JSON: {e}\nresponse: {content}")
    })?;

    let items = value
        .get("findings")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            anyhow::anyhow!("subconscious response missing 'findings' array\nresponse: {content}")
        })?;
    let findings = parse_finding_items(items);

    let learnings = if include_learnings {
        value
            .get("learnings")
            .and_then(serde_json::Value::as_array)
            .map(|arr| parse_learn_items(arr))
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    Ok(EvalOutcome {
        findings,
        learnings,
    })
}

/// Parse individual finding items from a JSON array, skipping malformed ones.
fn parse_finding_items(items: &[serde_json::Value]) -> Vec<Finding> {
    let mut findings = Vec::new();

    for (i, item) in items.iter().enumerate() {
        let Some(instruction) = item
            .get("instruction")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.trim().is_empty())
        else {
            tracing::warn!(
                item_index = i,
                "subconscious finding missing 'instruction', skipping"
            );
            continue;
        };

        let kind = item
            .get("kind")
            .and_then(|v| serde_json::from_value::<FindingKind>(v.clone()).ok())
            .unwrap_or(FindingKind::Violation);

        let Some(severity) = item
            .get("severity")
            .and_then(|v| serde_json::from_value::<Severity>(v.clone()).ok())
        else {
            tracing::warn!(
                item_index = i,
                "subconscious finding has invalid 'severity', skipping"
            );
            continue;
        };

        findings.push(Finding {
            kind,
            severity,
            instruction: instruction.to_string(),
        });
    }

    findings
}

/// Parse individual learn-signal items from a JSON array, skipping malformed ones.
fn parse_learn_items(items: &[serde_json::Value]) -> Vec<LearnSignal> {
    let mut learnings = Vec::new();

    for (i, item) in items.iter().enumerate() {
        let Some(summary) = item
            .get("summary")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.trim().is_empty())
        else {
            tracing::warn!(
                item_index = i,
                "subconscious learning missing 'summary', skipping"
            );
            continue;
        };

        let Some(signal_type) = item
            .get("signal_type")
            .and_then(|v| serde_json::from_value::<LearnSignalType>(v.clone()).ok())
        else {
            tracing::warn!(
                item_index = i,
                "subconscious learning has invalid 'signal_type', skipping"
            );
            continue;
        };

        learnings.push(LearnSignal {
            summary: summary.to_string(),
            signal_type,
        });
    }

    learnings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(content: &str) -> anyhow::Result<Vec<Finding>> {
        Ok(
            parse_subconscious_response(&ModelResponse::new(content.to_string(), vec![]), true)?
                .findings,
        )
    }

    fn parse_learnings(content: &str) -> anyhow::Result<Vec<LearnSignal>> {
        Ok(
            parse_subconscious_response(&ModelResponse::new(content.to_string(), vec![]), true)?
                .learnings,
        )
    }

    #[test]
    fn parses_valid_findings() {
        let findings = parse(
            r#"{"findings": [
                {"kind": "violation", "severity": "act", "instruction": "Stop doing X."},
                {"kind": "omission", "severity": "note", "instruction": "Remember Y next time."}
            ]}"#,
        )
        .unwrap();

        assert_eq!(findings.len(), 2);
        let first = findings.first().unwrap();
        assert_eq!(first.kind, FindingKind::Violation);
        assert_eq!(first.severity, Severity::Act);
        assert_eq!(first.instruction, "Stop doing X.");
        assert_eq!(findings.get(1).map(|f| f.kind), Some(FindingKind::Omission));
        assert_eq!(findings.get(1).map(|f| f.severity), Some(Severity::Note));
    }

    #[test]
    fn empty_findings_is_ok() {
        let findings = parse(r#"{"findings": []}"#).unwrap();
        assert!(findings.is_empty(), "empty array is the normal good case");
    }

    #[test]
    fn parses_despite_code_fences() {
        let findings = parse(
            "```json\n{\"findings\": [{\"kind\": \"omission\", \"severity\": \"act\", \"instruction\": \"Save it.\"}]}\n```",
        )
        .unwrap();
        assert_eq!(findings.len(), 1, "should parse despite fences");
    }

    #[test]
    fn malformed_items_are_skipped() {
        let findings = parse(
            r#"{"findings": [
                {"kind": "violation", "severity": "act"},
                {"severity": "bogus", "instruction": "bad severity"},
                {"kind": "omission", "severity": "note", "instruction": "valid one"}
            ]}"#,
        )
        .unwrap();

        assert_eq!(findings.len(), 1, "only the valid item should survive");
        assert_eq!(
            findings.first().map(|f| f.instruction.as_str()),
            Some("valid one")
        );
    }

    #[test]
    fn missing_kind_defaults_to_violation() {
        let findings =
            parse(r#"{"findings": [{"severity": "note", "instruction": "no kind given"}]}"#)
                .unwrap();
        assert_eq!(
            findings.first().map(|f| f.kind),
            Some(FindingKind::Violation),
            "missing kind should default rather than drop the finding"
        );
    }

    #[test]
    fn empty_instruction_is_skipped() {
        let findings = parse(
            r#"{"findings": [{"kind": "omission", "severity": "act", "instruction": "  "}]}"#,
        )
        .unwrap();
        assert!(findings.is_empty(), "blank instruction is useless, skip it");
    }

    #[test]
    fn invalid_json_errors() {
        assert!(parse("not json at all").is_err());
    }

    #[test]
    fn missing_findings_array_errors() {
        assert!(parse(r#"{"something": "else"}"#).is_err());
    }

    #[test]
    fn parses_valid_learnings() {
        let learnings = parse_learnings(
            r#"{"findings": [], "learnings": [
                {"summary": "User prefers bullet points.", "signal_type": "preference"},
                {"summary": "Retried the deploy after a transient 502.", "signal_type": "recovery"}
            ]}"#,
        )
        .unwrap();

        assert_eq!(learnings.len(), 2);
        assert_eq!(
            learnings.first().map(|l| l.signal_type),
            Some(LearnSignalType::Preference)
        );
        assert_eq!(
            learnings.get(1).map(|l| l.signal_type),
            Some(LearnSignalType::Recovery)
        );
    }

    #[test]
    fn learnings_dropped_when_not_requested() {
        let outcome = parse_subconscious_response(
            &ModelResponse::new(
                r#"{"findings": [], "learnings": [
                    {"summary": "User prefers bullet points.", "signal_type": "preference"}
                ]}"#
                .to_string(),
                vec![],
            ),
            false,
        )
        .unwrap();
        assert!(
            outcome.learnings.is_empty(),
            "learnings must be dropped when not requested"
        );
    }

    #[test]
    fn malformed_learnings_are_skipped() {
        let learnings = parse_learnings(
            r#"{"findings": [], "learnings": [
                {"summary": "no signal type"},
                {"signal_type": "preference"},
                {"summary": "  ", "signal_type": "recovery"},
                {"summary": "valid one", "signal_type": "preference"}
            ]}"#,
        )
        .unwrap();
        assert_eq!(learnings.len(), 1, "only the valid learning should survive");
        assert_eq!(
            learnings.first().map(|l| l.summary.as_str()),
            Some("valid one")
        );
    }

    #[test]
    fn missing_learnings_array_is_ok() {
        let learnings = parse_learnings(r#"{"findings": []}"#).unwrap();
        assert!(
            learnings.is_empty(),
            "absent learnings array parses to empty, not an error"
        );
    }
}
