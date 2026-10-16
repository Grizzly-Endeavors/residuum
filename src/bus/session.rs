//! Session vocabulary: the addresses, categories, lifecycle states and
//! registry entry every session publishes on the bus, plus the functions that
//! mint addresses and run ids.
//!
//! The session registry in `background` keeps the live entries; the types here
//! are what the bus and every layer below the runtime name, so they live with
//! the bus.

mod events;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub use events::publish_session_event;

use crate::bus::{ConversationTarget, EventTrigger, PulseOverlap, SessionAddress, SkillName};
use crate::config::ModelSelector;

/// The well-known address of the main agent. Never present in the registry
/// (main is not a session), but used as the `spawner` value for sessions
/// main creates directly.
pub const MAIN_ADDRESS: &str = "main";

/// The main agent's depth in the spawn tree.
pub const MAIN_DEPTH: u32 = 0;

/// Sender address for a message the owner types into a session from the web
/// UI's session panel. Not a session and never present in the registry:
/// the owner is not an agent, so a message to this address does not resolve.
pub const OWNER_ADDRESS: &str = "owner";

/// Prefix of the sender address a workbench artifact's message to a session
/// carries (`artifact:<name>`), and of an artifact session's source label.
/// Never present in the registry: an artifact is not an agent, so a message
/// to this address does not resolve. Session addresses never contain `:`,
/// so no session can be mistaken for an artifact.
pub const ARTIFACT_SENDER_PREFIX: &str = "artifact:";

/// Category label an artifact's message to a session carries as its
/// sender's category.
pub const ARTIFACT_SENDER_CATEGORY: &str = "artifact";

/// Prefix of a teammate address: `agent:<name>` for a teammate's main,
/// `agent:<name>/<session-address>` for one of its sessions. A message from
/// a teammate carries its sender's address in this form, so replying is a
/// `message_agent` call to it. Never present in this agent's registry:
/// session addresses never contain `:`.
pub const TEAMMATE_SENDER_PREFIX: &str = "agent:";

/// Category label a teammate's message carries as its sender's category.
pub const TEAMMATE_SENDER_CATEGORY: &str = "teammate";

/// The sender address (and source label) naming the workbench artifact
/// `name`.
#[must_use]
pub fn artifact_sender_address(name: &str) -> SessionAddress {
    SessionAddress::from(format!("{ARTIFACT_SENDER_PREFIX}{name}"))
}

/// How a session was started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SessionCategory {
    /// Pulses and scheduled actions.
    Scheduled,
    /// Non-owner-DM conversations and webhooks.
    External,
    /// Started by an agent: `subagent_spawn` or the subconscious learner.
    Spawned,
    /// Started by a workbench artifact through the sessions HTTP API. Its
    /// output stays with the artifact: never relayed to main, never routed
    /// to the inbox or notification channels.
    Artifact,
}

impl SessionCategory {
    /// Derive the category from the trigger that started the session.
    #[must_use]
    pub fn from_trigger(trigger: &EventTrigger) -> Self {
        match trigger {
            EventTrigger::Pulse | EventTrigger::Action => Self::Scheduled,
            EventTrigger::Agent => Self::Spawned,
            EventTrigger::Webhook(_) | EventTrigger::Conversation => Self::External,
            EventTrigger::Artifact(_) => Self::Artifact,
        }
    }

    /// Lowercase label used in addresses, logs, and the store.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Scheduled => "scheduled",
            Self::External => "external",
            Self::Spawned => "spawned",
            Self::Artifact => "artifact",
        }
    }

    /// Parse the label [`Self::as_str`] produces, as recorded in the session
    /// store. `None` for anything else.
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

/// A session's lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SessionState {
    /// The session is being forked (resources are being built); no turn has
    /// started yet.
    Forking,
    /// The run is registered and ready for its next turn but is waiting on a
    /// `max_concurrent` concurrency permit — either its first turn (right
    /// after forking) or a later one (right after idle). Distinct from
    /// `Forking`/`Idle` so a run stuck behind other live sessions is visibly
    /// queued rather than looking like it's still starting up or sitting
    /// idle with nothing to do.
    Queued,
    /// A turn is executing. Holds one concurrency permit.
    Running,
    /// The turn ended; the session is still live and will complete after its
    /// idle timeout, or resume immediately on new input.
    Idle,
    /// The idle timeout elapsed, or the session was stopped; memory is being
    /// finalized.
    Completing,
    /// Merged and closed. Immutable from here on.
    Completed,
}

impl SessionState {
    /// Lowercase label used in logs and the store.
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

    /// Parse the label [`Self::as_str`] produces, as recorded in the session
    /// store. `None` for anything else.
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

/// A live session's metadata, as tracked by the registry.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    /// Stable address for this session.
    pub address: SessionAddress,
    /// Unique identifier for the current run within this session.
    pub run_id: String,
    /// How this session was started.
    pub category: SessionCategory,
    /// What triggered this session (precise origin, e.g. `Pulse`, `Webhook(name)`).
    pub trigger: EventTrigger,
    /// Human-readable source label (e.g. `"pulse:email_check"`).
    pub source_label: String,
    /// Current lifecycle state.
    pub state: SessionState,
    /// The agent that started this session (`main`, or another session's
    /// address). `None` for `scheduled` and `external` sessions.
    pub spawner: Option<SessionAddress>,
    /// Depth from the main agent (main = 0).
    pub depth: u32,
    /// One-line description of what this session is doing (the task brief,
    /// prompt, or conversation label, truncated).
    pub purpose: String,
    /// Skill the session runs with, if any.
    pub agent_skill: Option<SkillName>,
    /// The model tier or explicit model this run executes on, carried onto a
    /// resume so a later run doesn't silently fall back to the default tier.
    pub model: ModelSelector,
    /// The `provider/model` id this run started on (see
    /// `SubAgentConfig::resolved_model`).
    pub resolved_model: Option<String>,
    /// The conversation this session's turn output replies to, for an
    /// `external` session started by a conversation message. `None` for
    /// `scheduled`/`spawned` sessions and webhook-triggered `external` ones,
    /// which have no conversation of their own to reply into.
    pub conversation_target: Option<ConversationTarget>,
    /// When this run started.
    pub started_at: DateTime<Utc>,
    /// This run's cumulative token usage, updated after every model call —
    /// see `SessionRegistry::accumulate_usage` in the background registry.
    /// Mirrored into the run's store record at completion so a finished
    /// run's totals survive too.
    pub usage: crate::bus::usage::SessionUsageTotals,
    /// Set when this run is a pulse fire that started while its previous run
    /// was still live. See [`PulseOverlap`].
    pub overlap: Option<PulseOverlap>,
}

/// Generate a unique run id, distinct from the session's address and unique
/// across all runs (including later runs of the same session).
#[must_use]
pub fn generate_run_id() -> String {
    let timestamp_ms = chrono::Utc::now().timestamp_millis();
    let rand_part: u32 = rand::random();
    format!("run-{timestamp_ms}-{rand_part:08x}")
}

/// Generate a human-readable, URL/filename-safe session address for a new
/// `scheduled` or `spawned` session, or a webhook's `external` session.
///
/// Prefixed by category and a slugified qualifier (e.g. a skill, pulse, or
/// webhook name), suffixed with a short random hex tag for uniqueness.
#[must_use]
pub fn generate_address(trigger: &EventTrigger, qualifier: &str) -> SessionAddress {
    let category = SessionCategory::from_trigger(trigger);
    let slug = slugify(qualifier);
    let suffix: u16 = rand::random();
    SessionAddress::from(format!("{category}-{slug}-{suffix:04x}"))
}

/// Lowercase, hyphenate, and cap a free-text qualifier for use in an address.
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
