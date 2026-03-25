//! Content sanitization for trace exports.
//!
//! Redacts potentially sensitive information (user messages, agent responses,
//! tool call arguments) from spans before they are exported to external
//! endpoints. Controlled by the `sanitize_content` config flag.

use crate::util::telemetry::CompletedSpan;

/// Field names whose values should be redacted in trace exports.
const SENSITIVE_FIELDS: &[&str] = &[
    "content",
    "input",
    "arguments",
    "prompt",
    "response",
    "message",
    "body",
    "text",
    "output",
    "tool_input",
    "tool_output",
    "user_message",
    "agent_message",
    "system_prompt",
];

/// Replacement value for redacted content.
const REDACTED: &str = "[REDACTED]";

/// Redact sensitive content from spans in-place before export.
///
/// Strips field values matching known sensitive field names and replaces
/// event messages with `[REDACTED]`. Structural metadata (span names,
/// targets, levels, timing, hierarchy) is preserved.
pub fn sanitize_spans(spans: &mut [CompletedSpan]) {
    for span in spans {
        sanitize_fields(&mut span.fields);
        for event in &mut span.events {
            if !event.message.is_empty() {
                event.message = REDACTED.to_string();
            }
            sanitize_fields(&mut event.fields);
        }
    }
}

/// Redact values of sensitive fields.
fn sanitize_fields(fields: &mut [(String, String)]) {
    for (key, value) in fields.iter_mut() {
        if SENSITIVE_FIELDS.iter().any(|&s| key == s) {
            *value = REDACTED.to_string();
        }
    }
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "test code indexes known-length slices"
)]
mod tests {
    use super::*;
    use crate::util::telemetry::SpanEvent;
    use std::time::{Duration, SystemTime};

    fn make_span(fields: Vec<(&str, &str)>, events: Vec<SpanEvent>) -> CompletedSpan {
        CompletedSpan {
            span_id: 1,
            parent_id: None,
            name: "test_span".to_string(),
            target: "residuum::test".to_string(),
            level: tracing::Level::INFO,
            start: SystemTime::now(),
            duration: Duration::from_millis(10),
            fields: fields
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            events,
        }
    }

    fn make_event(message: &str, fields: Vec<(&str, &str)>) -> SpanEvent {
        SpanEvent {
            timestamp: SystemTime::now(),
            level: tracing::Level::INFO,
            message: message.to_string(),
            fields: fields
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn redacts_sensitive_fields() {
        let mut spans = vec![make_span(
            vec![
                ("content", "secret user message"),
                ("correlation_id", "abc123"),
                ("prompt", "system instructions"),
            ],
            vec![],
        )];

        sanitize_spans(&mut spans);

        let fields: std::collections::HashMap<_, _> = spans[0].fields.iter().cloned().collect();
        assert_eq!(fields["content"], REDACTED);
        assert_eq!(fields["correlation_id"], "abc123"); // not sensitive
        assert_eq!(fields["prompt"], REDACTED);
    }

    #[test]
    fn redacts_event_messages() {
        let mut spans = vec![make_span(
            vec![],
            vec![make_event("user said hello", vec![("input", "hello")])],
        )];

        sanitize_spans(&mut spans);

        assert_eq!(spans[0].events[0].message, REDACTED);
        let fields: std::collections::HashMap<_, _> =
            spans[0].events[0].fields.iter().cloned().collect();
        assert_eq!(fields["input"], REDACTED);
    }

    #[test]
    fn preserves_structural_metadata() {
        let mut spans = vec![make_span(vec![("content", "secret")], vec![])];
        let original_name = spans[0].name.clone();
        let original_target = spans[0].target.clone();
        let original_level = spans[0].level;
        let original_id = spans[0].span_id;

        sanitize_spans(&mut spans);

        assert_eq!(spans[0].name, original_name);
        assert_eq!(spans[0].target, original_target);
        assert_eq!(spans[0].level, original_level);
        assert_eq!(spans[0].span_id, original_id);
    }

    #[test]
    fn empty_message_not_replaced() {
        let mut spans = vec![make_span(vec![], vec![make_event("", vec![])])];

        sanitize_spans(&mut spans);

        assert_eq!(spans[0].events[0].message, "");
    }
}
