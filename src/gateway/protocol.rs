//! WebSocket protocol types: client and server message frames.

use serde::{Deserialize, Serialize};

/// Messages sent from a WebSocket client to the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Send a user message to the agent.
    SendMessage {
        /// Client-generated correlation ID.
        id: String,
        /// The user message content.
        content: String,
    },
    /// Toggle verbose mode (tool call/result events).
    SetVerbose {
        /// Whether to receive tool events.
        enabled: bool,
    },
    /// Keepalive ping.
    Ping,
    /// Request the gateway to reload its configuration.
    Reload,
    /// Request a forced observation cycle.
    Observe,
    /// Request a forced reflection cycle.
    Reflect,
    /// Request a token usage summary.
    ContextRequest,
    /// Add a message to the inbox without triggering an agent turn.
    InboxAdd {
        /// The message body to add.
        body: String,
    },
}

/// Messages sent from the server to WebSocket clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// The agent began processing a queued message.
    TurnStarted {
        /// Correlation ID of the message being processed.
        reply_to: String,
    },
    /// A tool was invoked during the agent turn (verbose only).
    ToolCall {
        /// Name of the tool.
        name: String,
        /// Tool arguments as JSON.
        arguments: serde_json::Value,
    },
    /// A tool completed execution (verbose only).
    ToolResult {
        /// Name of the tool.
        name: String,
        /// Tool output text.
        output: String,
        /// Whether the tool returned an error.
        is_error: bool,
    },
    /// The agent's final text response.
    Response {
        /// Correlation ID of the original message.
        reply_to: String,
        /// The response content.
        content: String,
    },
    /// A system event from cron or pulse.
    SystemEvent {
        /// Source of the event (e.g. `"cron: my_job"` or `"pulse: my_check"`).
        source: String,
        /// The event content.
        content: String,
    },
    /// Intermediate text the agent emitted alongside tool calls.
    BroadcastResponse {
        /// The intermediate content.
        content: String,
    },
    /// An error related to a specific request.
    Error {
        /// Correlation ID of the original message, if applicable.
        reply_to: Option<String>,
        /// Error description.
        message: String,
    },
    /// Keepalive pong.
    Pong,
    /// The gateway is reloading its configuration.
    Reloading,
    /// Result of a manual memory operation (observe or reflect).
    Notice {
        /// Human-readable result message.
        message: String,
    },
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    #[test]
    fn client_message_deserialize_send_message() {
        let json = r#"{"type":"send_message","id":"abc-123","content":"hello"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        assert!(
            matches!(
                &msg,
                ClientMessage::SendMessage { id, content }
                    if id == "abc-123" && content == "hello"
            ),
            "should deserialize to SendMessage with correct fields"
        );
    }

    #[test]
    fn client_message_deserialize_set_verbose() {
        let json = r#"{"type":"set_verbose","enabled":true}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        assert!(
            matches!(&msg, ClientMessage::SetVerbose { enabled } if *enabled),
            "should deserialize to SetVerbose with enabled=true"
        );
    }

    #[test]
    fn client_message_deserialize_ping() {
        let json = r#"{"type":"ping"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        assert!(
            matches!(msg, ClientMessage::Ping),
            "should deserialize to Ping"
        );
    }

    #[test]
    fn server_message_serialize_response() {
        let msg = ServerMessage::Response {
            reply_to: "id-1".to_string(),
            content: "hello back".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(
            json.contains("\"type\":\"response\""),
            "should have type tag"
        );
        assert!(
            json.contains("\"reply_to\":\"id-1\""),
            "should have reply_to"
        );
    }

    #[test]
    fn server_message_serialize_pong() {
        let msg = ServerMessage::Pong;
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"pong"}"#, "pong should serialize cleanly");
    }

    #[test]
    fn server_message_serialize_error() {
        let msg = ServerMessage::Error {
            reply_to: Some("id-1".to_string()),
            message: "something failed".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"error\""), "should have type tag");
    }

    #[test]
    fn client_message_deserialize_reload() {
        let json = r#"{"type":"reload"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        assert!(
            matches!(msg, ClientMessage::Reload),
            "should deserialize to Reload"
        );
    }

    #[test]
    fn server_message_serialize_reloading() {
        let msg = ServerMessage::Reloading;
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(
            json, r#"{"type":"reloading"}"#,
            "reloading should serialize cleanly"
        );
    }

    #[test]
    fn client_message_deserialize_observe() {
        let json = r#"{"type":"observe"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        assert!(
            matches!(msg, ClientMessage::Observe),
            "should deserialize to Observe"
        );
    }

    #[test]
    fn client_message_deserialize_reflect() {
        let json = r#"{"type":"reflect"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        assert!(
            matches!(msg, ClientMessage::Reflect),
            "should deserialize to Reflect"
        );
    }

    #[test]
    fn server_message_serialize_notice() {
        let msg = ServerMessage::Notice {
            message: "observed successfully".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"notice\""), "should have type tag");
        assert!(
            json.contains("\"message\":\"observed successfully\""),
            "should have message field"
        );
    }

    #[test]
    fn server_message_serialize_broadcast_response() {
        let msg = ServerMessage::BroadcastResponse {
            content: "checking that for you".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(
            json.contains("\"type\":\"broadcast_response\""),
            "should have type tag"
        );
        assert!(
            json.contains("\"content\":\"checking that for you\""),
            "should have content field"
        );
    }

    #[test]
    fn client_message_deserialize_context_request() {
        let json = r#"{"type":"context_request"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        assert!(
            matches!(msg, ClientMessage::ContextRequest),
            "should deserialize to ContextRequest"
        );
    }

    #[test]
    fn client_message_deserialize_inbox_add() {
        let json = r#"{"type":"inbox_add","body":"remember to deploy tomorrow"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        assert!(
            matches!(&msg, ClientMessage::InboxAdd { body } if body == "remember to deploy tomorrow"),
            "should deserialize to InboxAdd with correct body"
        );
    }

    #[test]
    fn client_message_serialize_context_request() {
        let msg = ClientMessage::ContextRequest;
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(
            json, r#"{"type":"context_request"}"#,
            "context_request should serialize cleanly"
        );
    }

    #[test]
    fn client_message_serialize_inbox_add() {
        let msg = ClientMessage::InboxAdd {
            body: "hello world".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(
            json.contains("\"type\":\"inbox_add\""),
            "should have type tag"
        );
        assert!(
            json.contains("\"body\":\"hello world\""),
            "should have body field"
        );
    }

    #[test]
    fn client_message_invalid_type_fails() {
        let json = r#"{"type":"unknown_type"}"#;
        let result = serde_json::from_str::<ClientMessage>(json);
        assert!(result.is_err(), "unknown type should fail to deserialize");
    }
}
