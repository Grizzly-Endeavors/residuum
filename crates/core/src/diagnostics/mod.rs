#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Location {
    Line { line: u32 },
    LineColumn { line: u32, column: u32 },
    Path { path: String },
}

impl Location {
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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<Location>,
}

impl Diagnostic {
    #[must_use]
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            message: message.into(),
            location: None,
        }
    }

    #[must_use]
    pub fn error_at(message: impl Into<String>, location: Location) -> Self {
        Self {
            severity: Severity::Error,
            message: message.into(),
            location: Some(location),
        }
    }

    #[must_use]
    pub fn warning(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            message: message.into(),
            location: None,
        }
    }

    #[must_use]
    pub fn warning_at(message: impl Into<String>, location: Location) -> Self {
        Self {
            severity: Severity::Warning,
            message: message.into(),
            location: Some(location),
        }
    }

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
