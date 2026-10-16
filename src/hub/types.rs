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
use crate::background::team::{A2aVisibility, Actor, AgentSummary};
use crate::gateway::protocol::ServerMessage;

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
    /// failed events, which travel in their own frames, nor for the outcome
    /// of a `hub/config.toml` reload, which is `HubConfigReloaded`.
    Notice {
        level: NoticeLevel,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        agent: Option<String>,
        /// What the notice reports, when the web words it itself; `message`
        /// is then the detail behind that wording.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        kind: Option<HubNoticeKind>,
    },
    /// The hub finished an attempt to reload `hub/config.toml`. Sent after
    /// every attempt; it is the hub's only report of the outcome.
    HubConfigReloaded {
        /// The file loaded, so the running config now matches it. False when
        /// the load failed and the previous config stays in effect.
        ok: bool,
        /// The loaded config differs from the one that was running.
        changed: bool,
        /// The hub's own account of the outcome: what changed, or why the
        /// reload failed. Null when nothing changed.
        message: Option<String>,
    },
}

impl HubEvent {
    /// A notice whose text is all there is to it.
    #[must_use]
    pub fn notice(level: NoticeLevel, message: String, agent: Option<String>) -> Self {
        Self::Notice {
            level,
            message,
            agent,
            kind: None,
        }
    }
}

/// What a [`HubEvent::Notice`] reports, for the notices the web words itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum HubNoticeKind {
    /// The HTTP listener couldn't bind the address a reloaded config names,
    /// so it keeps serving on the old one.
    GatewayRebindFailed,
    /// `background.max_concurrent` changed; the new limit applies from the
    /// next restart.
    BackgroundLimitAfterRestart,
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
    /// The answer to a client's `ping`.
    Pong,
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
        frame: Box<ServerMessage>,
    },
    /// The connection fell behind and lost session frames, so it can't know
    /// what it missed. A client reads the sessions it follows again over
    /// HTTP.
    SessionRelayLagged,
    /// The System 1 (decision model) service's status: sent after the first
    /// snapshot and again whenever it changes.
    SystemOneStatus {
        status: crate::inference::system_one::SystemOneStatus,
    },
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
    /// Answered with `pong`. The socket handles a connection's messages in
    /// order, so a `pong` also confirms that everything sent before the
    /// `ping` (a `watch_team`, say) has taken effect.
    Ping,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::background::team::{AgentState, CreateAgentRequest};
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
            frame: Box::new(ServerMessage::SessionTurnStarted {
                address: "spawned-x".to_string(),
                run_id: "run-1".to_string(),
                turn_id: "t-1".to_string(),
            }),
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
}
