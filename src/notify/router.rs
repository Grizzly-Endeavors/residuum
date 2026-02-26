//! Notification router: loads NOTIFY.yml and dispatches to channels.

use std::collections::HashMap;
use std::path::Path;

use super::channels::{InboxChannel, NotificationChannel};
use super::loader::load_notify_config;
use super::types::{Notification, RouteOutcome};

/// Well-known built-in channel names.
const AGENT_WAKE: &str = "agent_wake";
const AGENT_FEED: &str = "agent_feed";
const INBOX: &str = "inbox";

/// Routes notifications to configured channels based on NOTIFY.yml.
///
/// Holds external channel implementations (ntfy, webhook). Built-in channels
/// (`agent_wake`, `agent_feed`) are signaled via flags on `RouteOutcome` — the
/// gateway acts on those flags. Inbox delivery is handled directly by the router.
pub struct NotificationRouter {
    external_channels: HashMap<String, Box<dyn NotificationChannel>>,
    inbox_channel: Option<InboxChannel>,
}

impl NotificationRouter {
    /// Create a new router with external channels and an inbox channel.
    #[must_use]
    pub fn new(
        external_channels: HashMap<String, Box<dyn NotificationChannel>>,
        inbox_channel: Option<InboxChannel>,
    ) -> Self {
        Self {
            external_channels,
            inbox_channel,
        }
    }

    /// Create an empty router with no channels.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            external_channels: HashMap::new(),
            inbox_channel: None,
        }
    }

    /// Deliver a notification directly to the inbox channel, bypassing NOTIFY.yml.
    ///
    /// Returns `true` if delivery succeeded, `false` if no inbox is configured
    /// or delivery failed.
    pub async fn deliver_to_inbox(&self, notification: &Notification) -> bool {
        if let Some(ref inbox) = self.inbox_channel {
            if let Err(e) = inbox.deliver(notification).await {
                tracing::warn!(
                    task = %notification.task_name,
                    error = %e,
                    "direct inbox delivery failed"
                );
                return false;
            }
            true
        } else {
            tracing::warn!(
                task = %notification.task_name,
                "direct inbox delivery requested but no inbox configured"
            );
            false
        }
    }

    /// Route a notification based on NOTIFY.yml.
    ///
    /// Loads NOTIFY.yml fresh each call (hot-reload pattern). Resolves which
    /// channels list this notification's task name, then:
    /// - Sets `agent_wake`/`agent_feed` flags on the outcome
    /// - Delivers to inbox directly
    /// - Dispatches to external channels in sequence (errors logged, not propagated)
    pub async fn route(&self, notification: &Notification, notify_path: &Path) -> RouteOutcome {
        let config = match load_notify_config(notify_path) {
            Ok(cfg) => cfg,
            Err(e) => {
                tracing::warn!(error = %e, "failed to load NOTIFY.yml, skipping routing");
                return RouteOutcome::default();
            }
        };

        let channels = config.channels_for_task(&notification.task_name);
        let mut outcome = RouteOutcome::default();

        for channel_name in channels {
            match channel_name {
                AGENT_WAKE => outcome.agent_wake = true,
                AGENT_FEED => outcome.agent_feed = true,
                INBOX => {
                    outcome.inbox = true;
                    if let Some(ref inbox) = self.inbox_channel {
                        if let Err(e) = inbox.deliver(notification).await {
                            tracing::warn!(
                                channel = "inbox",
                                task = %notification.task_name,
                                error = %e,
                                "inbox delivery failed"
                            );
                        }
                    } else {
                        tracing::warn!(
                            task = %notification.task_name,
                            "inbox channel referenced in NOTIFY.yml but no inbox configured"
                        );
                    }
                }
                ext_name => {
                    if let Some(channel) = self.external_channels.get(ext_name) {
                        match channel.deliver(notification).await {
                            Ok(()) => {
                                outcome.external_dispatched.push(ext_name.to_string());
                            }
                            Err(e) => {
                                tracing::warn!(
                                    channel = ext_name,
                                    task = %notification.task_name,
                                    error = %e,
                                    "external channel delivery failed"
                                );
                            }
                        }
                    } else {
                        tracing::warn!(
                            channel = ext_name,
                            task = %notification.task_name,
                            "unknown channel in NOTIFY.yml, skipping"
                        );
                    }
                }
            }
        }

        outcome
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use crate::notify::types::TaskSource;

    fn make_notification(task_name: &str) -> Notification {
        Notification {
            task_name: task_name.to_string(),
            summary: "test summary".to_string(),
            source: TaskSource::Pulse,
            timestamp: chrono::Utc::now(),
        }
    }

    #[tokio::test]
    async fn route_agent_wake_sets_flag() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("NOTIFY.yml");
        std::fs::write(&path, "agent_wake:\n  - my_task\n").unwrap();

        let router = NotificationRouter::empty();
        let notif = make_notification("my_task");
        let outcome = router.route(&notif, &path).await;

        assert!(outcome.agent_wake, "should set agent_wake flag");
        assert!(!outcome.agent_feed);
        assert!(!outcome.inbox);
    }

    #[tokio::test]
    async fn route_agent_feed_sets_flag() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("NOTIFY.yml");
        std::fs::write(&path, "agent_feed:\n  - my_task\n").unwrap();

        let router = NotificationRouter::empty();
        let notif = make_notification("my_task");
        let outcome = router.route(&notif, &path).await;

        assert!(!outcome.agent_wake);
        assert!(outcome.agent_feed, "should set agent_feed flag");
    }

    #[tokio::test]
    async fn route_inbox_delivers_item() {
        let dir = tempfile::tempdir().unwrap();
        let notify_path = dir.path().join("NOTIFY.yml");
        std::fs::write(&notify_path, "inbox:\n  - backup_task\n").unwrap();

        let inbox_dir = dir.path().join("inbox");
        std::fs::create_dir_all(&inbox_dir).unwrap();

        let inbox_channel = InboxChannel::new(&inbox_dir, chrono_tz::UTC);
        let router = NotificationRouter::new(HashMap::new(), Some(inbox_channel));

        let notif = make_notification("backup_task");
        let outcome = router.route(&notif, &notify_path).await;

        assert!(outcome.inbox, "should set inbox flag");

        // Verify the inbox item was written
        let items: Vec<_> = std::fs::read_dir(&inbox_dir)
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(items.len(), 1, "should create one inbox item");
    }

    #[tokio::test]
    async fn route_unrouted_task_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("NOTIFY.yml");
        std::fs::write(&path, "agent_feed:\n  - other_task\n").unwrap();

        let router = NotificationRouter::empty();
        let notif = make_notification("unknown_task");
        let outcome = router.route(&notif, &path).await;

        assert!(!outcome.agent_wake);
        assert!(!outcome.agent_feed);
        assert!(!outcome.inbox);
        assert!(outcome.external_dispatched.is_empty());
    }

    #[tokio::test]
    async fn route_missing_notify_yml_returns_default() {
        let path = std::path::Path::new("/tmp/nonexistent_notify_route_test.yml");
        let router = NotificationRouter::empty();
        let notif = make_notification("any_task");
        let outcome = router.route(&notif, path).await;

        assert!(!outcome.agent_wake);
        assert!(!outcome.agent_feed);
        assert!(!outcome.inbox);
    }

    #[tokio::test]
    async fn route_multiple_channels() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("NOTIFY.yml");
        std::fs::write(
            &path,
            "agent_wake:\n  - my_task\nagent_feed:\n  - my_task\n",
        )
        .unwrap();

        let router = NotificationRouter::empty();
        let notif = make_notification("my_task");
        let outcome = router.route(&notif, &path).await;

        assert!(outcome.agent_wake);
        assert!(outcome.agent_feed);
    }

    #[tokio::test]
    async fn deliver_to_inbox_with_channel() {
        let dir = tempfile::tempdir().unwrap();
        let inbox_dir = dir.path().join("inbox");
        std::fs::create_dir_all(&inbox_dir).unwrap();

        let inbox_channel = InboxChannel::new(&inbox_dir, chrono_tz::UTC);
        let router = NotificationRouter::new(HashMap::new(), Some(inbox_channel));

        let notif = make_notification("direct_task");
        let ok = router.deliver_to_inbox(&notif).await;
        assert!(ok, "should succeed when inbox is configured");

        let items: Vec<_> = std::fs::read_dir(&inbox_dir)
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(items.len(), 1, "should create one inbox item");
    }

    #[tokio::test]
    async fn deliver_to_inbox_without_channel() {
        let router = NotificationRouter::empty();
        let notif = make_notification("orphan_task");
        let ok = router.deliver_to_inbox(&notif).await;
        assert!(!ok, "should return false when no inbox is configured");
    }

    #[tokio::test]
    async fn route_unknown_external_channel_logged_not_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("NOTIFY.yml");
        std::fs::write(&path, "nonexistent_channel:\n  - my_task\n").unwrap();

        let router = NotificationRouter::empty();
        let notif = make_notification("my_task");
        let outcome = router.route(&notif, &path).await;

        // Should not panic and should return empty
        assert!(outcome.external_dispatched.is_empty());
    }
}
