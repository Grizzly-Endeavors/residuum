//! Background task types: task definitions, execution configs, and results.

use std::path::PathBuf;

use chrono::{DateTime, Utc};

use crate::bus::{AgentResultStatus, EventTrigger, PresetName};
use crate::config::BackgroundModelTier;

/// A background task to be executed by the spawner.
#[derive(Debug, Clone)]
pub(crate) struct BackgroundTask {
    /// Unique task identifier.
    pub id: String,
    /// Human-readable source label (e.g. `"pulse:email_check"`, `"action:deploy"`).
    pub source_label: String,
    /// Where this task originated.
    pub source: EventTrigger,
    /// Configuration for the sub-agent that runs this task.
    pub subagent_config: SubAgentConfig,
    /// The agent preset that will run this task.
    pub agent_preset: PresetName,
}

/// Configuration for a sub-agent background task.
#[derive(Debug, Clone)]
pub struct SubAgentConfig {
    /// The prompt/instructions for the sub-agent.
    pub prompt: String,
    /// Additional context to prepend (e.g. project context).
    pub context: Option<String>,
    /// Which model tier to use.
    pub model_tier: BackgroundModelTier,
}

/// The result of a completed background task.
#[derive(Debug, Clone)]
pub struct BackgroundResult {
    /// The task ID.
    pub id: String,
    /// Human-readable source label (e.g. `"pulse:email_check"`, `"action:deploy"`).
    pub source_label: String,
    /// Where this task originated.
    pub source: EventTrigger,
    /// Summary of the result (text output or error message).
    pub summary: String,
    /// Path to the transcript/log file (if written).
    pub transcript_path: Option<PathBuf>,
    /// Completion status.
    pub status: AgentResultStatus,
    /// When the task completed.
    pub timestamp: DateTime<Utc>,
    /// The agent preset that ran this task.
    pub agent_preset: PresetName,
}

/// Metadata tracked for a currently-running background task.
#[derive(Debug, Clone)]
pub struct ActiveTaskInfo {
    /// Human-readable source label (e.g. `"pulse:email_check"`, `"action:deploy"`).
    pub source_label: String,
    /// Where this task originated.
    pub source: EventTrigger,
    /// Execution type label (always `"sub_agent"`).
    pub execution_type: &'static str,
    /// Truncated prompt or command preview (at most 120 chars).
    pub prompt_preview: String,
    /// When the task was spawned (UTC).
    pub started_at: DateTime<Utc>,
}

/// Extract prompt preview from a sub-agent config (truncated to 120 chars).
pub(crate) fn execution_info(config: &SubAgentConfig) -> String {
    config.prompt.chars().take(120).collect()
}

/// Format a `BackgroundResult` for injection into the agent message stream.
#[must_use]
pub fn format_background_result(result: &BackgroundResult) -> String {
    let source_kind = result.source.as_str();

    let mut parts = vec![format!(
        "[Background Task Result]\nTask: {} ({})\nSource: {}\nStatus: {}",
        result.source_label, result.id, source_kind, result.status
    )];

    if !result.summary.is_empty() {
        parts.push(format!("Output:\n{}", result.summary));
    }

    if let Some(path) = &result.transcript_path {
        parts.push(format!("Transcript: {}", path.display()));
    }

    parts.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_tier_default_is_medium() {
        assert_eq!(
            BackgroundModelTier::default(),
            BackgroundModelTier::Medium,
            "default tier should be medium"
        );
    }

    #[test]
    fn format_background_result_completed() {
        let result = BackgroundResult {
            id: "bg-001".to_string(),
            source_label: "action:email_check".to_string(),
            source: EventTrigger::Action,
            summary: "3 new emails found".to_string(),
            transcript_path: None,
            status: AgentResultStatus::Completed,
            timestamp: Utc::now(),

            agent_preset: PresetName::from("general-purpose"),
        };

        let formatted = format_background_result(&result);
        assert!(
            formatted.contains("action:email_check"),
            "should contain source label"
        );
        assert!(formatted.contains("bg-001"), "should contain task id");
        assert!(formatted.contains("action"), "should contain source");
        assert!(formatted.contains("completed"), "should contain status");
        assert!(
            formatted.contains("3 new emails found"),
            "should contain summary"
        );
    }

    #[test]
    fn format_background_result_failed() {
        let result = BackgroundResult {
            id: "bg-002".to_string(),
            source_label: "agent:deploy_check".to_string(),
            source: EventTrigger::Agent,
            summary: String::new(),
            transcript_path: Some(PathBuf::from("/tmp/bg-002.log")),
            status: AgentResultStatus::Failed {
                error: "connection refused".to_string(),
            },
            timestamp: Utc::now(),

            agent_preset: PresetName::from("general-purpose"),
        };

        let formatted = format_background_result(&result);
        assert!(formatted.contains("failed"), "should contain status");
        assert!(
            formatted.contains("connection refused"),
            "should contain error"
        );
        assert!(
            formatted.contains("/tmp/bg-002.log"),
            "should contain transcript path"
        );
    }

    #[test]
    fn format_background_result_cancelled() {
        let result = BackgroundResult {
            id: "bg-003".to_string(),
            source_label: "pulse:long_task".to_string(),
            source: EventTrigger::Pulse,
            summary: "partial output".to_string(),
            transcript_path: None,
            status: AgentResultStatus::Cancelled,
            timestamp: Utc::now(),

            agent_preset: PresetName::from("general-purpose"),
        };

        let formatted = format_background_result(&result);
        assert!(formatted.contains("cancelled"), "should contain status");
        assert!(formatted.contains("pulse"), "should contain source");
        assert!(
            formatted.contains("partial output"),
            "should include non-empty summary"
        );
        assert!(
            !formatted.contains("Error:"),
            "cancelled task should not include Error: line"
        );
    }

    #[test]
    fn execution_info_subagent_truncates_at_120_chars() {
        let long_prompt = "x".repeat(200);
        let config = SubAgentConfig {
            prompt: long_prompt,
            context: None,
            model_tier: BackgroundModelTier::Medium,
        };
        let preview = execution_info(&config);
        assert_eq!(preview.len(), 120, "preview should be capped at 120 chars");
    }

    #[test]
    fn agent_result_status_display() {
        assert_eq!(AgentResultStatus::Completed.to_string(), "completed");
        assert_eq!(AgentResultStatus::Cancelled.to_string(), "cancelled");
        assert_eq!(
            AgentResultStatus::Failed {
                error: "timeout".to_string()
            }
            .to_string(),
            "failed: timeout"
        );
    }
}
