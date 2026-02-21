//! Episode transcript file persistence.
//!
//! Writes episode transcripts as JSONL files to `memory/episodes/YYYY-MM/DD/<id>.jsonl`.
//! Line 1 is a JSON meta object; subsequent lines are serialized [`Message`] values.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::IronclawError;
use crate::memory::types::Episode;
use crate::models::Message;

/// Metadata from the first line of an episode JSONL file.
#[derive(Debug, Deserialize)]
pub struct EpisodeMeta {
    /// Episode identifier (e.g., `"ep-001"`).
    pub id: String,
    /// Date of the episode.
    pub date: chrono::NaiveDate,
    /// Project or topic context tag.
    pub context: String,
    /// One-line summary of how the episode started.
    pub start: String,
    /// One-line summary of how the episode ended.
    pub end: String,
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
) -> Result<(), IronclawError> {
    let day_dir = episodes_dir.join(episode.date.format("%Y-%m/%d").to_string());
    tokio::fs::create_dir_all(&day_dir).await.map_err(|e| {
        IronclawError::Memory(format!(
            "failed to create episode directory at {}: {e}",
            day_dir.display()
        ))
    })?;

    let path = episode_jsonl_path(episodes_dir, episode);

    let meta = serde_json::json!({
        "type": "meta",
        "id": episode.id,
        "date": episode.date.to_string(),
        "start": episode.start,
        "end": episode.end,
        "context": episode.context,
    });

    let mut lines = Vec::with_capacity(messages.len() + 1);
    lines
        .push(serde_json::to_string(&meta).map_err(|e| {
            IronclawError::Memory(format!("failed to serialize episode meta: {e}"))
        })?);

    for msg in messages {
        lines.push(
            serde_json::to_string(msg)
                .map_err(|e| IronclawError::Memory(format!("failed to serialize message: {e}")))?,
        );
    }

    let file_content = lines.join("\n") + "\n";

    tokio::fs::write(&path, &file_content).await.map_err(|e| {
        IronclawError::Memory(format!(
            "failed to write episode transcript at {}: {e}",
            path.display()
        ))
    })
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

/// Read and parse a JSONL episode transcript file.
///
/// Returns the episode metadata and the list of messages. The first line is
/// parsed as [`EpisodeMeta`]; subsequent non-empty lines are parsed as [`Message`] values.
///
/// # Errors
/// Returns an error if the file cannot be read or any line fails to parse.
pub async fn read_episode_jsonl(path: &Path) -> Result<(EpisodeMeta, Vec<Message>), IronclawError> {
    let file_content = tokio::fs::read_to_string(path).await.map_err(|e| {
        IronclawError::Memory(format!(
            "failed to read episode file at {}: {e}",
            path.display()
        ))
    })?;

    let mut lines = file_content.lines();

    let meta_line = lines.next().ok_or_else(|| {
        IronclawError::Memory(format!("episode file is empty at {}", path.display()))
    })?;

    let meta: EpisodeMeta = serde_json::from_str(meta_line).map_err(|e| {
        IronclawError::Memory(format!(
            "failed to parse episode meta at {}: {e}",
            path.display()
        ))
    })?;

    let mut messages = Vec::new();
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let msg: Message = serde_json::from_str(line).map_err(|e| {
            IronclawError::Memory(format!(
                "failed to parse message line in {}: {e}",
                path.display()
            ))
        })?;
        messages.push(msg);
    }

    Ok((meta, messages))
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
            start: "user asked about files".to_string(),
            end: "listed directory contents".to_string(),
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
}
