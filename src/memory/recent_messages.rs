//! Persistence for recent (unobserved) messages across restarts.
//!
//! Messages accumulate in `recent_messages.json` until the observer
//! threshold is reached and an episode is created, at which point the
//! messages that episode covered are removed from the file. Messages
//! appended while the observer was running stay for the next cycle.

use std::path::Path;

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

use anyhow::Context;

use crate::inference::Message;
use crate::memory::types::Visibility;

/// A persisted message with observation metadata.
///
/// Wraps a [`Message`] with the context needed for the observer to derive
/// observation metadata (visibility) without requiring the agent to
/// re-examine the conversation on startup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentMessage {
    /// The underlying conversation message.
    #[serde(flatten)]
    pub message: Message,
    /// When this message was recorded.
    #[serde(with = "crate::time::minute_format")]
    pub timestamp: NaiveDateTime,
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
pub async fn load_recent_messages(path: &Path) -> anyhow::Result<Vec<RecentMessage>> {
    match tokio::fs::read_to_string(path).await {
        Ok(contents) if contents.trim().is_empty() => Ok(Vec::new()),
        Ok(contents) => serde_json::from_str(&contents)
            .with_context(|| format!("failed to parse recent messages at {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(anyhow::Error::new(e).context(format!(
            "failed to read recent messages at {}",
            path.display()
        ))),
    }
}

/// Result of loading recent messages for agent restore.
pub struct AgentRestore {
    /// Plain conversation messages (metadata stripped).
    pub messages: Vec<Message>,
    /// Timestamp of the last user-visible, user-role message (if any).
    pub last_user_message_at: Option<NaiveDateTime>,
}

/// Load recent messages for agent restore, extracting the last user timestamp.
///
/// Returns plain [`Message`] values for the agent's history, plus the
/// timestamp of the most recent user-visible user message so the agent
/// can seed its time context across restarts.
///
/// # Errors
/// Returns an error if the file exists but cannot be read or parsed.
pub async fn load_messages_for_agent(path: &Path) -> anyhow::Result<AgentRestore> {
    let recent = load_recent_messages(path).await?;

    let last_user_message_at = recent
        .iter()
        .rev()
        .find(|rm| {
            rm.message.role == crate::inference::Role::User && rm.visibility == Visibility::User
        })
        .map(|rm| rm.timestamp);

    let messages = recent.into_iter().map(|rm| rm.message).collect();

    Ok(AgentRestore {
        messages,
        last_user_message_at,
    })
}

/// Save recent messages to disk atomically (temp file + rename).
///
/// # Errors
/// Returns an error if the file cannot be written.
async fn save_recent_messages(path: &Path, messages: &[RecentMessage]) -> anyhow::Result<()> {
    let json =
        serde_json::to_string_pretty(messages).context("failed to serialize recent messages")?;

    crate::util::fs::atomic_write(path, &json).await
}

/// Serializes every read-modify-write of a recent-messages file. The main
/// loop appends after each turn while the background observer (see
/// `crate::gateway::post_turn`) removes the messages it just observed; two
/// rewrites interleaving would silently drop whichever landed first.
static REWRITE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Append messages to the recent messages file, wrapping each with metadata.
///
/// Loads existing messages, extends with new wrapped messages, and saves atomically.
/// The `tz` parameter determines the timezone used for the message timestamp.
///
/// # Errors
/// Returns an error if loading or saving fails.
pub async fn append_recent_messages(
    path: &Path,
    new_messages: &[Message],
    visibility: Visibility,
    tz: chrono_tz::Tz,
) -> anyhow::Result<()> {
    if new_messages.is_empty() {
        return Ok(());
    }
    let _guard = REWRITE_LOCK.lock().await;
    let mut existing = load_recent_messages(path).await?;
    let now = crate::time::now_local(tz);
    existing.extend(new_messages.iter().map(|msg| RecentMessage {
        message: msg.clone(),
        timestamp: now,
        visibility: visibility.clone(),
    }));
    save_recent_messages(path, &existing).await
}

/// Remove the first `observed` messages — the ones an observation cycle
/// loaded and just turned into an episode — keeping anything appended after
/// that cycle loaded the file.
///
/// # Errors
/// Returns an error if the file cannot be read or written.
pub async fn remove_observed_recent_messages(path: &Path, observed: usize) -> anyhow::Result<()> {
    let _guard = REWRITE_LOCK.lock().await;
    let mut existing = load_recent_messages(path).await?;
    existing.drain(..observed.min(existing.len()));
    save_recent_messages(path, &existing).await
}

#[cfg(test)]
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
    async fn agent_sender_round_trips_and_is_optional_on_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recent_messages.json");
        let relayed = Message::user("[Agent Message from spawned-a (spawned)]\ndone")
            .with_agent_sender(Some(crate::inference::AgentSender {
                address: "spawned-a".to_string(),
                category: "spawned".to_string(),
            }));
        append_recent_messages(&path, &[relayed], Visibility::Background, chrono_tz::UTC)
            .await
            .unwrap();
        let loaded = load_recent_messages(&path).await.unwrap();
        let sender = loaded
            .first()
            .and_then(|m| m.message.agent_sender.clone())
            .unwrap();
        assert_eq!(sender.address, "spawned-a");
        assert_eq!(sender.category, "spawned");

        // History written before the field existed still loads.
        let legacy = r#"[{"role":"user","content":"hi","timestamp":"2026-09-20T12:00","visibility":"user"}]"#;
        tokio::fs::write(&path, legacy).await.unwrap();
        let legacy_loaded = load_recent_messages(&path).await.unwrap();
        assert_eq!(
            legacy_loaded
                .first()
                .and_then(|m| m.message.agent_sender.clone()),
            None
        );
    }

    #[tokio::test]
    async fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recent_messages.json");

        let msgs = vec![sample_message("hello"), sample_message("world")];
        append_recent_messages(&path, &msgs, Visibility::User, chrono_tz::UTC)
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
            loaded.first().map(|m| &m.visibility),
            Some(&Visibility::User),
            "visibility should be preserved"
        );
    }

    #[tokio::test]
    async fn append_to_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recent_messages.json");

        append_recent_messages(
            &path,
            &[sample_message("first")],
            Visibility::User,
            chrono_tz::UTC,
        )
        .await
        .unwrap();

        let loaded = load_recent_messages(&path).await.unwrap();
        assert_eq!(loaded.len(), 1, "should have one message");
    }

    #[tokio::test]
    async fn append_preserves_existing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recent_messages.json");

        append_recent_messages(
            &path,
            &[sample_message("first")],
            Visibility::User,
            chrono_tz::UTC,
        )
        .await
        .unwrap();
        append_recent_messages(
            &path,
            &[sample_message("second")],
            Visibility::Background,
            chrono_tz::UTC,
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
    async fn remove_observed_empties_file_when_nothing_arrived_since() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recent_messages.json");

        append_recent_messages(
            &path,
            &[sample_message("first")],
            Visibility::User,
            chrono_tz::UTC,
        )
        .await
        .unwrap();
        remove_observed_recent_messages(&path, 1).await.unwrap();

        let loaded = load_recent_messages(&path).await.unwrap();
        assert!(loaded.is_empty(), "every message was observed");
    }

    #[tokio::test]
    async fn remove_observed_keeps_messages_appended_during_the_cycle() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recent_messages.json");
        append_recent_messages(
            &path,
            &[sample_message("observed-1"), sample_message("observed-2")],
            Visibility::User,
            chrono_tz::UTC,
        )
        .await
        .unwrap();
        let observed = load_recent_messages(&path).await.unwrap().len();

        // A turn ends while the observer's LLM call is in flight.
        append_recent_messages(
            &path,
            &[sample_message("arrived-later")],
            Visibility::User,
            chrono_tz::UTC,
        )
        .await
        .unwrap();
        remove_observed_recent_messages(&path, observed)
            .await
            .unwrap();

        let loaded = load_recent_messages(&path).await.unwrap();
        assert_eq!(loaded.len(), 1, "only the unobserved message remains");
        assert_eq!(loaded.first().unwrap().message.content, "arrived-later");
    }

    #[tokio::test]
    async fn concurrent_appends_and_removal_never_lose_a_message() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recent_messages.json");
        append_recent_messages(
            &path,
            &[sample_message("observed")],
            Visibility::User,
            chrono_tz::UTC,
        )
        .await
        .unwrap();

        let appends = (0..20).map(|i| {
            let path = path.clone();
            tokio::spawn(async move {
                append_recent_messages(
                    &path,
                    &[sample_message(&format!("later-{i}"))],
                    Visibility::User,
                    chrono_tz::UTC,
                )
                .await
                .unwrap();
            })
        });
        let removal = {
            let path = path.clone();
            tokio::spawn(async move { remove_observed_recent_messages(&path, 1).await.unwrap() })
        };
        let handles: Vec<_> = appends.collect();
        removal.await.unwrap();
        for h in handles {
            h.await.unwrap();
        }

        let loaded = load_recent_messages(&path).await.unwrap();
        let contents: Vec<&str> = loaded.iter().map(|m| m.message.content.as_str()).collect();
        assert_eq!(
            loaded.len(),
            20,
            "all 20 later messages survive: {contents:?}"
        );
        assert!(!contents.contains(&"observed"));
    }

    #[tokio::test]
    async fn append_empty_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recent_messages.json");

        append_recent_messages(&path, &[], Visibility::User, chrono_tz::UTC)
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

        append_recent_messages(
            &path,
            &[sample_message("hello")],
            Visibility::User,
            chrono_tz::UTC,
        )
        .await
        .unwrap();

        let restore = load_messages_for_agent(&path).await.unwrap();
        assert_eq!(restore.messages.len(), 1, "should return one message");
        assert_eq!(
            restore.messages.first().map(|m| m.content.as_str()),
            Some("hello"),
            "message content should be preserved"
        );
        assert!(
            restore.last_user_message_at.is_some(),
            "should extract last user message timestamp"
        );
    }

    #[tokio::test]
    async fn background_visibility_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recent_messages.json");

        append_recent_messages(
            &path,
            &[sample_message("system event")],
            Visibility::Background,
            chrono_tz::UTC,
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

    #[tokio::test]
    async fn last_user_timestamp_skips_background_messages() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recent_messages.json");

        // First: a real user message
        append_recent_messages(
            &path,
            &[sample_message("real user msg")],
            Visibility::User,
            chrono_tz::UTC,
        )
        .await
        .unwrap();

        // Second: a background system turn (uses user role internally)
        append_recent_messages(
            &path,
            &[sample_message("heartbeat prompt")],
            Visibility::Background,
            chrono_tz::UTC,
        )
        .await
        .unwrap();

        let restore = load_messages_for_agent(&path).await.unwrap();
        assert_eq!(restore.messages.len(), 2);
        // The timestamp should come from the first (user-visible) message,
        // not the second (background) message
        let all = load_recent_messages(&path).await.unwrap();
        let expected_ts = all.first().map(|rm| rm.timestamp);
        assert_eq!(
            restore.last_user_message_at, expected_ts,
            "should use timestamp from user-visible message, not background"
        );
    }

    #[tokio::test]
    async fn last_user_timestamp_none_when_only_background() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recent_messages.json");

        append_recent_messages(
            &path,
            &[sample_message("background event")],
            Visibility::Background,
            chrono_tz::UTC,
        )
        .await
        .unwrap();

        let restore = load_messages_for_agent(&path).await.unwrap();
        assert!(
            restore.last_user_message_at.is_none(),
            "should be None when all messages are Background"
        );
    }

    #[tokio::test]
    async fn load_recent_messages_corrupt_json_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recent_messages.json");
        tokio::fs::write(&path, "not valid json").await.unwrap();
        let result = load_recent_messages(&path).await;
        assert!(result.is_err(), "corrupt JSON should return Err");
    }
}
