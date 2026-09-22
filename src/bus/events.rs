//! Event types carried on the bus.

use std::fmt;
use std::path::PathBuf;

use chrono::NaiveDateTime;

use crate::bus::types::{SessionAddress, SkillName};
use crate::config::BackgroundModelTier;
use crate::inference::{ImageData, MessageSender};
use crate::interfaces::attachment::FileAttachment;
use crate::interfaces::types::MessageOrigin;

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
    /// A message in a conversation the interfaces admitted but that doesn't
    /// belong to the main agent's own conversation (a group chat, a channel,
    /// or a non-owner DM) — routed to that conversation's `external` session.
    Conversation,
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
            Self::Conversation => "conversation",
        }
    }
}

impl fmt::Display for EventTrigger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Webhook(name) => write!(f, "webhook:{name}"),
            other @ (Self::Pulse | Self::Action | Self::Agent | Self::Conversation) => {
                f.write_str(other.as_str())
            }
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
    /// Earlier conversation the sender's interface supplies as background
    /// (e.g. group chat messages leading up to an @mention).
    pub context: Option<String>,
}

impl MessageEvent {
    /// Build a `MessageEvent` addressed to the main agent from an internal,
    /// non-interface source (an agent-to-main relay, a delivery-failure
    /// notice). Always carries background origin — `endpoint: "background"`,
    /// no sender, no conversation — so `MessageOrigin::belongs_to_main`
    /// is true for it and it always reaches main, never a conversation
    /// session.
    #[must_use]
    pub fn from_background(content: String) -> Self {
        Self {
            id: format!("bg-{}", uuid::Uuid::new_v4()),
            content,
            origin: MessageOrigin {
                endpoint: "background".to_string(),
                sender: None,
                conversation: None,
            },
            timestamp: chrono::Utc::now().naive_utc(),
            images: Vec::new(),
            context: None,
        }
    }
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
    /// Conversation on the endpoint to deliver to (an ID from
    /// `list_conversations`). When `None` the interface routes by
    /// correlation ID: the conversation that started the turn, else the owner.
    pub conversation: Option<String>,
}

/// A conversation session's turn output, destined for its own conversation
/// on a chat interface.
///
/// Distinct from [`ResponseEvent`] (the main agent's replies and
/// `send_message` posts) so an interface's delivery code can enforce the
/// "never falls back to the owner's DM" rule for a session's own output
/// without touching the main agent's delivery path, which keeps its existing
/// owner-DM fallback on an unresolvable target.
#[derive(Debug, Clone)]
pub struct SessionResponseEvent {
    /// Address of the session that produced this output, for the error log
    /// and failure notice to main when delivery fails.
    pub session_address: SessionAddress,
    /// The conversation this output replies to, on the interface subscribed
    /// to this endpoint.
    pub conversation_id: String,
    /// Response body.
    pub content: String,
    /// Optional file attachment.
    pub attachment: Option<FileAttachment>,
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

/// Result from a completed agent session run.
#[derive(Debug, Clone)]
pub struct AgentResultEvent {
    /// Address of the session that produced this result.
    pub session_address: SessionAddress,
    /// Unique identifier of the run within the session.
    pub run_id: String,
    /// Human-readable source label (e.g. `"pulse:email_check"`, `"action:deploy"`).
    pub source_label: String,
    /// Skill the session ran with, if any.
    pub agent_skill: Option<SkillName>,
    /// What triggered this session.
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
            "[Session Result]\nSession: {} ({})\nTask: {}\nSource: {}\nStatus: {}",
            self.session_address,
            self.run_id,
            self.source_label,
            self.source.as_str(),
            self.status,
        );

        if !self.summary.is_empty() {
            out.push('\n');
            out.push_str("Output:\n");
            out.push_str(&self.summary);
        }

        match &self.transcript_path {
            Some(path) => {
                out.push_str("\nTranscript: ");
                out.push_str(&path.display().to_string());
            }
            None => out.push_str("\nTranscript: unavailable (failed to save)"),
        }

        out
    }
}

/// A message one agent sends to another by address (`message_agent` tool).
///
/// Delivered as an [`crate::agent::interrupt::Interrupt::AgentMessage`] to a
/// live session (an interrupt when its turn is running, a new turn's input
/// when it's idle). The main agent's own delivery reuses the existing
/// `MessageEvent`/`UserMessage` bus path instead, which already implements
/// the same interrupt-if-running/new-turn-if-idle behavior.
#[derive(Debug, Clone)]
pub struct AgentMessageEvent {
    /// Address of the sending agent (`"main"` or a session address).
    pub from: SessionAddress,
    /// The sender's category label (`"main"`, `"scheduled"`, `"external"`, or `"spawned"`).
    pub from_category: String,
    /// The message body.
    pub content: String,
    /// Hop count carried by this message: one more than the highest hop
    /// count among the inputs that drove the sending turn. External-origin
    /// input (a user message, a pulse/action firing, a webhook, a web
    /// sidebar message) is hop `0`.
    pub hop_count: u32,
}

impl AgentMessageEvent {
    /// Format this message for injection into the recipient's conversation,
    /// naming the sender's address and category so the recipient can reply.
    #[must_use]
    pub fn format_for_agent(&self) -> String {
        format!(
            "[Agent Message from {} ({})]\n{}",
            self.from, self.from_category, self.content
        )
    }
}

/// Request to spawn an agent session from any source.
#[derive(Debug, Clone)]
pub struct SpawnRequestEvent {
    /// Address the resulting session will run under, generated by the caller
    /// up front so a synchronous caller (e.g. the `subagent_spawn` tool) can
    /// hand it back immediately.
    pub address: SessionAddress,
    /// Skill to activate for this session, if any.
    pub skill: Option<SkillName>,
    /// Human-readable source label (e.g. `"pulse:email_check"`, `"agent:researcher"`).
    pub source_label: String,
    /// The prompt/instructions for the session.
    pub prompt: String,
    /// Additional context to prepend to the session's prompt.
    pub context: Option<String>,
    /// What triggered this spawn request.
    pub source: EventTrigger,
    /// Model tier to run the session at.
    pub model_tier: BackgroundModelTier,
    /// The agent that requested this spawn: `Some(address)` for a `spawned`
    /// session (main or another session), `None` for `scheduled` and
    /// `external` sessions, which have no spawner.
    pub spawner: Option<SessionAddress>,
    /// Depth from the main agent this session will run at (main = 0). A
    /// `scheduled`/`external` session is always depth 1; a spawned session is
    /// its spawner's depth plus 1.
    pub depth: u32,
    /// Hop count for the new run's first turn: `0` for a `scheduled`/
    /// `external` trigger (external-origin input), one more than the
    /// spawning turn's highest input hop count for an agent-initiated spawn,
    /// or the hop count of the message that triggered a resume.
    pub hop_count: u32,
    /// Who sent this session's kickoff message, for a `Conversation`-triggered
    /// spawn — carried onto the fork's opening message so the session sees
    /// the same `[From: name via interface (location)]` attribution the main
    /// agent shows. `None` for every other trigger.
    pub sender: Option<MessageSender>,
    /// The conversation this session replies to, for a `Conversation`-triggered
    /// spawn. `None` for every other trigger, which have no conversation of
    /// their own to reply into.
    pub conversation: Option<ConversationTarget>,
}

/// The conversation an `external` conversation session replies to: which
/// interface endpoint, and which conversation on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationTarget {
    /// Endpoint name the conversation lives on (e.g. `"discord"`).
    pub endpoint: String,
    /// Stable conversation id on that endpoint (an id `list_conversations`
    /// and `send_message` use to reach it).
    pub conversation_id: String,
}

impl SpawnRequestEvent {
    /// Combine this request's context and prompt into a turn's opening
    /// message, the same way a fresh fork's `TurnKickoff::Initial` does.
    /// Used when a spawn or resume attempt discovers the target address is
    /// already live and falls back to delivering this request's content into
    /// that run as a message instead of forking a second one.
    #[must_use]
    pub fn kickoff_text(&self) -> String {
        match &self.context {
            Some(ctx) => format!("{ctx}\n\n{}", self.prompt),
            None => self.prompt.clone(),
        }
    }
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
            session_address: SessionAddress::from("scheduled-check-0001"),
            run_id: "t1".into(),
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
            "[Session Result]\nSession: scheduled-check-0001 (t1)\nTask: pulse:check\nSource: pulse\nStatus: completed\nTranscript: unavailable (failed to save)"
        );
    }

    #[test]
    fn format_for_agent_with_summary_no_transcript() {
        let event = make_agent_result("some output", None);
        assert_eq!(
            event.format_for_agent(),
            "[Session Result]\nSession: scheduled-check-0001 (t1)\nTask: pulse:check\nSource: pulse\nStatus: completed\nOutput:\nsome output\nTranscript: unavailable (failed to save)"
        );
    }

    #[test]
    fn format_for_agent_empty_summary_with_transcript() {
        let event = make_agent_result("", Some("/tmp/transcript.txt"));
        assert_eq!(
            event.format_for_agent(),
            "[Session Result]\nSession: scheduled-check-0001 (t1)\nTask: pulse:check\nSource: pulse\nStatus: completed\nTranscript: /tmp/transcript.txt"
        );
    }

    #[test]
    fn agent_message_event_format_for_agent_names_sender_and_category() {
        let msg = AgentMessageEvent {
            from: SessionAddress::from("spawned-researcher-3f9a"),
            from_category: "spawned".to_string(),
            content: "found the answer".to_string(),
            hop_count: 0,
        };
        assert_eq!(
            msg.format_for_agent(),
            "[Agent Message from spawned-researcher-3f9a (spawned)]\nfound the answer"
        );
    }

    #[test]
    fn format_for_agent_with_summary_and_transcript() {
        let event = make_agent_result("some output", Some("/tmp/transcript.txt"));
        assert_eq!(
            event.format_for_agent(),
            "[Session Result]\nSession: scheduled-check-0001 (t1)\nTask: pulse:check\nSource: pulse\nStatus: completed\nOutput:\nsome output\nTranscript: /tmp/transcript.txt"
        );
    }
}
