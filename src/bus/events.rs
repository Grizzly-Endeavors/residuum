//! Event types carried on the bus.

use std::path::PathBuf;

use chrono::NaiveDateTime;

use crate::bus::types::PresetName;
use crate::interfaces::types::MessageOrigin;
use crate::models::ImageData;

// ---------------------------------------------------------------------------
// EventTrigger
// ---------------------------------------------------------------------------

/// What triggered a background event or notification.
#[derive(Debug, Clone)]
pub(crate) enum EventTrigger {
    /// A recurring pulse (cron-style schedule).
    Pulse,
    /// A one-shot action.
    Action,
    /// A subagent spawned the work.
    Agent,
    /// An inbound webhook with the given name.
    Webhook(String),
}

// ---------------------------------------------------------------------------
// HeartbeatStatus
// ---------------------------------------------------------------------------

/// Whether a background task's periodic heartbeat carried content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HeartbeatStatus {
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
pub(crate) enum AgentResultStatus {
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

// ---------------------------------------------------------------------------
// Event structs
// ---------------------------------------------------------------------------

/// Inbound message from a user or external source.
#[derive(Debug, Clone)]
pub(crate) struct MessageEvent {
    /// Correlation ID for reply routing.
    pub(crate) id: String,
    /// Message content.
    pub(crate) content: String,
    /// Where this message originated.
    pub(crate) origin: MessageOrigin,
    /// Local timestamp (see `crate::time::now_local`).
    pub(crate) timestamp: NaiveDateTime,
    /// Inline images attached to the message.
    pub(crate) images: Vec<ImageData>,
}

/// Agent response destined for an endpoint.
#[derive(Debug, Clone)]
pub(crate) struct ResponseEvent {
    /// Links back to the originating message.
    pub(crate) correlation_id: String,
    /// Response body.
    pub(crate) content: String,
    /// Local timestamp.
    pub(crate) timestamp: NaiveDateTime,
}

/// Push notification for notify channels.
#[derive(Debug, Clone)]
pub(crate) struct NotificationEvent {
    /// Short label for the notification.
    pub(crate) title: String,
    /// Body/details.
    pub(crate) content: String,
    /// What produced this notification.
    pub(crate) source: EventTrigger,
    /// Local timestamp.
    pub(crate) timestamp: NaiveDateTime,
}

/// Result from a completed background or subagent task.
#[derive(Debug, Clone)]
pub(crate) struct AgentResultEvent {
    /// Unique task identifier.
    pub(crate) task_id: String,
    /// Human-readable task name.
    pub(crate) task_name: String,
    /// Subagent preset from the registry.
    pub(crate) preset: PresetName,
    /// What triggered this task.
    pub(crate) source: EventTrigger,
    /// Whether the last heartbeat was substantive.
    pub(crate) heartbeat_status: HeartbeatStatus,
    /// Terminal status.
    pub(crate) status: AgentResultStatus,
    /// Human-readable summary of the result.
    pub(crate) summary: String,
    /// Path to the full conversation transcript, if saved.
    pub(crate) transcript_path: Option<PathBuf>,
    /// Local timestamp.
    pub(crate) timestamp: NaiveDateTime,
}

// ---------------------------------------------------------------------------
// BusEvent
// ---------------------------------------------------------------------------

/// An event published onto the bus.
#[derive(Debug, Clone)]
pub(crate) enum BusEvent {
    /// Inbound user/external message.
    Message(MessageEvent),
    /// Agent response to an endpoint.
    Response(ResponseEvent),
    /// Push notification.
    Notification(NotificationEvent),
    /// Completed background/subagent result.
    AgentResult(AgentResultEvent),
    /// Raw webhook payload.
    WebhookPayload {
        /// The raw body content.
        body: String,
        /// MIME content type, if known.
        content_type: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
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
            task_name: "summarize".into(),
            preset: PresetName::from("summarizer"),
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
                assert_eq!(ar.transcript_path, Some(PathBuf::from("/tmp/transcript.json")));
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
            task_name: "review".into(),
            preset: PresetName::from("reviewer"),
            source: EventTrigger::Agent,
            heartbeat_status: HeartbeatStatus::Ok,
            status: AgentResultStatus::Completed,
            summary: "all good".into(),
            transcript_path: Some(PathBuf::from("/var/log/transcript.json")),
            timestamp: sample_timestamp(),
        };
        let cloned = ar.clone();
        assert_eq!(cloned.task_id, "t2");
        assert_eq!(cloned.task_name, "review");
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
            _ => panic!("expected Failed variant"),
        }
    }

    #[test]
    fn event_trigger_webhook_debug() {
        let trigger = EventTrigger::Webhook("github".into());
        let debug = format!("{trigger:?}");
        assert!(debug.contains("github"), "Debug should contain webhook name");
    }
}
