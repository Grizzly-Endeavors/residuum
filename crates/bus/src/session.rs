mod events;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub use events::publish_session_event;

use crate::{ConversationTarget, EventTrigger, PulseOverlap, SessionAddress, SkillName};
use residuum_config::ModelSelector;

pub const MAIN_ADDRESS: &str = "main";

pub const MAIN_DEPTH: u32 = 0;

pub const OWNER_ADDRESS: &str = "owner";

pub const ARTIFACT_SENDER_PREFIX: &str = "artifact:";

pub const ARTIFACT_SENDER_CATEGORY: &str = "artifact";

pub const TEAMMATE_SENDER_PREFIX: &str = "agent:";

pub const TEAMMATE_SENDER_CATEGORY: &str = "teammate";

#[must_use]
pub fn artifact_sender_address(name: &str) -> SessionAddress {
    SessionAddress::from(format!("{ARTIFACT_SENDER_PREFIX}{name}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SessionCategory {
    Scheduled,
    External,
    Spawned,
    Artifact,
}

impl SessionCategory {
    #[must_use]
    pub fn from_trigger(trigger: &EventTrigger) -> Self {
        match trigger {
            EventTrigger::Pulse | EventTrigger::Action => Self::Scheduled,
            EventTrigger::Agent => Self::Spawned,
            EventTrigger::Webhook(_) | EventTrigger::Conversation => Self::External,
            EventTrigger::Artifact(_) => Self::Artifact,
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Scheduled => "scheduled",
            Self::External => "external",
            Self::Spawned => "spawned",
            Self::Artifact => "artifact",
        }
    }

    #[must_use]
    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "scheduled" => Some(Self::Scheduled),
            "external" => Some(Self::External),
            "spawned" => Some(Self::Spawned),
            "artifact" => Some(Self::Artifact),
            _ => None,
        }
    }
}

impl std::fmt::Display for SessionCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SessionState {
    Forking,
    Queued,
    Running,
    Idle,
    Completing,
    Completed,
}

impl SessionState {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Forking => "forking",
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Idle => "idle",
            Self::Completing => "completing",
            Self::Completed => "completed",
        }
    }

    #[must_use]
    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "forking" => Some(Self::Forking),
            "queued" => Some(Self::Queued),
            "running" => Some(Self::Running),
            "idle" => Some(Self::Idle),
            "completing" => Some(Self::Completing),
            "completed" => Some(Self::Completed),
            _ => None,
        }
    }
}

impl std::fmt::Display for SessionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub address: SessionAddress,
    pub run_id: String,
    pub category: SessionCategory,
    pub trigger: EventTrigger,
    pub source_label: String,
    pub state: SessionState,
    pub spawner: Option<SessionAddress>,
    pub depth: u32,
    pub purpose: String,
    pub agent_skill: Option<SkillName>,
    pub model: ModelSelector,
    pub resolved_model: Option<String>,
    pub conversation_target: Option<ConversationTarget>,
    pub started_at: DateTime<Utc>,
    pub usage: crate::usage::SessionUsageTotals,
    pub overlap: Option<PulseOverlap>,
}

#[must_use]
pub fn generate_run_id() -> String {
    let timestamp_ms = chrono::Utc::now().timestamp_millis();
    let rand_part: u32 = rand::random();
    format!("run-{timestamp_ms}-{rand_part:08x}")
}

#[must_use]
pub fn generate_address(trigger: &EventTrigger, qualifier: &str) -> SessionAddress {
    let category = SessionCategory::from_trigger(trigger);
    let slug = slugify(qualifier);
    let suffix: u16 = rand::random();
    SessionAddress::from(format!("{category}-{slug}-{suffix:04x}"))
}

#[must_use]
pub fn slugify(s: &str) -> String {
    let mut slug: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .take(24)
        .collect();
    while slug.contains("--") {
        slug = slug.replace("--", "-");
    }
    let trimmed = slug.trim_matches('-');
    if trimmed.is_empty() {
        "session".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_from_trigger() {
        assert_eq!(
            SessionCategory::from_trigger(&EventTrigger::Pulse),
            SessionCategory::Scheduled
        );
        assert_eq!(
            SessionCategory::from_trigger(&EventTrigger::Action),
            SessionCategory::Scheduled
        );
        assert_eq!(
            SessionCategory::from_trigger(&EventTrigger::Agent),
            SessionCategory::Spawned
        );
        assert_eq!(
            SessionCategory::from_trigger(&EventTrigger::Webhook("gh".into())),
            SessionCategory::External
        );
        assert_eq!(
            SessionCategory::from_trigger(&EventTrigger::Conversation),
            SessionCategory::External
        );
        assert_eq!(
            SessionCategory::from_trigger(&EventTrigger::Artifact("wiki".into())),
            SessionCategory::Artifact
        );
    }

    #[test]
    fn every_category_label_round_trips() {
        for category in [
            SessionCategory::Scheduled,
            SessionCategory::External,
            SessionCategory::Spawned,
            SessionCategory::Artifact,
        ] {
            assert_eq!(
                SessionCategory::from_label(category.as_str()),
                Some(category)
            );
        }
        assert_eq!(SessionCategory::from_label("mystery"), None);
    }

    #[test]
    fn artifact_session_address_is_prefixed_with_its_category() {
        let address = generate_address(&EventTrigger::Artifact("wiki".into()), "wiki graph");
        assert!(
            address.as_ref().starts_with("artifact-wiki-graph-"),
            "got {address}"
        );
        assert!(
            !address.as_ref().contains(':'),
            "session addresses never contain ':', so they can't collide with an artifact sender"
        );
    }

    #[test]
    fn generate_address_has_category_prefix_and_slug() {
        let addr = generate_address(&EventTrigger::Agent, "Research Helper");
        assert!(addr.as_ref().starts_with("spawned-research-helper-"));
    }

    #[test]
    fn generate_address_falls_back_when_qualifier_has_no_alnum() {
        let addr = generate_address(&EventTrigger::Pulse, "!!!");
        assert!(addr.as_ref().starts_with("scheduled-session-"));
    }

    #[test]
    fn generate_address_is_unique_across_calls() {
        let a = generate_address(&EventTrigger::Webhook("gh".into()), "gh");
        let b = generate_address(&EventTrigger::Webhook("gh".into()), "gh");
        assert_ne!(a, b);
    }

    #[test]
    fn generate_run_id_is_unique() {
        let a = generate_run_id();
        let b = generate_run_id();
        assert_ne!(a, b);
        assert!(a.starts_with("run-"));
    }
}
