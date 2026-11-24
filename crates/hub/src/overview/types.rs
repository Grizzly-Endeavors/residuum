use chrono::{DateTime, Utc};
use serde::Serialize;
use ts_rs::TS;

use residuum_bus::session::{SessionCategory, SessionInfo, SessionState};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum LastMessageRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TimePrecision {
    Minute,
    Day,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct LastMessage {
    pub role: LastMessageRole,
    pub preview: String,
    pub at: String,
    pub at_precision: TimePrecision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct LiveSession {
    pub address: String,
    pub run_id: String,
    pub category: SessionCategory,
    pub source_label: String,
    pub purpose: String,
    pub state: SessionState,
    #[ts(type = "string")]
    pub started_at: DateTime<Utc>,
}

impl LiveSession {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum UpcomingKind {
    Pulse,
    Action,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct UpcomingRun {
    pub kind: UpcomingKind,
    pub name: String,
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct OutboundProblem {
    pub task_id: String,
    pub remote_agent: String,
    pub status_text: Option<String>,
    #[ts(type = "string")]
    pub unreachable_since: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct AgentOverview {
    pub name: String,
    pub last_message: Option<LastMessage>,
    pub live_sessions: Vec<LiveSession>,
    pub upcoming: Vec<UpcomingRun>,
    pub inbox_unread: u32,
    pub outbound_problems: Vec<OutboundProblem>,
}

impl AgentOverview {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct OverviewResponse {
    pub boot_id: String,
    pub agents: Vec<AgentOverview>,
}
