//! Diagnostic types for strictly-parsed workspace and config files.
//!
//! [`Diagnostic`] is what every validator returns: a severity, a message, and
//! where the problem is. The validators themselves live beside the file types
//! they check, and the dispatcher that picks one by file path is
//! [`crate::diagnostics_dispatch`].

/// How serious a diagnostic is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// The affected entry, pulse, or file will not load or run until fixed.
    Error,
    /// Loading still succeeds, but the value is ignored, deprecated, or
    /// otherwise worth a second look.
    Warning,
}

/// Where in the file a diagnostic applies.
///
/// A parser that hands back a byte offset or a line/column (toml, `toml_edit`,
/// `serde_yaml_ng`, `serde_json`) gets [`Location::Line`]/[`Location::LineColumn`].
/// A problem found after parsing, where nothing tracks source position (an
/// unrecognized MCP transport, a duplicate pulse name), gets a [`Location::Path`]
/// naming the key instead — e.g. `mcpServers.filesystem` or `pulses.morning-check`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Location {
    /// A 1-based line number, with no column (e.g. a whole-pulse problem).
    Line { line: u32 },
    /// A 1-based line and column, as reported by the parser.
    LineColumn { line: u32, column: u32 },
    /// A key path into the parsed structure, for a problem with no source
    /// position (found only after the file parsed successfully).
    Path { path: String },
}

impl Location {
    /// Build a [`Location::LineColumn`] from a byte offset into `text`,
    /// translating it by counting newlines — for parsers (`toml`/`toml_edit`)
    /// that report a byte span rather than a line/column directly.
    #[must_use]
    pub fn from_byte_offset(text: &str, offset: usize) -> Self {
        let mut line: u32 = 1;
        let mut column: u32 = 1;
        for (idx, ch) in text.char_indices() {
            if idx >= offset {
                break;
            }
            if ch == '\n' {
                line += 1;
                column = 1;
            } else {
                column += 1;
            }
        }
        Self::LineColumn { line, column }
    }
}

impl std::fmt::Display for Location {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Line { line } => write!(f, "line {line}"),
            Self::LineColumn { line, column } => write!(f, "line {line}, column {column}"),
            Self::Path { path } => write!(f, "at {path}"),
        }
    }
}

/// One problem found in a strictly-parsed file: what it is, how serious it
/// is, and where it is when the parser or checker could tell us.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<Location>,
}

impl Diagnostic {
    /// An error with no known source position.
    #[must_use]
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            message: message.into(),
            location: None,
        }
    }

    /// An error at a known location.
    #[must_use]
    pub fn error_at(message: impl Into<String>, location: Location) -> Self {
        Self {
            severity: Severity::Error,
            message: message.into(),
            location: Some(location),
        }
    }

    /// A warning with no known source position.
    #[must_use]
    pub fn warning(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            message: message.into(),
            location: None,
        }
    }

    /// A warning at a known location.
    #[must_use]
    pub fn warning_at(message: impl Into<String>, location: Location) -> Self {
        Self {
            severity: Severity::Warning,
            message: message.into(),
            location: Some(location),
        }
    }

    /// Render as `"<file_label> line 14: message"`, or `"<file_label>: message"`
    /// when no location is known — the form both the agent tool result and the
    /// load/reload notices use.
    #[must_use]
    pub fn display(&self, file_label: &str) -> String {
        match &self.location {
            Some(location) => format!("{file_label} {location}: {}", self.message),
            None => format!("{file_label}: {}", self.message),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn location_from_byte_offset_first_line() {
        let text = "abc";
        assert_eq!(
            Location::from_byte_offset(text, 1),
            Location::LineColumn { line: 1, column: 2 }
        );
    }

    #[test]
    fn location_from_byte_offset_after_newline() {
        let text = "abc\ndef";
        assert_eq!(
            Location::from_byte_offset(text, 5),
            Location::LineColumn { line: 2, column: 2 }
        );
    }

    #[test]
    fn diagnostic_display_with_location() {
        let d = Diagnostic::error_at("bad value", Location::Line { line: 3 });
        assert_eq!(
            d.display("HEARTBEAT.yml"),
            "HEARTBEAT.yml line 3: bad value"
        );
    }

    #[test]
    fn diagnostic_display_without_location() {
        let d = Diagnostic::error("bad value");
        assert_eq!(d.display("config.toml"), "config.toml: bad value");
    }
}
