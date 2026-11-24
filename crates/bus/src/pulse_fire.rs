use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::events::PulseOverlap;
use crate::types::SessionAddress;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PulseFirePhase {
    Script,
    Session,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PulseFireStatus {
    Ok,
    Failed,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PulseFireOutcome {
    pub status: PulseFireStatus,
    pub at: DateTime<Utc>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PulseFireRun {
    pub fire_id: Option<String>,
    pub phase: PulseFirePhase,
    pub started_at: DateTime<Utc>,
    pub address: Option<SessionAddress>,
    pub run_id: Option<String>,
    pub overlap: Option<PulseOverlap>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PulseFireChange {
    Started,
    SessionRequested,
    SessionStarted,
    Finished {
        outcome: PulseFireOutcome,
        session_run_id: Option<String>,
    },
    Ended,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PulseFireEvent {
    pub pulse: String,
    pub fire_id: String,
    pub change: PulseFireChange,
    pub current_run: Option<PulseFireRun>,
}
