//! Auto Mode's verdict on one tool call, carried on the bus beside the call's
//! result. The decision model and rules live with the agent.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// What Auto Mode decided about one tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum AutoModeDecision {
    /// Checked; no deny rule matched, or an allow rule overrode the match.
    Allowed,
    /// Checked; a deny rule matched and no allow rule did. The call didn't run.
    Blocked,
    /// The decision model couldn't answer, so the call ran without a check.
    Unchecked,
}

/// Auto Mode's verdict on one tool call, shown beside the call's result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AutoModeVerdict {
    pub decision: AutoModeDecision,
    /// For `blocked`, the deny rule that matched. For `allowed`, the allow
    /// rule that overrode a matching deny rule, if one did.
    pub rule: Option<String>,
    /// How likely the decision model judged `rule` to apply, 0 to 1.
    pub probability: Option<f64>,
    /// For `unchecked`, why the check couldn't run, in plain words.
    pub reason: Option<String>,
    /// Input tokens the check cost.
    #[ts(type = "number | null")]
    pub input_tokens: Option<u64>,
}

impl AutoModeVerdict {
    /// The verdict for a check that could not run, carrying why.
    #[must_use]
    pub fn unchecked(reason: String) -> Self {
        Self {
            decision: AutoModeDecision::Unchecked,
            rule: None,
            probability: None,
            reason: Some(reason),
            input_tokens: None,
        }
    }

    /// The tool result the agent sees in place of a blocked call's output.
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
