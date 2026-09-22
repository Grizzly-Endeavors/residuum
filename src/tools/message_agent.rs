//! Agent messaging tool: sends text from this agent to another by address.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::background::messaging::{AgentMessenger, DeliveryOutcome};
use crate::bus::SessionAddress;
use crate::inference::ToolDefinition;

use super::{Tool, ToolError, ToolResult};

/// Tool for sending a message to another agent by address — main or any
/// session, live or completed.
pub struct MessageAgentTool {
    /// This agent's own address (`"main"` for the main agent).
    self_address: SessionAddress,
    /// This agent's own category label (`"main"` for the main agent).
    self_category: String,
    messenger: Arc<AgentMessenger>,
}

impl MessageAgentTool {
    /// Create a new `MessageAgentTool` that identifies itself as `self_address`
    /// (category `self_category`) to whoever it messages.
    #[must_use]
    pub fn new(
        self_address: SessionAddress,
        self_category: String,
        messenger: Arc<AgentMessenger>,
    ) -> Self {
        Self {
            self_address,
            self_category,
            messenger,
        }
    }
}

#[async_trait]
impl Tool for MessageAgentTool {
    fn name(&self) -> &'static str {
        "message_agent"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: "Send a text message to another agent by address — main, or any \
                          session (running, idle, or previously completed). A running session \
                          sees it as an interrupt at its next tool-call boundary; an idle one \
                          starts a new turn with it; a completed one is resumed as a new run at \
                          the same address. Every delivered message names your own address and \
                          category so the recipient can reply. Use list_agents to find \
                          addresses."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "to": {
                        "type": "string",
                        "description": "Address to message: \"main\", or a session address from list_agents."
                    },
                    "message": {
                        "type": "string",
                        "description": "The message body."
                    }
                },
                "required": ["to", "message"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let to = super::require_str(&arguments, "to")?;
        let message = super::require_str(&arguments, "message")?;

        if message.trim().is_empty() {
            return Err(ToolError::InvalidArguments(
                "message must not be empty".to_string(),
            ));
        }
        if to == self.self_address.as_ref() {
            return Ok(ToolResult::error("cannot message yourself"));
        }

        let outcome = self
            .messenger
            .send(
                to,
                self.self_address.clone(),
                self.self_category.clone(),
                message.to_string(),
            )
            .await;

        Ok(match outcome {
            DeliveryOutcome::Main => ToolResult::success("Message delivered to main.".to_string()),
            DeliveryOutcome::Live(address) => {
                ToolResult::success(format!("Message delivered to {address}."))
            }
            DeliveryOutcome::Resumed(address) => ToolResult::success(format!(
                "Session {address} had completed; message delivered by resuming it as a new run."
            )),
            DeliveryOutcome::Unknown => ToolResult::error(format!(
                "no such agent '{to}'. Use list_agents to see live sessions; a completed \
                 session's address only works again once it has run at least once."
            )),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::background::registry::SessionRegistry;

    fn make_tool(self_address: &str, self_category: &str) -> MessageAgentTool {
        let bus_handle = crate::bus::spawn_broker();
        let registry = Arc::new(SessionRegistry::new());
        let messenger = Arc::new(AgentMessenger::new(registry, bus_handle.publisher()));
        MessageAgentTool::new(
            SessionAddress::from(self_address),
            self_category.to_string(),
            messenger,
        )
    }

    #[tokio::test]
    async fn message_required() {
        let tool = make_tool("main", "main");
        let result = tool.execute(serde_json::json!({ "to": "main" })).await;
        assert!(result.is_err(), "should error on missing message");
    }

    #[tokio::test]
    async fn empty_message_rejected() {
        let tool = make_tool("main", "main");
        let result = tool
            .execute(serde_json::json!({ "to": "spawned-x-0001", "message": "  " }))
            .await;
        assert!(result.is_err(), "should error on empty message");
    }

    #[tokio::test]
    async fn cannot_message_self() {
        let tool = make_tool("spawned-researcher-0001", "spawned");
        let result = tool
            .execute(serde_json::json!({
                "to": "spawned-researcher-0001",
                "message": "hi"
            }))
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.output.contains("yourself"));
    }

    #[tokio::test]
    async fn unknown_address_reports_discovery_hint() {
        let tool = make_tool("main", "main");
        let result = tool
            .execute(serde_json::json!({
                "to": "spawned-ghost-0000",
                "message": "hello?"
            }))
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.output.contains("list_agents"));
    }

    #[tokio::test]
    async fn delivering_to_main_succeeds() {
        let tool = make_tool("spawned-researcher-0001", "spawned");
        let result = tool
            .execute(serde_json::json!({
                "to": "main",
                "message": "found it"
            }))
            .await
            .unwrap();
        assert!(!result.is_error, "got: {}", result.output);
        assert!(result.output.contains("main"));
    }
}
