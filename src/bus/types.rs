//! Core types for the pub/sub bus.

use std::fmt;

/// Well-known notification channel name for system-level notices and errors.
pub const SYSTEM_CHANNEL: &str = "system";

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
///
/// Each topic is a routing domain that can carry multiple event types.
/// Subscribers register interest in a specific `(TopicId, TypeId)` pair.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TopicId {
    /// Interactive endpoint turn activity (responses, tool calls, lifecycle, intermediate text).
    Endpoint(EndpointName),
    /// Background task orchestration (spawn requests, results, and optionally turn activity).
    Background,
    /// Inbound user messages for the main agent loop.
    UserMessage,
    /// Push notifications for a named channel (including the well-known "system" channel).
    Notification(NotifyName),
    /// The user inbox for incoming notifications.
    Inbox,
}

impl fmt::Display for TopicId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Endpoint(name) => write!(f, "endpoint:{name}"),
            Self::Background => f.write_str("background"),
            Self::UserMessage => f.write_str("user:message"),
            Self::Notification(name) => write!(f, "notification:{name}"),
            Self::Inbox => f.write_str("inbox"),
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
        assert_eq!(TopicId::Background, TopicId::Background);
        assert_eq!(
            TopicId::Endpoint(EndpointName::from("ws")),
            TopicId::Endpoint(EndpointName::from("ws"))
        );
    }

    #[test]
    fn topic_id_inequality() {
        assert_ne!(TopicId::UserMessage, TopicId::Inbox);
        assert_ne!(TopicId::Background, TopicId::UserMessage);
        assert_ne!(
            TopicId::Endpoint(EndpointName::from("ws")),
            TopicId::Endpoint(EndpointName::from("telegram"))
        );
    }

    #[test]
    fn topic_id_hash_consistency() {
        let mut set = HashSet::new();
        let topic = TopicId::Notification(NotifyName::from("ntfy"));
        set.insert(topic.clone());
        assert!(set.contains(&topic));
        assert!(set.contains(&TopicId::Notification(NotifyName::from("ntfy"))));
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
        assert_eq!(TopicId::Background.to_string(), "background");
        assert_eq!(
            TopicId::Endpoint(EndpointName::from("ws")).to_string(),
            "endpoint:ws"
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
    }

    #[test]
    fn bus_error_type_mismatch_display() {
        let err = BusError::TypeMismatch {
            expected: "Foo",
            topic: "bar".into(),
        };
        assert_eq!(err.to_string(), "type mismatch: expected Foo on topic bar");
    }
}
