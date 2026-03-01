//! Send message tool: proactive message delivery to external channels or inbox.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::models::ToolDefinition;
use crate::notify::router::NotificationRouter;
use crate::notify::types::{BuiltinChannel, ChannelTarget, Notification, TaskSource};

use super::{Tool, ToolError, ToolResult};

/// Tool for sending messages to external channels or the inbox.
pub struct SendMessageTool {
    router: Arc<NotificationRouter>,
    inbox_dir: PathBuf,
    tz: chrono_tz::Tz,
}

impl SendMessageTool {
    /// Create a new `SendMessageTool`.
    #[must_use]
    pub fn new(router: Arc<NotificationRouter>, inbox_dir: PathBuf, tz: chrono_tz::Tz) -> Self {
        Self {
            router,
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

        let target = ChannelTarget::parse(channel_name);

        match target {
            ChannelTarget::Builtin(BuiltinChannel::Inbox) => {
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
            ChannelTarget::Builtin(BuiltinChannel::AgentWake | BuiltinChannel::AgentFeed) => {
                Ok(ToolResult::error(
                    "send_message cannot target internal routing channels (agent_wake, agent_feed); \
                     use inbox or an external channel",
                ))
            }
            ChannelTarget::External(ext_name) => {
                if !self.router.has_external_channel(&ext_name) {
                    let available = self.router.external_channel_names();
                    return Ok(ToolResult::error(format!(
                        "unknown external channel '{ext_name}'; available: {}",
                        if available.is_empty() {
                            "(none configured)".to_string()
                        } else {
                            available.join(", ")
                        }
                    )));
                }

                let notification = Notification {
                    task_name: "send_message".to_string(),
                    summary: message.to_string(),
                    source: TaskSource::Agent,
                    timestamp: chrono::Utc::now(),
                };

                self.router
                    .deliver_to_external(&ext_name, &notification)
                    .await
                    .map_err(|e| {
                        ToolError::Execution(format!(
                            "failed to deliver to channel '{ext_name}': {e}"
                        ))
                    })?;

                Ok(ToolResult::success(format!(
                    "Message sent to channel '{ext_name}'"
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
    use std::collections::HashMap;

    use super::*;

    fn make_router() -> NotificationRouter {
        NotificationRouter::empty()
    }

    fn make_router_with_inbox(inbox_dir: &std::path::Path) -> NotificationRouter {
        use crate::notify::channels::InboxChannel;
        let inbox_channel = InboxChannel::new(inbox_dir, chrono_tz::UTC);
        NotificationRouter::new(HashMap::new(), Some(inbox_channel))
    }

    #[test]
    fn tool_name_and_definition() {
        let router = Arc::new(make_router());
        let tool = SendMessageTool::new(router, PathBuf::from("/tmp"), chrono_tz::UTC);
        assert_eq!(tool.name(), "send_message");
        assert_eq!(tool.definition().name, "send_message");
    }

    #[tokio::test]
    async fn send_to_inbox_creates_item() {
        let dir = tempfile::tempdir().unwrap();
        let inbox_dir = dir.path().join("inbox");
        std::fs::create_dir_all(&inbox_dir).unwrap();

        let router = Arc::new(make_router_with_inbox(&inbox_dir));
        let tool = SendMessageTool::new(router, inbox_dir.clone(), chrono_tz::UTC);

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
        let router = Arc::new(make_router());
        let tool = SendMessageTool::new(router, PathBuf::from("/tmp"), chrono_tz::UTC);

        let result = tool
            .execute(serde_json::json!({
                "channel": "agent_wake",
                "message": "test"
            }))
            .await
            .unwrap();

        assert!(result.is_error, "should error for agent_wake");
        assert!(
            result.output.contains("internal routing"),
            "should explain: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn send_to_agent_feed_returns_error() {
        let router = Arc::new(make_router());
        let tool = SendMessageTool::new(router, PathBuf::from("/tmp"), chrono_tz::UTC);

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
        let router = Arc::new(make_router());
        let tool = SendMessageTool::new(router, PathBuf::from("/tmp"), chrono_tz::UTC);

        let result = tool
            .execute(serde_json::json!({
                "channel": "nonexistent",
                "message": "test"
            }))
            .await
            .unwrap();

        assert!(result.is_error, "should error for unknown channel");
        assert!(
            result.output.contains("unknown external channel"),
            "should explain: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn send_missing_channel_returns_error() {
        let router = Arc::new(make_router());
        let tool = SendMessageTool::new(router, PathBuf::from("/tmp"), chrono_tz::UTC);

        let result = tool.execute(serde_json::json!({"message": "test"})).await;
        assert!(result.is_err(), "should error on missing channel");
    }

    #[tokio::test]
    async fn send_to_inbox_default_title() {
        let dir = tempfile::tempdir().unwrap();
        let inbox_dir = dir.path().join("inbox");
        std::fs::create_dir_all(&inbox_dir).unwrap();

        let router = Arc::new(make_router_with_inbox(&inbox_dir));
        let tool = SendMessageTool::new(router, inbox_dir.clone(), chrono_tz::UTC);

        let result = tool
            .execute(serde_json::json!({
                "channel": "inbox",
                "message": "A longer message that should be truncated for the title"
            }))
            .await
            .unwrap();

        assert!(!result.is_error, "should succeed: {}", result.output);

        // Verify the item was created with a derived title
        let items = crate::inbox::list_items(&inbox_dir).await.unwrap();
        assert_eq!(items.len(), 1);
        assert!(
            items[0].1.title.len() <= 60,
            "default title should be at most 60 chars"
        );
    }
}
