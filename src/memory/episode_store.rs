//! Episode transcript file persistence.
//!
//! Writes episode transcripts as JSONL files to `memory/episodes/YYYY-MM/DD/<id>.jsonl`.
//! Line 1 is a JSON meta object; subsequent lines are serialized [`Message`] values.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use anyhow::Context;

use crate::memory::types::Episode;
use crate::models::{Message, Role};

/// Metadata from the first line of an episode JSONL file.
#[derive(Debug, Deserialize)]
pub struct EpisodeMeta {
    /// Episode identifier (e.g., `"ep-001"`).
    pub id: String,
    /// Date of the episode.
    pub date: chrono::NaiveDate,
    /// Project or topic context tag.
    pub context: String,
}

/// Write an episode transcript file to the episodes directory.
///
/// Creates `{episodes_dir}/{YYYY-MM}/{DD}/{episode.id}.jsonl` as a JSONL file.
/// Line 1 is a meta JSON object; subsequent lines are serialized [`Message`] values.
/// Creates the date subdirectory if it doesn't exist.
///
/// # Errors
/// Returns an error if the file cannot be written.
pub(crate) async fn write_episode_transcript(
    episodes_dir: &Path,
    episode: &Episode,
    messages: &[Message],
) -> anyhow::Result<()> {
    let day_dir = episodes_dir.join(episode.date.format("%Y-%m/%d").to_string());
    tokio::fs::create_dir_all(&day_dir).await.with_context(|| {
        format!(
            "failed to create episode directory at {}",
            day_dir.display()
        )
    })?;

    let path = episode_jsonl_path(episodes_dir, episode);

    let meta = serde_json::json!({
        "type": "meta",
        "id": episode.id,
        "date": episode.date.to_string(),
        "context": episode.context,
    });

    let mut lines = Vec::with_capacity(messages.len() + 1);
    lines.push(serde_json::to_string(&meta).context("failed to serialize episode meta")?);

    for msg in messages {
        lines.push(serde_json::to_string(msg).context("failed to serialize message")?);
    }

    let file_content = lines.join("\n") + "\n";

    tokio::fs::write(&path, &file_content)
        .await
        .with_context(|| format!("failed to write episode transcript at {}", path.display()))
}

/// Get the path where an episode JSONL transcript would be written.
#[must_use]
pub(crate) fn episode_jsonl_path(episodes_dir: &Path, episode: &Episode) -> PathBuf {
    episodes_dir
        .join(episode.date.format("%Y-%m/%d").to_string())
        .join(format!("{}.jsonl", episode.id))
}

/// Get the path for the per-episode observations archive file.
#[must_use]
pub(crate) fn episode_obs_path(episodes_dir: &Path, episode: &Episode) -> PathBuf {
    episodes_dir
        .join(episode.date.format("%Y-%m/%d").to_string())
        .join(format!("{}.obs.json", episode.id))
}

/// Get the path for the per-episode interaction-pair index file.
#[must_use]
pub(crate) fn episode_idx_path(episodes_dir: &Path, episode: &Episode) -> PathBuf {
    episodes_dir
        .join(episode.date.format("%Y-%m/%d").to_string())
        .join(format!("{}.idx.jsonl", episode.id))
}

/// Read and parse a JSONL episode transcript file.
///
/// Returns the episode metadata and the list of messages. The first line is
/// parsed as [`EpisodeMeta`]; subsequent non-empty lines are parsed as [`Message`] values.
///
/// # Errors
/// Returns an error if the file cannot be read or any line fails to parse.
pub async fn read_episode_jsonl(path: &Path) -> anyhow::Result<(EpisodeMeta, Vec<Message>)> {
    let file_content = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("failed to read episode file at {}", path.display()))?;

    let mut lines = file_content.lines();

    let meta_line = lines
        .next()
        .ok_or_else(|| anyhow::anyhow!("episode file is empty at {}", path.display()))?;

    let meta: EpisodeMeta = serde_json::from_str(meta_line)
        .with_context(|| format!("failed to parse episode meta at {}", path.display()))?;

    let mut messages = Vec::new();
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let msg: Message = serde_json::from_str(line)
            .with_context(|| format!("failed to parse message line in {}", path.display()))?;
        messages.push(msg);
    }

    Ok((meta, messages))
}

/// Default number of message lines returned when no limit is specified.
const DEFAULT_LINES: usize = 50;

/// Hard maximum on the number of message lines per request.
const MAX_LINES: usize = 200;

/// Maximum characters shown for tool result content before truncation.
const MAX_TOOL_RESULT_CHARS: usize = 500;

/// Find an episode JSONL file by ID, searching the episodes directory recursively.
///
/// Returns `Ok(Some(path))` when found, `Ok(None)` when the directory is missing
/// or the file doesn't exist, and `Err` only on I/O failures reading directories.
///
/// # Errors
/// Returns an error if a directory cannot be read.
pub(crate) fn find_episode_path(
    episodes_dir: &Path,
    episode_id: &str,
) -> anyhow::Result<Option<PathBuf>> {
    if !episodes_dir.exists() {
        return Ok(None);
    }
    let target = format!("{episode_id}.jsonl");
    walk_for_file(episodes_dir, &target)
}

fn walk_for_file(dir: &Path, target: &str) -> anyhow::Result<Option<PathBuf>> {
    let entries = std::fs::read_dir(dir)
        .with_context(|| format!("failed to read episodes directory {}", dir.display()))?;

    for entry in entries {
        let path = entry.context("failed to read directory entry")?.path();
        if path.is_dir() {
            if let Some(found) = walk_for_file(&path, target)? {
                return Ok(Some(found));
            }
        } else if path.file_name().is_some_and(|n| n == target) {
            return Ok(Some(path));
        }
    }

    Ok(None)
}

/// Read a bounded range of lines from an episode JSONL file, formatted for LLM consumption.
///
/// Line numbering is 1-indexed: line 1 is the meta header, line 2+ are messages.
/// Always includes the meta header. When `from_line` is provided, only message
/// lines from that offset onward are included.
///
/// # Errors
/// Returns an error if the file cannot be read or the meta line cannot be parsed.
pub(crate) async fn read_episode_lines(
    path: &Path,
    from_line: Option<usize>,
    request_limit: Option<usize>,
) -> anyhow::Result<String> {
    let file_content = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("failed to read episode file at {}", path.display()))?;

    let all_lines: Vec<&str> = file_content.lines().collect();
    let total = all_lines.len();

    // Parse meta from line 1
    let meta_line = all_lines
        .first()
        .ok_or_else(|| anyhow::anyhow!("episode file is empty at {}", path.display()))?;
    let meta: EpisodeMeta = serde_json::from_str(meta_line)
        .with_context(|| format!("failed to parse episode meta at {}", path.display()))?;

    let limit = request_limit.map_or(DEFAULT_LINES, |l| l.clamp(1, MAX_LINES));

    // from_line is 1-indexed; message lines start at index 1 (line 2)
    // If from_line <= 1 or None, start from the first message line (index 1)
    let start_idx = from_line.map_or(1, |f| f.max(1));

    let end_idx = total.min(start_idx + limit);

    let mut parts: Vec<String> = Vec::new();

    // Header
    parts.push(format!(
        "Episode: {} | {} | {}",
        meta.id, meta.date, meta.context
    ));
    parts.push(String::new());

    // Message lines
    for idx in start_idx..end_idx {
        if let Some(raw) = all_lines.get(idx) {
            if raw.trim().is_empty() {
                continue;
            }
            // 1-indexed line number
            let line_num = idx + 1;
            match serde_json::from_str::<Message>(raw) {
                Ok(msg) => format_message_line(&mut parts, line_num, &msg),
                Err(e) => {
                    tracing::warn!(line = line_num, path = %path.display(), error = %e, "unparseable message line");
                    parts.push(format!("[line {line_num}] (unparseable)"));
                }
            }
        }
    }

    // Footer when showing a subset
    if start_idx > 1 || end_idx < total {
        parts.push(format!(
            "--- showing lines {}-{} of {total} total ---",
            start_idx + 1,
            end_idx
        ));
    }

    Ok(parts.join("\n"))
}

fn format_message_line(parts: &mut Vec<String>, line_num: usize, msg: &Message) {
    match msg.role {
        Role::Assistant if msg.tool_calls.is_some() => {
            if let Some(calls) = msg.tool_calls.as_deref() {
                let tools: Vec<&str> = calls.iter().map(|c| c.name.as_str()).collect();
                let calls_str = tools.join(", ");
                if msg.content.is_empty() {
                    parts.push(format!("[line {line_num}] Assistant: [calls: {calls_str}]"));
                } else {
                    parts.push(format!("[line {line_num}] Assistant: {}", msg.content));
                    parts.push(format!("  [calls: {calls_str}]"));
                }
            }
        }
        Role::Tool => {
            let display_content =
                crate::memory::truncate_at_char_boundary(&msg.content, MAX_TOOL_RESULT_CHARS);
            parts.push(format!("[line {line_num}] Tool: {display_content}"));
        }
        Role::System | Role::User | Role::Assistant => {
            let label = role_label(msg.role);
            parts.push(format!("[line {line_num}] {label}: {}", msg.content));
        }
    }
}

/// Capitalize the first letter of a role name for display labels.
fn role_label(role: Role) -> &'static str {
    match role {
        Role::System => "System",
        Role::User => "User",
        Role::Assistant => "Assistant",
        Role::Tool => "Tool",
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn sample_episode() -> Episode {
        Episode {
            id: "ep-001".to_string(),
            date: NaiveDate::from_ymd_opt(2026, 2, 19).unwrap(),
            context: "general".to_string(),
            observations: vec!["user prefers concise output".to_string()],
            source_episodes: vec![],
        }
    }

    #[tokio::test]
    async fn write_transcript_creates_file_in_month_dir() {
        let dir = tempfile::tempdir().unwrap();
        let episode = sample_episode();
        let messages = vec![Message::user("hello")];

        write_episode_transcript(dir.path(), &episode, &messages)
            .await
            .unwrap();

        let path = dir.path().join("2026-02/19/ep-001.jsonl");
        assert!(
            path.exists(),
            "transcript file should be created in date subdir"
        );

        let raw = tokio::fs::read_to_string(&path).await.unwrap();
        let first_line = raw.lines().next().unwrap();
        let meta: serde_json::Value = serde_json::from_str(first_line).unwrap();
        assert_eq!(
            meta.get("type").and_then(|v| v.as_str()),
            Some("meta"),
            "first line should be meta JSON"
        );
    }

    #[test]
    fn episode_jsonl_path_includes_month() {
        let episode = sample_episode();
        let path = episode_jsonl_path(std::path::Path::new("/ws/episodes"), &episode);
        assert_eq!(
            path,
            std::path::PathBuf::from("/ws/episodes/2026-02/19/ep-001.jsonl"),
            "path should include YYYY-MM/DD subdirectory with .jsonl extension"
        );
    }

    #[tokio::test]
    async fn read_episode_jsonl_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let episode = sample_episode();
        let messages = vec![Message::user("hello"), Message::assistant("world", None)];

        write_episode_transcript(dir.path(), &episode, &messages)
            .await
            .unwrap();

        let path = episode_jsonl_path(dir.path(), &episode);
        let (meta, loaded_messages) = read_episode_jsonl(&path).await.unwrap();

        assert_eq!(meta.id, "ep-001", "meta ID should round-trip");
        assert_eq!(meta.context, "general", "meta context should round-trip");
        assert_eq!(loaded_messages.len(), 2, "should have 2 messages");
        assert_eq!(
            loaded_messages.first().map(|m| m.content.as_str()),
            Some("hello"),
            "first message should round-trip"
        );
    }

    // --- find_episode_path tests ---

    #[tokio::test]
    async fn find_episode_path_basic() {
        let dir = tempfile::tempdir().unwrap();
        let episode = sample_episode();
        write_episode_transcript(dir.path(), &episode, &[Message::user("hi")])
            .await
            .unwrap();

        let found = find_episode_path(dir.path(), "ep-001").unwrap();
        assert!(found.is_some(), "should find existing episode");
        assert!(
            found.unwrap().ends_with("ep-001.jsonl"),
            "path should end with ep-001.jsonl"
        );
    }

    #[tokio::test]
    async fn find_episode_path_multi_month() {
        let dir = tempfile::tempdir().unwrap();
        let ep1 = sample_episode();
        let mut ep2 = sample_episode();
        ep2.id = "ep-002".to_string();
        ep2.date = NaiveDate::from_ymd_opt(2026, 3, 5).unwrap();

        write_episode_transcript(dir.path(), &ep1, &[Message::user("a")])
            .await
            .unwrap();
        write_episode_transcript(dir.path(), &ep2, &[Message::user("b")])
            .await
            .unwrap();

        let found = find_episode_path(dir.path(), "ep-002").unwrap();
        assert!(found.is_some(), "should find ep-002 in different month");
    }

    #[test]
    fn find_episode_path_not_found() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("2026-02/19")).unwrap();
        std::fs::write(dir.path().join("2026-02/19/ep-001.jsonl"), "").unwrap();

        let found = find_episode_path(dir.path(), "ep-999").unwrap();
        assert!(found.is_none(), "should return None for missing episode");
    }

    #[test]
    fn find_episode_path_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let found = find_episode_path(dir.path(), "ep-001").unwrap();
        assert!(found.is_none(), "should return None for empty dir");
    }

    #[test]
    fn find_episode_path_missing_dir() {
        let missing = Path::new("/tmp/nonexistent_episode_dir_test");
        let found = find_episode_path(missing, "ep-001").unwrap();
        assert!(found.is_none(), "should return None for missing dir");
    }

    #[test]
    fn find_episode_path_ignores_obs_json() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("2026-02/19");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("ep-001.obs.json"), "{}").unwrap();

        let found = find_episode_path(dir.path(), "ep-001").unwrap();
        assert!(found.is_none(), "should not match .obs.json files");
    }

    // --- read_episode_lines tests ---

    async fn write_sample_transcript(dir: &Path) -> PathBuf {
        let episode = sample_episode();
        let messages = vec![
            Message::user("hello world"),
            Message::assistant("I can help with that", None),
            Message::user("thanks"),
        ];
        write_episode_transcript(dir, &episode, &messages)
            .await
            .unwrap();
        episode_jsonl_path(dir, &episode)
    }

    #[tokio::test]
    async fn read_episode_lines_from_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_sample_transcript(dir.path()).await;

        let output = read_episode_lines(&path, None, None).await.unwrap();
        assert!(output.contains("Episode: ep-001"), "should have header");
        assert!(
            output.contains("[line 2] User: hello world"),
            "should show first message"
        );
        assert!(
            output.contains("[line 3] Assistant: I can help"),
            "should show assistant"
        );
        assert!(
            output.contains("[line 4] User: thanks"),
            "should show last message"
        );
        // All lines shown, no footer
        assert!(
            !output.contains("showing lines"),
            "should not have footer when showing all"
        );
    }

    #[tokio::test]
    async fn read_episode_lines_with_offset() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_sample_transcript(dir.path()).await;

        // from_line=2 means start at message index 2 (line 3 in 1-indexed)
        let output = read_episode_lines(&path, Some(2), Some(1)).await.unwrap();
        assert!(output.contains("Episode: ep-001"), "header always shown");
        assert!(!output.contains("[line 2]"), "should skip line 2");
        assert!(output.contains("[line 3]"), "should show line 3");
        assert!(
            !output.contains("[line 4]"),
            "should not show line 4 with limit 1"
        );
        assert!(output.contains("showing lines"), "should have footer");
    }

    #[tokio::test]
    async fn read_episode_lines_clamps_to_max() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_sample_transcript(dir.path()).await;

        // Request 999 lines, should clamp to MAX_LINES but still work
        let output = read_episode_lines(&path, None, Some(999)).await.unwrap();
        assert!(output.contains("Episode: ep-001"), "should have header");
        assert!(output.contains("[line 2]"), "should show messages");
    }

    #[tokio::test]
    async fn read_episode_lines_beyond_end() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_sample_transcript(dir.path()).await;

        // Start beyond the file
        let output = read_episode_lines(&path, Some(100), None).await.unwrap();
        assert!(output.contains("Episode: ep-001"), "header always shown");
        assert!(
            !output.contains("[line"),
            "no message lines when offset beyond end"
        );
    }

    #[tokio::test]
    async fn read_episode_lines_role_formatting() {
        let dir = tempfile::tempdir().unwrap();
        let episode = sample_episode();
        let messages = vec![
            Message::system("you are a test agent"),
            Message::user("do something"),
            Message::assistant(
                "",
                Some(vec![crate::models::ToolCall {
                    id: "c1".to_string(),
                    name: "exec".to_string(),
                    arguments: serde_json::json!({"command": "ls"}),
                }]),
            ),
            Message::tool("file.txt", "c1"),
        ];
        write_episode_transcript(dir.path(), &episode, &messages)
            .await
            .unwrap();
        let path = episode_jsonl_path(dir.path(), &episode);

        let output = read_episode_lines(&path, None, None).await.unwrap();
        assert!(output.contains("[line 2] System:"), "system role formatted");
        assert!(output.contains("[line 3] User:"), "user role formatted");
        assert!(
            output.contains("[calls: exec]"),
            "tool call shown compactly"
        );
        assert!(output.contains("[line 5] Tool:"), "tool result shown");
    }

    #[tokio::test]
    async fn read_episode_lines_tool_result_truncation() {
        let dir = tempfile::tempdir().unwrap();
        let episode = sample_episode();
        let long_content = "x".repeat(1000);
        let messages = vec![
            Message::user("run it"),
            Message::tool(long_content.clone(), "c1"),
        ];
        write_episode_transcript(dir.path(), &episode, &messages)
            .await
            .unwrap();
        let path = episode_jsonl_path(dir.path(), &episode);

        let output = read_episode_lines(&path, None, None).await.unwrap();
        assert!(
            output.contains("...(truncated)"),
            "long tool result should be truncated"
        );
        assert!(
            !output.contains(&long_content),
            "full long content should not appear"
        );
    }
}
