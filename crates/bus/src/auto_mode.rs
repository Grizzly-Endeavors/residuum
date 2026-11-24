use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum AutoModeDecision {
    Allowed,
    Blocked,
    Unchecked,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AutoModeVerdict {
    pub decision: AutoModeDecision,
    pub rule: Option<String>,
    pub probability: Option<f64>,
    pub reason: Option<String>,
    #[ts(type = "number | null")]
    pub input_tokens: Option<u64>,
    #[ts(type = "number | null")]
    pub output_tokens: Option<u64>,
}

impl AutoModeVerdict {
    #[must_use]
    pub fn unchecked(reason: String) -> Self {
        Self {
            decision: AutoModeDecision::Unchecked,
            rule: None,
            probability: None,
            reason: Some(reason),
            input_tokens: None,
            output_tokens: None,
        }
    }

    #[must_use]
    pub fn blocked_message(&self) -> String {
        let rule = self.rule.as_deref().unwrap_or("one of the deny rules");
        let sure = self
            .probability
            .map(|p| format!(" ({:.0}% sure)", p * 100.0))
            .unwrap_or_default();
        format!(
            "Auto Mode blocked this call, so it did not run: it matches the rule \"{rule}\"{sure}. \
             Try a different approach that the rule allows, or ask the user to do it or to change the rule."
        )
    }
}
