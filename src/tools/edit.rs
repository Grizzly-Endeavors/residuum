//! Find-and-replace file editing tool for the agent.
//!
//! Each edit names the exact text to change (`old_string`) and what replaces it
//! (`new_string`). Edits in one call apply in order to an in-memory copy and the
//! file is written once, so a batch lands completely or not at all.

use std::ops::Range;

use async_trait::async_trait;
use serde_json::Value;

use super::file_tracker::SharedFileTracker;
use super::path_policy::SharedPathPolicy;
use super::read::format_numbered_line;
use super::{Tool, ToolError, ToolResult};
use crate::inference::ToolDefinition;

/// Lines of unchanged context shown around each changed region in the result preview.
const PREVIEW_CONTEXT_LINES: usize = 2;

/// Maximum preview lines returned after an edit; the model can `read_file` for more.
const MAX_PREVIEW_LINES: usize = 60;

/// Maximum match locations listed when `old_string` is ambiguous.
const MAX_LISTED_MATCHES: usize = 10;

/// Tool that applies find-and-replace edits to a file.
pub struct EditTool {
    tracker: SharedFileTracker,
    policy: SharedPathPolicy,
}

impl EditTool {
    /// Create a new `EditTool` with shared file tracker and path policy.
    #[must_use]
    pub fn new(tracker: SharedFileTracker, policy: SharedPathPolicy) -> Self {
        Self { tracker, policy }
    }
}

/// One requested replacement within an `edit_file` call.
struct EditRequest {
    old_string: String,
    new_string: String,
    replace_all: bool,
}

/// How an edit's `old_string` was located in the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatchKind {
    Exact,
    /// Matched whole lines after trimming leading and trailing whitespace on each.
    IgnoringWhitespace,
}

/// Result of applying every edit in a call to the file text.
struct BatchOutcome {
    text: String,
    /// Byte ranges in `text` holding content written by the edits.
    changed_spans: Vec<Range<usize>>,
    replacements: usize,
    /// 1-based indexes of edits that only matched after ignoring whitespace.
    whitespace_matched: Vec<usize>,
}

/// Parse the `path` and `edits` arguments.
fn parse_edit_args(arguments: &Value) -> Result<(&str, Vec<EditRequest>), ToolError> {
    let path = arguments
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ToolError::InvalidArguments("missing required 'path' argument".to_string())
        })?;

    let raw_edits = arguments
        .get("edits")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            ToolError::InvalidArguments(
                "missing required 'edits' argument (a list of {old_string, new_string} objects)"
                    .to_string(),
            )
        })?;

    if raw_edits.is_empty() {
        return Err(ToolError::InvalidArguments(
            "'edits' is empty; include at least one {old_string, new_string} object".to_string(),
        ));
    }

    let edits = raw_edits
        .iter()
        .enumerate()
        .map(|(i, raw)| parse_edit_request(raw, i + 1))
        .collect::<Result<Vec<_>, _>>()?;

    Ok((path, edits))
}

/// Parse and validate one entry of the `edits` list; `number` is its 1-based position.
fn parse_edit_request(raw: &Value, number: usize) -> Result<EditRequest, ToolError> {
    let field = |name: &str| {
        raw.get(name).and_then(Value::as_str).ok_or_else(|| {
            ToolError::InvalidArguments(format!("edit {number} is missing string '{name}'"))
        })
    };
    let old_string = field("old_string")?;
    let new_string = field("new_string")?;
    let replace_all = raw
        .get("replace_all")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    if old_string.is_empty() {
        return Err(ToolError::InvalidArguments(format!(
            "edit {number} has an empty old_string; to create or overwrite a whole file use write_file"
        )));
    }
    if old_string == new_string {
        return Err(ToolError::InvalidArguments(format!(
            "edit {number} has identical old_string and new_string, so there is nothing to change"
        )));
    }

    Ok(EditRequest {
        old_string: old_string.to_string(),
        new_string: new_string.to_string(),
        replace_all,
    })
}

/// The line break most of `text`'s lines use, so a stray CRLF doesn't convert an LF file.
fn dominant_line_ending(text: &str) -> &'static str {
    let crlf = text.matches("\r\n").count();
    let lf_only = text.matches('\n').count() - crlf;
    if crlf > lf_only { "\r\n" } else { "\n" }
}

/// Rewrite every line break in `text` to `eol`, whatever style the model sent.
fn normalize_line_endings(text: &str, eol: &str) -> String {
    let lf = text.replace("\r\n", "\n");
    if eol == "\n" {
        lf
    } else {
        lf.replace('\n', eol)
    }
}

/// 1-based line number containing byte `offset` of `text`.
fn line_number_at(text: &str, offset: usize) -> usize {
    text.bytes().take(offset).filter(|&b| b == b'\n').count() + 1
}

/// Comma-separated start lines of `ranges`, capped at `MAX_LISTED_MATCHES`.
fn describe_match_lines(text: &str, ranges: &[Range<usize>]) -> String {
    let mut listed: Vec<String> = ranges
        .iter()
        .take(MAX_LISTED_MATCHES)
        .map(|r| line_number_at(text, r.start).to_string())
        .collect();
    if ranges.len() > MAX_LISTED_MATCHES {
        listed.push(format!("and {} more", ranges.len() - MAX_LISTED_MATCHES));
    }
    listed.join(", ")
}

/// Whether any line of `text` starts with a `read_file` line-number prefix like `  12\t`.
fn has_read_prefix(text: &str) -> bool {
    text.lines().any(|line| {
        let trimmed = line.trim_start();
        let digits = trimmed.bytes().take_while(u8::is_ascii_digit).count();
        digits > 0
            && trimmed
                .get(digits..)
                .is_some_and(|rest| rest.starts_with('\t'))
    })
}

/// One line of a file with its byte offsets.
struct FileLine<'a> {
    start: usize,
    /// Line text without its line break.
    content: &'a str,
    /// End of the line including its line break.
    end: usize,
}

/// Split `text` into lines, keeping each line's byte offsets.
fn file_lines(text: &str) -> Vec<FileLine<'_>> {
    let mut offset = 0;
    text.split_inclusive('\n')
        .map(|raw| {
            let content = raw
                .strip_suffix('\n')
                .map_or(raw, |s| s.strip_suffix('\r').unwrap_or(s));
            let line = FileLine {
                start: offset,
                content,
                end: offset + raw.len(),
            };
            offset += raw.len();
            line
        })
        .collect()
}

/// Find runs of whole lines in `text` that equal `old` line-for-line once each line is trimmed.
///
/// The returned ranges cover the matched lines' text, plus the final line break when `old`
/// ends with one, so replacing a range behaves like replacing an exact match of `old`.
fn whitespace_insensitive_matches(text: &str, old: &str) -> Vec<Range<usize>> {
    let includes_final_break = old.ends_with('\n');
    let wanted: Vec<&str> = old
        .trim_end_matches(['\r', '\n'])
        .split('\n')
        .map(str::trim)
        .collect();
    if wanted.iter().all(|line| line.is_empty()) {
        return Vec::new();
    }

    let lines = file_lines(text);
    lines
        .windows(wanted.len())
        .filter(|window| {
            window
                .iter()
                .zip(&wanted)
                .all(|(line, want)| line.content.trim() == *want)
        })
        .filter_map(|window| {
            let first = window.first()?;
            let last = window.last()?;
            let end = if includes_final_break {
                last.end
            } else {
                last.start + last.content.len()
            };
            Some(first.start..end)
        })
        .collect()
}

/// Locate where `old` should be replaced in `text`.
///
/// Tries an exact match first. Only when there is no exact match does it retry ignoring
/// whitespace, and that retry is used only if it finds exactly one place.
fn find_matches(
    text: &str,
    old: &str,
    replace_all: bool,
) -> Result<(Vec<Range<usize>>, MatchKind), String> {
    let exact: Vec<Range<usize>> = text
        .match_indices(old)
        .map(|(start, matched)| start..start + matched.len())
        .collect();

    if exact.len() == 1 || (exact.len() > 1 && replace_all) {
        return Ok((exact, MatchKind::Exact));
    }
    if exact.len() > 1 {
        return Err(format!(
            "old_string matches {} places (lines {}); include more surrounding lines so it \
             matches exactly one, or set replace_all to change every occurrence",
            exact.len(),
            describe_match_lines(text, &exact)
        ));
    }

    let loose = whitespace_insensitive_matches(text, old);
    match loose.len() {
        1 => Ok((loose, MatchKind::IgnoringWhitespace)),
        0 if has_read_prefix(old) => Err(
            "old_string was not found; it includes read_file's line-number prefix \
             (like `  12\\t`), which is not part of the file. Copy the text without it"
                .to_string(),
        ),
        0 => Err(
            "old_string was not found in the file; re-read the file and copy the text \
             exactly as it appears"
                .to_string(),
        ),
        n => Err(format!(
            "old_string was not found exactly, and ignoring whitespace it matches {n} places \
             (lines {}); copy the text exactly, including indentation",
            describe_match_lines(text, &loose)
        )),
    }
}

/// Record that `replaced` was overwritten with `new_len` bytes, keeping earlier spans accurate.
fn record_replacement(spans: &mut Vec<Range<usize>>, replaced: &Range<usize>, new_len: usize) {
    let removed_len = replaced.end - replaced.start;
    let new_end = replaced.start + new_len;
    for span in spans.iter_mut() {
        if span.start >= replaced.end {
            *span = (span.start - removed_len + new_len)..(span.end - removed_len + new_len);
        } else if span.end > replaced.start {
            let end = if span.end > replaced.end {
                span.end - removed_len + new_len
            } else {
                new_end
            };
            *span = span.start.min(replaced.start)..end.max(new_end);
        }
    }
    spans.push(replaced.start..new_end);
}

/// Apply every edit in order to `original`. On failure returns which edit failed and why.
fn apply_edits(original: &str, edits: &[EditRequest]) -> Result<BatchOutcome, String> {
    let eol = dominant_line_ending(original);
    let mut text = original.to_string();
    let mut changed_spans = Vec::new();
    let mut replacements = 0;
    let mut whitespace_matched = Vec::new();

    for (i, edit) in edits.iter().enumerate() {
        let number = i + 1;
        let old = normalize_line_endings(&edit.old_string, eol);
        let new = normalize_line_endings(&edit.new_string, eol);
        let (ranges, kind) = find_matches(&text, &old, edit.replace_all)
            .map_err(|reason| format!("edit {number} of {}: {reason}", edits.len()))?;

        if kind == MatchKind::IgnoringWhitespace {
            whitespace_matched.push(number);
        }
        // Back to front, so earlier ranges stay valid while later ones are rewritten.
        for range in ranges.iter().rev() {
            text.replace_range(range.clone(), &new);
            record_replacement(&mut changed_spans, range, new.len());
        }
        replacements += ranges.len();
    }

    Ok(BatchOutcome {
        text,
        changed_spans,
        replacements,
        whitespace_matched,
    })
}

/// Numbered lines around each changed span, merged where they overlap.
fn render_preview(text: &str, spans: &[Range<usize>]) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return "(the file is now empty)".to_string();
    }

    let mut windows: Vec<(usize, usize)> = spans
        .iter()
        .map(|span| {
            let first = line_number_at(text, span.start);
            let last = if span.end > span.start {
                line_number_at(text, span.end - 1)
            } else {
                first
            };
            (
                first.saturating_sub(PREVIEW_CONTEXT_LINES).max(1),
                (last + PREVIEW_CONTEXT_LINES).min(lines.len()),
            )
        })
        .collect();
    windows.sort_unstable();

    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (first, last) in windows {
        match merged.last_mut() {
            Some(prev) if first <= prev.1 + 1 => prev.1 = prev.1.max(last),
            _ => merged.push((first, last)),
        }
    }

    let mut rendered: Vec<String> = Vec::new();
    for (first, last) in merged {
        if !rendered.is_empty() {
            rendered.push("   …".to_string());
        }
        for line_num in first..=last {
            let line = lines.get(line_num - 1).copied().unwrap_or_default();
            rendered.push(format_numbered_line(line_num, line).0);
        }
    }
    if rendered.len() > MAX_PREVIEW_LINES {
        rendered.truncate(MAX_PREVIEW_LINES);
        rendered.push("   … (preview truncated; use read_file to see the rest)".to_string());
    }
    rendered.join("\n")
}

/// Success message: summary line, any whitespace-match notes, then the preview.
fn format_success(path: &str, outcome: &BatchOutcome) -> String {
    let plural = if outcome.replacements == 1 { "" } else { "s" };
    let mut header = vec![format!(
        "edited {path} ({} replacement{plural})",
        outcome.replacements
    )];
    header.extend(outcome.whitespace_matched.iter().map(|number| {
        format!(
            "note: edit {number} matched only after ignoring whitespace differences; its \
             new_string was written exactly as given, so check the indentation below"
        )
    }));
    format!(
        "{}\n\n{}",
        header.join("\n"),
        render_preview(&outcome.text, &outcome.changed_spans)
    )
}

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &'static str {
        "edit_file"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: "Edit an existing file by replacing exact text. Each entry in 'edits' \
                          replaces old_string with new_string; old_string must match the file \
                          exactly once (include surrounding lines to make it unique) unless \
                          replace_all is true. Edits apply in order, each seeing the result of \
                          the ones before it, and the file is only written if every edit \
                          succeeds. Copy old_string from read_file output without the \
                          line-number prefix. To delete text, use an empty new_string. The file \
                          must have been read with read_file first. Use this over write_file \
                          when changing part of an existing file."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to edit"
                    },
                    "edits": {
                        "type": "array",
                        "description": "Replacements to apply, in order. Use one entry for a single change.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "old_string": {
                                    "type": "string",
                                    "description": "Exact text to replace, copied from the file"
                                },
                                "new_string": {
                                    "type": "string",
                                    "description": "Text to put in its place (empty to delete)"
                                },
                                "replace_all": {
                                    "type": "boolean",
                                    "description": "Replace every occurrence of old_string instead of requiring exactly one (default: false)"
                                }
                            },
                            "required": ["old_string", "new_string"]
                        }
                    }
                },
                "required": ["path", "edits"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let (path, edits) = parse_edit_args(&arguments)?;

        if let Err(reason) = self
            .policy
            .read()
            .await
            .check_write(std::path::Path::new(path))
        {
            return Ok(ToolResult::error(reason));
        }

        if !tokio::fs::try_exists(path).await.unwrap_or(false) {
            return Ok(ToolResult::error(format!(
                "file {path} does not exist; use write_file to create it"
            )));
        }

        if !self.tracker.lock().await.has_been_read(path) {
            return Ok(ToolResult::error(format!(
                "file {path} has not been read; use read_file before editing"
            )));
        }

        let original = match tokio::fs::read_to_string(path).await {
            Ok(text) => text,
            Err(e) => return Ok(ToolResult::error(format!("failed to read {path}: {e}"))),
        };

        let outcome = match apply_edits(&original, &edits) {
            Ok(outcome) => outcome,
            Err(reason) => {
                return Ok(ToolResult::error(format!(
                    "{reason}. No changes were written to {path}"
                )));
            }
        };

        if !outcome.whitespace_matched.is_empty() {
            tracing::debug!(
                path = %path,
                edits = ?outcome.whitespace_matched,
                "edit matched only after ignoring whitespace"
            );
        }

        if let Err(e) = tokio::fs::write(path, &outcome.text).await {
            return Ok(ToolResult::error(format!("failed to write {path}: {e}")));
        }

        Ok(ToolResult::success(format_success(path, &outcome)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::file_tracker::FileTracker;
    use crate::tools::path_policy::PathPolicy;

    /// Create an `EditTool` with a pre-registered path in the tracker.
    async fn make_tool_with_file(path: &str) -> EditTool {
        let tracker = FileTracker::new_shared();
        tracker.lock().await.record_read(path);
        EditTool::new(tracker, PathPolicy::new_shared())
    }

    /// Create an `EditTool` with an empty tracker (nothing read).
    fn make_tool_no_reads() -> EditTool {
        EditTool::new(FileTracker::new_shared(), PathPolicy::new_shared())
    }

    /// Write a test file and return the tool with the path already read.
    async fn setup_file(dir: &tempfile::TempDir, name: &str, content: &str) -> (EditTool, String) {
        let file_path = dir.path().join(name);
        tokio::fs::write(&file_path, content).await.unwrap();
        let path_str = file_path.to_str().unwrap().to_string();
        let tool = make_tool_with_file(&path_str).await;
        (tool, path_str)
    }

    fn edit(old: &str, new: &str) -> Value {
        serde_json::json!({ "old_string": old, "new_string": new })
    }

    async fn run(tool: &EditTool, path: &str, edits: Vec<Value>) -> ToolResult {
        tool.execute(serde_json::json!({ "path": path, "edits": edits }))
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn single_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let (tool, path) = setup_file(&dir, "a.txt", "aaa\nbbb\nccc\n").await;

        let result = run(&tool, &path, vec![edit("bbb", "BBB")]).await;

        assert!(!result.is_error, "edit should succeed: {}", result.output);
        let updated = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(updated, "aaa\nBBB\nccc\n", "only bbb should change");
        assert!(
            result
                .output
                .starts_with(&format!("edited {path} (1 replacement)")),
            "summary should count one replacement: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn edits_apply_in_order_and_can_build_on_each_other() {
        let dir = tempfile::tempdir().unwrap();
        let (tool, path) = setup_file(&dir, "chain.txt", "fn old() {}\nold();\n").await;

        let result = run(
            &tool,
            &path,
            vec![
                edit("fn old() {}", "fn renamed() {}"),
                edit("fn renamed() {}", "fn renamed() { work() }"),
                edit("old();", "renamed();"),
            ],
        )
        .await;

        assert!(!result.is_error, "batch should succeed: {}", result.output);
        let updated = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(
            updated, "fn renamed() { work() }\nrenamed();\n",
            "each edit should see the result of the previous one"
        );
    }

    #[tokio::test]
    async fn failed_edit_leaves_file_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let (tool, path) = setup_file(&dir, "atomic.txt", "one\ntwo\n").await;

        let result = run(
            &tool,
            &path,
            vec![edit("one", "ONE"), edit("missing", "x"), edit("two", "TWO")],
        )
        .await;

        assert!(result.is_error, "a failing edit should fail the batch");
        assert!(
            result.output.starts_with("edit 2 of 3:"),
            "error should name the failing edit: {}",
            result.output
        );
        assert!(
            result.output.contains("No changes were written"),
            "error should say nothing was written: {}",
            result.output
        );
        let unchanged = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(unchanged, "one\ntwo\n", "file must not change");
    }

    #[tokio::test]
    async fn ambiguous_match_lists_lines() {
        let dir = tempfile::tempdir().unwrap();
        let (tool, path) = setup_file(&dir, "dup.txt", "x = 1\ny = 2\nx = 1\n").await;

        let result = run(&tool, &path, vec![edit("x = 1", "x = 3")]).await;

        assert!(result.is_error, "ambiguous old_string should fail");
        assert!(
            result.output.contains("matches 2 places (lines 1, 3)"),
            "error should list matching lines: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn replace_all_changes_every_occurrence() {
        let dir = tempfile::tempdir().unwrap();
        let (tool, path) = setup_file(&dir, "all.txt", "foo bar\nfoo\nbaz foo\n").await;

        let result = run(
            &tool,
            &path,
            vec![serde_json::json!({
                "old_string": "foo", "new_string": "qux", "replace_all": true
            })],
        )
        .await;

        assert!(
            !result.is_error,
            "replace_all should succeed: {}",
            result.output
        );
        let updated = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(
            updated, "qux bar\nqux\nbaz qux\n",
            "every foo should change"
        );
        assert!(
            result.output.contains("(3 replacements)"),
            "summary should count three replacements: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn not_found_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let (tool, path) = setup_file(&dir, "nf.txt", "hello\n").await;

        let result = run(&tool, &path, vec![edit("goodbye", "x")]).await;

        assert!(result.is_error, "missing old_string should fail");
        assert!(
            result.output.contains("was not found"),
            "error should say not found: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn read_prefix_in_old_string_gets_a_hint() {
        let dir = tempfile::tempdir().unwrap();
        let (tool, path) = setup_file(&dir, "prefix.txt", "hello\nworld\n").await;

        let result = run(&tool, &path, vec![edit("   2\tworld", "earth")]).await;

        assert!(result.is_error, "prefixed old_string should fail");
        assert!(
            result.output.contains("line-number prefix"),
            "error should point at the read_file prefix: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn whitespace_difference_matches_when_unique() {
        let dir = tempfile::tempdir().unwrap();
        let (tool, path) = setup_file(
            &dir,
            "ws.rs",
            "fn main() {\n\tlet x = 1;  \n\tprintln!(\"{x}\");\n}\n",
        )
        .await;

        let result = run(
            &tool,
            &path,
            vec![edit(
                "    let x = 1;\n    println!(\"{x}\");\n",
                "\tlet x = 2;\n\tprintln!(\"{x}\");\n",
            )],
        )
        .await;

        assert!(
            !result.is_error,
            "loose match should apply: {}",
            result.output
        );
        let updated = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(
            updated, "fn main() {\n\tlet x = 2;\n\tprintln!(\"{x}\");\n}\n",
            "matched lines should be replaced with new_string verbatim"
        );
        assert!(
            result
                .output
                .contains("edit 1 matched only after ignoring whitespace"),
            "output should flag the whitespace match: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn whitespace_match_must_be_unique() {
        let dir = tempfile::tempdir().unwrap();
        let (tool, path) = setup_file(&dir, "wsdup.txt", "  a\n  b\n\ta\n\tb\n").await;

        let result = run(&tool, &path, vec![edit("a\nb", "c")]).await;

        assert!(result.is_error, "ambiguous loose match should fail");
        assert!(
            result
                .output
                .contains("ignoring whitespace it matches 2 places (lines 1, 3)"),
            "error should list loose matches: {}",
            result.output
        );
        let unchanged = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(unchanged, "  a\n  b\n\ta\n\tb\n", "file must not change");
    }

    #[tokio::test]
    async fn empty_new_string_deletes() {
        let dir = tempfile::tempdir().unwrap();
        let (tool, path) = setup_file(&dir, "del.txt", "keep\nremove\nkeep2\n").await;

        let result = run(&tool, &path, vec![edit("remove\n", "")]).await;

        assert!(!result.is_error, "delete should succeed: {}", result.output);
        let updated = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(updated, "keep\nkeep2\n", "the line should be removed");
    }

    #[tokio::test]
    async fn crlf_file_keeps_crlf_line_endings() {
        let dir = tempfile::tempdir().unwrap();
        let (tool, path) = setup_file(&dir, "win.txt", "one\r\ntwo\r\nthree\r\n").await;

        let result = run(&tool, &path, vec![edit("one\ntwo", "uno\ndos\nextra")]).await;

        assert!(
            !result.is_error,
            "LF old_string should match CRLF file: {}",
            result.output
        );
        let updated = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(
            updated, "uno\r\ndos\r\nextra\r\nthree\r\n",
            "new lines should use the file's CRLF endings"
        );
    }

    #[tokio::test]
    async fn stray_crlf_does_not_convert_lf_file() {
        let dir = tempfile::tempdir().unwrap();
        let (tool, path) = setup_file(&dir, "mixed.txt", "a\r\nb\nc\nd\n").await;

        let result = run(&tool, &path, vec![edit("c\n", "c\nnew1\nnew2\n")]).await;

        assert!(!result.is_error, "edit should succeed: {}", result.output);
        let updated = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(
            updated, "a\r\nb\nc\nnew1\nnew2\nd\n",
            "new lines should follow the file's majority LF style"
        );
    }

    #[tokio::test]
    async fn missing_trailing_newline_is_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let (tool, path) = setup_file(&dir, "nonl.txt", "a\nb").await;

        let result = run(&tool, &path, vec![edit("a", "A")]).await;

        assert!(!result.is_error, "edit should succeed: {}", result.output);
        let updated = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(updated, "A\nb", "no trailing newline should be added");
    }

    #[tokio::test]
    async fn preview_line_numbers_account_for_later_edits_above() {
        let dir = tempfile::tempdir().unwrap();
        let body = (1..=12)
            .map(|n| format!("line{n}\n"))
            .collect::<Vec<_>>()
            .concat();
        let (tool, path) = setup_file(&dir, "preview.txt", &body).await;

        // The second edit inserts two lines above the first edit's change.
        let result = run(
            &tool,
            &path,
            vec![
                edit("line10\n", "line10\nADDED\n"),
                edit("line2\n", "line2\nX\nY\n"),
            ],
        )
        .await;

        assert!(!result.is_error, "edits should succeed: {}", result.output);
        assert!(
            result.output.contains("  13\tADDED"),
            "the first edit's line should be renumbered after the second: {}",
            result.output
        );
        assert!(
            result.output.contains("   3\tX\n   4\tY"),
            "the second edit's lines should appear numbered: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn file_not_found() {
        let tool = make_tool_with_file("/nonexistent/edit_target.txt").await;

        let result = run(&tool, "/nonexistent/edit_target.txt", vec![edit("a", "b")]).await;

        assert!(result.is_error, "nonexistent file should fail");
        assert!(
            result.output.contains("does not exist"),
            "error should mention file not found: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn not_read_first() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("unread.txt");
        tokio::fs::write(&file_path, "content\n").await.unwrap();

        let result = run(
            &make_tool_no_reads(),
            file_path.to_str().unwrap(),
            vec![edit("content", "x")],
        )
        .await;

        assert!(result.is_error, "unread file should fail");
        assert!(
            result.output.contains("has not been read"),
            "error should mention read requirement: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn invalid_arguments_are_rejected() {
        let tool = make_tool_no_reads();
        let cases = [
            (serde_json::json!({ "edits": [edit("a", "b")] }), "'path'"),
            (serde_json::json!({ "path": "/tmp/x" }), "'edits'"),
            (
                serde_json::json!({ "path": "/tmp/x", "edits": [] }),
                "empty",
            ),
            (
                serde_json::json!({ "path": "/tmp/x", "edits": [{ "old_string": "a" }] }),
                "'new_string'",
            ),
            (
                serde_json::json!({ "path": "/tmp/x", "edits": [edit("", "b")] }),
                "empty old_string",
            ),
            (
                serde_json::json!({ "path": "/tmp/x", "edits": [edit("same", "same")] }),
                "identical",
            ),
        ];

        for (arguments, expected) in cases {
            let err = tool.execute(arguments.clone()).await.unwrap_err();
            assert!(
                err.to_string().contains(expected),
                "{arguments} should fail mentioning {expected}: {err}"
            );
        }
    }

    #[test]
    fn tool_definition_check() {
        let tool = make_tool_no_reads();
        assert_eq!(tool.name(), "edit_file", "tool name should match");
        let def = tool.definition();
        assert_eq!(def.name, "edit_file", "definition name should match");
        assert_eq!(
            def.parameters.get("required"),
            Some(&serde_json::json!(["path", "edits"])),
            "path and edits should be required"
        );
    }
}
