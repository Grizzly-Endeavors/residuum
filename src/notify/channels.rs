//! Notification channel trait and built-in channel types.

use std::path::PathBuf;

use async_trait::async_trait;

use crate::bus::NotificationEvent;
use crate::inbox;

/// A channel that can deliver notifications.
#[async_trait]
pub trait NotificationChannel: Send + Sync {
    /// Channel name as configured.
    fn name(&self) -> &str;

    /// Channel type identifier (e.g. `"ntfy"`, `"webhook"`, `"inbox"`).
    fn channel_kind(&self) -> &'static str;

    /// Deliver a notification.
    ///
    /// Errors are logged, not propagated — a failed channel should not block
    /// other deliveries.
    async fn deliver(&self, notification: &NotificationEvent) -> anyhow::Result<()>;
}

/// Inbox channel: creates an `InboxItem` from the notification.
///
/// This is always a singleton — exactly one inbox channel exists per
/// Residuum instance, and its channel name is always `"inbox"`.
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
}

#[async_trait]
impl NotificationChannel for InboxChannel {
    fn name(&self) -> &'static str {
        "inbox"
    }

    fn channel_kind(&self) -> &'static str {
        "inbox"
    }

    async fn deliver(&self, notification: &NotificationEvent) -> anyhow::Result<()> {
        let source_label = format!("{}:{}", notification.source.as_str(), notification.title);

        let now = crate::time::now_local(self.tz);
        let event_time = notification
            .timestamp
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        let body = format!("[Originally at {event_time}]\n{}", notification.content);
        let item = inbox::InboxItem {
            title: notification.title.clone(),
            body,
            source: source_label,
            timestamp: now,
            read: false,
            attachments: Vec::new(),
        };

        let filename = inbox::generate_filename(&notification.title, now);
        inbox::save_item(&self.inbox_dir, &filename, &item).await?;

        Ok(())
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use crate::bus::EventTrigger;

    fn make_notification() -> NotificationEvent {
        NotificationEvent {
            title: "test_task".to_string(),
            content: "Something happened".to_string(),
            source: EventTrigger::Pulse,
            timestamp: chrono::NaiveDate::from_ymd_opt(2026, 3, 14)
                .unwrap()
                .and_hms_opt(12, 0, 0)
                .unwrap(),
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
        assert!(
            item.body.contains("[Originally at 2026-03-14 12:00:00]"),
            "body should contain event timestamp"
        );
        assert!(
            item.body.contains("Something happened"),
            "body should contain original content"
        );
        assert!(item.source.starts_with("pulse:"));
        assert!(!item.read);
    }

    #[test]
    fn inbox_channel_name() {
        let channel = InboxChannel::new("/tmp/inbox", chrono_tz::UTC);
        assert_eq!(channel.name(), "inbox");
    }
}
