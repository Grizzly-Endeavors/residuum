//! Structured log parsing and formatting for NDJSON log files.
//!
//! Parses JSON log lines produced by `tracing-subscriber`'s JSON formatter
//! and renders them as human-readable text with optional filtering by module
//! or log level. Used by `residuum logs`.

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

/// Display form of a log target: `residuum::` is removed from the front, and a
/// crate target `residuum_<x>::…` shows as `<x>::…`. A target without `::` is
/// returned unchanged.
#[must_use]
pub fn display_target(target: &str) -> String {
    let Some((first, rest)) = target.split_once("::") else {
        return target.to_string();
    };
    if first == "residuum" {
        return rest.to_string();
    }
    match first.strip_prefix("residuum_") {
        Some(krate) => format!("{krate}::{rest}"),
        None => target.to_string(),
    }
}

/// Module form of a log target: the target with its first segment removed. A
/// target without `::` is returned unchanged.
#[must_use]
pub fn module_target(target: &str) -> &str {
    target.split_once("::").map_or(target, |(_, rest)| rest)
}

/// Whether a `--module` value selects a log target.
///
/// `filter` is the value as typed. It matches when it is a plain string prefix
/// of the target, or when it, with one leading `residuum::` removed, is a plain
/// string prefix of the target's display form or module form.
#[must_use]
pub fn module_matches(filter: &str, target: &str) -> bool {
    let bare = filter.strip_prefix("residuum::").unwrap_or(filter);
    target.starts_with(filter)
        || display_target(target).starts_with(bare)
        || module_target(target).starts_with(bare)
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

    _ = write!(out, "[{}]", display_target(&entry.target));

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
    fn display_residuum_module() {
        assert_eq!(display_target("residuum::hub::runtime"), "hub::runtime");
    }

    #[test]
    fn display_crate_single_module() {
        assert_eq!(display_target("residuum_memory::search"), "memory::search");
    }

    #[test]
    fn display_crate_inference() {
        assert_eq!(
            display_target("residuum_inference::factory"),
            "inference::factory"
        );
    }

    #[test]
    fn display_crate_tracing_service() {
        assert_eq!(
            display_target("residuum_tracing_service::submit"),
            "tracing_service::submit"
        );
    }

    #[test]
    fn display_crate_config_root_module() {
        assert_eq!(display_target("residuum_config::load"), "config::load");
    }

    #[test]
    fn display_crate_config_bundled_module() {
        assert_eq!(
            display_target("residuum_config::agent_keys::store"),
            "config::agent_keys::store"
        );
    }

    #[test]
    fn display_crate_core_bundled_module() {
        assert_eq!(display_target("residuum_core::util::fs"), "core::util::fs");
    }

    #[test]
    fn display_crate_net_bundled_module() {
        assert_eq!(
            display_target("residuum_net::tunnel::v2"),
            "net::tunnel::v2"
        );
    }

    #[test]
    fn match_agent_residuum_module_by_display() {
        assert!(module_matches("agent", "residuum::agent::core"));
    }

    #[test]
    fn match_agent_config_bundled_module_by_module_form() {
        assert!(module_matches(
            "agent",
            "residuum_config::agent_keys::store"
        ));
    }

    #[test]
    fn match_memory_crate_by_display() {
        assert!(module_matches("memory", "residuum_memory::search"));
    }

    #[test]
    fn match_tunnel_in_net_by_module_form() {
        assert!(module_matches("tunnel", "residuum_net::tunnel::v2"));
    }

    #[test]
    fn match_net_crate_short_name_by_display() {
        assert!(module_matches("net", "residuum_net::pairing::gate"));
    }

    #[test]
    fn match_config_crate_short_name_by_display() {
        assert!(module_matches(
            "config",
            "residuum_config::agent_keys::store"
        ));
    }

    #[test]
    fn match_hub_residuum_module_by_display() {
        assert!(module_matches("hub", "residuum::hub::runtime"));
    }

    #[test]
    fn match_residuum_prefixed_filter_by_module_form() {
        assert!(module_matches(
            "residuum::tunnel::v2",
            "residuum_net::tunnel::v2::link"
        ));
    }

    #[test]
    fn match_residuum_prefixed_filter_by_display_after_strip() {
        assert!(module_matches(
            "residuum::mcp::client",
            "residuum_mcp::client"
        ));
    }

    #[test]
    fn match_crate_target_written_out() {
        assert!(module_matches(
            "residuum_net::tunnel",
            "residuum_net::tunnel::v2"
        ));
    }

    #[test]
    fn match_diagnostics_by_module_form_in_core() {
        assert!(module_matches("diagnostics", "residuum_core::diagnostics"));
    }

    #[test]
    fn match_diagnostics_by_display_in_dispatch_module() {
        assert!(module_matches(
            "diagnostics",
            "residuum::diagnostics_dispatch"
        ));
    }

    #[test]
    fn match_module_below_crate_by_its_own_name() {
        assert!(module_matches("search", "residuum_memory::search"));
    }

    #[test]
    fn match_rejects_unrelated_module() {
        assert!(!module_matches("tools", "residuum_net::tunnel::v2"));
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
