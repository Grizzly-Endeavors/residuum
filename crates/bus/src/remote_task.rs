//! A task this instance sent to a remote agent over A2A, as the bus carries
//! it to the web UI. The tracker that persists and watches it lives with A2A.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// How long an agent must stay unreachable before the sender gets a single
/// notice; retries continue either way.
pub const UNREACHABLE_NOTICE_AFTER: chrono::Duration = chrono::Duration::minutes(10);

/// One tracked outbound task, persisted at `{workspace}/a2a/outbound.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackedTask {
    pub sender_address: String,
    pub agent: String,
    pub task_id: String,
    pub context_id: String,
    /// A plain word from `task_state_str` — `"working"`, `"input_required"`,
    /// `"completed"`, etc.
    pub state: String,
    #[serde(default)]
    pub last_status_text: Option<String>,
    pub hop_count: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// When this task's agent was first found unreachable in the current
    /// unreachable streak; cleared on the next successful contact.
    #[serde(default)]
    pub first_unreachable_at: Option<DateTime<Utc>>,
    /// Whether the unreachable notice (sent once a streak passes
    /// [`UNREACHABLE_NOTICE_AFTER`]) has already fired for the current
    /// streak.
    #[serde(default)]
    pub unreachable_notified: bool,
    /// Whether a delivery has already fired for the current turn (since the
    /// last `track` call on the tracker) — a direct cancel and the
    /// background watcher can both observe the same terminal event for one
    /// turn, and only the first should reach the sender. Reset to `false`
    /// every time `track` records a new send or follow-up.
    #[serde(default)]
    pub notified_this_turn: bool,
    /// Set when the user stopped watching this task because its agent
    /// couldn't be reached to cancel it (see
    /// `stop_watching` on the tracker). A late update from the agent
    /// is ignored rather than reopening a task the user closed.
    #[serde(default)]
    pub stopped_by_user: bool,
}

impl TrackedTask {
    /// Not in a terminal state — still worth watching (or resuming a watch
    /// for, e.g. after a follow-up).
    #[must_use]
    pub fn is_open(&self) -> bool {
        !matches!(
            self.state.as_str(),
            "completed" | "failed" | "canceled" | "rejected"
        )
    }

    /// Waiting on the sender before it can make progress.
    #[must_use]
    pub fn awaits_reply(&self) -> bool {
        matches!(self.state.as_str(), "input_required" | "auth_required")
    }

    /// When the task's unreachable streak passes [`UNREACHABLE_NOTICE_AFTER`]
    /// and becomes one the sender and the user are told about. `None` for a
    /// task that isn't in a streak, is closed, or was stopped by the user.
    #[must_use]
    pub fn notice_due_at(&self) -> Option<DateTime<Utc>> {
        if !self.is_open() || self.stopped_by_user {
            return None;
        }
        self.first_unreachable_at
            .map(|since| since + UNREACHABLE_NOTICE_AFTER)
    }
}
