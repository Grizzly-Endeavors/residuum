//! Response parsing for the subconscious classifier LLM output.

use serde::Deserialize;

use super::{Finding, FindingKind, Severity};
use crate::models::ModelResponse;

/// Typed response from structured output mode.
#[derive(Deserialize)]
struct SubconsciousJsonResponse {
    findings: Vec<FindingItem>,
}

/// Single finding item within the typed response.
#[derive(Deserialize)]
struct FindingItem {
    kind: FindingKind,
    severity: Severity,
    instruction: String,
}

/// Parse the model's JSON response into findings.
///
/// An empty findings array is the normal "everything is fine" result, not an
/// error. Tries typed deserialization first (structured output path), then
/// falls back to `Value`-based parsing that skips malformed items.
///
/// # Errors
/// Returns an error if the response is not parseable JSON or lacks a
/// `findings` array.
pub(super) fn parse_subconscious_response(
    response: &ModelResponse,
) -> anyhow::Result<Vec<Finding>> {
    let content = response.content.trim();
    let json_str = crate::memory::strip_code_fences(content);

    // Fast path: typed deserialization (structured output)
    if let Ok(typed) = serde_json::from_str::<SubconsciousJsonResponse>(json_str) {
        return Ok(typed
            .findings
            .into_iter()
            .filter(|item| !item.instruction.trim().is_empty())
            .map(|item| Finding {
                kind: item.kind,
                severity: item.severity,
                instruction: item.instruction,
            })
            .collect());
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

    Ok(parse_finding_items(items))
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

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    fn parse(content: &str) -> anyhow::Result<Vec<Finding>> {
        parse_subconscious_response(&ModelResponse::new(content.to_string(), vec![]))
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
}
