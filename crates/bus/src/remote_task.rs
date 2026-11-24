use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const UNREACHABLE_NOTICE_AFTER: chrono::Duration = chrono::Duration::minutes(10);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackedTask {
    pub sender_address: String,
    pub agent: String,
    pub task_id: String,
    pub context_id: String,
    pub state: String,
    #[serde(default)]
    pub last_status_text: Option<String>,
    pub hop_count: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub first_unreachable_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub unreachable_notified: bool,
    #[serde(default)]
    pub notified_this_turn: bool,
    #[serde(default)]
    pub stopped_by_user: bool,
}

impl TrackedTask {
    #[must_use]
    pub fn is_open(&self) -> bool {
        !matches!(
            self.state.as_str(),
            "completed" | "failed" | "canceled" | "rejected"
        )
    }

    #[must_use]
    pub fn awaits_reply(&self) -> bool {
        matches!(self.state.as_str(), "input_required" | "auth_required")
    }

    #[must_use]
    pub fn notice_due_at(&self) -> Option<DateTime<Utc>> {
        if !self.is_open() || self.stopped_by_user {
            return None;
        }
        self.first_unreachable_at
            .map(|since| since + UNREACHABLE_NOTICE_AFTER)
    }
}
