//! Types for scheduled actions.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use residuum_config::ModelSelector;

/// A one-off scheduled action that fires at a specific time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduledAction {
    /// Unique identifier (format: `action-{8hex}`).
    pub id: String,
    /// Human-readable name for this action.
    pub name: String,
    /// The prompt to execute when the action fires.
    pub prompt: String,
    /// When this action should fire (UTC).
    pub run_at: DateTime<Utc>,
    /// Agent routing: `None` = plain session, `Some(skill)` = session with
    /// that skill activated. `Some("main")` is rejected both by the
    /// `schedule_action` tool and when this store loads from disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The session's model: a tier (`small`, `medium`, `large`) or an
    /// explicit `provider/model` id. `None` runs the `medium` tier. Stored as
    /// written, so one bad value can't make the whole file unreadable: see
    /// [`Self::model`].
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

    /// The model this action runs on: its `model_tier`, or the `medium` tier
    /// when it has none.
    ///
    /// # Errors
    /// Returns a plain-language message naming the value if `model_tier` is
    /// neither a tier nor a `provider/model` id.
    pub fn model(&self) -> Result<ModelSelector, String> {
        self.model_tier
            .as_deref()
            .map_or(Ok(ModelSelector::MEDIUM), str::parse)
    }
}
