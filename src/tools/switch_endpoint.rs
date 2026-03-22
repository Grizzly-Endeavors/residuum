//! Switch the active interactive endpoint for agent responses.

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::watch;

use crate::bus::{
    EndpointCapabilities, EndpointId, EndpointName, EndpointRegistry, NoticeEvent, NotifyName,
    Publisher, SYSTEM_CHANNEL, topics,
};
use crate::models::ToolDefinition;

use super::{Tool, ToolError, ToolResult};

/// Tool for switching the agent's output endpoint.
pub struct SwitchEndpointTool {
    registry: EndpointRegistry,
    override_tx: watch::Sender<Option<EndpointName>>,
    publisher: Publisher,
}

impl SwitchEndpointTool {
    /// Create a new `SwitchEndpointTool`.
    #[must_use]
    pub fn new(
        registry: EndpointRegistry,
        override_tx: watch::Sender<Option<EndpointName>>,
        publisher: Publisher,
    ) -> Self {
        Self {
            registry,
            override_tx,
            publisher,
        }
    }
}

#[async_trait]
impl Tool for SwitchEndpointTool {
    fn name(&self) -> &'static str {
        "switch_endpoint"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "switch_endpoint".to_string(),
            description: "Switch the active endpoint for subsequent responses. \
                Takes effect on the next turn. Use list_endpoints to see available \
                interactive endpoints."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "endpoint": {
                        "type": "string",
                        "description": "Endpoint identifier (e.g. 'discord', 'telegram', 'ws')"
                    }
                },
                "required": ["endpoint"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let endpoint_name = arguments
            .get("endpoint")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidArguments("endpoint is required".to_string()))?;

        let endpoint_id = EndpointId::from(endpoint_name);

        let Some(entry) = self.registry.get(&endpoint_id) else {
            let interactive_names: Vec<String> = self
                .registry
                .interactive()
                .iter()
                .map(|e| e.id.as_ref().to_string())
                .collect();
            let available_str = if interactive_names.is_empty() {
                "(none configured)".to_string()
            } else {
                interactive_names.join(", ")
            };
            return Ok(ToolResult::error(format!(
                "unknown endpoint '{endpoint_name}'; available interactive endpoints: {available_str}",
            )));
        };

        if !entry
            .capabilities
            .contains(EndpointCapabilities::INTERACTIVE)
        {
            let interactive_names: Vec<String> = self
                .registry
                .interactive()
                .iter()
                .map(|e| e.id.as_ref().to_string())
                .collect();
            let available_str = if interactive_names.is_empty() {
                "(none configured)".to_string()
            } else {
                interactive_names.join(", ")
            };
            return Ok(ToolResult::error(format!(
                "endpoint '{endpoint_name}' is not interactive; available: {available_str}",
            )));
        }

        let ep = EndpointName::from(endpoint_name);
        self.override_tx.send_replace(Some(ep.clone()));

        // Notify via system message so the user knows the agent switched.
        if let Err(e) = self
            .publisher
            .publish(
                topics::Notification(NotifyName::from(SYSTEM_CHANNEL)),
                NoticeEvent {
                    message: "Agent switched output to this endpoint.".to_string(),
                },
            )
            .await
        {
            tracing::warn!(error = %e, "failed to publish endpoint switch notice");
        }

        Ok(ToolResult::success(format!(
            "Switched output to '{}'. Subsequent responses will be sent there.",
            entry.display_name,
        )))
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use crate::bus::{self, EndpointEntry, NotifyName, TopicId};

    fn make_registry() -> EndpointRegistry {
        let registry = EndpointRegistry::new();
        registry.register(EndpointEntry {
            id: EndpointId::from("ws"),
            topic: TopicId::Endpoint(EndpointName::from("ws")),
            capabilities: EndpointCapabilities::INTERACTIVE,
            display_name: "WebSocket".to_string(),
        });
        registry.register(EndpointEntry {
            id: EndpointId::from("discord"),
            topic: TopicId::Endpoint(EndpointName::from("discord")),
            capabilities: EndpointCapabilities::INTERACTIVE,
            display_name: "Discord".to_string(),
        });
        registry.register(EndpointEntry {
            id: EndpointId::from("my-ntfy"),
            topic: TopicId::Notification(NotifyName::from("my-ntfy")),
            capabilities: EndpointCapabilities::NOTIFY_ONLY,
            display_name: "Ntfy (my-ntfy)".to_string(),
        });
        registry
    }

    #[tokio::test]
    async fn tool_name_and_definition() {
        let (tx, _rx) = watch::channel(None);
        let bus_handle = bus::spawn_broker();
        let tool = SwitchEndpointTool::new(EndpointRegistry::new(), tx, bus_handle.publisher());
        assert_eq!(tool.name(), "switch_endpoint");
        assert_eq!(tool.definition().name, "switch_endpoint");
    }

    #[tokio::test]
    async fn switch_to_valid_interactive_endpoint() {
        let registry = make_registry();
        let (tx, rx) = watch::channel(None);
        let bus_handle = bus::spawn_broker();
        let tool = SwitchEndpointTool::new(registry, tx, bus_handle.publisher());

        let result = tool
            .execute(serde_json::json!({"endpoint": "discord"}))
            .await
            .unwrap();
        assert!(!result.is_error, "should succeed: {}", result.output);
        assert!(
            result.output.contains("Discord"),
            "should mention display name"
        );

        let override_val = rx.borrow().clone();
        assert_eq!(override_val, Some(EndpointName::from("discord")));
    }

    #[tokio::test]
    async fn switch_to_nonexistent_endpoint_errors() {
        let registry = make_registry();
        let (tx, _rx) = watch::channel(None);
        let bus_handle = bus::spawn_broker();
        let tool = SwitchEndpointTool::new(registry, tx, bus_handle.publisher());

        let result = tool
            .execute(serde_json::json!({"endpoint": "nonexistent"}))
            .await
            .unwrap();
        assert!(result.is_error, "should error for unknown endpoint");
        assert!(result.output.contains("unknown endpoint"));
    }

    #[tokio::test]
    async fn switch_to_notify_only_endpoint_errors() {
        let registry = make_registry();
        let (tx, _rx) = watch::channel(None);
        let bus_handle = bus::spawn_broker();
        let tool = SwitchEndpointTool::new(registry, tx, bus_handle.publisher());

        let result = tool
            .execute(serde_json::json!({"endpoint": "my-ntfy"}))
            .await
            .unwrap();
        assert!(result.is_error, "should error for non-interactive");
        assert!(result.output.contains("not interactive"));
    }

    #[tokio::test]
    async fn missing_endpoint_param_errors() {
        let (tx, _rx) = watch::channel(None);
        let bus_handle = bus::spawn_broker();
        let tool = SwitchEndpointTool::new(EndpointRegistry::new(), tx, bus_handle.publisher());

        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err(), "should error on missing endpoint");
    }
}
