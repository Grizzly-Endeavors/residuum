//! The entries of the team event log and the page the hub API answers with.
//! Field names and JSON forms follow `docs/systems-usage/hub-http.md`.

use chrono::{DateTime, Utc};
use serde::Serialize;
use ts_rs::TS;

/// How serious an entry is. The log keeps the newest `warn` and `error`
/// entries longer than `info` ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TeamEventLevel {
    Info,
    Warn,
    Error,
}

/// What happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TeamEventKind {
    /// The hub process started.
    HubStarted,
    /// An agent finished starting.
    AgentStarted,
    /// An agent stopped, by request or because the hub shut down.
    AgentStopped,
    /// An agent couldn't start, or stopped without being asked to.
    AgentFailed,
    /// An agent was created.
    AgentCreated,
    /// An agent was deleted.
    AgentDeleted,
    /// A deleted agent was restored.
    AgentRestored,
    /// An agent answered the user in its main conversation.
    AgentReplied,
    /// A session started, other than a scheduled run.
    SessionStarted,
    /// A session ended, other than a scheduled run.
    SessionFinished,
    /// An agent added an item to the user's inbox.
    InboxItemAdded,
    /// A pulse or scheduled action finished its run.
    ScheduledRunFinished,
    /// The hub told everyone something.
    HubNotice,
}

/// A place in an agent's own view that an entry can point to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TeamEventPlace {
    Chat,
    Activity,
    Schedule,
    Files,
}

/// Where a client takes the user to see what an entry is about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum TeamEventTarget {
    /// One of an agent's places.
    AgentPlace {
        agent: String,
        place: TeamEventPlace,
    },
    /// One run of a session.
    Session { agent: String, run_id: String },
    /// One item of an agent's user inbox.
    InboxItem { agent: String, item_id: String },
}

/// One entry of the team event log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct TeamEvent {
    /// Increases with every entry within one hub process; ids are not
    /// reused, but evicted entries leave gaps.
    #[ts(type = "number")]
    pub id: u64,
    /// When it happened.
    #[ts(type = "string")]
    pub at: DateTime<Utc>,
    /// The agent it is about, when there is one.
    pub agent: Option<String>,
    /// What happened.
    pub kind: TeamEventKind,
    /// How serious it is.
    pub level: TeamEventLevel,
    /// One plain-language sentence saying what happened.
    pub summary: String,
    /// Where to take the user to see it, when there is such a place.
    pub target: Option<TeamEventTarget>,
}

/// An entry that has not been given its id yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTeamEvent {
    /// When it happened.
    pub at: DateTime<Utc>,
    /// The agent it is about, when there is one.
    pub agent: Option<String>,
    /// What happened.
    pub kind: TeamEventKind,
    /// How serious it is.
    pub level: TeamEventLevel,
    /// One plain-language sentence saying what happened.
    pub summary: String,
    /// Where to take the user to see it, when there is such a place.
    pub target: Option<TeamEventTarget>,
}

/// One page of the log, newest entry first: the answer to
/// `GET /api/hub/events`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct TeamEventPage {
    /// The id of the hub process that holds this log. A client that sees it
    /// change has a log from an earlier process and starts over.
    pub boot_id: String,
    /// The entries on this page, newest first.
    pub events: Vec<TeamEvent>,
    /// Pass as `before` to get the entries older than this page; `null` when
    /// there are none.
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
