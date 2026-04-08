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

async fn validate_file_attachment(
    fp: &str,
    endpoint_name: &str,
) -> Result<crate::interfaces::attachment::FileAttachment, String> {
    let path = std::path::Path::new(fp);
    let att = crate::interfaces::attachment::FileAttachment::from_path(path).await?;

    // Size limit: 50MB for Telegram, 25MB for others
    let limit: u64 = if endpoint_name.contains("telegram") {
        50 * 1024 * 1024
    } else {
        25 * 1024 * 1024
    };
    if att.size > limit {
        let size_mb = att.size / (1024 * 1024);
        let limit_mb = limit / (1024 * 1024);
        return Err(format!(
            "file '{}' is {}MB, exceeds {}MB limit for {endpoint_name}",
            att.filename, size_mb, limit_mb,
        ));
    }

    tracing::info!(
        filename = %att.filename,
        mime_type = %att.mime_type,
        size = att.size,
        endpoint = %endpoint_name,
        "file attachment published"
    );

    Ok(att)
}

fn build_success_message(
    endpoint_name: &str,
    file_path_str: Option<&str>,
    message: Option<&str>,
) -> String {
    if let Some(fp) = file_path_str {
        let filename = std::path::Path::new(fp)
            .file_name()
            .map_or_else(|| fp.to_string(), |n| n.to_string_lossy().to_string());
        if message.is_some() {
            format!("Message and file '{filename}' published to endpoint '{endpoint_name}'")
        } else {
            format!("File '{filename}' published to endpoint '{endpoint_name}'")
        }
    } else {
        format!("Message published to endpoint '{endpoint_name}'")
    }
}

#[async_trait]
impl Tool for SendMessageTool {
    fn name(&self) -> &'static str {
        "send_message"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: "Send a message and/or file attachment to a notification or interactive \
                endpoint. Use list_endpoints to see available targets."
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
                        "description": "Message body or caption text (optional if file_path is provided)"
                    },
                    "file_path": {
                        "type": "string",
                        "description": "Absolute path to a file to attach (optional if message is provided)"
                    },
                    "title": {
                        "type": "string",
                        "description": "Optional title for notifications (defaults to first 60 chars of message)"
                    }
                },
                "required": ["endpoint"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let endpoint_name = super::require_str(&arguments, "endpoint")?;
        let message = arguments.get("message").and_then(Value::as_str);
        let file_path_str = arguments.get("file_path").and_then(Value::as_str);
        let title = arguments.get("title").and_then(Value::as_str);

        // Must have at least one of message or file_path
        if message.is_none() && file_path_str.is_none() {
            return Ok(ToolResult::error(
                "at least one of 'message' or 'file_path' is required".to_string(),
            ));
        }

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

        // File attachments only supported on interactive endpoints
        let attachment = if let Some(fp) = file_path_str {
            if !is_interactive {
                return Ok(ToolResult::error(format!(
                    "endpoint '{endpoint_name}' does not support file attachments"
                )));
            }
            match validate_file_attachment(fp, endpoint_name).await {
                Ok(att) => Some(att),
                Err(e) => return Ok(ToolResult::error(e)),
            }
        } else {
            None
        };

        let now = chrono::Utc::now().naive_utc();
        let content = message.unwrap_or("").to_string();

        if is_notify {
            let notification = NotificationEvent {
                title: title.map_or_else(
                    || content.chars().take(60).collect::<String>(),
                    str::to_string,
                ),
                content: content.clone(),
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
                content,
                timestamp: now,
                attachment,
            };
            let topic = topics::Endpoint(EndpointName::from(endpoint_name));
            self.publisher.publish(topic, response).await.map_err(|e| {
                tracing::error!(error = %e, endpoint = %endpoint_name, "failed to publish response");
                ToolError::Execution(format!(
                    "failed to publish message to '{endpoint_name}': {e}"
                ))
            })?;
        }

        // Build success message
        let success_msg = build_success_message(endpoint_name, file_path_str, message);

        Ok(ToolResult::success(success_msg))
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
            topic: TopicId::Endpoint(EndpointName::from("ws")),
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

        let event: crate::bus::NotificationEvent = subscriber.recv().await.unwrap().unwrap();
        assert_eq!(event.title, "alert");
        assert_eq!(event.content, "test notification");
    }

    #[tokio::test]
    async fn send_to_interactive_endpoint_publishes_response() {
        let registry = make_registry();
        let bus_handle = crate::bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let mut subscriber = bus_handle
            .subscribe(topics::Endpoint(EndpointName::from("ws")))
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

        let event: crate::bus::ResponseEvent = subscriber.recv().await.unwrap().unwrap();
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

    #[tokio::test]
    async fn send_file_to_interactive_endpoint_publishes() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.pdf");
        tokio::fs::write(&file_path, b"fake pdf").await.unwrap();

        let registry = make_registry();
        let bus_handle = crate::bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let mut subscriber = bus_handle
            .subscribe(topics::Endpoint(EndpointName::from("ws")))
            .await
            .unwrap();
        let tool = SendMessageTool::new(registry, publisher);

        let result = tool
            .execute(serde_json::json!({
                "endpoint": "ws",
                "file_path": file_path.to_str().unwrap()
            }))
            .await
            .unwrap();

        assert!(!result.is_error, "should succeed: {}", result.output);
        assert!(result.output.contains("test.pdf"), "should mention filename: {}", result.output);

        let event: crate::bus::ResponseEvent = subscriber.recv().await.unwrap().unwrap();
        assert!(event.attachment.is_some(), "should have attachment");
        let att = event.attachment.unwrap();
        assert_eq!(att.filename, "test.pdf");
        assert_eq!(att.mime_type, "application/pdf");
    }

    #[tokio::test]
    async fn send_file_with_caption() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("photo.jpg");
        tokio::fs::write(&file_path, b"fake image").await.unwrap();

        let registry = make_registry();
        let bus_handle = crate::bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let mut subscriber = bus_handle
            .subscribe(topics::Endpoint(EndpointName::from("ws")))
            .await
            .unwrap();
        let tool = SendMessageTool::new(registry, publisher);

        let result = tool
            .execute(serde_json::json!({
                "endpoint": "ws",
                "message": "Check out this photo",
                "file_path": file_path.to_str().unwrap()
            }))
            .await
            .unwrap();

        assert!(!result.is_error, "should succeed: {}", result.output);

        let event: crate::bus::ResponseEvent = subscriber.recv().await.unwrap().unwrap();
        assert_eq!(event.content, "Check out this photo");
        assert!(event.attachment.is_some());
    }

    #[tokio::test]
    async fn send_no_message_no_file_returns_error() {
        let registry = make_registry();
        let publisher = make_publisher();
        let tool = SendMessageTool::new(registry, publisher);

        let result = tool
            .execute(serde_json::json!({
                "endpoint": "ws"
            }))
            .await
            .unwrap();

        assert!(result.is_error, "should error with no message or file");
        assert!(
            result.output.contains("at least one of"),
            "should explain: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn send_file_not_found_returns_error() {
        let registry = make_registry();
        let publisher = make_publisher();
        let tool = SendMessageTool::new(registry, publisher);

        let result = tool
            .execute(serde_json::json!({
                "endpoint": "ws",
                "file_path": "/tmp/nonexistent_residuum_test_file.pdf"
            }))
            .await
            .unwrap();

        assert!(result.is_error, "should error for missing file");
        assert!(
            result.output.contains("file not found"),
            "should mention not found: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn send_file_to_notify_endpoint_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("doc.pdf");
        tokio::fs::write(&file_path, b"content").await.unwrap();

        let registry = make_registry();
        let publisher = make_publisher();
        let tool = SendMessageTool::new(registry, publisher);

        let result = tool
            .execute(serde_json::json!({
                "endpoint": "my-ntfy",
                "file_path": file_path.to_str().unwrap()
            }))
            .await
            .unwrap();

        assert!(result.is_error, "should error for notify endpoint with file");
        assert!(
            result.output.contains("does not support file attachments"),
            "should explain: {}",
            result.output
        );
    }
}
