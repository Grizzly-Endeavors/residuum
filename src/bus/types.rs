//! Core types for the pub/sub bus.

use std::fmt;

// ---------------------------------------------------------------------------
// Newtype wrappers for TopicId parameters
// ---------------------------------------------------------------------------

macro_rules! newtype_string {
    ($name:ident, $doc:expr) => {
        #[doc = $doc]
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub struct $name(String);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_owned())
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }
    };
}

newtype_string!(EndpointId, "Unique identifier for a bus endpoint.");
newtype_string!(EndpointName, "Interactive endpoint identifier.");
newtype_string!(PresetName, "Subagent preset identifier.");
newtype_string!(NotifyName, "Notification channel identifier.");

// ---------------------------------------------------------------------------
// TopicId
// ---------------------------------------------------------------------------

/// Identifies a pub/sub topic on the bus.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TopicId {
    /// Inbound user messages for the main agent loop.
    UserMessage,
    /// Agent responses routed to a specific endpoint.
    Response(EndpointName),
    /// Tool call/result activity during a turn.
    ToolActivity(EndpointName),
    /// Turn start/end lifecycle events.
    TurnLifecycle(EndpointName),
    /// Intermediate model text during a turn.
    Intermediate(EndpointName),
    /// The user inbox.
    Inbox,
    /// Results from completed background tasks.
    BackgroundResult,
    /// Events emitted by running background tasks.
    BackgroundEvent,
    /// Request to spawn a sub-agent for a preset.
    SpawnRequest(PresetName),
    /// Push notifications for a named channel.
    Notification(NotifyName),
    /// System-wide messages (notices, errors, events).
    SystemMessage,
}

impl fmt::Display for TopicId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UserMessage => f.write_str("user:message"),
            Self::Response(name) => write!(f, "response:{name}"),
            Self::ToolActivity(name) => write!(f, "tool-activity:{name}"),
            Self::TurnLifecycle(name) => write!(f, "turn-lifecycle:{name}"),
            Self::Intermediate(name) => write!(f, "intermediate:{name}"),
            Self::Inbox => f.write_str("inbox"),
            Self::BackgroundResult => f.write_str("background:result"),
            Self::BackgroundEvent => f.write_str("background:event"),
            Self::SpawnRequest(name) => write!(f, "spawn-request:{name}"),
            Self::Notification(name) => write!(f, "notification:{name}"),
            Self::SystemMessage => f.write_str("system:message"),
        }
    }
}

// ---------------------------------------------------------------------------
// BusError
// ---------------------------------------------------------------------------

/// Errors returned by bus operations.
#[derive(Debug, thiserror::Error)]
pub enum BusError {
    /// The broker task has shut down.
    #[error("bus broker is shut down")]
    BrokerShutdown,
    /// Failed to send a command to the broker.
    #[error("failed to send to bus broker: {0}")]
    SendFailed(String),
    /// Received an event that could not be downcast to the expected type.
    #[error("type mismatch: expected {expected} on topic {topic}")]
    TypeMismatch {
        /// Name of the expected type.
        expected: &'static str,
        /// Topic where the mismatch occurred.
        topic: String,
    },
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn topic_id_equality() {
        assert_eq!(TopicId::UserMessage, TopicId::UserMessage);
        assert_eq!(TopicId::Inbox, TopicId::Inbox);
        assert_eq!(
            TopicId::Response(EndpointName::from("ws")),
            TopicId::Response(EndpointName::from("ws"))
        );
    }

    #[test]
    fn topic_id_inequality() {
        assert_ne!(TopicId::UserMessage, TopicId::Inbox);
        assert_ne!(
            TopicId::Response(EndpointName::from("ws")),
            TopicId::Response(EndpointName::from("telegram"))
        );
        assert_ne!(TopicId::BackgroundResult, TopicId::BackgroundEvent);
    }

    #[test]
    fn topic_id_hash_consistency() {
        let mut set = HashSet::new();
        let topic = TopicId::SpawnRequest(PresetName::from("summarizer"));
        set.insert(topic.clone());
        assert!(set.contains(&topic));
        assert!(set.contains(&TopicId::SpawnRequest(PresetName::from("summarizer"))));
    }

    #[test]
    fn endpoint_id_equality_and_hash() {
        let a = EndpointId::from("ws");
        let b = EndpointId::from("ws");
        assert_eq!(a, b);

        let mut set = HashSet::new();
        set.insert(a.clone());
        assert!(set.contains(&b));
    }

    #[test]
    fn endpoint_id_display() {
        let id = EndpointId::from("telegram");
        assert_eq!(id.to_string(), "telegram");
    }

    #[test]
    fn newtype_from_str() {
        let name = EndpointName::from("ws");
        assert_eq!(name.as_ref(), "ws");
    }

    #[test]
    fn newtype_from_string() {
        let name = PresetName::from("summarizer".to_string());
        assert_eq!(name.as_ref(), "summarizer");
    }

    #[test]
    fn newtype_display() {
        let name = NotifyName::from("my-ntfy");
        assert_eq!(name.to_string(), "my-ntfy");
    }

    #[test]
    fn topic_id_display() {
        assert_eq!(TopicId::UserMessage.to_string(), "user:message");
        assert_eq!(TopicId::Inbox.to_string(), "inbox");
        assert_eq!(TopicId::BackgroundResult.to_string(), "background:result");
        assert_eq!(TopicId::BackgroundEvent.to_string(), "background:event");
        assert_eq!(TopicId::SystemMessage.to_string(), "system:message");
        assert_eq!(
            TopicId::SpawnRequest(PresetName::from("review")).to_string(),
            "spawn-request:review"
        );
        assert_eq!(
            TopicId::Response(EndpointName::from("ws")).to_string(),
            "response:ws"
        );
        assert_eq!(
            TopicId::Notification(NotifyName::from("ntfy")).to_string(),
            "notification:ntfy"
        );
    }

    #[test]
    fn bus_error_display() {
        let err = BusError::BrokerShutdown;
        assert_eq!(err.to_string(), "bus broker is shut down");

        let err2 = BusError::SendFailed("channel closed".to_string());
        assert_eq!(
            err2.to_string(),
            "failed to send to bus broker: channel closed"
        );
    }
}
