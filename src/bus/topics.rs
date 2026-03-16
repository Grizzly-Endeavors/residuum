//! Typed topic definitions for the bus.
//!
//! Each topic struct maps to a specific event type via the [`Topic`] trait,
//! providing compile-time safety at publish/subscribe boundaries.

use super::types::{EndpointName, NotifyName, PresetName, TopicId};

// Re-export event types used by topic impls
use super::events::{
    AgentResultEvent, IntermediateEvent, MessageEvent, NotificationEvent, ResponseEvent,
    SpawnRequestEvent, SystemMessageEvent, ToolActivityEvent, TurnLifecycleEvent,
};

// ---------------------------------------------------------------------------
// Topic trait
// ---------------------------------------------------------------------------

/// Compile-time mapping from a topic struct to its event type and runtime key.
pub trait Topic {
    /// The event type carried on this topic.
    type Event: Clone + Send + Sync + 'static;

    /// The runtime key used by the broker for routing.
    fn topic_id(&self) -> TopicId;
}

// ---------------------------------------------------------------------------
// Topic structs
// ---------------------------------------------------------------------------

/// Inbound user messages destined for the main agent loop.
pub struct UserMessage;

impl Topic for UserMessage {
    type Event = MessageEvent;
    fn topic_id(&self) -> TopicId {
        TopicId::UserMessage
    }
}

/// Agent responses routed to a specific interactive endpoint.
pub struct Response(pub EndpointName);

impl Topic for Response {
    type Event = ResponseEvent;
    fn topic_id(&self) -> TopicId {
        TopicId::Response(self.0.clone())
    }
}

/// Tool call/result activity during a turn, routed per-endpoint.
pub struct ToolActivity(pub EndpointName);

impl Topic for ToolActivity {
    type Event = ToolActivityEvent;
    fn topic_id(&self) -> TopicId {
        TopicId::ToolActivity(self.0.clone())
    }
}

/// Turn start/end lifecycle events, routed per-endpoint.
pub struct TurnLifecycle(pub EndpointName);

impl Topic for TurnLifecycle {
    type Event = TurnLifecycleEvent;
    fn topic_id(&self) -> TopicId {
        TopicId::TurnLifecycle(self.0.clone())
    }
}

/// Intermediate model text during a turn, routed per-endpoint.
pub struct Intermediate(pub EndpointName);

impl Topic for Intermediate {
    type Event = IntermediateEvent;
    fn topic_id(&self) -> TopicId {
        TopicId::Intermediate(self.0.clone())
    }
}

/// Results from completed background/subagent tasks.
pub struct BackgroundResult;

impl Topic for BackgroundResult {
    type Event = AgentResultEvent;
    fn topic_id(&self) -> TopicId {
        TopicId::BackgroundResult
    }
}

/// Events emitted by running background tasks (reserved, no subscribers yet).
pub struct BackgroundEvent;

impl Topic for BackgroundEvent {
    type Event = AgentResultEvent;
    fn topic_id(&self) -> TopicId {
        TopicId::BackgroundEvent
    }
}

/// Request to spawn a sub-agent for a given preset.
pub struct SpawnRequest(pub PresetName);

impl Topic for SpawnRequest {
    type Event = SpawnRequestEvent;
    fn topic_id(&self) -> TopicId {
        TopicId::SpawnRequest(self.0.clone())
    }
}

/// Push notifications for a named channel.
pub struct Notification(pub NotifyName);

impl Topic for Notification {
    type Event = NotificationEvent;
    fn topic_id(&self) -> TopicId {
        TopicId::Notification(self.0.clone())
    }
}

/// The user inbox for incoming notifications.
pub struct Inbox;

impl Topic for Inbox {
    type Event = NotificationEvent;
    fn topic_id(&self) -> TopicId {
        TopicId::Inbox
    }
}

/// System-wide messages (notices, errors, events).
pub struct SystemMessage;

impl Topic for SystemMessage {
    type Event = SystemMessageEvent;
    fn topic_id(&self) -> TopicId {
        TopicId::SystemMessage
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_message_topic_id() {
        assert_eq!(UserMessage.topic_id(), TopicId::UserMessage);
    }

    #[test]
    fn response_topic_id() {
        let ep = EndpointName::from("ws");
        assert_eq!(Response(ep.clone()).topic_id(), TopicId::Response(ep));
    }

    #[test]
    fn tool_activity_topic_id() {
        let ep = EndpointName::from("discord");
        assert_eq!(
            ToolActivity(ep.clone()).topic_id(),
            TopicId::ToolActivity(ep)
        );
    }

    #[test]
    fn turn_lifecycle_topic_id() {
        let ep = EndpointName::from("telegram");
        assert_eq!(
            TurnLifecycle(ep.clone()).topic_id(),
            TopicId::TurnLifecycle(ep)
        );
    }

    #[test]
    fn intermediate_topic_id() {
        let ep = EndpointName::from("ws");
        assert_eq!(
            Intermediate(ep.clone()).topic_id(),
            TopicId::Intermediate(ep)
        );
    }

    #[test]
    fn background_result_topic_id() {
        assert_eq!(BackgroundResult.topic_id(), TopicId::BackgroundResult);
    }

    #[test]
    fn spawn_request_topic_id() {
        let preset = PresetName::from("summarizer");
        assert_eq!(
            SpawnRequest(preset.clone()).topic_id(),
            TopicId::SpawnRequest(preset)
        );
    }

    #[test]
    fn notification_topic_id() {
        let name = NotifyName::from("ntfy");
        assert_eq!(
            Notification(name.clone()).topic_id(),
            TopicId::Notification(name)
        );
    }

    #[test]
    fn inbox_topic_id() {
        assert_eq!(Inbox.topic_id(), TopicId::Inbox);
    }

    #[test]
    fn system_message_topic_id() {
        assert_eq!(SystemMessage.topic_id(), TopicId::SystemMessage);
    }
}
