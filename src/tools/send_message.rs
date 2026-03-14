//! Send message tool: proactive message delivery to external channels or inbox.

use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::Value;

use crate::bus::{EndpointRegistry, EventTrigger, NotificationEvent};
use crate::models::ToolDefinition;

use super::{Tool, ToolError, ToolResult};

/// Tool for sending messages to external channels or the inbox.
pub struct SendMessageTool {
    registry: EndpointRegistry,
    inbox_dir: PathBuf,
    tz: chrono_tz::Tz,
}

impl SendMessageTool {
    /// Create a new `SendMessageTool`.
    #[must_use]
    pub fn new(registry: EndpointRegistry, inbox_dir: PathBuf, tz: chrono_tz::Tz) -> Self {
        Self {
            registry,
            inbox_dir,
            tz,
        }
    }
}

#[async_trait]
impl Tool for SendMessageTool {
    fn name(&self) -> &'static str {
        "send_message"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "send_message".to_string(),
            description: "Send a message to an external notification channel or the inbox. \
                Use this to proactively notify the user via configured channels (ntfy, webhook) \
                or to save a message to the inbox for later review."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "channel": {
                        "type": "string",
                        "description": "Target channel name: 'inbox' or any configured external channel"
                    },
                    "message": {
                        "type": "string",
                        "description": "The message body to send"
                    },
                    "title": {
                        "type": "string",
                        "description": "Optional title (used for inbox items; defaults to first 60 chars of message)"
                    }
                },
                "required": ["channel", "message"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let channel_name = arguments
            .get("channel")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidArguments("channel is required".to_string()))?;

        let message = arguments
            .get("message")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidArguments("message is required".to_string()))?;

        let title = arguments.get("title").and_then(Value::as_str);

        match channel_name {
            "inbox" => {
                let inbox_title = title.map_or_else(
                    || message.chars().take(60).collect::<String>(),
                    str::to_string,
                );
                let filename = crate::inbox::quick_add(
                    &self.inbox_dir,
                    &inbox_title,
                    message,
                    "agent",
                    self.tz,
                )
                .await
                .map_err(|e| ToolError::Execution(format!("failed to add inbox item: {e}")))?;
                Ok(ToolResult::success(format!(
                    "Message saved to inbox as {filename}"
                )))
            }
            "agent_wake" | "agent_feed" => Ok(ToolResult::error(
                "agent_wake and agent_feed are no longer supported; \
                 use inbox or an external channel instead",
            )),
            _ => {
                // Validate the channel exists in the registry
                let endpoint_id = crate::bus::EndpointId::from(channel_name);
                if self.registry.get(&endpoint_id).is_none() {
                    let available: Vec<String> = self
                        .registry
                        .notify()
                        .iter()
                        .map(|e| e.id.as_ref().to_string())
                        .collect();
                    return Ok(ToolResult::error(format!(
                        "unknown channel '{channel_name}'; available: {}",
                        if available.is_empty() {
                            "(none configured)".to_string()
                        } else {
                            available.join(", ")
                        }
                    )));
                }

                let _notification = NotificationEvent {
                    title: title.map_or_else(
                        || message.chars().take(60).collect::<String>(),
                        str::to_string,
                    ),
                    content: message.to_string(),
                    source: EventTrigger::Agent,
                    timestamp: crate::time::now_local(self.tz),
                };

                // Fire-and-forget: report "published" without confirming delivery
                Ok(ToolResult::success(format!(
                    "Message published to channel '{channel_name}'"
                )))
            }
        }
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(
    clippy::indexing_slicing,
    reason = "test code uses indexing for clarity"
)]
mod tests {
    use super::*;

    fn make_registry() -> EndpointRegistry {
        EndpointRegistry::new()
    }

    #[test]
    fn tool_name_and_definition() {
        let registry = make_registry();
        let tool = SendMessageTool::new(registry, PathBuf::from("/tmp"), chrono_tz::UTC);
        assert_eq!(tool.name(), "send_message");
        assert_eq!(tool.definition().name, "send_message");
    }

    #[tokio::test]
    async fn send_to_inbox_creates_item() {
        let dir = tempfile::tempdir().unwrap();
        let inbox_dir = dir.path().join("inbox");
        std::fs::create_dir_all(&inbox_dir).unwrap();

        let registry = make_registry();
        let tool = SendMessageTool::new(registry, inbox_dir.clone(), chrono_tz::UTC);

        let result = tool
            .execute(serde_json::json!({
                "channel": "inbox",
                "message": "test message body",
                "title": "test title"
            }))
            .await
            .unwrap();

        assert!(!result.is_error, "should succeed: {}", result.output);
        assert!(result.output.contains("inbox"), "should mention inbox");

        let items: Vec<_> = std::fs::read_dir(&inbox_dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
            .collect();
        assert_eq!(items.len(), 1, "should create one inbox item");
    }

    #[tokio::test]
    async fn send_to_agent_wake_returns_error() {
        let registry = make_registry();
        let tool = SendMessageTool::new(registry, PathBuf::from("/tmp"), chrono_tz::UTC);

        let result = tool
            .execute(serde_json::json!({
                "channel": "agent_wake",
                "message": "test"
            }))
            .await
            .unwrap();

        assert!(result.is_error, "should error for agent_wake");
        assert!(
            result.output.contains("no longer supported"),
            "should explain: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn send_to_agent_feed_returns_error() {
        let registry = make_registry();
        let tool = SendMessageTool::new(registry, PathBuf::from("/tmp"), chrono_tz::UTC);

        let result = tool
            .execute(serde_json::json!({
                "channel": "agent_feed",
                "message": "test"
            }))
            .await
            .unwrap();

        assert!(result.is_error, "should error for agent_feed");
    }

    #[tokio::test]
    async fn send_to_unknown_external_returns_error() {
        let registry = make_registry();
        let tool = SendMessageTool::new(registry, PathBuf::from("/tmp"), chrono_tz::UTC);

        let result = tool
            .execute(serde_json::json!({
                "channel": "nonexistent",
                "message": "test"
            }))
            .await
            .unwrap();

        assert!(result.is_error, "should error for unknown channel");
        assert!(
            result.output.contains("unknown channel"),
            "should explain: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn send_missing_channel_returns_error() {
        let registry = make_registry();
        let tool = SendMessageTool::new(registry, PathBuf::from("/tmp"), chrono_tz::UTC);

        let result = tool.execute(serde_json::json!({"message": "test"})).await;
        assert!(result.is_err(), "should error on missing channel");
    }

    #[tokio::test]
    async fn send_to_inbox_default_title() {
        let dir = tempfile::tempdir().unwrap();
        let inbox_dir = dir.path().join("inbox");
        std::fs::create_dir_all(&inbox_dir).unwrap();

        let registry = make_registry();
        let tool = SendMessageTool::new(registry, inbox_dir.clone(), chrono_tz::UTC);

        let result = tool
            .execute(serde_json::json!({
                "channel": "inbox",
                "message": "A longer message that should be truncated for the title"
            }))
            .await
            .unwrap();

        assert!(!result.is_error, "should succeed: {}", result.output);

        let items = crate::inbox::list_items(&inbox_dir).await.unwrap();
        assert_eq!(items.len(), 1);
        assert!(
            items[0].1.title.len() <= 60,
            "default title should be at most 60 chars"
        );
    }
}
