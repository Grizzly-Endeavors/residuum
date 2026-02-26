//! Background task types: task definitions, execution configs, and results.

use std::fmt;
use std::path::PathBuf;

use chrono::{DateTime, Utc};

use crate::config::BackgroundModelTier;
use crate::notify::types::TaskSource;

/// A background task to be executed by the spawner.
#[derive(Debug, Clone)]
pub struct BackgroundTask {
    /// Unique task identifier.
    pub id: String,
    /// Human-readable task name (used for NOTIFY.yml routing).
    pub task_name: String,
    /// Where this task originated.
    pub source: TaskSource,
    /// How to execute the task.
    pub execution: Execution,
    /// How to route the result.
    pub routing: ResultRouting,
}

/// How to route a background task result.
#[derive(Debug, Clone)]
pub enum ResultRouting {
    /// Route through NOTIFY.yml based on `task_name`.
    Notify,
    /// Dispatch directly to the named channels (bypasses NOTIFY.yml).
    Direct(Vec<String>),
}

/// How to execute the task.
#[derive(Debug, Clone)]
pub enum Execution {
    /// Run a sub-agent LLM turn.
    SubAgent(SubAgentConfig),
    /// Run a shell script/command.
    Script(ScriptConfig),
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

/// Configuration for a script background task.
#[derive(Debug, Clone)]
pub struct ScriptConfig {
    /// Command to execute.
    pub command: String,
    /// Arguments to pass.
    pub args: Vec<String>,
    /// Working directory (defaults to workspace root).
    pub working_dir: Option<PathBuf>,
    /// Timeout in seconds (defaults to 120).
    pub timeout_secs: Option<u64>,
}

/// The result of a completed background task.
#[derive(Debug, Clone)]
pub struct BackgroundResult {
    /// The task ID.
    pub id: String,
    /// Human-readable task name.
    pub task_name: String,
    /// Where this task originated.
    pub source: TaskSource,
    /// Summary of the result (text output or error message).
    pub summary: String,
    /// Path to the transcript/log file (if written).
    pub transcript_path: Option<PathBuf>,
    /// Completion status.
    pub status: TaskStatus,
    /// When the task completed.
    pub timestamp: DateTime<Utc>,
    /// How the result should be routed.
    pub routing: ResultRouting,
}

/// Metadata tracked for a currently-running background task.
#[derive(Debug, Clone)]
pub struct ActiveTaskInfo {
    /// Human-readable task name.
    pub task_name: String,
    /// Where this task originated.
    pub source: TaskSource,
    /// Execution variant label: `"sub_agent"` or `"script"`.
    pub execution_type: &'static str,
    /// Truncated prompt or command preview (at most 120 chars).
    pub prompt_preview: String,
    /// When the task was spawned (UTC).
    pub started_at: DateTime<Utc>,
}

/// Extract display info from an `Execution` config.
pub(crate) fn execution_info(execution: &Execution) -> (&'static str, String) {
    match execution {
        Execution::SubAgent(cfg) => {
            let preview = cfg.prompt.chars().take(120).collect();
            ("sub_agent", preview)
        }
        Execution::Script(cfg) => {
            let mut s = cfg.command.clone();
            if !cfg.args.is_empty() {
                s.push(' ');
                s.push_str(&cfg.args.join(" "));
            }
            ("script", s.chars().take(120).collect())
        }
    }
}

/// Completion status of a background task.
#[derive(Debug, Clone)]
pub enum TaskStatus {
    /// Task completed successfully.
    Completed,
    /// Task was cancelled via its cancellation token.
    Cancelled,
    /// Task failed with an error.
    Failed {
        /// Error description.
        error: String,
    },
}

impl fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Completed => write!(f, "completed"),
            Self::Cancelled => write!(f, "cancelled"),
            Self::Failed { error } => write!(f, "failed: {error}"),
        }
    }
}

/// Format a `BackgroundResult` for injection into the agent message stream.
#[must_use]
pub fn format_background_result(result: &BackgroundResult) -> String {
    let source_label = result.source.as_str();

    let mut parts = vec![format!(
        "[Background Task Result]\nTask: {} ({})\nSource: {}\nStatus: {}",
        result.task_name, result.id, source_label, result.status
    )];

    if !result.summary.is_empty() {
        parts.push(format!("Output:\n{}", result.summary));
    }

    if let TaskStatus::Failed { error } = &result.status {
        parts.push(format!("Error: {error}"));
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
            task_name: "email_check".to_string(),
            source: TaskSource::Cron,
            summary: "3 new emails found".to_string(),
            transcript_path: None,
            status: TaskStatus::Completed,
            timestamp: Utc::now(),
            routing: ResultRouting::Notify,
        };

        let formatted = format_background_result(&result);
        assert!(
            formatted.contains("email_check"),
            "should contain task name"
        );
        assert!(formatted.contains("bg-001"), "should contain task id");
        assert!(formatted.contains("cron"), "should contain source");
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
            task_name: "deploy_check".to_string(),
            source: TaskSource::Agent,
            summary: String::new(),
            transcript_path: Some(PathBuf::from("/tmp/bg-002.log")),
            status: TaskStatus::Failed {
                error: "connection refused".to_string(),
            },
            timestamp: Utc::now(),
            routing: ResultRouting::Direct(vec!["inbox".to_string()]),
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
            task_name: "long_task".to_string(),
            source: TaskSource::Pulse,
            summary: "partial output".to_string(),
            transcript_path: None,
            status: TaskStatus::Cancelled,
            timestamp: Utc::now(),
            routing: ResultRouting::Notify,
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
        let (exec_type, preview) = execution_info(&Execution::SubAgent(config));
        assert_eq!(exec_type, "sub_agent");
        assert_eq!(preview.len(), 120, "preview should be capped at 120 chars");
    }

    #[test]
    fn execution_info_script_joins_args() {
        use std::path::PathBuf;
        let config = crate::background::types::ScriptConfig {
            command: "echo".to_string(),
            args: vec!["hello".to_string(), "world".to_string()],
            working_dir: Some(PathBuf::from("/tmp")),
            timeout_secs: None,
        };
        let (exec_type, preview) = execution_info(&Execution::Script(config));
        assert_eq!(exec_type, "script");
        assert_eq!(preview, "echo hello world");
    }

    #[test]
    fn execution_info_script_no_args() {
        use std::path::PathBuf;
        let config = crate::background::types::ScriptConfig {
            command: "pwd".to_string(),
            args: vec![],
            working_dir: Some(PathBuf::from("/tmp")),
            timeout_secs: None,
        };
        let (_exec_type, preview) = execution_info(&Execution::Script(config));
        assert_eq!(preview, "pwd", "no trailing space when args is empty");
    }

    #[test]
    fn task_status_display() {
        assert_eq!(TaskStatus::Completed.to_string(), "completed");
        assert_eq!(TaskStatus::Cancelled.to_string(), "cancelled");
        assert_eq!(
            TaskStatus::Failed {
                error: "timeout".to_string()
            }
            .to_string(),
            "failed: timeout"
        );
    }
}
