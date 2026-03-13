//! Notification router: dispatches to channels.

use std::collections::HashMap;

use super::channels::{InboxChannel, NotificationChannel};
use super::types::{BuiltinChannel, ChannelTarget, Notification, RouteOutcome};

/// Routes notifications to configured channels.
///
/// Holds external channel implementations (ntfy, webhook). Built-in channels
/// (`agent_wake`, `agent_feed`) are signaled via flags on `RouteOutcome` — the
/// gateway acts on those flags. Inbox delivery is handled directly by the router.
pub struct NotificationRouter {
    external_channels: tokio::sync::RwLock<HashMap<String, Box<dyn NotificationChannel>>>,
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
            external_channels: tokio::sync::RwLock::new(external_channels),
            inbox_channel,
        }
    }

    /// Create an empty router with no channels.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            external_channels: tokio::sync::RwLock::new(HashMap::new()),
            inbox_channel: None,
        }
    }

    /// Replace external channels in-place (e.g. after a config reload).
    pub async fn reload_channels(
        &self,
        new_channels: HashMap<String, Box<dyn NotificationChannel>>,
    ) {
        let mut guard = self.external_channels.write().await;
        let old_count = guard.len();
        *guard = new_channels;
        tracing::info!(
            old_count,
            new_count = guard.len(),
            "notification channels reloaded"
        );
    }

    /// Deliver a notification directly to the inbox channel, bypassing routing.
    ///
    /// Returns `Ok(true)` if delivery succeeded, `Ok(false)` if no inbox is configured,
    /// or `Err` if delivery failed.
    ///
    /// # Errors
    ///
    /// Returns an error if the inbox channel is configured but delivery fails.
    pub async fn deliver_to_inbox(&self, notification: &Notification) -> anyhow::Result<bool> {
        if let Some(ref inbox) = self.inbox_channel {
            inbox.deliver(notification).await.map_err(|e| {
                tracing::warn!(
                    task = %notification.task_name,
                    error = %e,
                    "direct inbox delivery failed"
                );
                e
            })?;
            Ok(true)
        } else {
            tracing::warn!(
                task = %notification.task_name,
                "direct inbox delivery requested but no inbox configured"
            );
            Ok(false)
        }
    }

    /// Route a notification to the given channels.
    ///
    /// Resolves which channels to dispatch to, then:
    /// - Sets `agent_wake`/`agent_feed` flags on the outcome
    /// - Delivers to inbox directly
    /// - Dispatches to external channels in sequence (errors logged, not propagated)
    pub async fn route(
        &self,
        notification: &Notification,
        channels: &[ChannelTarget],
    ) -> RouteOutcome {
        let mut outcome = RouteOutcome::default();

        if channels.is_empty() {
            tracing::warn!(
                task = %notification.task_name,
                "notification routed to zero channels"
            );
            outcome.unrouted = true;
            return outcome;
        }

        for target in channels {
            match target {
                ChannelTarget::Builtin(BuiltinChannel::AgentWake) => outcome.agent_wake = true,
                ChannelTarget::Builtin(BuiltinChannel::AgentFeed) => outcome.agent_feed = true,
                ChannelTarget::Builtin(BuiltinChannel::Inbox) => {
                    if let Some(ref inbox) = self.inbox_channel {
                        match inbox.deliver(notification).await {
                            Ok(()) => outcome.inbox = true,
                            Err(e) => {
                                tracing::warn!(
                                    channel = "inbox",
                                    task = %notification.task_name,
                                    error = %e,
                                    "inbox delivery failed"
                                );
                            }
                        }
                    } else {
                        tracing::warn!(
                            task = %notification.task_name,
                            "inbox channel referenced but no inbox configured"
                        );
                    }
                }
                ChannelTarget::External(ext_name) => {
                    let guard = self.external_channels.read().await;
                    if let Some(channel) = guard.get(ext_name.as_str()) {
                        match channel.deliver(notification).await {
                            Ok(()) => {
                                outcome.external_dispatched.push(ext_name.clone());
                            }
                            Err(e) => {
                                tracing::warn!(
                                    channel = %ext_name,
                                    task = %notification.task_name,
                                    error = %e,
                                    "external channel delivery failed"
                                );
                            }
                        }
                    } else {
                        tracing::warn!(
                            channel = %ext_name,
                            task = %notification.task_name,
                            "unknown channel, skipping"
                        );
                    }
                }
            }
        }

        outcome
    }

    /// Deliver a notification to a specific external channel by name.
    ///
    /// Unlike `route()`, this propagates errors so the caller can report success/failure.
    ///
    /// # Errors
    /// Returns an error if the channel is not found or delivery fails.
    pub async fn deliver_to_external(
        &self,
        channel_name: &str,
        notification: &Notification,
    ) -> anyhow::Result<()> {
        let guard = self.external_channels.read().await;
        let channel = guard
            .get(channel_name)
            .ok_or_else(|| anyhow::anyhow!("unknown external channel: {channel_name}"))?;
        channel.deliver(notification).await
    }

    /// Check if a named external channel is configured.
    pub async fn has_external_channel(&self, name: &str) -> bool {
        self.external_channels.read().await.contains_key(name)
    }

    /// List the names of all configured external channels.
    pub async fn external_channel_names(&self) -> Vec<String> {
        self.external_channels
            .read()
            .await
            .keys()
            .cloned()
            .collect()
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
        let router = NotificationRouter::empty();
        let notif = make_notification("my_task");
        let channels = vec![ChannelTarget::Builtin(BuiltinChannel::AgentWake)];
        let outcome = router.route(&notif, &channels).await;

        assert!(outcome.agent_wake, "should set agent_wake flag");
        assert!(!outcome.agent_feed);
        assert!(!outcome.inbox);
    }

    #[tokio::test]
    async fn route_agent_feed_sets_flag() {
        let router = NotificationRouter::empty();
        let notif = make_notification("my_task");
        let channels = vec![ChannelTarget::Builtin(BuiltinChannel::AgentFeed)];
        let outcome = router.route(&notif, &channels).await;

        assert!(!outcome.agent_wake);
        assert!(outcome.agent_feed, "should set agent_feed flag");
    }

    #[tokio::test]
    async fn route_inbox_delivers_item() {
        let dir = tempfile::tempdir().unwrap();
        let inbox_dir = dir.path().join("inbox");
        std::fs::create_dir_all(&inbox_dir).unwrap();

        let inbox_channel = InboxChannel::new(&inbox_dir, chrono_tz::UTC);
        let router = NotificationRouter::new(HashMap::new(), Some(inbox_channel));

        let notif = make_notification("backup_task");
        let channels = vec![ChannelTarget::Builtin(BuiltinChannel::Inbox)];
        let outcome = router.route(&notif, &channels).await;

        assert!(outcome.inbox, "should set inbox flag");

        // Verify the inbox item was written
        let items: Vec<_> = std::fs::read_dir(&inbox_dir)
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(items.len(), 1, "should create one inbox item");
    }

    /// When channels is empty, `route()` returns an outcome with `unrouted = true`
    /// and emits a `tracing::warn` to surface the misconfiguration.
    #[tokio::test]
    async fn route_unrouted_task_returns_empty() {
        let router = NotificationRouter::empty();
        let notif = make_notification("unknown_task");
        let channels: Vec<ChannelTarget> = vec![];
        let outcome = router.route(&notif, &channels).await;

        assert!(!outcome.agent_wake);
        assert!(!outcome.agent_feed);
        assert!(!outcome.inbox);
        assert!(outcome.external_dispatched.is_empty());
        assert!(
            outcome.unrouted,
            "should set unrouted flag when channels is empty"
        );
    }

    #[tokio::test]
    async fn route_multiple_channels() {
        let router = NotificationRouter::empty();
        let notif = make_notification("my_task");
        let channels = vec![
            ChannelTarget::Builtin(BuiltinChannel::AgentWake),
            ChannelTarget::Builtin(BuiltinChannel::AgentFeed),
        ];
        let outcome = router.route(&notif, &channels).await;

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
        let result = router.deliver_to_inbox(&notif).await;
        assert!(
            matches!(result, Ok(true)),
            "should return Ok(true) when inbox is configured and delivery succeeds"
        );

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
        let result = router.deliver_to_inbox(&notif).await;
        assert!(
            matches!(result, Ok(false)),
            "should return Ok(false) when no inbox is configured"
        );
    }

    #[tokio::test]
    async fn route_unknown_external_channel_logged_not_error() {
        let router = NotificationRouter::empty();
        let notif = make_notification("my_task");
        let channels = vec![ChannelTarget::External("nonexistent_channel".to_string())];
        let outcome = router.route(&notif, &channels).await;

        // Should not panic and should return empty
        assert!(outcome.external_dispatched.is_empty());
    }

    #[tokio::test]
    async fn has_external_channel_empty_router() {
        let router = NotificationRouter::empty();
        assert!(!router.has_external_channel("ntfy").await);
    }

    #[tokio::test]
    async fn external_channel_names_empty() {
        let router = NotificationRouter::empty();
        assert!(router.external_channel_names().await.is_empty());
    }

    #[tokio::test]
    async fn reload_channels_replaces_map() {
        let router = NotificationRouter::empty();
        assert!(
            router.external_channel_names().await.is_empty(),
            "should start empty"
        );

        // Create a mock channel map (using InboxChannel via trait object)
        let dir = tempfile::tempdir().unwrap();
        let inbox_dir = dir.path().join("inbox");
        std::fs::create_dir_all(&inbox_dir).unwrap();

        let mut channels: HashMap<String, Box<dyn NotificationChannel>> = HashMap::new();
        channels.insert(
            "test-inbox".to_string(),
            Box::new(InboxChannel::new(&inbox_dir, chrono_tz::UTC)),
        );

        router.reload_channels(channels).await;

        assert!(router.has_external_channel("test-inbox").await);
        assert_eq!(router.external_channel_names().await.len(), 1);

        // Replace with empty map
        router.reload_channels(HashMap::new()).await;
        assert!(router.external_channel_names().await.is_empty());
    }
}
