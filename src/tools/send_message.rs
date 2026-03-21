//! Send message tool: proactive message delivery to notification and interactive endpoints.

use async_trait::async_trait;
use serde_json::Value;

use crate::bus::{
    EndpointCapabilities, EndpointId, EndpointName, EndpointRegistry, EventTrigger,
    NotificationEvent, NotifyName, Publisher, ResponseEvent, topics,
};
use crate::models::ToolDefinition;

use super::{Tool, ToolError, ToolResult};

/// Tool for sending messages to notification or interactive endpoints.
pub struct SendMessageTool {
    registry: EndpointRegistry,
    publisher: Publisher,
}

impl SendMessageTool {
    /// Create a new `SendMessageTool`.
    #[must_use]
    pub fn new(registry: EndpointRegistry, publisher: Publisher) -> Self {
        Self {
            registry,
            publisher,
        }
    }

    /// Collect names of all sendable endpoints (interactive + notify).
    fn sendable_endpoint_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .registry
            .interactive()
            .iter()
            .chain(self.registry.notify().iter())
            .map(|e| e.id.as_ref().to_string())
            .collect();
        names.sort();
        names
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
            description: "Send a one-off message to a notification or interactive endpoint. \
                Use list_endpoints to see available targets."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "endpoint": {
                        "type": "string",
                        "description": "Target endpoint name (any interactive or notification endpoint)"
                    },
                    "message": {
                        "type": "string",
                        "description": "The message body to send"
                    },
                    "title": {
                        "type": "string",
                        "description": "Optional title for notifications (defaults to first 60 chars of message)"
                    }
                },
                "required": ["endpoint", "message"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let endpoint_name = arguments
            .get("endpoint")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidArguments("endpoint is required".to_string()))?;

        let message = arguments
            .get("message")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidArguments("message is required".to_string()))?;

        let title = arguments.get("title").and_then(Value::as_str);

        let endpoint_id = EndpointId::from(endpoint_name);
        let Some(entry) = self.registry.get(&endpoint_id) else {
            let available = self.sendable_endpoint_names();
            return Ok(ToolResult::error(format!(
                "unknown endpoint '{endpoint_name}'; available: {}",
                if available.is_empty() {
                    "(none configured)".to_string()
                } else {
                    available.join(", ")
                }
            )));
        };

        let is_interactive = entry
            .capabilities
            .contains(EndpointCapabilities::INTERACTIVE);
        let is_notify = entry
            .capabilities
            .contains(EndpointCapabilities::NOTIFY_ONLY);

        if !is_interactive && !is_notify {
            let available = self.sendable_endpoint_names();
            return Ok(ToolResult::error(format!(
                "endpoint '{endpoint_name}' does not accept messages; available: {}",
                if available.is_empty() {
                    "(none configured)".to_string()
                } else {
                    available.join(", ")
                }
            )));
        }

        let now = chrono::Utc::now().naive_utc();

        if is_notify {
            let notification = NotificationEvent {
                title: title.map_or_else(
                    || message.chars().take(60).collect::<String>(),
                    str::to_string,
                ),
                content: message.to_string(),
                source: EventTrigger::Agent,
                timestamp: now,
            };
            let topic = topics::Notification(NotifyName::from(endpoint_name));
            self.publisher
                .publish(topic, notification)
                .await
                .map_err(|e| {
                    tracing::error!(error = %e, endpoint = %endpoint_name, "failed to publish notification");
                    ToolError::Execution(format!(
                        "failed to publish message to '{endpoint_name}': {e}"
                    ))
                })?;
        } else {
            let response = ResponseEvent {
                correlation_id: String::new(),
                content: message.to_string(),
                timestamp: now,
            };
            let topic = topics::Response(EndpointName::from(endpoint_name));
            self.publisher.publish(topic, response).await.map_err(|e| {
                tracing::error!(error = %e, endpoint = %endpoint_name, "failed to publish response");
                ToolError::Execution(format!(
                    "failed to publish message to '{endpoint_name}': {e}"
                ))
            })?;
        }

        Ok(ToolResult::success(format!(
            "Message published to endpoint '{endpoint_name}'"
        )))
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use crate::bus::{EndpointEntry, TopicId};

    fn make_registry() -> EndpointRegistry {
        let registry = EndpointRegistry::new();
        registry.register(EndpointEntry {
            id: EndpointId::from("ws"),
            topic: TopicId::Response(EndpointName::from("ws")),
            capabilities: EndpointCapabilities::INTERACTIVE,
            display_name: "WebSocket".to_string(),
        });
        registry.register(EndpointEntry {
            id: EndpointId::from("my-ntfy"),
            topic: TopicId::Notification(NotifyName::from("my-ntfy")),
            capabilities: EndpointCapabilities::NOTIFY_ONLY,
            display_name: "Ntfy (my-ntfy)".to_string(),
        });
        registry.register(EndpointEntry {
            id: EndpointId::from("inbox"),
            topic: TopicId::Inbox,
            capabilities: EndpointCapabilities::INPUT_ONLY,
            display_name: "Inbox".to_string(),
        });
        registry
    }

    fn make_publisher() -> Publisher {
        let bus_handle = crate::bus::spawn_broker();
        bus_handle.publisher()
    }

    #[tokio::test]
    async fn tool_name_and_definition() {
        let registry = EndpointRegistry::new();
        let publisher = make_publisher();
        let tool = SendMessageTool::new(registry, publisher);
        assert_eq!(tool.name(), "send_message");
        assert_eq!(tool.definition().name, "send_message");
    }

    #[tokio::test]
    async fn send_to_notify_endpoint_publishes() {
        let registry = make_registry();
        let bus_handle = crate::bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let mut subscriber = bus_handle
            .subscribe(topics::Notification(NotifyName::from("my-ntfy")))
            .await
            .unwrap();
        let tool = SendMessageTool::new(registry, publisher);

        let result = tool
            .execute(serde_json::json!({
                "endpoint": "my-ntfy",
                "message": "test notification",
                "title": "alert"
            }))
            .await
            .unwrap();

        assert!(!result.is_error, "should succeed: {}", result.output);
        assert!(result.output.contains("my-ntfy"));

        let event = subscriber.recv().await.unwrap().unwrap();
        assert_eq!(event.title, "alert");
        assert_eq!(event.content, "test notification");
    }

    #[tokio::test]
    async fn send_to_interactive_endpoint_publishes_response() {
        let registry = make_registry();
        let bus_handle = crate::bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let mut subscriber = bus_handle
            .subscribe(topics::Response(EndpointName::from("ws")))
            .await
            .unwrap();
        let tool = SendMessageTool::new(registry, publisher);

        let result = tool
            .execute(serde_json::json!({
                "endpoint": "ws",
                "message": "proactive message"
            }))
            .await
            .unwrap();

        assert!(!result.is_error, "should succeed: {}", result.output);

        let event = subscriber.recv().await.unwrap().unwrap();
        assert_eq!(event.content, "proactive message");
    }

    #[tokio::test]
    async fn send_to_inbox_returns_error() {
        let registry = make_registry();
        let publisher = make_publisher();
        let tool = SendMessageTool::new(registry, publisher);

        let result = tool
            .execute(serde_json::json!({
                "endpoint": "inbox",
                "message": "test"
            }))
            .await
            .unwrap();

        assert!(result.is_error, "should error for inbox");
        assert!(
            result.output.contains("does not accept messages"),
            "should explain: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn send_to_unknown_endpoint_returns_error() {
        let registry = make_registry();
        let publisher = make_publisher();
        let tool = SendMessageTool::new(registry, publisher);

        let result = tool
            .execute(serde_json::json!({
                "endpoint": "nonexistent",
                "message": "test"
            }))
            .await
            .unwrap();

        assert!(result.is_error, "should error for unknown");
        assert!(result.output.contains("unknown endpoint"));
    }

    #[tokio::test]
    async fn send_missing_endpoint_returns_error() {
        let registry = EndpointRegistry::new();
        let publisher = make_publisher();
        let tool = SendMessageTool::new(registry, publisher);

        let result = tool.execute(serde_json::json!({"message": "test"})).await;
        assert!(result.is_err(), "should error on missing endpoint");
    }
}
