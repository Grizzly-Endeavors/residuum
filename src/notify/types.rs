//! Notification types and channel registry.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::Deserialize;

/// Well-known built-in channel names with compile-time distinction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinChannel {
    /// Inject into agent feed, start a turn if idle.
    AgentWake,
    /// Inject into agent feed, wait for next interaction.
    AgentFeed,
    /// Store as an `InboxItem`, surface as unread count.
    Inbox,
}

impl BuiltinChannel {
    /// String representation for serialization and display.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AgentWake => "agent_wake",
            Self::AgentFeed => "agent_feed",
            Self::Inbox => "inbox",
        }
    }

    /// Parse a string into a built-in channel, if it matches.
    #[must_use]
    pub fn parse_name(s: &str) -> Option<Self> {
        match s {
            "agent_wake" => Some(Self::AgentWake),
            "agent_feed" => Some(Self::AgentFeed),
            "inbox" => Some(Self::Inbox),
            _ => None,
        }
    }
}

/// A parsed channel target: either a built-in channel or a named external channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelTarget {
    /// A well-known built-in channel.
    Builtin(BuiltinChannel),
    /// A named external channel (ntfy, webhook, etc.).
    External(String),
}

impl ChannelTarget {
    /// Parse a channel name into a `ChannelTarget`.
    ///
    /// Tries `BuiltinChannel::parse_name` first, falls back to `External`.
    #[must_use]
    pub fn parse(name: &str) -> Self {
        match BuiltinChannel::parse_name(name) {
            Some(builtin) => Self::Builtin(builtin),
            None => Self::External(name.to_string()),
        }
    }
}

/// Parse a list of channel name strings into `ChannelTarget` values.
#[must_use]
pub fn parse_channel_list(names: &[String]) -> Vec<ChannelTarget> {
    names.iter().map(|n| ChannelTarget::parse(n)).collect()
}

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

/// A notification to be routed to channels.
pub struct Notification {
    /// Task name for identification.
    pub task_name: String,
    /// Human-readable summary of the result.
    pub summary: String,
    /// Where the task originated.
    pub source: TaskSource,
    /// When the notification was created.
    pub timestamp: DateTime<Utc>,
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
}

/// Outcome of routing a notification through channels.
#[derive(Debug, Default)]
pub struct RouteOutcome {
    /// Whether the result should wake the agent (start a turn if idle).
    pub agent_wake: bool,
    /// Whether the result should be passively fed to the agent.
    pub agent_feed: bool,
    /// Whether the result should be saved to the inbox.
    pub inbox: bool,
    /// Names of external channels that were dispatched to.
    pub external_dispatched: Vec<String>,
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "test code uses indexing for clarity"
)]
mod tests {
    use super::*;

    #[test]
    fn builtin_channel_roundtrip() {
        for builtin in [
            BuiltinChannel::AgentWake,
            BuiltinChannel::AgentFeed,
            BuiltinChannel::Inbox,
        ] {
            let s = builtin.as_str();
            assert_eq!(
                BuiltinChannel::parse_name(s),
                Some(builtin),
                "roundtrip failed for {s}"
            );
        }
    }

    #[test]
    fn builtin_channel_from_str_unknown() {
        assert_eq!(BuiltinChannel::parse_name("ntfy"), None);
        assert_eq!(BuiltinChannel::parse_name(""), None);
    }

    #[test]
    fn channel_target_parse_builtin() {
        assert_eq!(
            ChannelTarget::parse("agent_wake"),
            ChannelTarget::Builtin(BuiltinChannel::AgentWake)
        );
        assert_eq!(
            ChannelTarget::parse("inbox"),
            ChannelTarget::Builtin(BuiltinChannel::Inbox)
        );
    }

    #[test]
    fn channel_target_parse_external() {
        assert_eq!(
            ChannelTarget::parse("ntfy"),
            ChannelTarget::External("ntfy".to_string())
        );
        assert_eq!(
            ChannelTarget::parse("my_webhook"),
            ChannelTarget::External("my_webhook".to_string())
        );
    }

    #[test]
    fn parse_channel_list_mixed() {
        let names: Vec<String> = vec![
            "agent_wake".to_string(),
            "ntfy".to_string(),
            "inbox".to_string(),
        ];
        let targets = parse_channel_list(&names);
        assert_eq!(targets.len(), 3);
        assert_eq!(
            targets[0],
            ChannelTarget::Builtin(BuiltinChannel::AgentWake)
        );
        assert_eq!(targets[1], ChannelTarget::External("ntfy".to_string()));
        assert_eq!(targets[2], ChannelTarget::Builtin(BuiltinChannel::Inbox));
    }

    #[test]
    fn parse_channel_list_empty() {
        let targets = parse_channel_list(&[]);
        assert!(targets.is_empty());
    }

    #[test]
    fn channels_for_task_finds_matches() {
        let mut map = HashMap::new();
        map.insert(
            "agent_feed".to_string(),
            vec!["email_check".to_string(), "deploy_check".to_string()],
        );
        map.insert("inbox".to_string(), vec!["backup".to_string()]);
        map.insert(
            "ntfy".to_string(),
            vec!["email_check".to_string(), "backup".to_string()],
        );
        let cfg = ChannelsConfig(map);

        let mut channels = cfg.channels_for_task("email_check");
        channels.sort_unstable();
        assert_eq!(channels, vec!["agent_feed", "ntfy"]);
    }

    #[test]
    fn channels_for_task_no_matches() {
        let mut map = HashMap::new();
        map.insert("agent_feed".to_string(), vec!["email_check".to_string()]);
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
