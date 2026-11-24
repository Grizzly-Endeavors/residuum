use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use residuum_config::ModelSelector;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduledAction {
    pub id: String,
    pub name: String,
    pub prompt: String,
    pub run_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_tier: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl ScheduledAction {
    #[must_use]
    pub fn generate_id() -> String {
        format!("action-{:08x}", rand::random::<u32>())
    }

    pub fn model(&self) -> Result<ModelSelector, String> {
        self.model_tier
            .as_deref()
            .map_or(Ok(ModelSelector::MEDIUM), str::parse)
    }
}
