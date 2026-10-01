//! The shapes of the team overview: the answer to `GET /api/hub/overview`
//! and the `overview` of the hub WebSocket's `agent_overview` frame. Field
//! names and JSON forms follow `docs/systems-usage/hub-http.md`.

use chrono::{DateTime, Utc};
use serde::Serialize;
use ts_rs::TS;

use crate::background::registry::{SessionCategory, SessionInfo, SessionState};

/// Who wrote the message an agent's overview shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum LastMessageRole {
    User,
    Assistant,
}

/// How exactly [`LastMessage::at`] says when the message was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TimePrecision {
    /// The time is the minute the message was written.
    Minute,
    /// Only the day is known: the message was read from an episode, which
    /// keeps the date and not the time. `at` is the start of that day.
    Day,
}

/// The newest message of an agent's main conversation that the user saw.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct LastMessage {
    /// Whether the user or the agent wrote it.
    pub role: LastMessageRole,
    /// The message as one line of plain text, at most 200 characters.
    pub preview: String,
    /// When it was written, as RFC 3339 with an offset.
    pub at: String,
    /// How exactly `at` says when.
    pub at_precision: TimePrecision,
}

/// A session run that is going on in a running agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct LiveSession {
    /// The session's stable address.
    pub address: String,
    /// This run's id.
    pub run_id: String,
    /// How the session was started.
    pub category: SessionCategory,
    /// What started it, such as `pulse:email_check` or `artifact:notes`.
    pub source_label: String,
    /// One line saying what the run is doing.
    pub purpose: String,
    /// Where the run is in its life.
    pub state: SessionState,
    /// When the run started.
    #[ts(type = "string")]
    pub started_at: DateTime<Utc>,
}

impl LiveSession {
    /// The overview's view of a run in an agent's session registry.
    #[must_use]
    pub fn of(info: &SessionInfo) -> Self {
        Self {
            address: info.address.to_string(),
            run_id: info.run_id.clone(),
            category: info.category,
            source_label: info.source_label.clone(),
            purpose: info.purpose.clone(),
            state: info.state,
            started_at: info.started_at,
        }
    }
}

/// What an upcoming run is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum UpcomingKind {
    Pulse,
    Action,
}

/// A pulse or scheduled action that will run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct UpcomingRun {
    /// Whether it is a pulse or a scheduled action.
    pub kind: UpcomingKind,
    /// The pulse's or action's name.
    pub name: String,
    /// When it runs next, as RFC 3339 with an offset.
    pub at: String,
}

/// A task the agent sent to a remote agent that has been unreachable for
/// longer than the notice threshold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct OutboundProblem {
    /// The tracked task's id.
    pub task_id: String,
    /// The remote agent's name.
    pub remote_agent: String,
    /// The last status the remote agent reported, if it reported one.
    pub status_text: Option<String>,
    /// When the remote agent was first found unreachable in this streak.
    #[ts(type = "string")]
    pub unreachable_since: DateTime<Utc>,
}

/// What Home shows about one agent beyond its state, activity and summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct AgentOverview {
    /// The agent's name.
    pub name: String,
    /// The newest message the user saw in the main conversation, if any.
    pub last_message: Option<LastMessage>,
    /// The session runs going on now, oldest first. None for an agent that
    /// isn't running.
    pub live_sessions: Vec<LiveSession>,
    /// The next runs of the agent's pulses and scheduled actions, soonest
    /// first, at most three.
    pub upcoming: Vec<UpcomingRun>,
    /// How many items in the agent's user inbox are unread.
    pub inbox_unread: u32,
    /// Remote agents the running agent has been unable to reach for a while.
    pub outbound_problems: Vec<OutboundProblem>,
}

impl AgentOverview {
    /// An overview of the agent `name` that shows nothing yet.
    #[must_use]
    pub fn empty(name: &str) -> Self {
        Self {
            name: name.to_string(),
            last_message: None,
            live_sessions: Vec::new(),
            upcoming: Vec::new(),
            inbox_unread: 0,
            outbound_problems: Vec::new(),
        }
    }
}

/// Every agent's overview: the answer to `GET /api/hub/overview`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct OverviewResponse {
    /// The hub process that answered, the same id `hub_boot` announces.
    pub boot_id: String,
    /// One overview per agent, sorted by name.
    pub agents: Vec<AgentOverview>,
}
