//! Shapes shared by the agent host, the hub HTTP API, and the hub
//! WebSocket. Field names and JSON forms follow
//! `docs/systems-usage/hub-http.md`.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::directory::AgentDirectory;
use super::overview::AgentOverview;
use super::team_events::TeamEvent;
use crate::gateway::protocol::ServerMessage;

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
    /// The agent's folder name, which a restore uses.
    pub name: String,
    /// The name people saw. The same as `name` when no separate name was set.
    #[serde(default)]
    pub display_name: String,
    /// When the agent was deleted.
    #[ts(type = "string")]
    pub deleted_at: DateTime<Utc>,
    /// The workspace checkpoint a restore uses by default: the last one
    /// taken before the deletion.
    pub checkpoint_id: String,
}

/// Main-conversation activity for one agent, for the rail and Home.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct AgentActivity {
    /// A main turn is in progress.
    pub busy: bool,
    /// When the current main turn began, while `busy`.
    #[ts(type = "string | null")]
    pub busy_since: Option<DateTime<Utc>>,
    /// Main-conversation messages no web client has shown yet.
    pub unread: u32,
}

/// Hub-level events, published on the hub bus and sent over `/api/hub/ws`
/// (one frame each, tagged by `type`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
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
        #[ts(optional)]
        agent: Option<String>,
    },
    /// The hub finished an attempt to reload `hub/config.toml`. Sent after
    /// every attempt, beside the notice that tells the user about it.
    HubConfigReloaded {
        /// The file loaded, so the running config now matches it. False when
        /// the load failed and the previous config stays in effect.
        ok: bool,
        /// The loaded config differs from the one that was running.
        changed: bool,
        /// The text of the notice sent beside this frame: what changed, or
        /// why the reload failed. Null when nothing changed.
        message: Option<String>,
    },
}

/// Severity of a [`HubEvent::Notice`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum NoticeLevel {
    Info,
    Warn,
    Error,
}

/// Every hosted agent with its activity and stopping set: the answer to
/// `GET /api/hub/agents` and the body of the hub WebSocket's snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct AgentListResponse {
    /// Every agent, sorted by name.
    pub agents: Vec<AgentSummary>,
    /// Main-conversation activity of every agent, by name.
    pub activity: BTreeMap<String, AgentActivity>,
    /// Names of running agents whose stop has begun but not finished.
    pub stopping: Vec<String>,
}

impl AgentListResponse {
    /// Read the directory's agents, activity and stopping set.
    #[must_use]
    pub fn of(directory: &dyn AgentDirectory) -> Self {
        let mut agents = directory.list();
        agents.sort_by(|a, b| a.name.cmp(&b.name));
        let mut stopping = directory.stopping();
        stopping.sort();
        Self {
            agents,
            activity: directory.activity().into_iter().collect(),
            stopping,
        }
    }
}

/// The deleted agents that can be restored: the answer to
/// `GET /api/hub/agents/deleted`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct DeletedAgentListResponse {
    /// Newest deletion first.
    pub agents: Vec<DeletedAgent>,
}

/// Frames the hub WebSocket sends on its own account, rather than forwarding
/// a [`HubEvent`] from the hub bus.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum HubSocketFrame {
    /// The first frame of every connection: which hub process answered.
    HubBoot { boot_id: String },
    /// The agents with their activity, sent after `hub_boot` and again
    /// whenever the connection fell behind and lost events.
    AgentsSnapshot(AgentListResponse),
    /// An entry was added to the team event log.
    TeamEvent {
        /// The hub process whose log the entry is in.
        boot_id: String,
        event: TeamEvent,
    },
    /// Something in an agent's overview changed. It replaces the client's
    /// copy of that agent's overview.
    AgentOverview { overview: AgentOverview },
    /// An artifact was added, or one of its files changed, whether or not
    /// any agent is running.
    ArtifactUpdated { name: String },
    /// An artifact's page or folder is gone.
    ArtifactRemoved { name: String },
    /// A session subscription is active. Sent in answer to a subscribe
    /// message, before any frame the subscription delivers.
    Subscribed {
        /// Which subscribe message this answers.
        kind: SessionSubscriptionKind,
        /// The agent of a `session` subscription.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        agent: Option<String>,
        /// The address of a `session` subscription.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        address: Option<String>,
        /// The artifact of an `artifact_sessions` subscription.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        artifact: Option<String>,
    },
    /// An event of a session the connection follows.
    SessionFrame {
        /// The agent the session runs on.
        agent: String,
        /// The `session_*` frame exactly as the agent's own socket sends it.
        frame: ServerMessage,
    },
    /// The connection fell behind and lost session frames, so it can't know
    /// what it missed. A client reads the sessions it follows again over
    /// HTTP.
    SessionRelayLagged,
}

/// The kind of subscription a `subscribed` frame confirms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SessionSubscriptionKind {
    /// One session of one agent.
    Session,
    /// Every session an artifact started, on any agent.
    ArtifactSessions,
}

/// What a client sends on the hub WebSocket.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum HubClientMessage {
    /// Replace the set of team paths this connection watches. A prefix is
    /// `team` or a path under `team/`, the spelling the change feed uses.
    WatchTeam { prefixes: Vec<String> },
    /// The window for the push device `device_id` is now in front of the
    /// user (`active`) or not. The client repeats `active: true` every 30
    /// seconds while it stays so; the hub sends no push to such a device.
    Presence { device_id: String, active: bool },
    /// Follow every event of one session on one agent.
    SubscribeSession { agent: String, address: String },
    /// Stop following a session.
    UnsubscribeSession { agent: String, address: String },
    /// Follow every event of every session labelled `artifact:<artifact>`,
    /// on any agent, including sessions that start after the subscription.
    SubscribeArtifactSessions { artifact: String },
    /// Stop following an artifact's sessions.
    UnsubscribeArtifactSessions { artifact: String },
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

        let since = DateTime::parse_from_rfc3339("2026-09-30T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let activity = HubEvent::AgentActivity {
            name: "scout".to_string(),
            activity: AgentActivity {
                busy: true,
                busy_since: Some(since),
                unread: 2,
            },
        };
        assert_eq!(
            serde_json::to_value(activity).unwrap(),
            json!({
                "type": "agent_activity",
                "name": "scout",
                "busy": true,
                "busy_since": "2026-09-30T12:00:00Z",
                "unread": 2,
            })
        );
        let idle = HubEvent::AgentActivity {
            name: "scout".to_string(),
            activity: AgentActivity::default(),
        };
        assert_eq!(
            serde_json::to_value(idle).unwrap(),
            json!({ "type": "agent_activity", "name": "scout", "busy": false, "busy_since": null, "unread": 0 })
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
    fn a_stopping_agent_is_announced_by_name() {
        let stopping = HubEvent::AgentStopping {
            name: "scout".to_string(),
        };
        assert_eq!(
            serde_json::to_value(stopping).unwrap(),
            json!({ "type": "agent_stopping", "name": "scout" })
        );
    }

    #[test]
    fn a_config_reload_frame_says_whether_it_worked_and_whether_anything_changed() {
        let changed = HubEvent::HubConfigReloaded {
            ok: true,
            changed: true,
            message: Some("hub configuration reloaded: timezone".to_string()),
        };
        assert_eq!(
            serde_json::to_value(changed).unwrap(),
            json!({
                "type": "hub_config_reloaded",
                "ok": true,
                "changed": true,
                "message": "hub configuration reloaded: timezone",
            })
        );
        let unchanged = HubEvent::HubConfigReloaded {
            ok: true,
            changed: false,
            message: None,
        };
        assert_eq!(
            serde_json::to_value(unchanged).unwrap(),
            json!({ "type": "hub_config_reloaded", "ok": true, "changed": false, "message": null })
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
    fn the_socket_frames_the_hub_sends_itself_are_tagged() {
        let boot = HubSocketFrame::HubBoot {
            boot_id: "b-1".to_string(),
        };
        assert_eq!(
            serde_json::to_value(boot).unwrap(),
            json!({ "type": "hub_boot", "boot_id": "b-1" })
        );
        let snapshot = HubSocketFrame::AgentsSnapshot(AgentListResponse {
            agents: vec![scout()],
            activity: BTreeMap::from([("scout".to_string(), AgentActivity::default())]),
            stopping: vec!["scout".to_string()],
        });
        let value = serde_json::to_value(snapshot).unwrap();
        assert_eq!(value.get("type"), Some(&json!("agents_snapshot")));
        assert_eq!(value.get("stopping"), Some(&json!(["scout"])));
        assert_eq!(
            value.get("activity"),
            Some(&json!({ "scout": { "busy": false, "busy_since": null, "unread": 0 } }))
        );
        assert_eq!(value.pointer("/agents/0/name"), Some(&json!("scout")));
    }

    #[test]
    fn the_artifact_and_session_relay_frames_are_tagged() {
        let updated = HubSocketFrame::ArtifactUpdated {
            name: "chart".to_string(),
        };
        assert_eq!(
            serde_json::to_value(updated).unwrap(),
            json!({ "type": "artifact_updated", "name": "chart" })
        );
        let removed = HubSocketFrame::ArtifactRemoved {
            name: "chart".to_string(),
        };
        assert_eq!(
            serde_json::to_value(removed).unwrap(),
            json!({ "type": "artifact_removed", "name": "chart" })
        );
        let session = HubSocketFrame::Subscribed {
            kind: SessionSubscriptionKind::Session,
            agent: Some("scout".to_string()),
            address: Some("spawned-x".to_string()),
            artifact: None,
        };
        assert_eq!(
            serde_json::to_value(session).unwrap(),
            json!({ "type": "subscribed", "kind": "session", "agent": "scout", "address": "spawned-x" })
        );
        let artifact = HubSocketFrame::Subscribed {
            kind: SessionSubscriptionKind::ArtifactSessions,
            agent: None,
            address: None,
            artifact: Some("chart".to_string()),
        };
        assert_eq!(
            serde_json::to_value(artifact).unwrap(),
            json!({ "type": "subscribed", "kind": "artifact_sessions", "artifact": "chart" })
        );
        assert_eq!(
            serde_json::to_value(HubSocketFrame::SessionRelayLagged).unwrap(),
            json!({ "type": "session_relay_lagged" })
        );
        let relayed = HubSocketFrame::SessionFrame {
            agent: "scout".to_string(),
            frame: ServerMessage::SessionTurnStarted {
                address: "spawned-x".to_string(),
                run_id: "run-1".to_string(),
                turn_id: "t-1".to_string(),
            },
        };
        assert_eq!(
            serde_json::to_value(relayed).unwrap(),
            json!({
                "type": "session_frame",
                "agent": "scout",
                "frame": {
                    "type": "session_turn_started",
                    "address": "spawned-x",
                    "run_id": "run-1",
                    "turn_id": "t-1",
                },
            })
        );
    }

    #[test]
    fn a_client_can_watch_team_paths() {
        let message: HubClientMessage =
            serde_json::from_value(json!({ "type": "watch_team", "prefixes": ["team/wiki"] }))
                .unwrap();
        assert_eq!(
            message,
            HubClientMessage::WatchTeam {
                prefixes: vec!["team/wiki".to_string()]
            }
        );
    }

    #[test]
    fn a_client_subscribes_to_sessions_by_agent_or_artifact() {
        let read = |value: serde_json::Value| -> HubClientMessage {
            serde_json::from_value(value).unwrap()
        };
        assert_eq!(
            read(json!({ "type": "subscribe_session", "agent": "scout", "address": "spawned-x" })),
            HubClientMessage::SubscribeSession {
                agent: "scout".to_string(),
                address: "spawned-x".to_string()
            }
        );
        assert_eq!(
            read(
                json!({ "type": "unsubscribe_session", "agent": "scout", "address": "spawned-x" })
            ),
            HubClientMessage::UnsubscribeSession {
                agent: "scout".to_string(),
                address: "spawned-x".to_string()
            }
        );
        assert_eq!(
            read(json!({ "type": "subscribe_artifact_sessions", "artifact": "chart" })),
            HubClientMessage::SubscribeArtifactSessions {
                artifact: "chart".to_string()
            }
        );
        assert_eq!(
            read(json!({ "type": "unsubscribe_artifact_sessions", "artifact": "chart" })),
            HubClientMessage::UnsubscribeArtifactSessions {
                artifact: "chart".to_string()
            }
        );
    }

    #[test]
    fn a_client_reports_its_push_device_present_or_not() {
        let message: HubClientMessage = serde_json::from_value(
            json!({ "type": "presence", "device_id": "phone-1", "active": true }),
        )
        .unwrap();
        assert_eq!(
            message,
            HubClientMessage::Presence {
                device_id: "phone-1".to_string(),
                active: true
            }
        );
        for incomplete in [
            json!({ "type": "presence", "device_id": "phone-1" }),
            json!({ "type": "presence", "active": true }),
            json!({ "type": "presence", "device_id": "phone-1", "active": "yes" }),
        ] {
            assert!(
                serde_json::from_value::<HubClientMessage>(incomplete.clone()).is_err(),
                "{incomplete} is not a presence report"
            );
        }
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
