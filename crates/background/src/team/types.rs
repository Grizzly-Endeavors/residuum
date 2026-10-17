//! The agent lifecycle types the team shares: what the hub reports about
//! each hosted agent, who acted on one, and why a lifecycle or lookup call
//! failed. Field names and JSON forms follow
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

/// What kind of failure moved an agent to [`AgentState::Failed`], so a client
/// can offer the matching next step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum AgentErrorKind {
    /// Start-up rejected the agent's configuration.
    Config,
    /// Another agent already uses the agent's Teams port.
    PortConflict,
    /// The agent panicked, or its event loop ended on its own.
    Crash,
    /// Any other failure.
    Other,
}

/// The error that moved an agent to [`AgentState::Failed`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentLastError {
    /// Plain-language description, safe to show the user.
    pub message: String,
    /// What kind of failure this was.
    pub kind: AgentErrorKind,
    /// The underlying error text, without the explanation and next steps
    /// that `message` wraps around it.
    pub reason: String,
    /// When the failure happened.
    #[ts(type = "string")]
    pub at: DateTime<Utc>,
}

/// One hosted agent as the hub API and hub WebSocket describe it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentSummary {
    /// The agent's folder name: its URL, its A2A path, and the key its
    /// checkpoint history is stored under.
    pub name: String,
    /// The name people see and address (`agent:<display name>`). The same as
    /// `name` when no separate name was set.
    #[serde(default)]
    pub display_name: String,
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
    /// Whether the agent's config has a complete `[teams]` section. The relay
    /// accepts Teams messages for the agent while this is set. A stopped agent
    /// stays configured; delivery retries until its listener is back.
    #[serde(default)]
    pub teams_configured: bool,
}

impl AgentSummary {
    /// The name people see. A summary from an older client, or one built
    /// before the config was read, has an empty `display_name` and shows
    /// the folder name.
    #[must_use]
    pub fn label(&self) -> &str {
        if self.display_name.is_empty() {
            &self.name
        } else {
            &self.display_name
        }
    }
}

/// Who performed a lifecycle action.
#[derive(Debug, Clone, PartialEq, Eq, TS)]
#[ts(export, type = "\"user\" | `agent:${string}`")]
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

/// Why a lifecycle or lookup call failed. The HTTP layer maps these to
/// status codes: `NotFound` and `NoDeletedAgent` 404, `InvalidName`/`InvalidRequest` 400,
/// `AlreadyExists`/`NotRunning` 409, `ShuttingDown` 503, `Failed` 500.
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
    /// The hub is shutting down and refused the operation; this is expected
    /// and temporary, not a real failure.
    #[error("{0}")]
    ShuttingDown(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scout() -> AgentSummary {
        AgentSummary {
            name: "scout".to_string(),
            display_name: "scout".to_string(),
            state: AgentState::Running,
            last_error: None,
            autostart: true,
            role: Some("Research".to_string()),
            a2a_visibility: A2aVisibility::Private,
            teams_configured: false,
        }
    }

    #[test]
    fn summary_matches_the_contract_shape() {
        assert_eq!(
            serde_json::to_value(scout()).unwrap(),
            json!({
                "name": "scout",
                "display_name": "scout",
                "state": "running",
                "last_error": null,
                "autostart": true,
                "role": "Research",
                "a2a_visibility": "private",
                "teams_configured": false,
            })
        );
    }

    #[test]
    fn a_last_error_carries_its_kind_and_the_reason_without_the_wrapper() {
        let error = AgentLastError {
            message: "scout couldn't start: config error: bad model. Fix its settings.".to_string(),
            kind: AgentErrorKind::PortConflict,
            reason: "config error: bad model".to_string(),
            at: DateTime::parse_from_rfc3339("2026-09-30T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        };
        assert_eq!(
            serde_json::to_value(error).unwrap(),
            json!({
                "message": "scout couldn't start: config error: bad model. Fix its settings.",
                "kind": "port_conflict",
                "reason": "config error: bad model",
                "at": "2026-09-30T12:00:00Z",
            })
        );
        for (kind, wire) in [
            (AgentErrorKind::Config, "config"),
            (AgentErrorKind::Crash, "crash"),
            (AgentErrorKind::Other, "other"),
        ] {
            assert_eq!(serde_json::to_value(kind).unwrap(), json!(wire));
        }
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
