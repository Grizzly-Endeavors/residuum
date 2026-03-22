//! Types for scheduled actions.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A one-off scheduled action that fires at a specific time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledAction {
    /// Unique identifier (format: `action-{8hex}`).
    pub id: String,
    /// Human-readable name for this action.
    pub name: String,
    /// The prompt to execute when the action fires.
    pub prompt: String,
    /// When this action should fire (UTC).
    pub run_at: DateTime<Utc>,
    /// Agent routing: `None` = default sub-agent, `Some("main")` = main wake turn,
    /// `Some(preset)` = sub-agent with named preset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Model tier override (e.g. "small", "medium", "large").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_tier: Option<String>,
    /// When this action was created.
    pub created_at: DateTime<Utc>,
}

impl ScheduledAction {
    /// Generate a unique action ID in the form `action-{8 hex digits}`.
    #[must_use]
    pub fn generate_id() -> String {
        format!("action-{:08x}", rand::random::<u32>())
    }
}
