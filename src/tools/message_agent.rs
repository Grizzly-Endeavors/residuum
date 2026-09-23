//! Agent messaging tool: sends text from this agent to another by address.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::agent::HopCounter;
use crate::background::messaging::{AgentMessenger, DeliveryOutcome};
use crate::background::registry::{MAIN_ADDRESS, SessionCategory};
use crate::bus::SessionAddress;
use crate::inference::ToolDefinition;

use super::{Tool, ToolError, ToolResult};

/// Why an `artifact` session's message to `main` is refused: its work
/// belongs to the artifact that started it, and anything the user needs to
/// see goes to their inbox instead.
const ARTIFACT_TO_MAIN_REFUSAL: &str = "artifact sessions can't reach the main conversation: \
     your responses are shown to the artifact that started you. To bring something to the \
     user's attention, file an inbox item with user_inbox_add instead.";

/// Tool for sending a message to another agent by address — main or any
/// session, live or completed.
pub struct MessageAgentTool {
    /// This agent's own address (`"main"` for the main agent).
    self_address: SessionAddress,
    /// This agent's own category label (`"main"` for the main agent).
    self_category: String,
    messenger: Arc<AgentMessenger>,
    /// This agent's current-turn hop counter — the outgoing message carries
    /// one more than the highest hop count among the inputs driving this
    /// turn.
    hop_counter: HopCounter,
}

impl MessageAgentTool {
    /// Create a new `MessageAgentTool` that identifies itself as `self_address`
    /// (category `self_category`) to whoever it messages.
    #[must_use]
    pub fn new(
        self_address: SessionAddress,
        self_category: String,
        messenger: Arc<AgentMessenger>,
        hop_counter: HopCounter,
    ) -> Self {
        Self {
            self_address,
            self_category,
            messenger,
            hop_counter,
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
        if to == MAIN_ADDRESS && self.self_category == SessionCategory::Artifact.as_str() {
            return Ok(ToolResult::error(ARTIFACT_TO_MAIN_REFUSAL));
        }

        let outcome = self
            .messenger
            .send(
                to,
                self.self_address.clone(),
                self.self_category.clone(),
                message.to_string(),
                self.hop_counter.outgoing(),
            )
            .await;

        Ok(match outcome {
            Ok(DeliveryOutcome::Main) => {
                ToolResult::success("Message delivered to main.".to_string())
            }
            Ok(DeliveryOutcome::Live(address)) => {
                ToolResult::success(format!("Message delivered to {address}."))
            }
            Ok(DeliveryOutcome::Resumed(address)) => ToolResult::success(format!(
                "Session {address} had completed; message delivered by resuming it as a new run."
            )),
            Ok(DeliveryOutcome::Queued(address)) => ToolResult::success(format!(
                "Session {address} is completing; your message will be delivered once it \
                 finishes, resuming it as a new run."
            )),
            Ok(DeliveryOutcome::Unknown) => ToolResult::error(format!(
                "no such agent '{to}'. Use list_agents to see live sessions; a completed \
                 session's address only works again once it has run at least once."
            )),
            Err(e) => ToolResult::error(e.to_string()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::background::HopLimits;
    use crate::background::registry::SessionRegistry;
    use crate::background::store::SessionStore;

    fn make_tool(self_address: &str, self_category: &str) -> MessageAgentTool {
        let bus_handle = crate::bus::spawn_broker();
        let registry = Arc::new(SessionRegistry::new());
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SessionStore::new(dir.path().to_path_buf()));
        let messenger = Arc::new(AgentMessenger::new(
            registry,
            bus_handle.publisher(),
            store,
            HopLimits { soft: 8, hard: 32 },
        ));
        MessageAgentTool::new(
            SessionAddress::from(self_address),
            self_category.to_string(),
            messenger,
            HopCounter::new(0),
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

    #[tokio::test]
    async fn artifact_session_cannot_message_main() {
        let tool = make_tool("artifact-wiki-0001", "artifact");
        let result = tool
            .execute(serde_json::json!({
                "to": "main",
                "message": "look at this"
            }))
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(
            result.output.contains("can't reach the main conversation"),
            "got: {}",
            result.output
        );
        assert!(result.output.contains("user_inbox_add"));
    }

    #[tokio::test]
    async fn artifact_session_can_still_message_other_sessions() {
        let tool = make_tool("artifact-wiki-0001", "artifact");
        let result = tool
            .execute(serde_json::json!({
                "to": "spawned-ghost-0000",
                "message": "hello?"
            }))
            .await
            .unwrap();
        assert!(
            !result.output.contains("main conversation"),
            "only `main` is refused, got: {}",
            result.output
        );
    }
}
