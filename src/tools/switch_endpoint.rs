//! Switch the active interactive endpoint for agent responses.

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::watch;

use crate::bus::{
    BusEvent, EndpointCapabilities, EndpointId, EndpointRegistry, Publisher, TopicId,
};
use crate::models::ToolDefinition;

use super::{Tool, ToolError, ToolResult};

/// Tool for switching the agent's output endpoint.
pub struct SwitchEndpointTool {
    registry: EndpointRegistry,
    override_tx: watch::Sender<Option<TopicId>>,
    publisher: Publisher,
}

impl SwitchEndpointTool {
    /// Create a new `SwitchEndpointTool`.
    #[must_use]
    pub fn new(
        registry: EndpointRegistry,
        override_tx: watch::Sender<Option<TopicId>>,
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
            let available: Vec<String> = self
                .registry
                .interactive()
                .iter()
                .map(|e| e.id.as_ref().to_string())
                .collect();
            return Ok(ToolResult::error(format!(
                "unknown endpoint '{endpoint_name}'; available interactive endpoints: {}",
                if available.is_empty() {
                    "(none configured)".to_string()
                } else {
                    available.join(", ")
                }
            )));
        };

        if !entry
            .capabilities
            .contains(EndpointCapabilities::INTERACTIVE)
        {
            let available: Vec<String> = self
                .registry
                .interactive()
                .iter()
                .map(|e| e.id.as_ref().to_string())
                .collect();
            return Ok(ToolResult::error(format!(
                "endpoint '{endpoint_name}' is not interactive; available: {}",
                if available.is_empty() {
                    "(none configured)".to_string()
                } else {
                    available.join(", ")
                }
            )));
        }

        let new_topic = entry.topic.clone();
        self.override_tx.send_replace(Some(entry.topic));

        // Notify the new endpoint so the user there knows the agent switched to them.
        drop(
            self.publisher
                .publish(
                    new_topic,
                    BusEvent::Notice {
                        message: "Agent switched output to this endpoint.".to_string(),
                    },
                )
                .await,
        );

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
    use crate::bus::{self, EndpointEntry, EndpointName, NotifyName};

    fn make_registry() -> EndpointRegistry {
        let registry = EndpointRegistry::new();
        registry.register(EndpointEntry {
            id: EndpointId::from("ws"),
            topic: TopicId::Interactive(EndpointName::from("ws")),
            capabilities: EndpointCapabilities::INTERACTIVE,
            display_name: "WebSocket".to_string(),
        });
        registry.register(EndpointEntry {
            id: EndpointId::from("discord"),
            topic: TopicId::Interactive(EndpointName::from("discord")),
            capabilities: EndpointCapabilities::INTERACTIVE,
            display_name: "Discord".to_string(),
        });
        registry.register(EndpointEntry {
            id: EndpointId::from("my-ntfy"),
            topic: TopicId::Notify(NotifyName::from("my-ntfy")),
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
        assert_eq!(
            override_val,
            Some(TopicId::Interactive(EndpointName::from("discord")))
        );
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
