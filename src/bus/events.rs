//! Event types carried on the bus.

use std::fmt;
use std::path::PathBuf;

use chrono::NaiveDateTime;

use crate::bus::types::SkillName;
use crate::config::BackgroundModelTier;
use crate::interfaces::attachment::FileAttachment;
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
}

impl fmt::Display for EventTrigger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Webhook(name) => write!(f, "webhook:{name}"),
            other @ (Self::Pulse | Self::Action | Self::Agent) => f.write_str(other.as_str()),
        }
    }
}

// ---------------------------------------------------------------------------
// ResultDisposition
// ---------------------------------------------------------------------------

/// Sentinel an agent puts in its summary to signal nothing needs surfacing.
pub const HEARTBEAT_OK: &str = "HEARTBEAT_OK";

/// Sentinel an agent puts in its summary to signal the result cannot wait.
///
/// Deliberately distinct from the word "urgent" so that a summary *about*
/// something urgent does not escalate itself by accident.
pub const HEARTBEAT_URGENT: &str = "HEARTBEAT_URGENT";

/// What the producing agent signalled should happen with its result.
///
/// The agent that ran the task is the only participant with the full
/// transcript and the task's intent, so it makes this call itself via
/// sentinel strings in its summary rather than deferring to a downstream
/// classifier working from a summary alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultDisposition {
    /// Nothing worth surfacing; discard without delivering anywhere.
    Silent,
    /// Ordinary result; file it for review.
    Normal,
    /// Needs attention now; push it as well as filing it.
    Urgent,
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
    /// Optional file attachment.
    pub attachment: Option<FileAttachment>,
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
    /// Whether this needs attention now. Drives platform interruption level.
    pub urgent: bool,
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

/// Result from a completed background or subagent task.
#[derive(Debug, Clone)]
pub struct AgentResultEvent {
    /// Unique task identifier.
    pub task_id: String,
    /// Human-readable source label (e.g. `"pulse:email_check"`, `"action:deploy"`).
    pub source_label: String,
    /// Skill the sub-agent ran with, if any.
    pub agent_skill: Option<SkillName>,
    /// What triggered this task.
    pub source: EventTrigger,
    /// What the producing agent signalled should happen with this result.
    pub disposition: ResultDisposition,
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
        let mut out = format!(
            "[Background Task Result]\nTask: {} ({})\nSource: {}\nStatus: {}",
            self.source_label,
            self.task_id,
            self.source.as_str(),
            self.status,
        );

        if !self.summary.is_empty() {
            out.push('\n');
            out.push_str("Output:\n");
            out.push_str(&self.summary);
        }

        if let Some(ref path) = self.transcript_path {
            out.push_str("\nTranscript: ");
            out.push_str(&path.display().to_string());
        }

        out
    }
}

/// Request to spawn a sub-agent from any source.
#[derive(Debug, Clone)]
pub struct SpawnRequestEvent {
    /// Skill to activate for this sub-agent, if any.
    pub skill: Option<SkillName>,
    /// Human-readable source label (e.g. `"pulse:email_check"`, `"agent:researcher"`).
    pub source_label: String,
    /// The prompt/instructions for the sub-agent.
    pub prompt: String,
    /// Additional context to prepend to the sub-agent's prompt.
    pub context: Option<String>,
    /// What triggered this spawn request.
    pub source: EventTrigger,
    /// Model tier to run the sub-agent at.
    pub model_tier: BackgroundModelTier,
    /// Render SOUL.md, AGENTS.md, and MEMORY.md into the sub-agent's prompt.
    pub include_identity: bool,
}

/// Operational notice broadcast to connected endpoints.
#[derive(Debug, Clone)]
pub struct NoticeEvent {
    /// Human-readable notice message.
    pub message: String,
}

/// Multi-line command output meant for inline rendering in a chat surface.
///
/// Distinct from [`NoticeEvent`]: notices are transient toasts in the web UI,
/// while inline output lands in the chat stream so the user can scan it.
#[derive(Debug, Clone)]
pub struct InlineOutputEvent {
    /// Body to render inline.
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;
    use crate::bus::types::SkillName;

    fn make_agent_result(summary: &str, transcript: Option<&str>) -> AgentResultEvent {
        AgentResultEvent {
            task_id: "t1".into(),
            source_label: "pulse:check".into(),
            agent_skill: Some(SkillName::from("default")),
            source: EventTrigger::Pulse,
            disposition: ResultDisposition::Silent,
            status: AgentResultStatus::Completed,
            summary: summary.into(),
            transcript_path: transcript.map(std::path::PathBuf::from),
            timestamp: NaiveDate::from_ymd_opt(2026, 3, 13)
                .unwrap()
                .and_hms_opt(12, 0, 0)
                .unwrap(),
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
    fn event_trigger_display() {
        assert_eq!(
            EventTrigger::Webhook("github".into()).to_string(),
            "webhook:github"
        );
        assert_eq!(EventTrigger::Pulse.to_string(), "pulse");
        assert_eq!(EventTrigger::Action.to_string(), "action");
        assert_eq!(EventTrigger::Agent.to_string(), "agent");
    }

    #[test]
    fn agent_result_status_display() {
        assert_eq!(AgentResultStatus::Completed.to_string(), "completed");
        assert_eq!(AgentResultStatus::Cancelled.to_string(), "cancelled");
        assert_eq!(
            AgentResultStatus::Failed {
                error: "timeout".into()
            }
            .to_string(),
            "failed: timeout"
        );
    }

    #[test]
    fn format_for_agent_empty_summary_no_transcript() {
        let event = make_agent_result("", None);
        assert_eq!(
            event.format_for_agent(),
            "[Background Task Result]\nTask: pulse:check (t1)\nSource: pulse\nStatus: completed"
        );
    }

    #[test]
    fn format_for_agent_with_summary_no_transcript() {
        let event = make_agent_result("some output", None);
        assert_eq!(
            event.format_for_agent(),
            "[Background Task Result]\nTask: pulse:check (t1)\nSource: pulse\nStatus: completed\nOutput:\nsome output"
        );
    }

    #[test]
    fn format_for_agent_empty_summary_with_transcript() {
        let event = make_agent_result("", Some("/tmp/transcript.txt"));
        assert_eq!(
            event.format_for_agent(),
            "[Background Task Result]\nTask: pulse:check (t1)\nSource: pulse\nStatus: completed\nTranscript: /tmp/transcript.txt"
        );
    }

    #[test]
    fn format_for_agent_with_summary_and_transcript() {
        let event = make_agent_result("some output", Some("/tmp/transcript.txt"));
        assert_eq!(
            event.format_for_agent(),
            "[Background Task Result]\nTask: pulse:check (t1)\nSource: pulse\nStatus: completed\nOutput:\nsome output\nTranscript: /tmp/transcript.txt"
        );
    }
}
