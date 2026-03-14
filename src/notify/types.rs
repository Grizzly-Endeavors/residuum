//! Notification types and channel registry.

use std::collections::HashMap;

use serde::Deserialize;

/// Channel registry loaded from CHANNELS.yml.
///
/// Maps channel names to lists of task names that should be routed there.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ChannelsConfig(pub HashMap<String, Vec<String>>);

impl ChannelsConfig {
    /// Return all channel names that list the given task name.
    #[must_use]
    pub fn channels_for_task(&self, task_name: &str) -> Vec<&str> {
        self.0
            .iter()
            .filter(|(_, tasks)| tasks.iter().any(|t| t == task_name))
            .map(|(channel, _)| channel.as_str())
            .collect()
    }
}

/// Where the background task originated.
#[derive(Debug, Clone, Copy)]
pub enum TaskSource {
    /// Result from a pulse check (HEARTBEAT.yml).
    Pulse,
    /// Result from a scheduled action.
    Action,
    /// Result from an agent-spawned background task.
    Agent,
}

impl TaskSource {
    /// Lowercase label for display and serialization.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pulse => "pulse",
            Self::Action => "action",
            Self::Agent => "agent",
        }
    }
}

/// A single resolved external channel configuration.
#[derive(Debug, Clone)]
pub struct ExternalChannelConfig {
    /// Channel name (key from channel definitions).
    pub name: String,
    /// Channel type and type-specific settings.
    pub kind: ExternalChannelKind,
}

/// Channel type with type-specific configuration.
#[derive(Debug, Clone)]
pub enum ExternalChannelKind {
    /// Ntfy push notification channel.
    Ntfy {
        /// Ntfy server URL.
        url: String,
        /// Topic to publish to.
        topic: String,
        /// Message priority (default: `"default"`).
        priority: Option<String>,
    },
    /// Webhook HTTP channel.
    Webhook {
        /// Endpoint URL.
        url: String,
        /// HTTP method (default: `"POST"`).
        method: Option<String>,
        /// Additional headers.
        headers: Vec<(String, String)>,
    },
    /// Native macOS notification channel.
    Macos {
        /// Default notification category.
        default_category: Option<String>,
        /// Default interruption level.
        default_priority: Option<String>,
        /// Throttle window in seconds.
        throttle_window_secs: Option<u64>,
        /// Play notification sound.
        sound: Option<bool>,
        /// Display name in banners.
        app_name: Option<String>,
        /// Base URL for "Open" action.
        web_url: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_for_task_finds_matches() {
        let mut map = HashMap::new();
        map.insert(
            "ntfy".to_string(),
            vec!["email_check".to_string(), "deploy_check".to_string()],
        );
        map.insert("inbox".to_string(), vec!["backup".to_string()]);
        map.insert(
            "webhook".to_string(),
            vec!["email_check".to_string(), "backup".to_string()],
        );
        let cfg = ChannelsConfig(map);

        let mut channels = cfg.channels_for_task("email_check");
        channels.sort_unstable();
        assert_eq!(channels, vec!["ntfy", "webhook"]);
    }

    #[test]
    fn channels_for_task_no_matches() {
        let mut map = HashMap::new();
        map.insert("ntfy".to_string(), vec!["email_check".to_string()]);
        let cfg = ChannelsConfig(map);

        let channels = cfg.channels_for_task("unknown_task");
        assert!(channels.is_empty(), "unrouted task should return empty");
    }

    #[test]
    fn empty_config_returns_no_channels() {
        let cfg = ChannelsConfig::default();
        let channels = cfg.channels_for_task("anything");
        assert!(channels.is_empty());
    }
}
