//! File reading tool for the agent.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use base64::Engine;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};

use super::file_tracker::SharedFileTracker;
use crate::path_policy::SharedPathPolicy;
use residuum_agent::Tool;
use residuum_inference::{ImageData, ToolDefinition, sniff_image_media_type};
use residuum_inference::{ToolError, ToolResult};
use residuum_workspace::inbox::attachment::MAX_IMAGE_INLINE_SIZE;
use residuum_workspace::team_files::{FileStamp, TeamFiles};

/// Default maximum lines returned when no explicit offset/limit is given.
const DEFAULT_MAX_LINES: usize = 2000;

/// Maximum characters per output line before truncation.
const MAX_CHARS_PER_LINE: usize = 2000;

/// How a completed read is recorded in the tracker.
enum ReadRecord {
    /// An agent-private file: only that it was read.
    Private,
    /// A team file, with its stamp from just before the read (`None` when it
    /// couldn't be taken).
    Team(Option<FileStamp>),
}

/// Tool that reads file contents with numbered lines.
pub struct ReadTool {
    tracker: SharedFileTracker,
    /// The agent's workspace root. A relative `path` argument is resolved
    /// against this rather than the process's current directory, which is
    /// shared across every agent hosted in the same process.
    workspace_root: PathBuf,
    /// Supplies the `team/` namespace, when the agent belongs to a team.
    policy: Option<SharedPathPolicy>,
}

impl ReadTool {
    /// Create a new `ReadTool` with a shared file tracker and the workspace
    /// root a relative `path` argument resolves against.
    #[must_use]
    pub fn new(tracker: SharedFileTracker, workspace_root: PathBuf) -> Self {
        Self {
            tracker,
            workspace_root,
            policy: None,
        }
    }

    /// Resolve `team/...` paths through the team namespace held by `policy`,
    /// and record the version of each team file read so a later write can
    /// detect a change made since.
    #[must_use]
    pub fn with_policy(mut self, policy: SharedPathPolicy) -> Self {
        self.policy = Some(policy);
        self
    }

    /// The agent's view of the team namespace, if it has one.
    async fn team(&self) -> Option<TeamFiles> {
        match &self.policy {
            Some(policy) => policy.read().await.team().cloned(),
            None => None,
        }
    }

    /// Resolve a `path` argument, and for a team file take its stamp. The
    /// stamp is taken before the file is read, so a write that lands
    /// mid-read shows up as a conflict on the next write instead of going
    /// unnoticed.
    async fn locate(&self, path: &str) -> (PathBuf, ReadRecord) {
        let Some(team) = self.team().await else {
            return (self.workspace_root.join(path), ReadRecord::Private);
        };
        let resolved = team.resolve(path);
        if !team.is_team_path(&resolved) {
            return (resolved, ReadRecord::Private);
        }
        let stamp = team.coordinator().stamp(&resolved).await.ok();
        (resolved, ReadRecord::Team(stamp))
    }

    /// Record a completed read.
    async fn record_read(&self, resolved: &Path, record: ReadRecord) {
        let key = resolved.to_string_lossy();
        let mut tracker = self.tracker.lock().await;
        match record {
            ReadRecord::Team(stamp) => tracker.record_team_read(&key, stamp),
            ReadRecord::Private => tracker.record_read(&key),
        }
    }

    /// Record that a team file was found missing: the stamp taken before the
    /// read (version `None`) becomes the file's known-absent state. Ignored
    /// when the stamp shows the file existed a moment ago, or for private
    /// files.
    async fn record_absent(&self, resolved: &Path, record: ReadRecord) {
        if let ReadRecord::Team(Some(stamp)) = &record
            && stamp.version.is_none()
        {
            self.record_read(resolved, record).await;
        }
    }

    /// Read an image file, base64-encode it, and return as a tool result with inline image data.
    #[expect(clippy::cast_precision_loss, reason = "file size in KB display only")]
    async fn read_image(
        &self,
        display_path: &str,
        resolved_path: &Path,
        size: u64,
        mime: &str,
        record: ReadRecord,
    ) -> Result<ToolResult, ToolError> {
        let bytes = match tokio::fs::read(resolved_path).await {
            Ok(b) => b,
            Err(e) => {
                return Ok(ToolResult::error(format!(
                    "failed to read {display_path}: {e}"
                )));
            }
        };

        let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let filename = resolved_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();

        let size_kb = size as f64 / 1024.0;
        let summary = format!("[Image: {filename}, {size_kb:.1} KB]");

        self.record_read(resolved_path, record).await;

        // The extension is only a fallback: providers reject an image whose
        // label disagrees with its bytes.
        let media_type = sniff_image_media_type(&bytes).unwrap_or(mime);

        Ok(ToolResult::success_with_images(
            summary,
            vec![ImageData {
                media_type: media_type.to_string(),
                data: encoded,
            }],
        ))
    }
}

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: format!(
                "Read the contents of a file. Each output line is prefixed with its line \
                 number and a tab (e.g. `   1\\thello`); the prefix is not part of the file, \
                 so leave it out of edit_file's old_string. Unless a limit is given, returns \
                 at most {DEFAULT_MAX_LINES} lines or {} characters per call, whichever comes \
                 first, and the header names the offset to continue from. There is no file \
                 size limit, and the output header reports the file's total size and line \
                 count. Lines longer than {MAX_CHARS_PER_LINE} characters are cut. Image files \
                 (JPEG, PNG, GIF, WebP) are returned as inline images for visual inspection \
                 instead of raw bytes, capped by the model API's inline image size limit.",
                crate::PAGE_BUDGET
            ),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute or relative path to the file to read. A path starting with \"team/\" is in the shared team folder"
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Line number to start reading from (0-based, default: 0)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of lines to read (default: 2000, and a page without a limit also stops at a character budget)"
                    }
                },
                "required": ["path"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let path = arguments
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ToolError::InvalidArguments("missing required 'path' argument".to_string())
            })?;

        let offset = arguments.get("offset").and_then(Value::as_u64).unwrap_or(0);
        let explicit_limit = arguments.get("limit").and_then(Value::as_u64);

        // `path` as given by the model (absolute or workspace-relative) is
        // kept for display; every filesystem operation and the tracker key
        // use `resolved`, which resolves against the workspace root instead
        // of the process's current directory — see `workspace_root`'s doc
        // comment.
        let (resolved, record) = self.locate(path).await;

        let metadata = match tokio::fs::metadata(&resolved).await {
            Ok(m) => m,
            Err(e) => {
                // A read that finds a team file gone still tells the tracker
                // so: the agent has now seen the deletion, and its next write
                // recreating the file must not conflict with the version it
                // read before the file was removed.
                if e.kind() == std::io::ErrorKind::NotFound {
                    self.record_absent(&resolved, record).await;
                }
                return Ok(ToolResult::error(format!("failed to read {path}: {e}")));
            }
        };
        let total_size = metadata.len();

        // Check if this is a supported image file — return inline image data.
        // The size cap here is a model API fact (the maximum image size the
        // provider accepts inline), not a residuum-imposed limit.
        if let Some(mime) = image_mime_type(&resolved) {
            if total_size > u64::from(MAX_IMAGE_INLINE_SIZE) {
                return Ok(ToolResult::error(format!(
                    "image {path} is too large to send inline ({total_size} bytes, max \
                     {MAX_IMAGE_INLINE_SIZE} bytes — this is a model API limit)"
                )));
            }
            return self
                .read_image(path, &resolved, total_size, mime, record)
                .await;
        }

        // Text files have no size limit: page through by offset/limit rather
        // than loading the whole file into memory. `start` and `effective_limit`
        // are resolved against line numbers as we stream, not against a
        // pre-counted total, so a file of any size can be paged with bounded
        // memory use.
        // `try_from` rather than `as`: a fallible conversion clamped to
        // `usize::MAX` on overflow, so no truncating cast is needed here.
        let start = usize::try_from(offset).unwrap_or(usize::MAX);

        // Every call is capped, including one that pages with an offset, so a
        // read never returns the rest of a large file in one result.
        let limit = explicit_limit.map_or(DEFAULT_MAX_LINES, |l| {
            usize::try_from(l).unwrap_or(usize::MAX)
        });
        // A default page also stops at the page budget, so it fits under the
        // history limit and is never cut again by the turn loop. An explicit
        // limit is the caller's choice and is honoured in full.
        let char_budget = if explicit_limit.is_some() {
            usize::MAX
        } else {
            crate::PAGE_BUDGET
        };

        let file = match tokio::fs::File::open(&resolved).await {
            Ok(f) => f,
            Err(e) => return Ok(ToolResult::error(format!("failed to read {path}: {e}"))),
        };
        let page = match TextPage::read(file, start, limit, char_budget).await {
            Ok(page) => page,
            Err(e) => return Ok(ToolResult::error(format!("failed to read {path}: {e}"))),
        };

        // Record read in tracker, keyed by the resolved path so it matches
        // what write_file/edit_file check against regardless of how this
        // call's path argument was spelled.
        self.record_read(&resolved, record).await;

        Ok(ToolResult::success(page.render(total_size, offset)))
    }
}

/// One page of a text file, as `read_file` shows it.
struct TextPage {
    /// The shown lines, numbered.
    lines: Vec<String>,
    /// How many shown lines were cut at [`MAX_CHARS_PER_LINE`].
    truncated: usize,
    /// Lines in the whole file.
    total_lines: usize,
    /// The first and last line numbers shown (1-based), when any are.
    shown: Option<(usize, usize)>,
}

impl TextPage {
    /// Stream `file`, keeping the lines after the first `start`, up to
    /// `limit` lines and `char_budget` characters of numbered lines (at least
    /// one line). Lines past the page are still counted, so `total_lines` is
    /// the file's.
    async fn read(
        file: tokio::fs::File,
        start: usize,
        limit: usize,
        char_budget: usize,
    ) -> std::io::Result<Self> {
        let mut lines_stream = BufReader::new(file).lines();
        let mut page = Self {
            lines: Vec::new(),
            truncated: 0,
            total_lines: 0,
            shown: None,
        };
        let mut body_chars: usize = 0;
        let mut page_full = false;

        while let Some(line) = lines_stream.next_line().await? {
            page.total_lines += 1;
            let line_no = page.total_lines;
            if line_no <= start || page_full || page.lines.len() >= limit {
                continue;
            }

            let (formatted, was_truncated) = format_numbered_line(line_no, &line);
            // Each shown line costs its characters plus the newline joining it.
            let line_chars = formatted.chars().count() + 1;
            if !page.lines.is_empty() && body_chars + line_chars > char_budget {
                page_full = true;
                continue;
            }
            body_chars += line_chars;
            page.truncated += usize::from(was_truncated);
            page.lines.push(formatted);
            let first = page.shown.map_or(line_no, |(first, _)| first);
            page.shown = Some((first, line_no));
        }
        Ok(page)
    }

    /// The result text: a header, then the numbered lines. The header always
    /// reports the file's size and line count, so a paged or truncated view
    /// never hides how much more there is to see; a partial page also names
    /// its range and the offset to continue from.
    fn render(&self, total_size: u64, offset: u64) -> String {
        let total_lines = self.total_lines;
        let mut header: Vec<String> = vec![format!(
            "file: {total_size} bytes, {total_lines} line(s) total"
        )];
        match self.shown {
            Some((first, last)) if first > 1 || last < total_lines => {
                let mut notice = format!("showing lines {first}-{last} of {total_lines}");
                if last < total_lines {
                    _ = write!(
                        notice,
                        "; continue with offset={last}, or use exec with grep to find \
                         specific content"
                    );
                }
                header.push(notice);
            }
            None if total_lines > 0 => {
                header.push(format!(
                    "offset {offset} is at or beyond the file's {total_lines} line(s); nothing to show"
                ));
            }
            Some(_) | None => {}
        }
        if self.truncated > 0 {
            header.push(format!(
                "warning: {} line(s) exceeded {MAX_CHARS_PER_LINE} characters and were truncated",
                self.truncated
            ));
        }

        let header = header.join("\n");
        if self.lines.is_empty() {
            header
        } else {
            format!("{header}\n\n{}", self.lines.join("\n"))
        }
    }
}

/// Format one line the way `read_file` shows it: right-aligned line number, a tab, the text.
///
/// Lines longer than `MAX_CHARS_PER_LINE` are cut at a UTF-8 boundary; the returned flag
/// reports whether that happened.
pub(super) fn format_numbered_line(line_num: usize, line: &str) -> (String, bool) {
    if line.len() > MAX_CHARS_PER_LINE {
        let boundary = line.floor_char_boundary(MAX_CHARS_PER_LINE);
        let truncated = line.get(..boundary).unwrap_or_default();
        (format!("{line_num:>4}\t{truncated} ... (truncated)"), true)
    } else {
        (format!("{line_num:>4}\t{line}"), false)
    }
}

/// Return the MIME type for a supported image extension, or `None`.
fn image_mime_type(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_tracker::FileTracker;

    /// A workspace root that doesn't correspond to any real directory, for
    /// tests that always pass an absolute `path` (join drops the base for
    /// an absolute joined component, so its value never matters there).
    fn unused_workspace_root() -> std::path::PathBuf {
        std::path::PathBuf::from("/tmp/residuum-test-workspace-unused")
    }

    fn make_tool() -> ReadTool {
        ReadTool::new(FileTracker::new_shared(), unused_workspace_root())
    }

    #[tokio::test]
    async fn read_file_success() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        tokio::fs::write(&file_path, "line 1\nline 2\nline 3\n")
            .await
            .unwrap();

        let tool = make_tool();
        let result = tool
            .execute(serde_json::json!({ "path": file_path.to_str().unwrap() }))
            .await
            .unwrap();

        assert!(!result.is_error, "read should succeed");
        assert!(
            result.output.contains("line 1"),
            "output should contain file content"
        );
        assert!(
            result.output.contains("line 3"),
            "output should contain all lines"
        );
    }

    #[tokio::test]
    async fn read_file_with_offset_and_limit() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        tokio::fs::write(&file_path, "line 1\nline 2\nline 3\nline 4\nline 5\n")
            .await
            .unwrap();

        let tool = make_tool();
        let result = tool
            .execute(serde_json::json!({
                "path": file_path.to_str().unwrap(),
                "offset": 1,
                "limit": 2
            }))
            .await
            .unwrap();

        assert!(!result.is_error, "read should succeed");
        assert!(result.output.contains("line 2"), "should start from offset");
        assert!(
            result.output.contains("line 3"),
            "should include limited lines"
        );
        assert!(
            !result.output.contains("line 1"),
            "should not include lines before offset"
        );
        assert!(
            !result.output.contains("line 4"),
            "should not include lines beyond limit"
        );
    }

    #[tokio::test]
    async fn read_file_not_found() {
        let tool = make_tool();
        let result = tool
            .execute(serde_json::json!({ "path": "/nonexistent/file.txt" }))
            .await
            .unwrap();

        assert!(result.is_error, "missing file should be an error result");
    }

    #[tokio::test]
    async fn read_file_missing_path() {
        let tool = make_tool();
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err(), "missing path should return ToolError");
    }

    #[test]
    fn read_tool_definition() {
        let tool = make_tool();
        assert_eq!(tool.name(), "read_file", "tool name should match");
        let def = tool.definition();
        assert_eq!(def.name, "read_file", "definition name should match");
    }

    #[tokio::test]
    async fn output_prefixes_line_numbers() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("numbered.txt");
        tokio::fs::write(&file_path, "hello\nworld\n")
            .await
            .unwrap();

        let tool = make_tool();
        let result = tool
            .execute(serde_json::json!({ "path": file_path.to_str().unwrap() }))
            .await
            .unwrap();

        assert!(
            result.output.ends_with("\n\n   1\thello\n   2\tworld"),
            "each line should be its number, a tab, then the text: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn default_line_limit_caps_at_2000() {
        use std::fmt::Write;
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("big.txt");
        let mut big_content = String::new();
        for i in 1..=3000 {
            _ = writeln!(big_content, "{i}");
        }
        tokio::fs::write(&file_path, &big_content).await.unwrap();

        let tool = make_tool();
        let result = tool
            .execute(serde_json::json!({ "path": file_path.to_str().unwrap() }))
            .await
            .unwrap();

        assert!(!result.is_error, "read should succeed");
        assert!(
            result.output.contains("3000 line(s) total"),
            "should report total line count"
        );
        assert!(
            result.output.contains("showing lines 1-2000 of 3000"),
            "should show the default-limited range"
        );
        // Count actual content lines (skip warning lines)
        let content_lines: Vec<&str> = result.output.lines().filter(|l| l.contains('\t')).collect();
        assert_eq!(
            content_lines.len(),
            2000,
            "should return exactly 2000 content lines"
        );
    }

    #[tokio::test]
    async fn explicit_limit_can_exceed_default() {
        use std::fmt::Write;
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("big2.txt");
        let mut big_content = String::new();
        for i in 1..=2500 {
            _ = writeln!(big_content, "line {i}");
        }
        tokio::fs::write(&file_path, &big_content).await.unwrap();

        let tool = make_tool();
        let result = tool
            .execute(serde_json::json!({
                "path": file_path.to_str().unwrap(),
                "limit": 2500
            }))
            .await
            .unwrap();

        assert!(!result.is_error, "read should succeed");
        assert!(
            !result.output.contains("warning:"),
            "no warning when explicit limit is used"
        );
        assert!(
            !result.output.contains("showing lines"),
            "no partial-range notice when the explicit limit covers the whole file"
        );
        let content_lines: Vec<&str> = result.output.lines().filter(|l| l.contains('\t')).collect();
        assert_eq!(
            content_lines.len(),
            2500,
            "should return all 2500 lines with explicit limit"
        );
    }

    #[tokio::test]
    async fn offset_without_limit_is_capped_at_default() {
        use std::fmt::Write;
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("paged.txt");
        let mut content = String::new();
        for i in 1..=3000 {
            _ = writeln!(content, "{i}");
        }
        tokio::fs::write(&file_path, &content).await.unwrap();

        let tool = make_tool();
        let result = tool
            .execute(serde_json::json!({
                "path": file_path.to_str().unwrap(),
                "offset": 500
            }))
            .await
            .unwrap();

        assert!(!result.is_error, "read should succeed");
        assert!(
            result.output.contains("showing lines 501-2500 of 3000"),
            "an offset with no limit should return one default page: {}",
            result.output
        );
        let content_lines: Vec<&str> = result.output.lines().filter(|l| l.contains('\t')).collect();
        assert_eq!(
            content_lines.len(),
            2000,
            "an offset with no limit should return at most the default line count"
        );
    }

    #[tokio::test]
    async fn paging_hint_only_when_lines_remain() {
        use std::fmt::Write;
        let dir = tempfile::tempdir().unwrap();

        let short_path = dir.path().join("short.txt");
        tokio::fs::write(&short_path, "one\ntwo\nthree\n")
            .await
            .unwrap();
        let tool = make_tool();
        let whole = tool
            .execute(serde_json::json!({ "path": short_path.to_str().unwrap() }))
            .await
            .unwrap();
        assert!(
            !whole.output.contains("exec with grep"),
            "a read that covers the whole file should not suggest paging: {}",
            whole.output
        );

        let tail = tool
            .execute(serde_json::json!({
                "path": short_path.to_str().unwrap(),
                "offset": 1
            }))
            .await
            .unwrap();
        assert!(
            tail.output.contains("showing lines 2-3 of 3"),
            "a tail read should still report its range: {}",
            tail.output
        );
        assert!(
            !tail.output.contains("exec with grep"),
            "a tail read that reaches the end of the file should not suggest paging: {}",
            tail.output
        );

        let long_path = dir.path().join("long.txt");
        let mut long_content = String::new();
        for i in 1..=2500 {
            _ = writeln!(long_content, "line {i}");
        }
        tokio::fs::write(&long_path, &long_content).await.unwrap();
        let capped = tool
            .execute(serde_json::json!({ "path": long_path.to_str().unwrap() }))
            .await
            .unwrap();
        assert!(
            capped.output.contains("exec with grep"),
            "a read cut short by the cap should suggest paging: {}",
            capped.output
        );
    }

    /// A file of `count` lines of 80 characters each, as written and as text.
    async fn wide_lines_file(dir: &Path, count: usize) -> PathBuf {
        use std::fmt::Write as _;
        let path = dir.join("wide-lines.txt");
        let content = (1..=count).fold(String::new(), |mut text, n| {
            _ = writeln!(text, "{n:>6} {}", "x".repeat(73));
            text
        });
        tokio::fs::write(&path, content).await.unwrap();
        path
    }

    /// The `offset=N` a paged read says to continue from.
    fn continue_offset(output: &str) -> usize {
        let (_, rest) = output.split_once("continue with offset=").unwrap();
        rest.split(|c: char| !c.is_ascii_digit())
            .next()
            .unwrap()
            .parse()
            .unwrap()
    }

    #[tokio::test]
    async fn a_default_page_fits_under_the_history_limit_and_names_the_next_offset() {
        let dir = tempfile::tempdir().unwrap();
        let path = wide_lines_file(dir.path(), 1000).await;
        let tool = make_tool();

        let first = tool
            .execute(serde_json::json!({ "path": path.to_str().unwrap() }))
            .await
            .unwrap();

        assert!(
            first.output.chars().count() <= residuum_agent::HISTORY_RESULT_MAX_CHARS,
            "a default page is never cut again by the turn loop: {} characters",
            first.output.chars().count()
        );
        let next = continue_offset(&first.output);
        assert!(
            first.output.contains(&format!(
                "showing lines 1-{next} of 1000; continue with offset={next}"
            )),
            "{}",
            first.output.lines().take(3).collect::<Vec<_>>().join("\n")
        );
        let last_line = first.output.lines().last().unwrap();
        assert!(
            last_line.starts_with(&format!("{next:>4}\t")),
            "the page ends at the line the header names: {last_line}"
        );

        let second = tool
            .execute(serde_json::json!({ "path": path.to_str().unwrap(), "offset": next }))
            .await
            .unwrap();
        assert!(
            second
                .output
                .contains(&format!("showing lines {}-", next + 1)),
            "the next page starts right after the first: {}",
            second.output.lines().take(3).collect::<Vec<_>>().join("\n")
        );
    }

    #[tokio::test]
    async fn an_explicit_limit_is_not_cut_to_the_page_budget() {
        let dir = tempfile::tempdir().unwrap();
        let path = wide_lines_file(dir.path(), 1000).await;
        let tool = make_tool();

        let result = tool
            .execute(serde_json::json!({ "path": path.to_str().unwrap(), "limit": 1000 }))
            .await
            .unwrap();

        let content_lines = result.output.lines().filter(|l| l.contains('\t')).count();
        assert_eq!(content_lines, 1000, "every requested line is returned");
        assert!(!result.output.contains("continue with offset"));
    }

    #[tokio::test]
    async fn line_char_truncation_and_warning() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("wide.txt");
        let long_line = "x".repeat(3000);
        let short_line = "short";
        let file_content = format!("{long_line}\n{short_line}\n");
        tokio::fs::write(&file_path, &file_content).await.unwrap();

        let tool = make_tool();
        let result = tool
            .execute(serde_json::json!({ "path": file_path.to_str().unwrap() }))
            .await
            .unwrap();

        assert!(!result.is_error, "read should succeed");
        assert!(
            result.output.contains("1 line(s) exceeded 2000 characters"),
            "should warn about truncated lines"
        );
        assert!(
            result.output.contains("(truncated)"),
            "truncated lines should be marked"
        );
    }

    #[tokio::test]
    async fn tracker_records_path_after_read() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("track.txt");
        tokio::fs::write(&file_path, "content").await.unwrap();

        let tracker = FileTracker::new_shared();
        let tool = ReadTool::new(std::sync::Arc::clone(&tracker), unused_workspace_root());

        tool.execute(serde_json::json!({ "path": file_path.to_str().unwrap() }))
            .await
            .unwrap();

        assert!(
            tracker
                .lock()
                .await
                .has_been_read(file_path.to_str().unwrap()),
            "tracker should record the read path"
        );
    }

    #[tokio::test]
    async fn read_image_file_returns_inline_image() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("photo.jpg");
        // Write fake JPEG bytes (real image not needed — just testing encoding path)
        tokio::fs::write(&file_path, b"\xFF\xD8\xFF\xE0fake jpeg data")
            .await
            .unwrap();

        let tool = make_tool();
        let result = tool
            .execute(serde_json::json!({ "path": file_path.to_str().unwrap() }))
            .await
            .unwrap();

        assert!(!result.is_error, "image read should succeed");
        assert!(
            result.output.contains("[Image:"),
            "output should contain image summary: {}",
            result.output,
        );
        assert_eq!(result.images.len(), 1, "should have one inline image");
        assert_eq!(
            result.images.first().unwrap().media_type,
            "image/jpeg",
            "media type should be image/jpeg"
        );
        assert!(
            !result.images.first().unwrap().data.is_empty(),
            "base64 data should be non-empty"
        );
    }

    #[tokio::test]
    async fn read_text_file_has_no_images() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("readme.md");
        tokio::fs::write(&file_path, "# Hello\nworld\n")
            .await
            .unwrap();

        let tool = make_tool();
        let result = tool
            .execute(serde_json::json!({ "path": file_path.to_str().unwrap() }))
            .await
            .unwrap();

        assert!(!result.is_error, "text read should succeed");
        assert!(
            result.images.is_empty(),
            "text files should not return images"
        );
    }

    async fn read_image_block(file_name: &str, bytes: &[u8]) -> ImageData {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join(file_name);
        tokio::fs::write(&file_path, bytes).await.unwrap();

        let result = make_tool()
            .execute(serde_json::json!({ "path": file_path.to_str().unwrap() }))
            .await
            .unwrap();

        assert!(!result.is_error, "image read should succeed");
        result.images.into_iter().next().unwrap()
    }

    async fn read_image_media_type(file_name: &str, bytes: &[u8]) -> String {
        read_image_block(file_name, bytes).await.media_type
    }

    #[tokio::test]
    async fn read_image_labels_by_bytes_for_each_format() {
        let png = b"\x89PNG\r\n\x1A\nfake png";
        let jpeg = b"\xFF\xD8\xFF\xE0fake jpeg data";
        let gif = b"GIF89a fake gif";
        let webp = b"RIFF\x24\x00\x00\x00WEBPVP8 fake webp";

        for (name, bytes, expected) in [
            ("a.png", png.as_slice(), "image/png"),
            ("a.jpg", jpeg.as_slice(), "image/jpeg"),
            ("a.gif", gif.as_slice(), "image/gif"),
            ("a.webp", webp.as_slice(), "image/webp"),
        ] {
            let image = read_image_block(name, bytes).await;
            assert_eq!(image.media_type, expected, "{name}");

            // The same bytes in stored history under a wrong label are
            // recognized from the base64 text alone.
            let mislabeled = ImageData {
                media_type: "image/bmp".to_string(),
                data: image.data,
            };
            assert_eq!(mislabeled.request_media_type(), expected, "{name} history");
        }
    }

    #[tokio::test]
    async fn read_jpeg_bytes_named_png_is_labeled_jpeg() {
        let media_type = read_image_media_type("render.png", b"\xFF\xD8\xFF\xE0fake jpeg").await;
        assert_eq!(media_type, "image/jpeg", "bytes win over the extension");
    }

    #[tokio::test]
    async fn read_png_bytes_named_jpg_is_labeled_png() {
        let media_type = read_image_media_type("shot.jpg", b"\x89PNG\r\n\x1A\nfake png").await;
        assert_eq!(media_type, "image/png", "bytes win over the extension");
    }

    #[tokio::test]
    async fn read_image_with_unrecognized_bytes_falls_back_to_extension() {
        let media_type = read_image_media_type("odd.webp", b"not any known image format").await;
        assert_eq!(media_type, "image/webp", "extension is the fallback");
    }

    #[tokio::test]
    async fn read_image_records_in_tracker() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.png");
        tokio::fs::write(&file_path, b"\x89PNG\r\n\x1A\nfake png")
            .await
            .unwrap();

        let tracker = FileTracker::new_shared();
        let tool = ReadTool::new(std::sync::Arc::clone(&tracker), unused_workspace_root());

        tool.execute(serde_json::json!({ "path": file_path.to_str().unwrap() }))
            .await
            .unwrap();

        assert!(
            tracker
                .lock()
                .await
                .has_been_read(file_path.to_str().unwrap()),
            "tracker should record image file read"
        );
    }

    #[tokio::test]
    async fn read_file_over_old_ten_mb_cap_is_not_refused() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("huge.bin");
        let file = tokio::fs::File::create(&file_path).await.unwrap();
        // Sparse file past the old 10 MB hard cap; content is all NUL bytes,
        // which are valid UTF-8, so this exercises size handling without
        // writing real data to disk.
        file.set_len(11 * 1024 * 1024).await.unwrap();
        drop(file);

        let tool = make_tool();
        let result = tool
            .execute(serde_json::json!({ "path": file_path.to_str().unwrap() }))
            .await
            .unwrap();

        assert!(
            !result.is_error,
            "a file over the old 10 MB cap should no longer be refused: {}",
            result.output
        );
        assert!(
            result.output.contains("11534336 bytes"),
            "should report the true total size: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn read_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("empty.txt");
        tokio::fs::write(&file_path, "").await.unwrap();

        let tool = make_tool();
        let result = tool
            .execute(serde_json::json!({ "path": file_path.to_str().unwrap() }))
            .await
            .unwrap();

        assert!(!result.is_error, "reading empty file should succeed");
        assert!(result.images.is_empty(), "empty file should have no images");
    }

    #[tokio::test]
    async fn relative_path_resolves_against_workspace_root_not_process_cwd() {
        // The process's current directory here is the crate root (however
        // `cargo test` was invoked), not `workspace_root` — so this only
        // passes if the relative path is joined against `workspace_root`.
        let workspace_root = tempfile::tempdir().unwrap();
        tokio::fs::write(workspace_root.path().join("relative.txt"), "from workspace")
            .await
            .unwrap();

        let tool = ReadTool::new(
            FileTracker::new_shared(),
            workspace_root.path().to_path_buf(),
        );
        let result = tool
            .execute(serde_json::json!({ "path": "relative.txt" }))
            .await
            .unwrap();

        assert!(
            !result.is_error,
            "a relative path should resolve against the workspace root: {}",
            result.output
        );
        assert!(
            result.output.contains("from workspace"),
            "should read the file from the workspace root: {}",
            result.output
        );
    }

    #[test]
    fn image_mime_type_detection() {
        assert_eq!(image_mime_type(Path::new("photo.jpg")), Some("image/jpeg"),);
        assert_eq!(image_mime_type(Path::new("photo.JPEG")), Some("image/jpeg"),);
        assert_eq!(image_mime_type(Path::new("icon.png")), Some("image/png"),);
        assert_eq!(image_mime_type(Path::new("anim.gif")), Some("image/gif"),);
        assert_eq!(
            image_mime_type(Path::new("modern.webp")),
            Some("image/webp"),
        );
        assert_eq!(image_mime_type(Path::new("document.txt")), None,);
        assert_eq!(image_mime_type(Path::new("noext")), None,);
    }
}
