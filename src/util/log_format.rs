//! Structured log parsing and formatting for NDJSON log files.
//!
//! Parses JSON log lines produced by `tracing-subscriber`'s JSON formatter
//! and renders them as human-readable text with optional filtering by module
//! or log level. Designed for reuse by both `residuum logs` and future
//! `residuum report`.

use std::fmt::Write;

/// A single parsed log entry from a JSON log line.
#[derive(serde::Deserialize)]
pub struct LogEntry {
    pub timestamp: String,
    pub level: String,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub fields: serde_json::Value,
    #[serde(default)]
    pub spans: Vec<SpanEntry>,
}

/// A span in the active span stack at the time of the log event.
#[derive(serde::Deserialize)]
pub struct SpanEntry {
    pub name: String,
    #[serde(flatten)]
    pub fields: serde_json::Map<String, serde_json::Value>,
}

/// Log severity levels in ascending order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    /// Parse a level string (case-insensitive).
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_uppercase().as_str() {
            "TRACE" => Some(Self::Trace),
            "DEBUG" => Some(Self::Debug),
            "INFO" => Some(Self::Info),
            "WARN" => Some(Self::Warn),
            "ERROR" => Some(Self::Error),
            _ => None,
        }
    }
}

/// Expand a shorthand module name to a full target prefix.
///
/// Accepts either a bare module name (`agent`) or a fully qualified
/// target prefix (`residuum::agent::core`).
#[must_use]
pub fn expand_module_filter(module: &str) -> String {
    if module.contains("::") {
        module.to_string()
    } else {
        format!("residuum::{module}")
    }
}

/// Check whether a log entry's target matches the given module filter.
#[must_use]
pub fn matches_module(target: &str, module_filter: &str) -> bool {
    target.starts_with(module_filter)
}

/// Check whether a log entry's level meets the minimum severity threshold.
#[must_use]
pub fn meets_level(entry_level: &str, min_level: LogLevel) -> bool {
    LogLevel::parse(entry_level).is_some_and(|l| l >= min_level)
}

/// Try to parse a JSON log line into a `LogEntry`.
#[must_use]
pub fn parse_line(line: &str) -> Option<LogEntry> {
    serde_json::from_str(line).ok()
}

/// Format a parsed log entry as a human-readable string.
///
/// Output format:
/// ```text
/// 2026-03-23T10:15:30  INFO  [agent::core] processing user message  correlation_id=abc123
/// ```
///
/// When `color` is true, the level is ANSI-colored by severity.
#[must_use]
pub fn format_entry(entry: &LogEntry) -> String {
    format_entry_impl(entry, false)
}

/// Format a parsed log entry with optional ANSI color on the level field.
#[must_use]
pub fn format_entry_colored(entry: &LogEntry) -> String {
    format_entry_impl(entry, true)
}

fn format_entry_impl(entry: &LogEntry, color: bool) -> String {
    use owo_colors::OwoColorize;

    let mut out = String::with_capacity(128);

    // Timestamp — trim sub-microsecond precision for readability
    let ts = truncate_timestamp(&entry.timestamp);
    _ = write!(out, "{ts}  ");

    // Level — right-padded to 5 chars, optionally colored
    if color {
        let padded = format!("{:<5}", entry.level);
        let colored = match entry.level.to_ascii_uppercase().as_str() {
            "ERROR" => format!("{}", padded.red().bold()),
            "WARN" => format!("{}", padded.yellow()),
            "INFO" => format!("{}", padded.green()),
            "DEBUG" | "TRACE" => format!("{}", padded.dimmed()),
            _ => padded,
        };
        _ = write!(out, "{colored}  ");
    } else {
        _ = write!(out, "{:<5}  ", entry.level);
    }

    // Target — strip residuum:: prefix for brevity
    let short_target = entry
        .target
        .strip_prefix("residuum::")
        .unwrap_or(&entry.target);
    _ = write!(out, "[{short_target}]");

    // Message from fields
    if let Some(msg) = entry.fields.get("message").and_then(|v| v.as_str()) {
        _ = write!(out, " {msg}");
    }

    // Extra structured fields (skip "message" since we already printed it)
    append_fields(&mut out, &entry.fields);

    // Span context fields (flattened from the span stack)
    for span in &entry.spans {
        for (k, v) in &span.fields {
            _ = write!(out, "  {k}={}", format_value(v));
        }
    }

    out
}

/// Append non-message fields from the event's field map.
fn append_fields(out: &mut String, fields: &serde_json::Value) {
    let Some(obj) = fields.as_object() else {
        return;
    };
    for (k, v) in obj {
        if k == "message" {
            continue;
        }
        _ = write!(out, "  {k}={}", format_value(v));
    }
}

/// Format a JSON value for display in key=value pairs.
fn format_value(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::Array(_)
        | serde_json::Value::Object(_) => v.to_string(),
    }
}

/// Truncate an RFC 3339 timestamp to seconds precision for readability.
fn truncate_timestamp(ts: &str) -> &str {
    // tracing-subscriber emits timestamps like "2026-03-23T10:15:30.123456Z"
    // Truncate at the dot to get "2026-03-23T10:15:30"
    ts.split_once('.').map_or(ts, |(before, _)| before)
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(
    clippy::indexing_slicing,
    reason = "test code indexes known-length slices"
)]
mod tests {
    use super::*;

    fn sample_json_line() -> &'static str {
        r#"{"timestamp":"2026-03-23T10:15:30.123456Z","level":"INFO","target":"residuum::agent::core","fields":{"message":"processing user message","source":"web"},"spans":[{"name":"gateway"},{"name":"inbound_message","correlation_id":"abc123"}]}"#
    }

    #[test]
    fn parse_valid_json_line() {
        let entry = parse_line(sample_json_line()).unwrap();
        assert_eq!(entry.level, "INFO");
        assert_eq!(entry.target, "residuum::agent::core");
        assert_eq!(
            entry.fields.get("message").unwrap().as_str().unwrap(),
            "processing user message"
        );
        assert_eq!(entry.spans.len(), 2);
        assert_eq!(entry.spans[0].name, "gateway");
        assert_eq!(entry.spans[1].name, "inbound_message");
    }

    #[test]
    fn parse_line_with_span_fields() {
        let entry = parse_line(sample_json_line()).unwrap();
        let span = &entry.spans[1];
        assert_eq!(
            span.fields.get("correlation_id").unwrap().as_str().unwrap(),
            "abc123"
        );
    }

    #[test]
    fn parse_invalid_line_returns_none() {
        assert!(parse_line("not json at all").is_none());
        assert!(parse_line("").is_none());
    }

    #[test]
    fn parse_partial_json_returns_none() {
        assert!(parse_line(r#"{"timestamp":"2026-03-23"#).is_none());
    }

    #[test]
    fn module_filter_shorthand_expansion() {
        assert_eq!(expand_module_filter("agent"), "residuum::agent");
        assert_eq!(expand_module_filter("mcp"), "residuum::mcp");
        assert_eq!(
            expand_module_filter("residuum::mcp::client"),
            "residuum::mcp::client"
        );
    }

    #[test]
    fn module_matching() {
        let target = "residuum::agent::core";
        assert!(matches_module(target, "residuum::agent"));
        assert!(!matches_module(target, "residuum::tools"));
        assert!(matches_module(target, "residuum::agent::core"));
        assert!(!matches_module(target, "residuum::agent::core::deep"));
    }

    #[test]
    fn level_filtering() {
        assert!(meets_level("ERROR", LogLevel::Warn));
        assert!(meets_level("WARN", LogLevel::Warn));
        assert!(!meets_level("INFO", LogLevel::Warn));
        assert!(!meets_level("DEBUG", LogLevel::Warn));
        assert!(!meets_level("TRACE", LogLevel::Warn));
    }

    #[test]
    fn level_filtering_all_levels_pass_trace() {
        for level in &["TRACE", "DEBUG", "INFO", "WARN", "ERROR"] {
            assert!(meets_level(level, LogLevel::Trace));
        }
    }

    #[test]
    fn level_parse_case_insensitive() {
        assert_eq!(LogLevel::parse("info"), Some(LogLevel::Info));
        assert_eq!(LogLevel::parse("INFO"), Some(LogLevel::Info));
        assert_eq!(LogLevel::parse("Info"), Some(LogLevel::Info));
        assert_eq!(LogLevel::parse("bogus"), None);
    }

    #[test]
    fn format_entry_human_readable() {
        let entry = parse_line(sample_json_line()).unwrap();
        let formatted = format_entry(&entry);

        assert!(formatted.contains("2026-03-23T10:15:30"));
        assert!(formatted.contains("INFO"));
        assert!(formatted.contains("[agent::core]"));
        assert!(formatted.contains("processing user message"));
        assert!(formatted.contains("source=web"));
        assert!(formatted.contains("correlation_id=abc123"));
        // Should NOT contain the full "residuum::" prefix in target
        assert!(!formatted.contains("[residuum::agent::core]"));
    }

    #[test]
    fn format_entry_strips_timestamp_precision() {
        let entry = parse_line(sample_json_line()).unwrap();
        let formatted = format_entry(&entry);
        assert!(formatted.starts_with("2026-03-23T10:15:30"));
        assert!(!formatted.contains(".123456Z"));
    }

    #[test]
    fn format_entry_minimal_fields() {
        let line = r#"{"timestamp":"2026-03-23T10:00:00Z","level":"DEBUG","target":"residuum::tools","fields":{"message":"hello"},"spans":[]}"#;
        let entry = parse_line(line).unwrap();
        let formatted = format_entry(&entry);
        assert!(formatted.contains("DEBUG"));
        assert!(formatted.contains("[tools]"));
        assert!(formatted.contains("hello"));
    }
}
