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
use residuum_background::team::{A2aVisibility, Actor, AgentSummary};
use residuum_protocol::ServerMessage;

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, TS)]
#[ts(export)]
pub struct AgentPatch {
    #[serde(default)]
    pub autostart: Option<bool>,
    #[serde(default)]
    pub a2a_visibility: Option<A2aVisibility>,
}

impl AgentPatch {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.autostart.is_none() && self.a2a_visibility.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, TS)]
#[ts(export)]
pub struct RestoreAgentRequest {
    pub name: String,
    #[serde(default)]
    pub checkpoint_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct DeletedAgent {
    pub name: String,
    #[serde(default)]
    pub display_name: String,
    #[ts(type = "string")]
    pub deleted_at: DateTime<Utc>,
    pub checkpoint_id: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct AgentActivity {
    pub busy: bool,
    #[ts(type = "string | null")]
    pub busy_since: Option<DateTime<Utc>>,
    pub unread: u32,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub restart_pending: bool,
}

/// Hub-level events, published on the hub bus and sent over `/api/hub/ws`
/// (one frame each, tagged by `type`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum HubEvent {
    AgentState {
        agent: AgentSummary,
    },
    AgentStopping {
        name: String,
    },
    AgentCreated {
        agent: AgentSummary,
        by: Actor,
    },
    AgentRestored {
        agent: AgentSummary,
        by: Actor,
    },
    AgentDeleted {
        name: String,
        by: Actor,
    },
    AgentActivity {
        name: String,
        #[serde(flatten)]
        activity: AgentActivity,
    },
    Notice {
        level: NoticeLevel,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        agent: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        kind: Option<HubNoticeKind>,
    },
    HubConfigReloaded {
        ok: bool,
        changed: bool,
        message: Option<String>,
    },
}

impl HubEvent {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum HubNoticeKind {
    GatewayRebindFailed,
    BackgroundLimitAfterRestart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum NoticeLevel {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct AgentListResponse {
    pub agents: Vec<AgentSummary>,
    pub activity: BTreeMap<String, AgentActivity>,
    pub stopping: Vec<String>,
}

impl AgentListResponse {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct DeletedAgentListResponse {
    pub agents: Vec<DeletedAgent>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum HubSocketFrame {
    HubBoot {
        boot_id: String,
    },
    Pong,
    AgentsSnapshot(AgentListResponse),
    TeamEvent {
        boot_id: String,
        event: TeamEvent,
    },
    AgentOverview {
        overview: AgentOverview,
    },
    ArtifactUpdated {
        name: String,
    },
    ArtifactRemoved {
        name: String,
    },
    Subscribed {
        kind: SessionSubscriptionKind,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        agent: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        address: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        artifact: Option<String>,
    },
    SessionFrame {
        agent: String,
        frame: Box<ServerMessage>,
    },
    SessionRelayLagged,
    SystemOneStatus {
        status: residuum_inference::system_one::SystemOneStatus,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SessionSubscriptionKind {
    Session,
    ArtifactSessions,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum HubClientMessage {
    WatchTeam { prefixes: Vec<String> },
    Presence { device_id: String, active: bool },
    SubscribeSession { agent: String, address: String },
    UnsubscribeSession { agent: String, address: String },
    SubscribeArtifactSessions { artifact: String },
    UnsubscribeArtifactSessions { artifact: String },
    Ping,
}

#[cfg(test)]
mod tests {
    use super::*;
    use residuum_background::team::{AgentState, CreateAgentRequest};
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
                restart_pending: false,
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
        let pending = HubEvent::AgentActivity {
            name: "scout".to_string(),
            activity: AgentActivity {
                busy: true,
                busy_since: Some(since),
                unread: 0,
                restart_pending: true,
            },
        };
        assert_eq!(
            serde_json::to_value(pending).unwrap(),
            json!({
                "type": "agent_activity",
                "name": "scout",
                "busy": true,
                "busy_since": "2026-09-30T12:00:00Z",
                "unread": 0,
                "restart_pending": true,
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
