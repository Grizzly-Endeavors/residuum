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
    /// An error related to a specific request.
    Error {
        /// Correlation ID of the original message, if applicable.
        reply_to: Option<String>,
        /// Error description.
        message: String,
    },
    /// Keepalive pong.
    Pong,
}

impl ServerMessage {
    /// Whether this message is only sent to clients with verbose mode enabled.
    #[must_use]
    pub fn is_verbose_only(&self) -> bool {
        matches!(self, Self::ToolCall { .. } | Self::ToolResult { .. })
    }
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
    fn is_verbose_only_tool_call() {
        let msg = ServerMessage::ToolCall {
            name: "exec".to_string(),
            arguments: serde_json::json!({}),
        };
        assert!(msg.is_verbose_only(), "ToolCall should be verbose-only");
    }

    #[test]
    fn is_verbose_only_tool_result() {
        let msg = ServerMessage::ToolResult {
            name: "exec".to_string(),
            output: "ok".to_string(),
            is_error: false,
        };
        assert!(msg.is_verbose_only(), "ToolResult should be verbose-only");
    }

    #[test]
    fn is_verbose_only_response() {
        let msg = ServerMessage::Response {
            reply_to: "id".to_string(),
            content: "hi".to_string(),
        };
        assert!(
            !msg.is_verbose_only(),
            "Response should not be verbose-only"
        );
    }

    #[test]
    fn is_verbose_only_system_event() {
        let msg = ServerMessage::SystemEvent {
            source: "cron".to_string(),
            content: "job done".to_string(),
        };
        assert!(
            !msg.is_verbose_only(),
            "SystemEvent should not be verbose-only"
        );
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
    fn client_message_invalid_type_fails() {
        let json = r#"{"type":"unknown_type"}"#;
        let result = serde_json::from_str::<ClientMessage>(json);
        assert!(result.is_err(), "unknown type should fail to deserialize");
    }
}
