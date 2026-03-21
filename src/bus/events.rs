//! Event types carried on the bus.

use std::fmt;
use std::path::PathBuf;

use chrono::NaiveDateTime;

use crate::bus::types::PresetName;
use crate::config::BackgroundModelTier;
use crate::interfaces::types::MessageOrigin;
use crate::models::ImageData;

// ---------------------------------------------------------------------------
// EventTrigger
// ---------------------------------------------------------------------------

/// What triggered a background event or notification.
#[derive(Debug, Clone)]
pub enum EventTrigger {
    /// A recurring pulse (cron-style schedule).
    Pulse,
    /// A one-shot action.
    Action,
    /// A subagent spawned the work.
    Agent,
    /// An inbound webhook with the given name.
    Webhook(String),
}

impl EventTrigger {
    /// Lowercase label for display and serialization.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Pulse => "pulse",
            Self::Action => "action",
            Self::Agent => "agent",
            Self::Webhook(_) => "webhook",
        }
    }

    /// Human-readable label that includes the webhook name when applicable.
    #[must_use]
    pub fn display_label(&self) -> String {
        match self {
            Self::Webhook(name) => format!("webhook:{name}"),
            Self::Pulse | Self::Action | Self::Agent => self.as_str().to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// HeartbeatStatus
// ---------------------------------------------------------------------------

/// Whether a background task's periodic heartbeat carried content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeartbeatStatus {
    /// Heartbeat with no meaningful content.
    Ok,
    /// Heartbeat that produced user-visible output.
    Substantive,
}

// ---------------------------------------------------------------------------
// AgentResultStatus
// ---------------------------------------------------------------------------

/// Terminal status of a background/subagent task.
#[derive(Debug, Clone)]
pub enum AgentResultStatus {
    /// Task finished successfully.
    Completed,
    /// Task was cancelled before completion.
    Cancelled,
    /// Task failed with an error.
    Failed {
        /// Description of what went wrong.
        error: String,
    },
}

impl fmt::Display for AgentResultStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Completed => write!(f, "completed"),
            Self::Cancelled => write!(f, "cancelled"),
            Self::Failed { error } => write!(f, "failed: {error}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Event structs
// ---------------------------------------------------------------------------

/// Inbound message from a user or external source.
#[derive(Debug, Clone)]
pub struct MessageEvent {
    /// Correlation ID for reply routing.
    pub id: String,
    /// Message content.
    pub content: String,
    /// Where this message originated.
    pub origin: MessageOrigin,
    /// Local timestamp (see `crate::time::now_local`).
    pub timestamp: NaiveDateTime,
    /// Inline images attached to the message.
    pub images: Vec<ImageData>,
}

/// Agent response destined for an endpoint.
#[derive(Debug, Clone)]
pub struct ResponseEvent {
    /// Links back to the originating message.
    pub correlation_id: String,
    /// Response body.
    pub content: String,
    /// Local timestamp.
    pub timestamp: NaiveDateTime,
}

/// Push notification for notify channels.
#[derive(Debug, Clone)]
pub struct NotificationEvent {
    /// Short label for the notification.
    pub title: String,
    /// Body/details.
    pub content: String,
    /// What produced this notification.
    pub source: EventTrigger,
    /// Local timestamp.
    pub timestamp: NaiveDateTime,
}

/// Tool invocation sent by the agent during a turn.
#[derive(Debug, Clone)]
pub struct ToolCallEvent {
    /// Links back to the originating message.
    pub correlation_id: String,
    /// Unique identifier for this tool invocation.
    pub tool_call_id: String,
    /// Tool name.
    pub name: String,
    /// Tool arguments.
    pub arguments: serde_json::Value,
}

/// Result of a tool execution.
#[derive(Debug, Clone)]
pub struct ToolResultEvent {
    /// Links back to the originating message.
    pub correlation_id: String,
    /// Matches the originating tool call.
    pub tool_call_id: String,
    /// Tool name.
    pub name: String,
    /// Tool output text.
    pub output: String,
    /// Whether the tool reported an error.
    pub is_error: bool,
}

/// Intermediate model text emitted during a turn.
#[derive(Debug, Clone)]
pub struct IntermediateEvent {
    /// Links back to the originating message.
    pub correlation_id: String,
    /// Partial/intermediate content.
    pub content: String,
}

/// System event surfaced to endpoints.
#[derive(Debug, Clone)]
pub struct SystemEventData {
    /// Links back to the originating message.
    pub correlation_id: String,
    /// Source label (e.g. pulse name, action name).
    pub source: String,
    /// Event content.
    pub content: String,
    /// Local timestamp.
    pub timestamp: NaiveDateTime,
}

/// Result from a completed background or subagent task.
#[derive(Debug, Clone)]
pub struct AgentResultEvent {
    /// Unique task identifier.
    pub task_id: String,
    /// Human-readable source label (e.g. `"pulse:email_check"`, `"action:deploy"`).
    pub source_label: String,
    /// Subagent preset that ran the task.
    pub agent_preset: PresetName,
    /// What triggered this task.
    pub source: EventTrigger,
    /// Whether the last heartbeat was substantive.
    pub heartbeat_status: HeartbeatStatus,
    /// Terminal status.
    pub status: AgentResultStatus,
    /// Human-readable summary of the result.
    pub summary: String,
    /// Path to the full conversation transcript, if saved.
    pub transcript_path: Option<PathBuf>,
    /// Local timestamp.
    pub timestamp: NaiveDateTime,
}

impl AgentResultEvent {
    /// Format this result for injection into the agent's conversation context.
    #[must_use]
    pub fn format_for_agent(&self) -> String {
        let mut parts = vec![format!(
            "[Background Task Result]\nTask: {} ({})\nSource: {}\nStatus: {}",
            self.source_label,
            self.task_id,
            self.source.as_str(),
            self.status,
        )];

        if !self.summary.is_empty() {
            parts.push(format!("Output:\n{}", self.summary));
        }

        if let Some(ref path) = self.transcript_path {
            parts.push(format!("Transcript: {}", path.display()));
        }

        parts.join("\n")
    }
}

/// Request to spawn a sub-agent from any source.
#[derive(Debug, Clone)]
pub struct SpawnRequestEvent {
    /// Subagent preset to use for this spawn.
    pub preset: PresetName,
    /// Human-readable source label (e.g. `"pulse:email_check"`, `"agent:researcher"`).
    pub source_label: String,
    /// The prompt/instructions for the sub-agent.
    pub prompt: String,
    /// Additional context to prepend (e.g. project context).
    pub context: Option<String>,
    /// What triggered this spawn request.
    pub source: EventTrigger,
    /// Override the preset's model tier.
    pub model_tier_override: Option<BackgroundModelTier>,
}

/// Operational notice broadcast to connected endpoints.
#[derive(Debug, Clone)]
pub struct NoticeEvent {
    /// Human-readable notice message.
    pub message: String,
}

/// An error tied to a specific agent turn, broadcast to connected endpoints.
#[derive(Debug, Clone)]
pub struct ErrorEvent {
    /// Links back to the originating message.
    pub correlation_id: String,
    /// Error description.
    pub message: String,
}

// ---------------------------------------------------------------------------
// Typed topic event enums
// ---------------------------------------------------------------------------

/// Tool activity during a turn (call or result).
#[derive(Debug, Clone)]
pub enum ToolActivityEvent {
    /// A tool was invoked by the agent.
    Call(ToolCallEvent),
    /// A tool execution completed.
    Result(ToolResultEvent),
}

/// Turn lifecycle transitions.
#[derive(Debug, Clone)]
pub enum TurnLifecycleEvent {
    /// Agent turn has started processing.
    Started {
        /// Links back to the originating message.
        correlation_id: String,
    },
    /// Agent turn has finished processing.
    Ended {
        /// Links back to the originating message.
        correlation_id: String,
    },
}

/// System-wide messages (notices, errors, events).
#[derive(Debug, Clone)]
pub enum SystemMessageEvent {
    /// Operational notice (reload status, memory progress, command responses).
    Notice {
        /// Human-readable notice message.
        message: String,
    },
    /// An error tied to a specific agent turn.
    Error {
        /// Links back to the originating message.
        correlation_id: String,
        /// Error description.
        message: String,
    },
    /// System event surfaced to endpoints (pulse, action, etc.).
    Event(SystemEventData),
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(clippy::panic, reason = "test assertions")]
#[expect(clippy::indexing_slicing, reason = "test assertions")]
mod tests {
    use chrono::NaiveDate;

    use super::*;

    fn sample_timestamp() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 3, 13)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
    }

    #[test]
    fn agent_result_clone_preserves_all_fields() {
        let ar = AgentResultEvent {
            task_id: "t2".into(),
            source_label: "agent:review".into(),
            agent_preset: PresetName::from("reviewer"),
            source: EventTrigger::Agent,
            heartbeat_status: HeartbeatStatus::Ok,
            status: AgentResultStatus::Completed,
            summary: "all good".into(),
            transcript_path: Some(PathBuf::from("/var/log/transcript.json")),
            timestamp: sample_timestamp(),
        };
        let cloned = ar.clone();
        assert_eq!(cloned.task_id, "t2");
        assert_eq!(cloned.source_label, "agent:review");
        assert_eq!(cloned.summary, "all good");
        assert_eq!(
            cloned.transcript_path,
            Some(PathBuf::from("/var/log/transcript.json"))
        );
    }

    #[test]
    fn heartbeat_status_equality() {
        assert_eq!(HeartbeatStatus::Ok, HeartbeatStatus::Ok);
        assert_eq!(HeartbeatStatus::Substantive, HeartbeatStatus::Substantive);
        assert_ne!(HeartbeatStatus::Ok, HeartbeatStatus::Substantive);
    }

    #[test]
    fn agent_result_status_failed_clone() {
        let status = AgentResultStatus::Failed {
            error: "timeout".into(),
        };
        let cloned = status.clone();
        match cloned {
            AgentResultStatus::Failed { error } => assert_eq!(error, "timeout"),
            AgentResultStatus::Completed | AgentResultStatus::Cancelled => {
                panic!("expected Failed variant")
            }
        }
    }

    #[test]
    fn event_trigger_webhook_debug() {
        let trigger = EventTrigger::Webhook("github".into());
        let debug = format!("{trigger:?}");
        assert!(
            debug.contains("github"),
            "Debug should contain webhook name"
        );
    }

    #[test]
    fn event_trigger_as_str() {
        assert_eq!(EventTrigger::Pulse.as_str(), "pulse");
        assert_eq!(EventTrigger::Action.as_str(), "action");
        assert_eq!(EventTrigger::Agent.as_str(), "agent");
        assert_eq!(EventTrigger::Webhook("github".into()).as_str(), "webhook");
        // Webhook name does not affect the label.
        assert_eq!(EventTrigger::Webhook("custom".into()).as_str(), "webhook");
    }

    #[test]
    fn streaming_event_clone() {
        let tc = ToolCallEvent {
            correlation_id: "c1".into(),
            tool_call_id: "t1".into(),
            name: "read".into(),
            arguments: serde_json::json!({"path": "/tmp"}),
        };
        let cloned = tc.clone();
        assert_eq!(cloned.name, "read");
        assert_eq!(cloned.arguments["path"], "/tmp");

        let se = SystemEventData {
            correlation_id: "c1".into(),
            source: "pulse".into(),
            content: "check".into(),
            timestamp: sample_timestamp(),
        };
        let cloned_se = se.clone();
        assert_eq!(cloned_se.source, "pulse");
    }
}
