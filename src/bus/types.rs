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

newtype_string!(EndpointName, "Interactive endpoint identifier.");
newtype_string!(PresetName, "Subagent preset identifier.");
newtype_string!(WebhookName, "Named webhook identifier.");
newtype_string!(NotifyName, "Notification channel identifier.");

// ---------------------------------------------------------------------------
// TopicId
// ---------------------------------------------------------------------------

/// Identifies a pub/sub topic on the bus.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TopicId {
    /// The main agent processing loop.
    AgentMain,
    /// A subagent preset topic.
    AgentPreset(PresetName),
    /// An interactive endpoint (e.g. websocket, telegram).
    Interactive(EndpointName),
    /// A notification channel.
    Notify(NotifyName),
    /// The user inbox.
    Inbox,
    /// Results from completed background tasks.
    BackgroundResult,
    /// Events emitted by running background tasks.
    BackgroundEvent,
    /// A named webhook.
    Webhook(WebhookName),
}

impl fmt::Display for TopicId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AgentMain => f.write_str("agent:main"),
            Self::AgentPreset(name) => write!(f, "agent:preset:{name}"),
            Self::Interactive(name) => write!(f, "interactive:{name}"),
            Self::Notify(name) => write!(f, "notify:{name}"),
            Self::Inbox => f.write_str("inbox"),
            Self::BackgroundResult => f.write_str("background:result"),
            Self::BackgroundEvent => f.write_str("background:event"),
            Self::Webhook(name) => write!(f, "webhook:{name}"),
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
        assert_eq!(TopicId::AgentMain, TopicId::AgentMain);
        assert_eq!(TopicId::Inbox, TopicId::Inbox);
        assert_eq!(
            TopicId::Interactive(EndpointName::from("ws")),
            TopicId::Interactive(EndpointName::from("ws"))
        );
    }

    #[test]
    fn topic_id_inequality() {
        assert_ne!(TopicId::AgentMain, TopicId::Inbox);
        assert_ne!(
            TopicId::Interactive(EndpointName::from("ws")),
            TopicId::Interactive(EndpointName::from("telegram"))
        );
        assert_ne!(TopicId::BackgroundResult, TopicId::BackgroundEvent);
    }

    #[test]
    fn topic_id_hash_consistency() {
        let mut set = HashSet::new();
        let topic = TopicId::AgentPreset(PresetName::from("summarizer"));
        set.insert(topic.clone());
        assert!(set.contains(&topic));
        assert!(set.contains(&TopicId::AgentPreset(PresetName::from("summarizer"))));
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
        let name = WebhookName::from("github");
        assert_eq!(name.to_string(), "github");
    }

    #[test]
    fn topic_id_display() {
        assert_eq!(TopicId::AgentMain.to_string(), "agent:main");
        assert_eq!(TopicId::Inbox.to_string(), "inbox");
        assert_eq!(TopicId::BackgroundResult.to_string(), "background:result");
        assert_eq!(TopicId::BackgroundEvent.to_string(), "background:event");
        assert_eq!(
            TopicId::AgentPreset(PresetName::from("review")).to_string(),
            "agent:preset:review"
        );
        assert_eq!(
            TopicId::Interactive(EndpointName::from("ws")).to_string(),
            "interactive:ws"
        );
        assert_eq!(
            TopicId::Notify(NotifyName::from("ntfy")).to_string(),
            "notify:ntfy"
        );
        assert_eq!(
            TopicId::Webhook(WebhookName::from("deploy")).to_string(),
            "webhook:deploy"
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
