//! Shapes shared by the agent host, the hub HTTP API, and the hub
//! WebSocket. Field names and JSON forms follow
//! `docs/systems-usage/hub-http.md`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Lifecycle state of a hosted agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum AgentState {
    /// The runtime is being built; not yet serving.
    Starting,
    /// Serving: adapters, event loop, and HTTP routes are live.
    Running,
    /// Not running, by choice (never started, or stopped).
    Stopped,
    /// Not running because start-up or the running event loop failed.
    Failed,
}

impl std::fmt::Display for AgentState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
        })
    }
}

/// Whether other agents can reach this agent's A2A card without a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum A2aVisibility {
    /// The card is open; every other A2A route still needs a key or sibling.
    Public,
    /// Everything answers 404 to callers without a key or sibling.
    Private,
}

/// The error that moved an agent to [`AgentState::Failed`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentLastError {
    /// Plain-language description, safe to show the user.
    pub message: String,
    /// When the failure happened.
    #[ts(type = "string")]
    pub at: DateTime<Utc>,
}

/// One hosted agent as the hub API and hub WebSocket describe it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentSummary {
    /// The agent's name: its directory name and identity everywhere.
    pub name: String,
    /// Current lifecycle state.
    pub state: AgentState,
    /// Set only while `state` is [`AgentState::Failed`].
    pub last_error: Option<AgentLastError>,
    /// Whether the agent starts with the hub.
    pub autostart: bool,
    /// One-line role from the agent's wiki role page, if it has one.
    pub role: Option<String>,
    /// The agent's A2A visibility.
    pub a2a_visibility: A2aVisibility,
}

/// Who performed a lifecycle action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Actor {
    /// The user, through the web UI, CLI, or API.
    User,
    /// A hosted agent, by name (for example through `agent_create`).
    Agent(String),
}

impl Actor {
    /// Wire form: `"user"` or `"agent:<name>"`.
    #[must_use]
    pub fn wire(&self) -> String {
        match self {
            Self::User => "user".to_string(),
            Self::Agent(name) => format!("agent:{name}"),
        }
    }
}

impl Serialize for Actor {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.wire())
    }
}

/// Body of `POST /api/hub/agents`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, TS)]
#[ts(export)]
pub struct CreateAgentRequest {
    /// The new agent's name (validated with the agent-name rules).
    pub name: String,
    /// Optional role description, delivered as the agent's first message.
    #[serde(default)]
    pub description: Option<String>,
    /// Copy `providers.toml` from this existing agent.
    #[serde(default)]
    pub models_from: Option<String>,
    /// Raw `providers.toml` to use when `models_from` is absent.
    #[serde(default)]
    pub providers_toml: Option<String>,
    /// A2A visibility; defaults to private for user-created agents.
    #[serde(default)]
    pub a2a_visibility: Option<A2aVisibility>,
    /// The hop count the creating agent's message chain has reached, set by
    /// `agent_create` from the caller's turn. Not part of the request body:
    /// a user-created agent starts a chain at zero.
    #[serde(skip)]
    #[ts(skip)]
    pub creator_hop: u32,
}

/// Body of `PATCH /api/hub/agents/{name}`: at least one field is set.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, TS)]
#[ts(export)]
pub struct AgentPatch {
    /// New autostart flag.
    #[serde(default)]
    pub autostart: Option<bool>,
    /// New A2A visibility.
    #[serde(default)]
    pub a2a_visibility: Option<A2aVisibility>,
}

impl AgentPatch {
    /// Whether the patch changes nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.autostart.is_none() && self.a2a_visibility.is_none()
    }
}

/// Result of deleting an agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct DeleteOutcome {
    /// Always true on success.
    pub deleted: bool,
    /// The checkpoint taken of the agent's directory before removal, when
    /// one could be recorded.
    pub checkpoint_id: Option<String>,
}

/// Body of `POST /api/hub/agents/restore`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, TS)]
#[ts(export)]
pub struct RestoreAgentRequest {
    /// The deleted agent's name.
    pub name: String,
    /// The workspace checkpoint to restore the agent's files from; the
    /// latest one the deleted agent has when absent.
    #[serde(default)]
    pub checkpoint_id: Option<String>,
}

/// One deleted agent whose checkpoint history is still on disk, so it can be
/// restored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct DeletedAgent {
    /// The agent's name.
    pub name: String,
    /// When the agent was deleted.
    #[ts(type = "string")]
    pub deleted_at: DateTime<Utc>,
    /// The workspace checkpoint a restore uses by default: the last one
    /// taken before the deletion.
    pub checkpoint_id: String,
}

/// Main-conversation activity for one agent, for the switcher.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct AgentActivity {
    /// A main turn is in progress.
    pub busy: bool,
    /// Main-conversation messages no web client has shown yet.
    pub unread: u32,
}

/// Hub-level events, published on the hub bus and sent over `/api/hub/ws`
/// (one frame each, tagged by `type`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HubEvent {
    /// An agent's state, autostart, or visibility changed.
    AgentState { agent: AgentSummary },
    /// A running agent's stop began. `summary()`/`list()` still report it
    /// `Running` until the stop finishes (up to the stop timeout), but from
    /// this moment the team router already refuses teammate messages for it;
    /// the relay-facing agent list stops advertising it at the same time.
    AgentStopping { name: String },
    /// An agent was created.
    AgentCreated { agent: AgentSummary, by: Actor },
    /// A deleted agent was restored.
    AgentRestored { agent: AgentSummary, by: Actor },
    /// An agent was deleted.
    AgentDeleted { name: String, by: Actor },
    /// An agent's main-conversation activity changed.
    AgentActivity {
        name: String,
        #[serde(flatten)]
        activity: AgentActivity,
    },
    /// A hub notice for the user. Never used for created, deleted, or
    /// failed events, which travel in their own frames.
    Notice {
        level: NoticeLevel,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        agent: Option<String>,
    },
}

/// Severity of a [`HubEvent::Notice`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeLevel {
    Info,
    Warn,
    Error,
}

/// Why a lifecycle or lookup call failed. The HTTP layer maps these to
/// status codes: `NotFound` and `NoDeletedAgent` 404, `InvalidName`/`InvalidRequest` 400,
/// `AlreadyExists`/`NotRunning` 409, `Failed` 500.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LifecycleError {
    /// No agent has this name.
    #[error("no agent named '{0}'")]
    NotFound(String),
    /// The name breaks the agent-name rules; the message says which.
    #[error("{0}")]
    InvalidName(String),
    /// The request is malformed; the message says how.
    #[error("{0}")]
    InvalidRequest(String),
    /// An agent (or directory) with this name already exists.
    #[error("an agent named '{0}' already exists")]
    AlreadyExists(String),
    /// No deleted agent with this name has checkpoint history to restore.
    #[error("there is no deleted agent named '{0}' to restore")]
    NoDeletedAgent(String),
    /// The agent exists but is not running.
    #[error("{name} is {state}")]
    NotRunning { name: String, state: AgentState },
    /// The operation failed; the message is safe to show the user.
    #[error("{0}")]
    Failed(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scout() -> AgentSummary {
        AgentSummary {
            name: "scout".to_string(),
            state: AgentState::Running,
            last_error: None,
            autostart: true,
            role: Some("Research".to_string()),
            a2a_visibility: A2aVisibility::Private,
        }
    }

    #[test]
    fn summary_matches_the_contract_shape() {
        assert_eq!(
            serde_json::to_value(scout()).unwrap(),
            json!({
                "name": "scout",
                "state": "running",
                "last_error": null,
                "autostart": true,
                "role": "Research",
                "a2a_visibility": "private",
            })
        );
    }

    #[test]
    fn hub_events_are_tagged_frames() {
        let created = HubEvent::AgentCreated {
            agent: scout(),
            by: Actor::Agent("atlas".to_string()),
        };
        let value = serde_json::to_value(created).unwrap();
        assert_eq!(value.get("type"), Some(&json!("agent_created")));
        assert_eq!(value.get("by"), Some(&json!("agent:atlas")));

        let activity = HubEvent::AgentActivity {
            name: "scout".to_string(),
            activity: AgentActivity {
                busy: true,
                unread: 2,
            },
        };
        assert_eq!(
            serde_json::to_value(activity).unwrap(),
            json!({ "type": "agent_activity", "name": "scout", "busy": true, "unread": 2 })
        );

        let deleted = HubEvent::AgentDeleted {
            name: "scout".to_string(),
            by: Actor::User,
        };
        assert_eq!(
            serde_json::to_value(deleted).unwrap(),
            json!({ "type": "agent_deleted", "name": "scout", "by": "user" })
        );
    }

    #[test]
    fn create_request_and_patch_accept_optional_fields() {
        let request: CreateAgentRequest =
            serde_json::from_value(json!({ "name": "nova", "models_from": "scout" })).unwrap();
        assert_eq!(request.models_from.as_deref(), Some("scout"));
        assert_eq!(request.a2a_visibility, None);

        let empty: AgentPatch = serde_json::from_value(json!({})).unwrap();
        assert!(empty.is_empty());
        let visibility: AgentPatch =
            serde_json::from_value(json!({ "a2a_visibility": "public" })).unwrap();
        assert_eq!(visibility.a2a_visibility, Some(A2aVisibility::Public));
    }

    #[test]
    fn not_running_error_reads_plainly() {
        let err = LifecycleError::NotRunning {
            name: "scout".to_string(),
            state: AgentState::Stopped,
        };
        assert_eq!(err.to_string(), "scout is stopped");
    }
}
