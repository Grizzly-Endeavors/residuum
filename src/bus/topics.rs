//! Typed topic definitions for the bus.
//!
//! Each topic struct is a routing domain that can carry multiple event types.
//! The [`Carries`] marker trait declares which event types are valid for a
//! topic, providing compile-time safety at publish/subscribe boundaries.

use super::events::{
    AgentResultEvent, ErrorEvent, InlineOutputEvent, IntermediateEvent, MessageEvent, NoticeEvent,
    NotificationEvent, ResponseEvent, SessionEvent, SessionResponseEvent, SpawnRequestEvent,
    ToolActivityEvent, TurnLifecycleEvent, WorkbenchEvent,
};
use super::types::{EndpointName, NotifyName, TopicId};

// ---------------------------------------------------------------------------
// Topic and Carries traits
// ---------------------------------------------------------------------------

/// A routing domain on the bus, identified by a runtime [`TopicId`].
pub trait Topic {
    /// The runtime key used by the broker for routing.
    fn topic_id(&self) -> TopicId;
}

/// Marker trait declaring that topic `Self` can carry events of type `E`.
///
/// This provides compile-time safety: publishing or subscribing to an
/// unsupported `(topic, event)` pair is a type error.
pub trait Carries<E: Clone + Send + Sync + 'static>: Topic {}

// ---------------------------------------------------------------------------
// Topic structs
// ---------------------------------------------------------------------------

/// Interactive endpoint turn activity.
///
/// Carries responses, tool call/result activity, turn lifecycle transitions,
/// and intermediate model text for a specific named endpoint.
pub struct Endpoint(pub EndpointName);

impl Topic for Endpoint {
    fn topic_id(&self) -> TopicId {
        TopicId::Endpoint(self.0.clone())
    }
}

impl Carries<ResponseEvent> for Endpoint {}
impl Carries<ToolActivityEvent> for Endpoint {}
impl Carries<TurnLifecycleEvent> for Endpoint {}
impl Carries<IntermediateEvent> for Endpoint {}
impl Carries<SessionResponseEvent> for Endpoint {}

/// Background task orchestration: spawn requests and task results.
pub struct Background;

impl Topic for Background {
    fn topic_id(&self) -> TopicId {
        TopicId::Background
    }
}

impl Carries<AgentResultEvent> for Background {}
impl Carries<SpawnRequestEvent> for Background {}

/// Agent session activity: lifecycle transitions and session-tagged turn
/// events, for surfaces that follow sessions live (the web UI).
pub struct Sessions;

impl Topic for Sessions {
    fn topic_id(&self) -> TopicId {
        TopicId::Sessions
    }
}

impl Carries<SessionEvent> for Sessions {}

/// Inbound user messages destined for the main agent loop.
pub struct UserMessage;

impl Topic for UserMessage {
    fn topic_id(&self) -> TopicId {
        TopicId::UserMessage
    }
}

impl Carries<MessageEvent> for UserMessage {}

/// Push notifications for a named channel.
///
/// The well-known channel `"system"` (see [`super::types::SYSTEM_CHANNEL`])
/// carries operational notices and errors broadcast to all connected endpoints.
pub struct Notification(pub NotifyName);

impl Topic for Notification {
    fn topic_id(&self) -> TopicId {
        TopicId::Notification(self.0.clone())
    }
}

impl Carries<NotificationEvent> for Notification {}
impl Carries<NoticeEvent> for Notification {}
impl Carries<InlineOutputEvent> for Notification {}
impl Carries<ErrorEvent> for Notification {}

/// The user inbox for incoming notifications.
pub struct Inbox;

impl Topic for Inbox {
    fn topic_id(&self) -> TopicId {
        TopicId::Inbox
    }
}

impl Carries<NotificationEvent> for Inbox {}

/// Workbench tool file changes, for web UI views showing a tool live.
pub struct Workbench;

impl Topic for Workbench {
    fn topic_id(&self) -> TopicId {
        TopicId::Workbench
    }
}

impl Carries<WorkbenchEvent> for Workbench {}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_topic_id() {
        let ep = EndpointName::from("ws");
        assert_eq!(Endpoint(ep.clone()).topic_id(), TopicId::Endpoint(ep));
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
    fn user_message_topic_id() {
        assert_eq!(UserMessage.topic_id(), TopicId::UserMessage);
    }

    #[test]
    fn background_topic_id() {
        assert_eq!(Background.topic_id(), TopicId::Background);
    }

    #[test]
    fn sessions_topic_id() {
        assert_eq!(Sessions.topic_id(), TopicId::Sessions);
    }

    #[test]
    fn inbox_topic_id() {
        assert_eq!(Inbox.topic_id(), TopicId::Inbox);
    }
}
