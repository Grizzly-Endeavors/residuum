use chrono::{DateTime, Utc};
use serde::Serialize;
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TeamEventLevel {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TeamEventKind {
    HubStarted,
    AgentStarted,
    AgentStopped,
    AgentFailed,
    AgentCreated,
    AgentDeleted,
    AgentRestored,
    AgentReplied,
    SessionStarted,
    SessionFinished,
    InboxItemAdded,
    ScheduledRunFinished,
    HubNotice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TeamEventPlace {
    Chat,
    Activity,
    Schedule,
    Files,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum TeamEventTarget {
    AgentPlace {
        agent: String,
        place: TeamEventPlace,
    },
    Session {
        agent: String,
        run_id: String,
    },
    InboxItem {
        agent: String,
        item_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct TeamEvent {
    #[ts(type = "number")]
    pub id: u64,
    #[ts(type = "string")]
    pub at: DateTime<Utc>,
    pub agent: Option<String>,
    pub kind: TeamEventKind,
    pub level: TeamEventLevel,
    pub summary: String,
    pub target: Option<TeamEventTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTeamEvent {
    pub at: DateTime<Utc>,
    pub agent: Option<String>,
    pub kind: TeamEventKind,
    pub level: TeamEventLevel,
    pub summary: String,
    pub target: Option<TeamEventTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct TeamEventPage {
    pub boot_id: String,
    pub events: Vec<TeamEvent>,
    #[ts(type = "number | null")]
    pub next_before: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_entry_serializes_to_the_contract_shape() {
        let event = TeamEvent {
            id: 7,
            at: DateTime::parse_from_rfc3339("2026-09-30T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            agent: Some("atlas".to_string()),
            kind: TeamEventKind::ScheduledRunFinished,
            level: TeamEventLevel::Error,
            summary: "atlas's pulse \"email_check\" failed".to_string(),
            target: Some(TeamEventTarget::Session {
                agent: "atlas".to_string(),
                run_id: "run-1".to_string(),
            }),
        };
        assert_eq!(
            serde_json::to_value(event).unwrap(),
            json!({
                "id": 7,
                "at": "2026-09-30T12:00:00Z",
                "agent": "atlas",
                "kind": "scheduled_run_finished",
                "level": "error",
                "summary": "atlas's pulse \"email_check\" failed",
                "target": { "kind": "session", "agent": "atlas", "run_id": "run-1" },
            })
        );
    }

    #[test]
    fn every_target_is_tagged_by_its_kind() {
        let place = TeamEventTarget::AgentPlace {
            agent: "atlas".to_string(),
            place: TeamEventPlace::Schedule,
        };
        assert_eq!(
            serde_json::to_value(place).unwrap(),
            json!({ "kind": "agent_place", "agent": "atlas", "place": "schedule" })
        );
        let item = TeamEventTarget::InboxItem {
            agent: "atlas".to_string(),
            item_id: "20260930_note".to_string(),
        };
        assert_eq!(
            serde_json::to_value(item).unwrap(),
            json!({ "kind": "inbox_item", "agent": "atlas", "item_id": "20260930_note" })
        );
    }

    #[test]
    fn every_kind_has_its_snake_case_name() {
        let names: Vec<_> = [
            TeamEventKind::HubStarted,
            TeamEventKind::AgentStarted,
            TeamEventKind::AgentStopped,
            TeamEventKind::AgentFailed,
            TeamEventKind::AgentCreated,
            TeamEventKind::AgentDeleted,
            TeamEventKind::AgentRestored,
            TeamEventKind::AgentReplied,
            TeamEventKind::SessionStarted,
            TeamEventKind::SessionFinished,
            TeamEventKind::InboxItemAdded,
            TeamEventKind::ScheduledRunFinished,
            TeamEventKind::HubNotice,
        ]
        .iter()
        .map(|kind| serde_json::to_value(kind).unwrap())
        .collect();
        assert_eq!(
            names,
            [
                "hub_started",
                "agent_started",
                "agent_stopped",
                "agent_failed",
                "agent_created",
                "agent_deleted",
                "agent_restored",
                "agent_replied",
                "session_started",
                "session_finished",
                "inbox_item_added",
                "scheduled_run_finished",
                "hub_notice",
            ]
        );
    }
}
