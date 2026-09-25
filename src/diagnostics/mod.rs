//! Diagnostics for strictly-parsed workspace/config files.
//!
//! One validator per file type — `config.toml`, `providers.toml`,
//! `config/channels.toml`, `config/mcp.json`, `config/a2a.json`,
//! `HEARTBEAT.yml`, and a skill's `SKILL.md` frontmatter — each built
//! directly on the loader or validation function that file's real load path
//! already uses, so a diagnostic can never disagree with what loading
//! actually rejects or skips. [`diagnose`] is the single entry point: it
//! picks the validator by the file's path and returns diagnostics for
//! `content`, or `None` if the path isn't one of the files this module
//! understands.
//!
//! Reused in three places: after an agent `write_file`/`edit_file` call
//! (`src/tools/write.rs`, `src/tools/edit.rs`), the `POST
//! /api/workspace/validate` endpoint and the raw config save handlers
//! (`src/gateway/web/workspace.rs`, `config.rs`, `providers.rs`), and the
//! load/reload notices for these files.

use std::path::{Path, PathBuf};

/// How serious a diagnostic is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
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

/// The directories `diagnose` needs to resolve which file a path refers to
/// and, for `config.toml`/`providers.toml`, to validate against the sibling
/// file on disk.
#[derive(Debug, Clone)]
pub struct DiagnosticsPaths {
    /// The app config directory (`~/.residuum/` by default), holding
    /// `config.toml` and `providers.toml`.
    pub config_dir: PathBuf,
    /// The workspace root, holding `config/channels.toml`, `config/mcp.json`,
    /// `config/a2a.json`, `HEARTBEAT.yml`, and skill `SKILL.md` files.
    pub workspace_dir: PathBuf,
}

/// Pick the validator for `path` and return diagnostics for `content`.
///
/// Returns `None` if `path` isn't one of the strictly-parsed files this
/// module understands — callers treat that as "nothing to check", not as a
/// problem.
///
/// `config.toml`/`providers.toml` are matched by their canonical location
/// under `paths.config_dir`; `config/channels.toml`, `config/mcp.json`, and
/// `config/a2a.json` by their canonical location under `paths.workspace_dir`.
/// `HEARTBEAT.yml` and `SKILL.md` are matched by filename alone, regardless
/// of directory, matching how the rest of the codebase already recognizes
/// them (see `is_heartbeat_file` in `src/gateway/web/workspace.rs` and the
/// skill scanner, which accepts a `SKILL.md` anywhere under the skills root).
#[must_use]
pub fn diagnose(path: &Path, content: &str, paths: &DiagnosticsPaths) -> Option<Vec<Diagnostic>> {
    let file_name = path.file_name().and_then(|n| n.to_str())?;

    if paths_match(path, &paths.config_dir.join("config.toml")) {
        return Some(crate::config::Config::diagnose_toml(
            content,
            &paths.config_dir,
        ));
    }
    if paths_match(path, &paths.config_dir.join("providers.toml")) {
        return Some(crate::config::Config::diagnose_providers_toml(
            content,
            &paths.config_dir,
        ));
    }

    let layout = crate::workspace::layout::WorkspaceLayout::new(&paths.workspace_dir);
    if paths_match(path, &layout.channels_toml()) {
        return Some(crate::workspace::config::diagnose_channels_toml(content));
    }
    if paths_match(path, &layout.mcp_json()) {
        return Some(crate::workspace::config::diagnose_mcp_json(content));
    }
    if paths_match(path, &layout.a2a_agents_json()) {
        return Some(crate::a2a::client::config::diagnose_a2a_json(content));
    }

    if file_name == "HEARTBEAT.yml" {
        return Some(crate::pulse::types::diagnose_heartbeat(content));
    }
    if file_name == "SKILL.md" {
        return Some(crate::skills::diagnose_skill_md(content));
    }

    None
}

/// Compare `path` against `target` (an absolute path this module builds from
/// a known directory), tolerating `path` being relative to the process's
/// working directory or given in a different but equivalent form. Falls back
/// to a plain equality check if either side can't be canonicalized (e.g. the
/// file doesn't exist yet), which is the common case for a brand-new file
/// being validated before its first write.
fn paths_match(path: &Path, target: &Path) -> bool {
    if path == target {
        return true;
    }
    match (path.canonicalize(), target.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
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

    #[test]
    fn diagnose_returns_none_for_unrelated_file() {
        let dir = tempfile::tempdir().unwrap();
        let paths = DiagnosticsPaths {
            config_dir: dir.path().to_path_buf(),
            workspace_dir: dir.path().join("workspace"),
        };
        assert!(diagnose(Path::new("notes.md"), "hello", &paths).is_none());
    }

    #[test]
    fn diagnose_dispatches_config_toml_by_canonical_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("providers.toml"),
            "[models]\nmain = \"a/b\"\n",
        )
        .unwrap();
        let paths = DiagnosticsPaths {
            config_dir: dir.path().to_path_buf(),
            workspace_dir: dir.path().join("workspace"),
        };
        let diagnostics = diagnose(&dir.path().join("config.toml"), "not valid toml [", &paths)
            .expect("config.toml should be recognized");
        assert!(
            !diagnostics.is_empty(),
            "invalid TOML should produce a diagnostic"
        );
        assert_eq!(diagnostics.first().unwrap().severity, Severity::Error);
    }

    #[test]
    fn diagnose_dispatches_heartbeat_by_filename_regardless_of_directory() {
        let dir = tempfile::tempdir().unwrap();
        let paths = DiagnosticsPaths {
            config_dir: dir.path().to_path_buf(),
            workspace_dir: dir.path().join("workspace"),
        };
        let diagnostics = diagnose(
            Path::new("some/nested/HEARTBEAT.yml"),
            "pulses:\n  - name: a\n    schedule: 30m\n",
            &paths,
        )
        .expect("HEARTBEAT.yml should be recognized regardless of directory");
        assert!(diagnostics.is_empty(), "valid heartbeat has no diagnostics");
    }
}
