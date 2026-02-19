//! Persistence for recent (unobserved) messages across sessions.
//!
//! Messages accumulate in `recent_messages.json` until the observer
//! threshold is reached and an episode is created, at which point
//! the file is cleared.

use std::path::Path;

use crate::error::IronclawError;
use crate::models::Message;

/// Load recent messages from disk.
///
/// Returns an empty vec if the file does not exist.
///
/// # Errors
/// Returns an error if the file exists but cannot be read or parsed.
pub async fn load_recent_messages(path: &Path) -> Result<Vec<Message>, IronclawError> {
    match tokio::fs::read_to_string(path).await {
        Ok(contents) if contents.trim().is_empty() => Ok(Vec::new()),
        Ok(contents) => serde_json::from_str(&contents).map_err(|e| {
            IronclawError::Memory(format!(
                "failed to parse recent messages at {}: {e}",
                path.display()
            ))
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(IronclawError::Memory(format!(
            "failed to read recent messages at {}: {e}",
            path.display()
        ))),
    }
}

/// Save messages to disk atomically (temp file + rename).
///
/// # Errors
/// Returns an error if the file cannot be written.
async fn save_recent_messages(path: &Path, messages: &[Message]) -> Result<(), IronclawError> {
    let json = serde_json::to_string_pretty(messages)
        .map_err(|e| IronclawError::Memory(format!("failed to serialize recent messages: {e}")))?;

    let dir = path.parent().ok_or_else(|| {
        IronclawError::Memory(format!(
            "recent messages path has no parent directory: {}",
            path.display()
        ))
    })?;

    let tmp_path = dir.join(".recent_messages.json.tmp");

    tokio::fs::write(&tmp_path, &json).await.map_err(|e| {
        IronclawError::Memory(format!(
            "failed to write temporary recent messages at {}: {e}",
            tmp_path.display()
        ))
    })?;

    tokio::fs::rename(&tmp_path, path).await.map_err(|e| {
        IronclawError::Memory(format!(
            "failed to rename recent messages from {} to {}: {e}",
            tmp_path.display(),
            path.display()
        ))
    })?;

    Ok(())
}

/// Append messages to the recent messages file.
///
/// Loads existing messages, extends with new ones, and saves atomically.
///
/// # Errors
/// Returns an error if loading or saving fails.
pub async fn append_recent_messages(
    path: &Path,
    new_messages: &[Message],
) -> Result<(), IronclawError> {
    if new_messages.is_empty() {
        return Ok(());
    }
    let mut existing = load_recent_messages(path).await?;
    existing.extend(new_messages.iter().cloned());
    save_recent_messages(path, &existing).await
}

/// Clear the recent messages file (write an empty array).
///
/// # Errors
/// Returns an error if the file cannot be written.
pub async fn clear_recent_messages(path: &Path) -> Result<(), IronclawError> {
    save_recent_messages(path, &[]).await
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use crate::models::Role;

    fn sample_message(content: &str) -> Message {
        Message {
            role: Role::User,
            content: content.to_string(),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    #[tokio::test]
    async fn load_missing_file_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recent_messages.json");
        let messages = load_recent_messages(&path).await.unwrap();
        assert!(messages.is_empty(), "missing file should return empty vec");
    }

    #[tokio::test]
    async fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recent_messages.json");

        let msgs = vec![sample_message("hello"), sample_message("world")];
        save_recent_messages(&path, &msgs).await.unwrap();

        let loaded = load_recent_messages(&path).await.unwrap();
        assert_eq!(loaded.len(), 2, "should load two messages");
        assert_eq!(
            loaded.first().map(|m| m.content.as_str()),
            Some("hello"),
            "first message should match"
        );
    }

    #[tokio::test]
    async fn append_to_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recent_messages.json");

        append_recent_messages(&path, &[sample_message("first")])
            .await
            .unwrap();

        let loaded = load_recent_messages(&path).await.unwrap();
        assert_eq!(loaded.len(), 1, "should have one message");
    }

    #[tokio::test]
    async fn append_preserves_existing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recent_messages.json");

        append_recent_messages(&path, &[sample_message("first")])
            .await
            .unwrap();
        append_recent_messages(&path, &[sample_message("second")])
            .await
            .unwrap();

        let loaded = load_recent_messages(&path).await.unwrap();
        assert_eq!(loaded.len(), 2, "should have two messages");
    }

    #[tokio::test]
    async fn clear_empties_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recent_messages.json");

        append_recent_messages(&path, &[sample_message("first")])
            .await
            .unwrap();
        clear_recent_messages(&path).await.unwrap();

        let loaded = load_recent_messages(&path).await.unwrap();
        assert!(loaded.is_empty(), "cleared file should return empty vec");
    }

    #[tokio::test]
    async fn append_empty_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recent_messages.json");

        append_recent_messages(&path, &[]).await.unwrap();
        assert!(
            !path.exists(),
            "appending nothing should not create the file"
        );
    }

    #[tokio::test]
    async fn load_empty_file_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recent_messages.json");
        tokio::fs::write(&path, "").await.unwrap();

        let loaded = load_recent_messages(&path).await.unwrap();
        assert!(loaded.is_empty(), "empty file should return empty vec");
    }
}
