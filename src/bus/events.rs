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

/// Request to spawn a sub-agent from any source.
#[derive(Debug, Clone)]
pub struct SpawnRequestEvent {
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

// ---------------------------------------------------------------------------
// BusEvent
// ---------------------------------------------------------------------------

/// An event published onto the bus.
#[derive(Debug, Clone)]
pub enum BusEvent {
    /// Inbound user/external message.
    Message(MessageEvent),
    /// Agent response to an endpoint.
    Response(ResponseEvent),
    /// Push notification.
    Notification(NotificationEvent),
    /// Completed background/subagent result.
    AgentResult(AgentResultEvent),
    /// Request to spawn a sub-agent.
    SpawnRequest(SpawnRequestEvent),
    /// Tool invocation from the agent.
    ToolCall(ToolCallEvent),
    /// Tool execution result.
    ToolResult(ToolResultEvent),
    /// Intermediate model text.
    Intermediate(IntermediateEvent),
    /// System event (pulse, action, etc.).
    SystemEvent(SystemEventData),
    /// Agent turn has started processing.
    TurnStarted {
        /// Links back to the originating message.
        correlation_id: String,
    },
    /// Agent turn has finished processing.
    TurnEnded {
        /// Links back to the originating message.
        correlation_id: String,
    },
    /// Raw webhook payload.
    WebhookPayload {
        /// The raw body content.
        body: String,
        /// MIME content type, if known.
        content_type: Option<String>,
    },
    /// An error tied to a specific agent turn.
    Error {
        /// Links back to the originating message.
        correlation_id: String,
        /// Error description.
        message: String,
    },
    /// Operational notice (reload status, memory progress, command responses).
    Notice {
        /// Human-readable notice message.
        message: String,
    },
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "test assertions use wildcard for non-matching variants"
)]
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

    fn sample_origin() -> MessageOrigin {
        MessageOrigin {
            endpoint: "test".to_string(),
            sender_name: "tester".to_string(),
            sender_id: "t-1".to_string(),
        }
    }

    #[test]
    fn bus_event_message_variant() {
        let event = BusEvent::Message(MessageEvent {
            id: "m1".into(),
            content: "hello".into(),
            origin: sample_origin(),
            timestamp: sample_timestamp(),
            images: vec![],
        });
        match event {
            BusEvent::Message(msg) => {
                assert_eq!(msg.id, "m1");
                assert_eq!(msg.content, "hello");
            }
            _ => panic!("expected Message variant"),
        }
    }

    #[test]
    fn bus_event_response_variant() {
        let event = BusEvent::Response(ResponseEvent {
            correlation_id: "m1".into(),
            content: "reply".into(),
            timestamp: sample_timestamp(),
        });
        match event {
            BusEvent::Response(resp) => assert_eq!(resp.correlation_id, "m1"),
            _ => panic!("expected Response variant"),
        }
    }

    #[test]
    fn bus_event_notification_variant() {
        let event = BusEvent::Notification(NotificationEvent {
            title: "alert".into(),
            content: "something happened".into(),
            source: EventTrigger::Pulse,
            timestamp: sample_timestamp(),
        });
        match event {
            BusEvent::Notification(n) => assert_eq!(n.title, "alert"),
            _ => panic!("expected Notification variant"),
        }
    }

    #[test]
    fn bus_event_agent_result_variant() {
        let event = BusEvent::AgentResult(AgentResultEvent {
            task_id: "t1".into(),
            source_label: "action:summarize".into(),
            agent_preset: PresetName::from("summarizer"),
            source: EventTrigger::Action,
            heartbeat_status: HeartbeatStatus::Substantive,
            status: AgentResultStatus::Completed,
            summary: "done".into(),
            transcript_path: Some(PathBuf::from("/tmp/transcript.json")),
            timestamp: sample_timestamp(),
        });
        match event {
            BusEvent::AgentResult(ar) => {
                assert_eq!(ar.task_id, "t1");
                assert_eq!(
                    ar.transcript_path,
                    Some(PathBuf::from("/tmp/transcript.json"))
                );
            }
            _ => panic!("expected AgentResult variant"),
        }
    }

    #[test]
    fn bus_event_webhook_payload_variant() {
        let event = BusEvent::WebhookPayload {
            body: r#"{"push":"data"}"#.into(),
            content_type: Some("application/json".into()),
        };
        let cloned = event.clone();
        match cloned {
            BusEvent::WebhookPayload { body, content_type } => {
                assert_eq!(body, r#"{"push":"data"}"#);
                assert_eq!(content_type.as_deref(), Some("application/json"));
            }
            _ => panic!("expected WebhookPayload variant"),
        }
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
    fn bus_event_spawn_request_variant() {
        let event = BusEvent::SpawnRequest(SpawnRequestEvent {
            source_label: "agent:review".into(),
            prompt: "review the PR".into(),
            context: None,
            source: EventTrigger::Agent,
            model_tier_override: None,
        });
        match event {
            BusEvent::SpawnRequest(sr) => {
                assert_eq!(sr.source_label, "agent:review");
                assert_eq!(sr.prompt, "review the PR");
                assert!(sr.context.is_none());
                assert!(sr.model_tier_override.is_none());
            }
            _ => panic!("expected SpawnRequest variant"),
        }
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
            _ => panic!("expected Failed variant"),
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
    fn bus_event_tool_call_variant() {
        let event = BusEvent::ToolCall(ToolCallEvent {
            correlation_id: "m1".into(),
            tool_call_id: "tc1".into(),
            name: "search".into(),
            arguments: serde_json::json!({"query": "rust"}),
        });
        match event {
            BusEvent::ToolCall(tc) => {
                assert_eq!(tc.correlation_id, "m1");
                assert_eq!(tc.tool_call_id, "tc1");
                assert_eq!(tc.name, "search");
                assert_eq!(tc.arguments["query"], "rust");
            }
            _ => panic!("expected ToolCall variant"),
        }
    }

    #[test]
    fn bus_event_tool_result_variant() {
        let event = BusEvent::ToolResult(ToolResultEvent {
            correlation_id: "m1".into(),
            tool_call_id: "tc1".into(),
            name: "search".into(),
            output: "found 3 results".into(),
            is_error: false,
        });
        match event {
            BusEvent::ToolResult(tr) => {
                assert_eq!(tr.tool_call_id, "tc1");
                assert!(!tr.is_error);
            }
            _ => panic!("expected ToolResult variant"),
        }
    }

    #[test]
    fn bus_event_tool_result_error_variant() {
        let event = BusEvent::ToolResult(ToolResultEvent {
            correlation_id: "m1".into(),
            tool_call_id: "tc2".into(),
            name: "fetch".into(),
            output: "connection refused".into(),
            is_error: true,
        });
        match event {
            BusEvent::ToolResult(tr) => {
                assert!(tr.is_error);
                assert_eq!(tr.output, "connection refused");
            }
            _ => panic!("expected ToolResult variant"),
        }
    }

    #[test]
    fn bus_event_intermediate_variant() {
        let event = BusEvent::Intermediate(IntermediateEvent {
            correlation_id: "m1".into(),
            content: "thinking...".into(),
        });
        match event {
            BusEvent::Intermediate(im) => {
                assert_eq!(im.correlation_id, "m1");
                assert_eq!(im.content, "thinking...");
            }
            _ => panic!("expected Intermediate variant"),
        }
    }

    #[test]
    fn bus_event_system_event_variant() {
        let event = BusEvent::SystemEvent(SystemEventData {
            correlation_id: "m1".into(),
            source: "daily-summary".into(),
            content: "3 tasks completed".into(),
            timestamp: sample_timestamp(),
        });
        match event {
            BusEvent::SystemEvent(se) => {
                assert_eq!(se.source, "daily-summary");
                assert_eq!(se.content, "3 tasks completed");
            }
            _ => panic!("expected SystemEvent variant"),
        }
    }

    #[test]
    fn bus_event_turn_started_variant() {
        let event = BusEvent::TurnStarted {
            correlation_id: "m1".into(),
        };
        match event {
            BusEvent::TurnStarted { correlation_id } => assert_eq!(correlation_id, "m1"),
            _ => panic!("expected TurnStarted variant"),
        }
    }

    #[test]
    fn bus_event_turn_ended_variant() {
        let event = BusEvent::TurnEnded {
            correlation_id: "m1".into(),
        };
        match event {
            BusEvent::TurnEnded { correlation_id } => assert_eq!(correlation_id, "m1"),
            _ => panic!("expected TurnEnded variant"),
        }
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

    #[test]
    fn bus_event_error_variant() {
        let event = BusEvent::Error {
            correlation_id: "c1".into(),
            message: "something broke".into(),
        };
        match event {
            BusEvent::Error {
                correlation_id,
                message,
            } => {
                assert_eq!(correlation_id, "c1");
                assert_eq!(message, "something broke");
            }
            _ => panic!("expected Error variant"),
        }
    }

    #[test]
    fn bus_event_notice_variant() {
        let event = BusEvent::Notice {
            message: "reloading config".into(),
        };
        match event {
            BusEvent::Notice { message } => assert_eq!(message, "reloading config"),
            _ => panic!("expected Notice variant"),
        }
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
}
