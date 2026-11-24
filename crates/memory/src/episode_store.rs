use std::path::{Path, PathBuf};

use serde::Deserialize;

use anyhow::Context;

use crate::types::{Episode, SourceTag};
use residuum_inference::{Message, Role};

#[derive(Debug, Deserialize)]
pub struct EpisodeMeta {
    pub id: String,
    pub date: chrono::NaiveDate,
    #[serde(flatten)]
    pub source: SourceTag,
    #[serde(default)]
    pub narrative: Option<String>,
}

#[cfg(any(test, feature = "test-support"))]
pub async fn write_episode_transcript(
    episodes_dir: &Path,
    episode: &Episode,
    messages: &[Message],
) -> anyhow::Result<()> {
    write_episode_transcript_tagged(episodes_dir, episode, messages, &SourceTag::main(), None).await
}

pub async fn write_episode_transcript_tagged(
    episodes_dir: &Path,
    episode: &Episode,
    messages: &[Message],
    tag: &SourceTag,
    narrative: Option<&str>,
) -> anyhow::Result<()> {
    let day_dir = episodes_dir.join(episode.date.format("%Y-%m/%d").to_string());
    tokio::fs::create_dir_all(&day_dir).await.with_context(|| {
        format!(
            "failed to create episode directory at {}",
            day_dir.display()
        )
    })?;

    let path = episode_jsonl_path(episodes_dir, episode);

    let mut meta = serde_json::json!({
        "type": "meta",
        "id": episode.id,
        "date": episode.date.to_string(),
    });
    if let serde_json::Value::Object(map) = &mut meta {
        if let Ok(serde_json::Value::Object(tag_map)) = serde_json::to_value(tag) {
            map.extend(tag_map);
        }
        if let Some(n) = narrative {
            map.insert(
                "narrative".to_string(),
                serde_json::Value::String(n.to_string()),
            );
        }
    }

    let mut lines = Vec::with_capacity(messages.len() + 1);
    lines.push(serde_json::to_string(&meta).context("failed to serialize episode meta")?);

    for msg in messages {
        let archived = (!msg.thinking.is_empty()).then(|| msg.without_replay_data());
        lines.push(
            serde_json::to_string(archived.as_ref().unwrap_or(msg))
                .context("failed to serialize message")?,
        );
    }

    let file_content = lines.join("\n") + "\n";

    tokio::fs::write(&path, &file_content)
        .await
        .with_context(|| format!("failed to write episode transcript at {}", path.display()))
}

#[must_use]
pub fn episode_jsonl_path(episodes_dir: &Path, episode: &Episode) -> PathBuf {
    episodes_dir
        .join(episode.date.format("%Y-%m/%d").to_string())
        .join(format!("{}.jsonl", episode.id))
}

#[must_use]
pub(crate) fn episode_obs_path(episodes_dir: &Path, episode: &Episode) -> PathBuf {
    episodes_dir
        .join(episode.date.format("%Y-%m/%d").to_string())
        .join(format!("{}.obs.json", episode.id))
}

#[must_use]
pub(crate) fn episode_idx_path(episodes_dir: &Path, episode: &Episode) -> PathBuf {
    episodes_dir
        .join(episode.date.format("%Y-%m/%d").to_string())
        .join(format!("{}.idx.jsonl", episode.id))
}

const COMPLETION_MARKER_EXT: &str = "done";

#[must_use]
pub(crate) fn episode_marker_path(episodes_dir: &Path, episode: &Episode) -> PathBuf {
    episodes_dir
        .join(episode.date.format("%Y-%m/%d").to_string())
        .join(format!("{}.{COMPLETION_MARKER_EXT}", episode.id))
}

pub(crate) async fn write_completion_marker(
    episodes_dir: &Path,
    episode: &Episode,
) -> anyhow::Result<()> {
    let path = episode_marker_path(episodes_dir, episode);
    let body = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    residuum_core::util::fs::atomic_write(&path, body)
        .await
        .with_context(|| {
            format!(
                "failed to write episode completion marker at {}",
                path.display()
            )
        })
}

pub(crate) fn find_interrupted_episodes(episodes_dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    if !episodes_dir.exists() {
        return Ok(Vec::new());
    }

    let mut transcripts = Vec::new();
    collect_transcripts(episodes_dir, &mut transcripts)?;

    let mut interrupted = Vec::new();
    for transcript in transcripts {
        let has_marker = transcript.with_extension(COMPLETION_MARKER_EXT).exists();
        let has_obs_archive = transcript.with_extension("obs.json").exists();
        if has_marker || has_obs_archive {
            continue;
        }

        let episode_id = transcript
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");
        tracing::warn!(
            episode_id,
            path = %transcript.display(),
            "interrupted episode persistence: transcript has no completion marker or observation archive, so its observations and search chunks were never durably written and it is invisible to memory search; inspect the transcript and re-observe or delete it"
        );
        interrupted.push(transcript);
    }

    Ok(interrupted)
}

fn collect_transcripts(dir: &Path, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    let entries = std::fs::read_dir(dir)
        .with_context(|| format!("failed to read episodes directory {}", dir.display()))?;

    for entry in entries {
        let path = entry.context("failed to read directory entry")?.path();
        if path.is_dir() {
            collect_transcripts(&path, out)?;
        } else if is_episode_transcript(&path) {
            out.push(path);
        }
    }

    Ok(())
}

fn is_episode_transcript(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "jsonl")
        && path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.strip_prefix("ep-"))
            .is_some_and(|s| s.parse::<u32>().is_ok())
}

pub fn transcripts_newest_first(episodes_dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    if !episodes_dir.exists() {
        return Ok(Vec::new());
    }
    let mut transcripts = Vec::new();
    collect_transcripts(episodes_dir, &mut transcripts)?;
    transcripts.sort_by_key(|path| std::cmp::Reverse(episode_number(path)));
    Ok(transcripts)
}

fn episode_number(transcript: &Path) -> u32 {
    transcript
        .file_stem()
        .and_then(|s| s.to_str())
        .and_then(|s| s.strip_prefix("ep-"))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

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

pub const DEFAULT_LINES: usize = 50;

pub const MAX_LINES: usize = 200;

const MAX_TOOL_RESULT_CHARS: usize = 500;

pub fn find_episode_path(episodes_dir: &Path, episode_id: &str) -> anyhow::Result<Option<PathBuf>> {
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

pub async fn read_episode_lines(
    path: &Path,
    from_line: Option<usize>,
    request_limit: Option<usize>,
    expand_line: Option<usize>,
) -> anyhow::Result<String> {
    let file_content = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("failed to read episode file at {}", path.display()))?;

    let all_lines: Vec<&str> = file_content.lines().collect();
    let total = all_lines.len();

    let meta_line = all_lines
        .first()
        .ok_or_else(|| anyhow::anyhow!("episode file is empty at {}", path.display()))?;
    let meta: EpisodeMeta = serde_json::from_str(meta_line)
        .with_context(|| format!("failed to parse episode meta at {}", path.display()))?;

    let mut parts: Vec<String> = Vec::new();

    parts.push(format!("Episode: {} | {}", meta.id, meta.date));
    parts.push(String::new());

    if let Some(target) = expand_line {
        let idx = target.saturating_sub(1);
        match all_lines.get(idx).filter(|_| target >= 2) {
            None => {
                parts.push(format!(
                    "line {target} does not exist; the episode has {total} line(s) total \
                     (line 1 is the meta header, message lines start at 2)"
                ));
            }
            Some(raw) if raw.trim().is_empty() => {
                parts.push(format!("[line {target}] (blank line)"));
            }
            Some(raw) => match serde_json::from_str::<Message>(raw) {
                Ok(msg) => format_message_line(&mut parts, target, &msg, true),
                Err(e) => {
                    tracing::warn!(line = target, path = %path.display(), error = %e, "unparseable message line");
                    parts.push(format!("[line {target}] (unparseable)"));
                }
            },
        }
        return Ok(parts.join("\n"));
    }

    let limit = request_limit.map_or(DEFAULT_LINES, |l| l.clamp(1, MAX_LINES));

    let start_idx = from_line.map_or(1, |f| f.max(1));

    let end_idx = total.min(start_idx + limit);

    for idx in start_idx..end_idx {
        if let Some(raw) = all_lines.get(idx) {
            if raw.trim().is_empty() {
                continue;
            }
            let line_num = idx + 1;
            match serde_json::from_str::<Message>(raw) {
                Ok(msg) => format_message_line(&mut parts, line_num, &msg, false),
                Err(e) => {
                    tracing::warn!(line = line_num, path = %path.display(), error = %e, "unparseable message line");
                    parts.push(format!("[line {line_num}] (unparseable)"));
                }
            }
        }
    }

    if start_idx > 1 || end_idx < total {
        parts.push(format!(
            "--- showing lines {}-{} of {total} total ---",
            start_idx + 1,
            end_idx
        ));
    }

    Ok(parts.join("\n"))
}

pub async fn next_episode_id(episodes_dir: &Path) -> anyhow::Result<String> {
    let max_num = max_episode_num(episodes_dir)?;
    let id = format!("ep-{:03}", max_num + 1);
    tracing::debug!(episode_id = %id, "assigned next episode ID");
    Ok(id)
}

#[cfg(any(test, feature = "test-support"))]
pub fn latest_episode_id(episodes_dir: &Path) -> anyhow::Result<Option<String>> {
    let max_num = max_episode_num(episodes_dir)?;
    if max_num == 0 {
        Ok(None)
    } else {
        Ok(Some(format!("ep-{max_num:03}")))
    }
}

pub async fn older_main_episode_id(
    episodes_dir: &Path,
    before: Option<&str>,
) -> anyhow::Result<Option<String>> {
    let limit = match before {
        None => None,
        Some(cursor) => {
            let Some(num) = cursor
                .strip_prefix("ep-")
                .and_then(|s| s.parse::<u32>().ok())
            else {
                return Ok(None);
            };
            Some(num)
        }
    };

    for transcript in transcripts_newest_first(episodes_dir)? {
        let num = episode_number(&transcript);
        if limit.is_some_and(|limit| num >= limit) {
            continue;
        }
        match read_episode_meta(&transcript).await {
            Ok(meta) if meta.source.is_session() => {}
            Ok(meta) => return Ok(Some(meta.id)),
            Err(err) => tracing::warn!(
                error = format!("{err:#}"),
                path = %transcript.display(),
                "skipping an episode whose meta line can't be read while paging main chat history",
            ),
        }
    }
    Ok(None)
}

async fn read_episode_meta(path: &Path) -> anyhow::Result<EpisodeMeta> {
    use tokio::io::AsyncBufReadExt;

    let file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("failed to open episode file at {}", path.display()))?;
    let mut first = String::new();
    tokio::io::BufReader::new(file)
        .read_line(&mut first)
        .await
        .with_context(|| format!("failed to read episode file at {}", path.display()))?;
    serde_json::from_str(&first)
        .with_context(|| format!("failed to parse episode meta at {}", path.display()))
}

fn max_episode_num(dir: &Path) -> anyhow::Result<u32> {
    if !dir.exists() {
        return Ok(0);
    }
    let mut max = 0_u32;
    walk_for_max(dir, &mut max)?;
    Ok(max)
}

fn walk_for_max(dir: &Path, max: &mut u32) -> anyhow::Result<()> {
    let entries = std::fs::read_dir(dir)
        .with_context(|| format!("failed to read episodes directory {}", dir.display()))?;

    for entry in entries {
        let entry = entry.context("failed to read directory entry")?;
        let path = entry.path();

        if path.is_dir() {
            walk_for_max(&path, max)?;
        } else if path.extension().is_some_and(|ext| ext == "jsonl")
            && let Some(n) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| s.strip_prefix("ep-"))
                .and_then(|s| s.parse::<u32>().ok())
        {
            *max = (*max).max(n);
        }
    }

    Ok(())
}

pub fn format_message_line(parts: &mut Vec<String>, line_num: usize, msg: &Message, full: bool) {
    match msg.role {
        Role::Assistant if msg.tool_calls.is_some() => {
            let calls = msg.tool_calls.as_deref().unwrap_or(&[]);
            let tools: Vec<&str> = calls.iter().map(|c| c.name.as_str()).collect();
            let calls_str = tools.join(", ");
            if msg.content.is_empty() {
                parts.push(format!("[line {line_num}] Assistant: [calls: {calls_str}]"));
            } else {
                parts.push(format!("[line {line_num}] Assistant: {}", msg.content));
                parts.push(format!("  [calls: {calls_str}]"));
            }
        }
        Role::Tool if full || msg.content.len() <= MAX_TOOL_RESULT_CHARS => {
            parts.push(format!("[line {line_num}] Tool: {}", msg.content));
        }
        Role::Tool => {
            let display_content =
                crate::truncate_at_char_boundary(&msg.content, MAX_TOOL_RESULT_CHARS);
            let original_chars = msg.content.chars().count();
            parts.push(format!(
                "[line {line_num}] Tool: {display_content} (showing {MAX_TOOL_RESULT_CHARS} of \
                 {original_chars} chars; use memory_get with expand_line={line_num} to see the \
                 full result)"
            ));
        }
        Role::System | Role::User | Role::Assistant => {
            let label = match msg.role {
                Role::System => "System",
                Role::User => "User",
                Role::Assistant => "Assistant",
                Role::Tool => unreachable!(),
            };
            parts.push(format!(
                "[line {line_num}] {label}: {}",
                msg.attributed_content()
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn sample_episode() -> Episode {
        Episode {
            id: "ep-001".to_string(),
            date: NaiveDate::from_ymd_opt(2026, 2, 19).unwrap(),
            observations: vec!["user prefers concise output".to_string()],
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
        assert_eq!(loaded_messages.len(), 2, "should have 2 messages");
        assert_eq!(
            loaded_messages.first().map(|m| m.content.as_str()),
            Some("hello"),
            "first message should round-trip"
        );
    }

    #[tokio::test]
    async fn episode_meta_without_source_tag_loads_as_main() {
        let dir = tempfile::tempdir().unwrap();
        let episode = sample_episode();
        let day_dir = dir.path().join(episode.date.format("%Y-%m/%d").to_string());
        tokio::fs::create_dir_all(&day_dir).await.unwrap();
        let path = episode_jsonl_path(dir.path(), &episode);
        let legacy_meta = serde_json::json!({
            "type": "meta",
            "id": episode.id,
            "date": episode.date.to_string(),
        });
        tokio::fs::write(&path, format!("{legacy_meta}\n"))
            .await
            .unwrap();

        let (meta, messages) = read_episode_jsonl(&path).await.unwrap();
        assert_eq!(meta.id, "ep-001");
        assert!(
            !meta.source.is_session(),
            "a legacy meta line with no source tag should load as untagged"
        );
        assert!(meta.narrative.is_none());
        assert!(messages.is_empty());
    }

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

        let output = read_episode_lines(&path, None, None, None).await.unwrap();
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
        assert!(
            !output.contains("showing lines"),
            "should not have footer when showing all"
        );
    }

    #[tokio::test]
    async fn read_episode_lines_with_offset() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_sample_transcript(dir.path()).await;

        let output = read_episode_lines(&path, Some(2), Some(1), None)
            .await
            .unwrap();
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

        let output = read_episode_lines(&path, None, Some(999), None)
            .await
            .unwrap();
        assert!(output.contains("Episode: ep-001"), "should have header");
        assert!(output.contains("[line 2]"), "should show messages");
    }

    #[tokio::test]
    async fn read_episode_lines_beyond_end() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_sample_transcript(dir.path()).await;

        let output = read_episode_lines(&path, Some(100), None, None)
            .await
            .unwrap();
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
                Some(vec![residuum_inference::ToolCall {
                    id: "c1".to_string(),
                    name: "exec".to_string(),
                    arguments: serde_json::json!({"command": "ls"}),
                    server: None,
                }]),
            ),
            Message::tool("file.txt", "c1"),
        ];
        write_episode_transcript(dir.path(), &episode, &messages)
            .await
            .unwrap();
        let path = episode_jsonl_path(dir.path(), &episode);

        let output = read_episode_lines(&path, None, None, None).await.unwrap();
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

        let output = read_episode_lines(&path, None, None, None).await.unwrap();
        assert!(
            output.contains("...(truncated)"),
            "long tool result should be truncated"
        );
        assert!(
            !output.contains(&long_content),
            "full long content should not appear"
        );
        assert!(
            output.contains("showing 500 of 1000 chars"),
            "truncation notice should report the original length: {output}"
        );
        assert!(
            output.contains("expand_line=3"),
            "truncation notice should point at expand_line for this line: {output}"
        );

        let full = read_episode_lines(&path, None, None, Some(3))
            .await
            .unwrap();
        assert!(
            full.contains(&long_content),
            "expand_line should return the full untruncated content: {full}"
        );
        assert!(
            !full.contains("...(truncated)"),
            "expand_line output should not be truncated: {full}"
        );
    }

    #[tokio::test]
    async fn read_episode_lines_expand_line_out_of_range() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_sample_transcript(dir.path()).await;

        let output = read_episode_lines(&path, None, None, Some(999))
            .await
            .unwrap();
        assert!(
            output.contains("does not exist"),
            "an out-of-range expand_line should say so, not panic or error: {output}"
        );
    }

    #[tokio::test]
    async fn next_episode_id_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let episodes_dir = dir.path().join("episodes");
        tokio::fs::create_dir_all(&episodes_dir).await.unwrap();

        let id = next_episode_id(&episodes_dir).await.unwrap();
        assert_eq!(id, "ep-001", "empty dir should return ep-001");
    }

    #[tokio::test]
    async fn next_episode_id_missing_dir() {
        let dir = tempfile::tempdir().unwrap();
        let episodes_dir = dir.path().join("episodes");
        let id = next_episode_id(&episodes_dir).await.unwrap();
        assert_eq!(id, "ep-001", "missing dir should return ep-001");
    }

    #[tokio::test]
    async fn next_episode_id_scans_jsonl_files() {
        let dir = tempfile::tempdir().unwrap();
        let episodes_dir = dir.path().join("episodes");
        let month_dir = episodes_dir.join("2026-02/19");
        tokio::fs::create_dir_all(&month_dir).await.unwrap();

        tokio::fs::write(month_dir.join("ep-001.jsonl"), "")
            .await
            .unwrap();
        tokio::fs::write(month_dir.join("ep-003.jsonl"), "")
            .await
            .unwrap();

        let id = next_episode_id(&episodes_dir).await.unwrap();
        assert_eq!(id, "ep-004", "should find max and increment");
    }

    #[tokio::test]
    async fn read_episode_jsonl_empty_file_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ep-001.jsonl");
        tokio::fs::write(&path, "").await.unwrap();
        let result = read_episode_jsonl(&path).await;
        assert!(result.is_err(), "empty file should return Err");
    }

    #[tokio::test]
    async fn read_episode_jsonl_malformed_message_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ep-001.jsonl");
        let content =
            "{\"type\":\"meta\",\"id\":\"ep-001\",\"date\":\"2026-02-19\"}\nnot valid json\n";
        tokio::fs::write(&path, content).await.unwrap();
        let result = read_episode_jsonl(&path).await;
        assert!(result.is_err(), "malformed message line should return Err");
    }

    #[tokio::test]
    async fn latest_episode_id_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let id = latest_episode_id(dir.path()).unwrap();
        assert!(id.is_none(), "empty dir should return None");
    }

    #[tokio::test]
    async fn latest_episode_id_scans_jsonl_files() {
        let dir = tempfile::tempdir().unwrap();
        let episodes_dir = dir.path().join("episodes");
        let month_dir = episodes_dir.join("2026-02/19");
        tokio::fs::create_dir_all(&month_dir).await.unwrap();
        tokio::fs::write(month_dir.join("ep-001.jsonl"), "")
            .await
            .unwrap();
        tokio::fs::write(month_dir.join("ep-003.jsonl"), "")
            .await
            .unwrap();

        let id = latest_episode_id(&episodes_dir).unwrap();
        assert_eq!(id, Some("ep-003".to_string()));
    }

    async fn write_tagged(dir: &Path, id: &str, tag: &SourceTag) {
        let mut episode = sample_episode();
        episode.id = id.to_string();
        write_episode_transcript_tagged(dir, &episode, &[Message::user(id)], tag, None)
            .await
            .unwrap();
    }

    fn pulse_tag() -> SourceTag {
        SourceTag::session("scheduled-wiki-lint-2b42", "run-1", "scheduled")
    }

    #[tokio::test]
    async fn older_main_episode_id_starts_at_the_newest_main_episode() {
        let dir = tempfile::tempdir().unwrap();
        write_tagged(dir.path(), "ep-001", &SourceTag::main()).await;
        write_tagged(dir.path(), "ep-002", &SourceTag::main()).await;
        write_tagged(dir.path(), "ep-003", &pulse_tag()).await;

        let newest = older_main_episode_id(dir.path(), None).await.unwrap();
        assert_eq!(newest.as_deref(), Some("ep-002"));
    }

    #[tokio::test]
    async fn older_main_episode_id_skips_session_runs_and_gaps() {
        let dir = tempfile::tempdir().unwrap();
        write_tagged(dir.path(), "ep-001", &SourceTag::main()).await;
        write_tagged(dir.path(), "ep-003", &pulse_tag()).await;
        write_tagged(
            dir.path(),
            "ep-004",
            &SourceTag::session("spawned-a", "run-2", "spawned"),
        )
        .await;
        write_tagged(dir.path(), "ep-005", &SourceTag::main()).await;

        let older = older_main_episode_id(dir.path(), Some("ep-005"))
            .await
            .unwrap();
        assert_eq!(older.as_deref(), Some("ep-001"));
    }

    #[tokio::test]
    async fn older_main_episode_id_is_none_with_only_session_runs() {
        let dir = tempfile::tempdir().unwrap();
        write_tagged(dir.path(), "ep-001", &pulse_tag()).await;

        assert!(
            older_main_episode_id(dir.path(), None)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn older_main_episode_id_none_when_at_first() {
        let dir = tempfile::tempdir().unwrap();
        write_tagged(dir.path(), "ep-001", &SourceTag::main()).await;

        let older = older_main_episode_id(dir.path(), Some("ep-001"))
            .await
            .unwrap();
        assert!(older.is_none());
    }

    #[tokio::test]
    async fn older_main_episode_id_malformed_cursor() {
        let dir = tempfile::tempdir().unwrap();
        write_tagged(dir.path(), "ep-001", &SourceTag::main()).await;

        let older = older_main_episode_id(dir.path(), Some("garbage"))
            .await
            .unwrap();
        assert!(older.is_none());
    }

    #[tokio::test]
    async fn older_main_episode_id_reads_an_untagged_legacy_episode_as_main() {
        let dir = tempfile::tempdir().unwrap();
        let day_dir = dir.path().join("2026-02/19");
        tokio::fs::create_dir_all(&day_dir).await.unwrap();
        tokio::fs::write(
            day_dir.join("ep-001.jsonl"),
            "{\"type\":\"meta\",\"id\":\"ep-001\",\"date\":\"2026-02-19\"}\n",
        )
        .await
        .unwrap();

        let newest = older_main_episode_id(dir.path(), None).await.unwrap();
        assert_eq!(newest.as_deref(), Some("ep-001"));
    }

    #[tokio::test]
    async fn older_main_episode_id_steps_past_an_unreadable_meta_line() {
        let dir = tempfile::tempdir().unwrap();
        write_tagged(dir.path(), "ep-001", &SourceTag::main()).await;
        let day_dir = dir.path().join("2026-02/19");
        tokio::fs::write(day_dir.join("ep-002.jsonl"), "not json\n")
            .await
            .unwrap();

        let newest = older_main_episode_id(dir.path(), None).await.unwrap();
        assert_eq!(newest.as_deref(), Some("ep-001"));
    }

    #[tokio::test]
    async fn completion_marker_written_and_recognized() {
        let dir = tempfile::tempdir().unwrap();
        let episode = sample_episode();
        write_episode_transcript(dir.path(), &episode, &[Message::user("hi")])
            .await
            .unwrap();
        write_completion_marker(dir.path(), &episode).await.unwrap();

        let marker = episode_marker_path(dir.path(), &episode);
        assert!(marker.exists(), "completion marker file should be written");

        let interrupted = find_interrupted_episodes(dir.path()).unwrap();
        assert!(
            interrupted.is_empty(),
            "an episode with a completion marker is not interrupted"
        );
    }

    #[tokio::test]
    async fn interrupted_episode_detected_when_only_transcript_exists() {
        let dir = tempfile::tempdir().unwrap();
        let episode = sample_episode();
        write_episode_transcript(dir.path(), &episode, &[Message::user("hi")])
            .await
            .unwrap();

        let interrupted = find_interrupted_episodes(dir.path()).unwrap();
        assert_eq!(
            interrupted.len(),
            1,
            "a transcript with no marker and no obs archive is interrupted"
        );
        assert!(
            interrupted
                .first()
                .is_some_and(|p| p.ends_with("ep-001.jsonl")),
            "the orphaned transcript path should be reported"
        );
    }

    #[tokio::test]
    async fn legacy_episode_with_obs_archive_not_flagged() {
        let dir = tempfile::tempdir().unwrap();
        let episode = sample_episode();
        write_episode_transcript(dir.path(), &episode, &[Message::user("hi")])
            .await
            .unwrap();
        let obs_path = episode_obs_path(dir.path(), &episode);
        tokio::fs::write(&obs_path, "[]").await.unwrap();

        let interrupted = find_interrupted_episodes(dir.path()).unwrap();
        assert!(
            interrupted.is_empty(),
            "an episode with a legacy obs archive is treated as complete"
        );
    }

    #[tokio::test]
    async fn reconciliation_isolates_interrupted_from_healthy() {
        let dir = tempfile::tempdir().unwrap();

        let healthy = sample_episode();
        write_episode_transcript(dir.path(), &healthy, &[Message::user("a")])
            .await
            .unwrap();
        write_completion_marker(dir.path(), &healthy).await.unwrap();

        let mut interrupted_ep = sample_episode();
        interrupted_ep.id = "ep-002".to_string();
        write_episode_transcript(dir.path(), &interrupted_ep, &[Message::user("b")])
            .await
            .unwrap();

        let interrupted = find_interrupted_episodes(dir.path()).unwrap();
        assert_eq!(
            interrupted.len(),
            1,
            "only the interrupted episode is flagged"
        );
        assert!(
            interrupted
                .first()
                .is_some_and(|p| p.ends_with("ep-002.jsonl")),
            "the healthy episode must not be flagged"
        );
    }

    #[test]
    fn find_interrupted_episodes_missing_dir_is_empty() {
        let missing = Path::new("/tmp/nonexistent_reconcile_dir_test");
        let interrupted = find_interrupted_episodes(missing).unwrap();
        assert!(
            interrupted.is_empty(),
            "a missing episodes dir yields no interrupted episodes"
        );
    }

    #[test]
    fn idx_jsonl_is_not_mistaken_for_a_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("2026-02/19");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("ep-001.idx.jsonl"), "{}\n").unwrap();

        let interrupted = find_interrupted_episodes(dir.path()).unwrap();
        assert!(
            interrupted.is_empty(),
            "an .idx.jsonl file is not an episode transcript"
        );
    }

    #[tokio::test]
    async fn an_episode_keeps_the_readable_thinking_and_not_the_replay_data() {
        let dir = tempfile::tempdir().unwrap();
        let episode = sample_episode();
        let messages = vec![
            Message::user("what is it?"),
            residuum_inference::Message::assistant("The answer is 42.".to_string(), None)
                .with_thinking(vec![residuum_inference::ThinkingBlock {
                    text: "private reasoning about the answer".to_string(),
                    signature: Some("sig-opaque-token".to_string()),
                    redacted: Some("encrypted-blob".to_string()),
                    part: None,
                    origin: None,
                    binding: None,
                }]),
        ];
        write_episode_transcript(dir.path(), &episode, &messages)
            .await
            .unwrap();
        let path = episode_jsonl_path(dir.path(), &episode);

        let raw = tokio::fs::read_to_string(&path).await.unwrap();
        let (_, loaded) = read_episode_jsonl(&path).await.unwrap();

        assert!(raw.contains("private reasoning about the answer"));
        assert!(!raw.contains("sig-opaque-token"), "{raw}");
        assert!(!raw.contains("encrypted-blob"), "{raw}");
        assert_eq!(
            loaded.last().unwrap().thinking,
            [residuum_inference::ThinkingBlock::text(
                "private reasoning about the answer"
            )]
        );
    }

    #[tokio::test]
    async fn reading_an_episode_back_to_the_model_leaves_out_the_thinking() {
        let dir = tempfile::tempdir().unwrap();
        let episode = sample_episode();
        let messages = vec![
            Message::user("what is it?"),
            residuum_inference::Message::assistant("The answer is 42.".to_string(), None)
                .with_thinking(vec![residuum_inference::ThinkingBlock {
                    text: "private reasoning about the answer".to_string(),
                    signature: Some("sig-opaque-token".to_string()),
                    redacted: Some("encrypted-blob".to_string()),
                    part: None,
                    origin: None,
                    binding: None,
                }]),
        ];
        write_episode_transcript(dir.path(), &episode, &messages)
            .await
            .unwrap();
        let path = episode_jsonl_path(dir.path(), &episode);

        let output = read_episode_lines(&path, None, None, None).await.unwrap();
        let expanded = read_episode_lines(&path, None, None, Some(3))
            .await
            .unwrap();

        for shown in [output, expanded] {
            assert!(shown.contains("The answer is 42."));
            for leaked in ["private reasoning", "sig-opaque-token", "encrypted-blob"] {
                assert!(
                    !shown.contains(leaked),
                    "{leaked} must not reach the model: {shown}"
                );
            }
        }
    }
}
