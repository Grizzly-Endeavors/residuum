//! Persistence for recent (unobserved) messages across sessions.
//!
//! Messages accumulate in `recent_messages.json` until the observer
//! threshold is reached and an episode is created, at which point
//! the file is cleared.

use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::IronclawError;
use crate::memory::types::Visibility;
use crate::models::Message;

/// Provides a default timestamp for backward-compatible deserialization.
fn default_timestamp() -> DateTime<Utc> {
    Utc::now()
}

/// A persisted message with session metadata.
///
/// Wraps a [`Message`] with the context needed for the observer to derive
/// observation metadata (project context, visibility) without requiring the
/// agent to re-examine the conversation on startup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentMessage {
    /// The underlying conversation message.
    #[serde(flatten)]
    pub message: Message,
    /// When this message was added to the session.
    #[serde(default = "default_timestamp")]
    pub timestamp: DateTime<Utc>,
    /// Workspace context at the time this message was recorded.
    #[serde(default)]
    pub project_context: String,
    /// Whether this message came from a user-visible or background turn.
    #[serde(default)]
    pub visibility: Visibility,
}

/// Load recent messages from disk.
///
/// Returns an empty vec if the file does not exist.
///
/// # Errors
/// Returns an error if the file exists but cannot be read or parsed.
pub async fn load_recent_messages(path: &Path) -> Result<Vec<RecentMessage>, IronclawError> {
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

/// Load recent messages as plain [`Message`] values, stripping metadata.
///
/// Used when restoring the agent session at startup — the agent only needs
/// the conversation messages, not the observation metadata.
///
/// # Errors
/// Returns an error if the file exists but cannot be read or parsed.
pub async fn load_messages_for_agent(path: &Path) -> Result<Vec<Message>, IronclawError> {
    let recent = load_recent_messages(path).await?;
    Ok(recent.into_iter().map(|rm| rm.message).collect())
}

/// Save recent messages to disk atomically (temp file + rename).
///
/// # Errors
/// Returns an error if the file cannot be written.
async fn save_recent_messages(
    path: &Path,
    messages: &[RecentMessage],
) -> Result<(), IronclawError> {
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

/// Append messages to the recent messages file, wrapping each with metadata.
///
/// Loads existing messages, extends with new wrapped messages, and saves atomically.
///
/// # Errors
/// Returns an error if loading or saving fails.
pub async fn append_recent_messages(
    path: &Path,
    new_messages: &[Message],
    project_context: &str,
    visibility: Visibility,
) -> Result<(), IronclawError> {
    if new_messages.is_empty() {
        return Ok(());
    }
    let mut existing = load_recent_messages(path).await?;
    let now = Utc::now();
    existing.extend(new_messages.iter().map(|msg| RecentMessage {
        message: msg.clone(),
        timestamp: now,
        project_context: project_context.to_string(),
        visibility: visibility.clone(),
    }));
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

    fn sample_message(content: &str) -> Message {
        Message::user(content)
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
        append_recent_messages(&path, &msgs, "test/project", Visibility::User)
            .await
            .unwrap();

        let loaded = load_recent_messages(&path).await.unwrap();
        assert_eq!(loaded.len(), 2, "should load two messages");
        assert_eq!(
            loaded.first().map(|m| m.message.content.as_str()),
            Some("hello"),
            "first message content should match"
        );
        assert_eq!(
            loaded.first().map(|m| m.project_context.as_str()),
            Some("test/project"),
            "project_context should be preserved"
        );
        assert_eq!(
            loaded.first().map(|m| &m.visibility),
            Some(&Visibility::User),
            "visibility should be preserved"
        );
    }

    #[tokio::test]
    async fn append_to_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recent_messages.json");

        append_recent_messages(&path, &[sample_message("first")], "ctx", Visibility::User)
            .await
            .unwrap();

        let loaded = load_recent_messages(&path).await.unwrap();
        assert_eq!(loaded.len(), 1, "should have one message");
    }

    #[tokio::test]
    async fn append_preserves_existing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recent_messages.json");

        append_recent_messages(&path, &[sample_message("first")], "ctx", Visibility::User)
            .await
            .unwrap();
        append_recent_messages(
            &path,
            &[sample_message("second")],
            "ctx",
            Visibility::Background,
        )
        .await
        .unwrap();

        let loaded = load_recent_messages(&path).await.unwrap();
        assert_eq!(loaded.len(), 2, "should have two messages");
        assert_eq!(
            loaded.get(1).map(|m| &m.visibility),
            Some(&Visibility::Background),
            "second message should have Background visibility"
        );
    }

    #[tokio::test]
    async fn clear_empties_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recent_messages.json");

        append_recent_messages(&path, &[sample_message("first")], "ctx", Visibility::User)
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

        append_recent_messages(&path, &[], "ctx", Visibility::User)
            .await
            .unwrap();
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

    #[tokio::test]
    async fn load_messages_for_agent_strips_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recent_messages.json");

        append_recent_messages(&path, &[sample_message("hello")], "ctx", Visibility::User)
            .await
            .unwrap();

        let messages = load_messages_for_agent(&path).await.unwrap();
        assert_eq!(messages.len(), 1, "should return one message");
        assert_eq!(
            messages.first().map(|m| m.content.as_str()),
            Some("hello"),
            "message content should be preserved"
        );
    }

    #[tokio::test]
    async fn background_visibility_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recent_messages.json");

        append_recent_messages(
            &path,
            &[sample_message("system event")],
            "pulse",
            Visibility::Background,
        )
        .await
        .unwrap();

        let loaded = load_recent_messages(&path).await.unwrap();
        assert_eq!(
            loaded.first().map(|m| &m.visibility),
            Some(&Visibility::Background),
            "Background visibility should round-trip"
        );
    }
}
