//! List available I/O endpoints for the agent.

use async_trait::async_trait;
use serde_json::Value;

use crate::bus::EndpointRegistry;
use crate::models::ToolDefinition;

use super::{Tool, ToolError, ToolResult};

/// Tool for listing available endpoints grouped by category.
pub struct ListEndpointsTool {
    registry: EndpointRegistry,
}

impl ListEndpointsTool {
    /// Create a new `ListEndpointsTool`.
    #[must_use]
    pub fn new(registry: EndpointRegistry) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl Tool for ListEndpointsTool {
    fn name(&self) -> &'static str {
        "list_endpoints"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "list_endpoints".to_string(),
            description: "List available communication endpoints. \
                Shows interactive endpoints (for switch_endpoint and send_message) \
                and notification endpoints (for send_message only)."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        }
    }

    async fn execute(&self, _arguments: Value) -> Result<ToolResult, ToolError> {
        let interactive = self.registry.interactive();
        let notify = self.registry.notify();

        if interactive.is_empty() && notify.is_empty() {
            return Ok(ToolResult::success("No endpoints configured."));
        }

        let mut lines = Vec::new();

        if !interactive.is_empty() {
            lines.push("Interactive endpoints (for switch_endpoint / send_message):".to_string());
            let mut sorted = interactive;
            sorted.sort_by(|a, b| a.id.as_ref().cmp(b.id.as_ref()));
            for entry in &sorted {
                lines.push(format!(
                    "  {} \u{2014} {}",
                    entry.id.as_ref(),
                    entry.display_name
                ));
            }
        }

        if !notify.is_empty() {
            if !lines.is_empty() {
                lines.push(String::new());
            }
            lines.push("Notification endpoints (for send_message):".to_string());
            let mut sorted = notify;
            sorted.sort_by(|a, b| a.id.as_ref().cmp(b.id.as_ref()));
            for entry in &sorted {
                lines.push(format!(
                    "  {} \u{2014} {}",
                    entry.id.as_ref(),
                    entry.display_name
                ));
            }
        }

        Ok(ToolResult::success(lines.join("\n")))
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use crate::bus::{
        EndpointCapabilities, EndpointEntry, EndpointId, EndpointName, NotifyName, TopicId,
    };

    #[test]
    fn tool_name_and_definition() {
        let registry = EndpointRegistry::new();
        let tool = ListEndpointsTool::new(registry);
        assert_eq!(tool.name(), "list_endpoints");
        assert_eq!(tool.definition().name, "list_endpoints");
    }

    #[tokio::test]
    async fn empty_registry_returns_no_endpoints() {
        let registry = EndpointRegistry::new();
        let tool = ListEndpointsTool::new(registry);
        let result = tool.execute(serde_json::json!({})).await.unwrap();
        assert!(!result.is_error);
        assert_eq!(result.output, "No endpoints configured.");
    }

    #[tokio::test]
    async fn mixed_registry_groups_correctly() {
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
        // inbox and webhooks should be excluded (INPUT_ONLY)
        registry.register(EndpointEntry {
            id: EndpointId::from("inbox"),
            topic: TopicId::Inbox,
            capabilities: EndpointCapabilities::INPUT_ONLY,
            display_name: "Inbox".to_string(),
        });

        let tool = ListEndpointsTool::new(registry);
        let result = tool.execute(serde_json::json!({})).await.unwrap();
        assert!(!result.is_error);

        assert!(
            result.output.contains("Interactive endpoints"),
            "should have interactive header"
        );
        assert!(result.output.contains("ws"), "should list ws");
        assert!(result.output.contains("discord"), "should list discord");
        assert!(
            result.output.contains("Notification endpoints"),
            "should have notify header"
        );
        assert!(result.output.contains("my-ntfy"), "should list ntfy");
        assert!(!result.output.contains("Inbox"), "should exclude inbox");
    }

    #[tokio::test]
    async fn only_interactive_endpoints() {
        let registry = EndpointRegistry::new();
        registry.register(EndpointEntry {
            id: EndpointId::from("ws"),
            topic: TopicId::Endpoint(EndpointName::from("ws")),
            capabilities: EndpointCapabilities::INTERACTIVE,
            display_name: "WebSocket".to_string(),
        });

        let tool = ListEndpointsTool::new(registry);
        let result = tool.execute(serde_json::json!({})).await.unwrap();
        assert!(!result.is_error);
        assert!(result.output.contains("Interactive endpoints"));
        assert!(!result.output.contains("Notification endpoints"));
    }

    #[tokio::test]
    async fn only_notify_endpoints() {
        let registry = EndpointRegistry::new();
        registry.register(EndpointEntry {
            id: EndpointId::from("my-ntfy"),
            topic: TopicId::Notification(NotifyName::from("my-ntfy")),
            capabilities: EndpointCapabilities::NOTIFY_ONLY,
            display_name: "Ntfy (my-ntfy)".to_string(),
        });

        let tool = ListEndpointsTool::new(registry);
        let result = tool.execute(serde_json::json!({})).await.unwrap();
        assert!(!result.is_error);
        assert!(!result.output.contains("Interactive endpoints"));
        assert!(result.output.contains("Notification endpoints"));
    }
}
