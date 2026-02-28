//! Notification types and channel registry.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
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
mod tests {
    use super::*;

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
