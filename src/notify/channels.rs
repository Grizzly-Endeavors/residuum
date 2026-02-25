//! Notification channel trait and built-in channel types.

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::inbox;

use super::types::Notification;

/// A channel that can deliver notifications.
#[async_trait]
pub trait NotificationChannel: Send + Sync {
    /// Channel name as configured.
    fn name(&self) -> &str;

    /// Deliver a notification.
    ///
    /// Errors are logged, not propagated — a failed channel should not block
    /// other deliveries.
    async fn deliver(&self, notification: &Notification) -> anyhow::Result<()>;
}

/// Inbox channel: creates an `InboxItem` from the notification.
pub struct InboxChannel {
    inbox_dir: PathBuf,
    tz: chrono_tz::Tz,
}

impl InboxChannel {
    /// Create a new inbox channel.
    #[must_use]
    pub fn new(inbox_dir: impl Into<PathBuf>, tz: chrono_tz::Tz) -> Self {
        Self {
            inbox_dir: inbox_dir.into(),
            tz,
        }
    }

    /// Inbox directory path.
    #[must_use]
    pub fn inbox_dir(&self) -> &Path {
        &self.inbox_dir
    }
}

#[async_trait]
impl NotificationChannel for InboxChannel {
    fn name(&self) -> &'static str {
        "inbox"
    }

    async fn deliver(&self, notification: &Notification) -> anyhow::Result<()> {
        let source_label = match notification.source {
            super::types::TaskSource::Pulse => format!("pulse:{}", notification.task_name),
            super::types::TaskSource::Cron => format!("cron:{}", notification.task_name),
        };

        let now = crate::time::now_local(self.tz);
        let item = inbox::InboxItem {
            title: notification.task_name.clone(),
            body: notification.summary.clone(),
            source: source_label,
            timestamp: now,
            read: false,
            attachments: Vec::new(),
        };

        let filename = inbox::generate_filename(&notification.task_name, self.tz);
        inbox::save_item(&self.inbox_dir, &filename, &item).await?;

        Ok(())
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use crate::notify::types::TaskSource;

    fn make_notification() -> Notification {
        Notification {
            task_name: "test_task".to_string(),
            summary: "Something happened".to_string(),
            source: TaskSource::Pulse,
            timestamp: chrono::Utc::now(),
        }
    }

    #[tokio::test]
    async fn inbox_channel_creates_item() {
        let dir = tempfile::tempdir().unwrap();
        let channel = InboxChannel::new(dir.path(), chrono_tz::UTC);

        let notif = make_notification();
        channel.deliver(&notif).await.unwrap();

        // Verify a JSON file was created in the directory
        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
            .collect();
        assert_eq!(entries.len(), 1, "should create one inbox item");

        let first_entry = entries.first().unwrap();
        let item = inbox::load_item(&first_entry.path()).await.unwrap();
        assert_eq!(item.title, "test_task");
        assert_eq!(item.body, "Something happened");
        assert!(item.source.starts_with("pulse:"));
        assert!(!item.read);
    }

    #[test]
    fn inbox_channel_name() {
        let channel = InboxChannel::new("/tmp/inbox", chrono_tz::UTC);
        assert_eq!(channel.name(), "inbox");
    }
}
