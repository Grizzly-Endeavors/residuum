//! A2A client tools: discover, delegate to, and list remote A2A agents.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::a2a::client::A2aClient;
use crate::a2a::registry::{RemoteAgent, SharedA2aRegistry};
use crate::models::ToolDefinition;

use super::{Tool, ToolError, ToolResult};

// ── Discover ────────────────────────────────────────────────────────────

/// Tool that discovers a remote A2A agent by fetching its Agent Card.
pub struct A2aDiscoverTool {
    registry: SharedA2aRegistry,
    client: Arc<A2aClient>,
}

impl A2aDiscoverTool {
    /// Create a new `A2aDiscoverTool`.
    #[must_use]
    pub fn new(registry: SharedA2aRegistry, client: Arc<A2aClient>) -> Self {
        Self { registry, client }
    }
}

#[async_trait]
impl Tool for A2aDiscoverTool {
    fn name(&self) -> &'static str {
        "a2a_discover"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "a2a_discover".to_string(),
            description: "Discover a remote A2A agent by URL. Fetches the agent's \
                Agent Card from /.well-known/agent.json and registers it for future \
                delegation. If a 'name' is provided, the agent is stored under that name; \
                otherwise the agent's self-reported name is used."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "Base URL of the remote agent (e.g. 'http://other-agent:8080')"
                    },
                    "name": {
                        "type": "string",
                        "description": "Optional logical name for this agent (defaults to agent's self-reported name)"
                    },
                    "secret": {
                        "type": "string",
                        "description": "Optional bearer token for authenticating with the remote agent"
                    }
                },
                "required": ["url"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let url = arguments
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidArguments("url is required".to_string()))?;

        let url = url.trim_end_matches('/');
        let name = arguments.get("name").and_then(Value::as_str);
        let secret = arguments
            .get("secret")
            .and_then(Value::as_str)
            .map(String::from);

        let card = match self.client.discover(url).await {
            Ok(card) => card,
            Err(e) => return Ok(ToolResult::error(format!("failed to discover agent at {url}: {e}"))),
        };

        let agent_name = name
            .map(String::from)
            .unwrap_or_else(|| card.name.clone());

        let skills_summary: Vec<String> = card
            .skills
            .iter()
            .map(|s| format!("- {} ({}): {}", s.name, s.id, s.description))
            .collect();
        let skills_text = if skills_summary.is_empty() {
            "(no skills declared)".to_string()
        } else {
            skills_summary.join("\n")
        };

        let mut reg = self.registry.lock().await;
        reg.add(RemoteAgent {
            name: agent_name.clone(),
            url: url.to_string(),
            secret,
            card: Some(card.clone()),
        });

        Ok(ToolResult::success(format!(
            "Discovered agent '{agent_name}' at {url}\n\
             Description: {}\n\
             Version: {}\n\
             Skills:\n{skills_text}",
            card.description, card.version
        )))
    }
}

// ── Delegate ────────────────────────────────────────────────────────────

/// Tool that delegates a task to a known remote A2A agent.
pub struct A2aDelegateTool {
    registry: SharedA2aRegistry,
    client: Arc<A2aClient>,
}

impl A2aDelegateTool {
    /// Create a new `A2aDelegateTool`.
    #[must_use]
    pub fn new(registry: SharedA2aRegistry, client: Arc<A2aClient>) -> Self {
        Self { registry, client }
    }
}

#[async_trait]
impl Tool for A2aDelegateTool {
    fn name(&self) -> &'static str {
        "a2a_delegate"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "a2a_delegate".to_string(),
            description: "Delegate a task to a remote A2A agent. Sends a message to the \
                named agent and returns the agent's response. The agent must already be \
                known (configured or discovered via a2a_discover)."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "agent": {
                        "type": "string",
                        "description": "Name of the remote agent (as configured or discovered)"
                    },
                    "message": {
                        "type": "string",
                        "description": "The task message to send to the remote agent"
                    },
                    "task_id": {
                        "type": "string",
                        "description": "Optional task identifier (auto-generated if omitted)"
                    },
                    "session_id": {
                        "type": "string",
                        "description": "Optional session identifier for grouping related tasks"
                    }
                },
                "required": ["agent", "message"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let agent_name = arguments
            .get("agent")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidArguments("agent is required".to_string()))?;

        let message = arguments
            .get("message")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidArguments("message is required".to_string()))?;

        let task_id = arguments
            .get("task_id")
            .and_then(Value::as_str)
            .map(String::from)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        let session_id = arguments
            .get("session_id")
            .and_then(Value::as_str)
            .map(String::from);

        // Look up the agent
        let (url, secret) = {
            let reg = self.registry.lock().await;
            let agent = match reg.get(agent_name) {
                Some(a) => a,
                None => {
                    let available: Vec<String> =
                        reg.list().iter().map(|a| a.name.clone()).collect();
                    return Ok(ToolResult::error(format!(
                        "unknown agent '{agent_name}'; known agents: {}",
                        if available.is_empty() {
                            "(none)".to_string()
                        } else {
                            available.join(", ")
                        }
                    )));
                }
            };
            (agent.url.clone(), agent.secret.clone())
        };

        tracing::info!(
            agent = agent_name,
            task_id = task_id.as_str(),
            url = url.as_str(),
            "delegating task to remote a2a agent"
        );

        let task = match self
            .client
            .send_task(&url, message, &task_id, session_id.as_deref(), secret.as_deref())
            .await
        {
            Ok(task) => task,
            Err(e) => {
                return Ok(ToolResult::error(format!(
                    "failed to delegate to agent '{agent_name}': {e}"
                )));
            }
        };

        // Extract the response text from the task
        let response_text = extract_task_response(&task);
        let state = serde_json::to_string(&task.status.state).unwrap_or_default();

        Ok(ToolResult::success(format!(
            "Agent '{agent_name}' responded (task {}, state: {state}):\n\n{response_text}",
            task.id
        )))
    }
}

// ── List Agents ─────────────────────────────────────────────────────────

/// Tool that lists all known remote A2A agents.
pub struct A2aListAgentsTool {
    registry: SharedA2aRegistry,
}

impl A2aListAgentsTool {
    /// Create a new `A2aListAgentsTool`.
    #[must_use]
    pub fn new(registry: SharedA2aRegistry) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl Tool for A2aListAgentsTool {
    fn name(&self) -> &'static str {
        "a2a_list_agents"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "a2a_list_agents".to_string(),
            description: "List all known remote A2A agents (from config and runtime discovery). \
                Shows each agent's name, URL, and discovered capabilities."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        }
    }

    async fn execute(&self, _arguments: Value) -> Result<ToolResult, ToolError> {
        let reg = self.registry.lock().await;
        let agents = reg.list();

        if agents.is_empty() {
            return Ok(ToolResult::success(
                "No remote A2A agents configured or discovered.\n\n\
                 Use a2a_discover to add agents, or configure them in config.toml under [a2a.agents].",
            ));
        }

        let mut lines = Vec::with_capacity(agents.len());
        for agent in agents {
            let status = if agent.card.is_some() {
                "discovered"
            } else {
                "not yet discovered"
            };
            let mut entry = format!("- {} ({}) [{}]", agent.name, agent.url, status);

            if let Some(card) = &agent.card {
                entry.push_str(&format!("\n  Description: {}", card.description));
                if !card.skills.is_empty() {
                    let skill_names: Vec<&str> =
                        card.skills.iter().map(|s| s.name.as_str()).collect();
                    entry.push_str(&format!("\n  Skills: {}", skill_names.join(", ")));
                }
            }

            lines.push(entry);
        }

        Ok(ToolResult::success(format!(
            "Known remote A2A agents ({}):\n\n{}",
            agents.len(),
            lines.join("\n\n")
        )))
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

/// Extract readable response text from a completed A2A task.
fn extract_task_response(task: &crate::a2a::types::Task) -> String {
    // Try artifacts first (the canonical output)
    for artifact in &task.artifacts {
        for part in &artifact.parts {
            if let crate::a2a::types::Part::Text { text } = part {
                if !text.is_empty() {
                    return text.clone();
                }
            }
        }
    }

    // Fall back to the status message
    if let Some(msg) = &task.status.message {
        for part in &msg.parts {
            if let crate::a2a::types::Part::Text { text } = part {
                if !text.is_empty() {
                    return text.clone();
                }
            }
        }
    }

    // Fall back to last agent message in history
    for msg in task.history.iter().rev() {
        if msg.role == crate::a2a::types::A2aRole::Agent {
            for part in &msg.parts {
                if let crate::a2a::types::Part::Text { text } = part {
                    if !text.is_empty() {
                        return text.clone();
                    }
                }
            }
        }
    }

    "(no text response)".to_string()
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::a2a::registry::A2aRegistry;
    use crate::a2a::types::{
        A2aMessage, A2aRole, AgentCard, Artifact, Part, Task, TaskState, TaskStatus,
    };

    fn sample_card() -> AgentCard {
        AgentCard {
            name: "TestAgent".to_string(),
            description: "A test agent".to_string(),
            url: "http://localhost:9999/a2a".to_string(),
            version: "0.2".to_string(),
            capabilities: None,
            skills: vec![],
            default_input_modes: vec!["text/plain".to_string()],
            default_output_modes: vec!["text/plain".to_string()],
            authentication: None,
        }
    }

    fn sample_task(response: &str) -> Task {
        Task {
            id: "task-1".to_string(),
            session_id: None,
            status: TaskStatus {
                state: TaskState::Completed,
                message: Some(A2aMessage {
                    role: A2aRole::Agent,
                    parts: vec![Part::Text {
                        text: response.to_string(),
                    }],
                    metadata: HashMap::new(),
                }),
                timestamp: None,
            },
            history: vec![],
            artifacts: vec![Artifact {
                name: Some("response".to_string()),
                description: None,
                parts: vec![Part::Text {
                    text: response.to_string(),
                }],
                index: Some(0),
            }],
            metadata: HashMap::new(),
        }
    }

    #[test]
    fn discover_tool_name_and_definition() {
        let registry = A2aRegistry::new_shared();
        let client = Arc::new(A2aClient::new().unwrap());
        let tool = A2aDiscoverTool::new(registry, client);
        assert_eq!(tool.name(), "a2a_discover");
        let def = tool.definition();
        assert_eq!(def.name, "a2a_discover");
        assert!(
            def.parameters.get("required").is_some(),
            "should have required fields"
        );
    }

    #[test]
    fn delegate_tool_name_and_definition() {
        let registry = A2aRegistry::new_shared();
        let client = Arc::new(A2aClient::new().unwrap());
        let tool = A2aDelegateTool::new(registry, client);
        assert_eq!(tool.name(), "a2a_delegate");
        let def = tool.definition();
        assert_eq!(def.name, "a2a_delegate");
    }

    #[test]
    fn list_agents_tool_name_and_definition() {
        let registry = A2aRegistry::new_shared();
        let tool = A2aListAgentsTool::new(registry);
        assert_eq!(tool.name(), "a2a_list_agents");
    }

    #[tokio::test]
    async fn list_agents_empty_registry() {
        let registry = A2aRegistry::new_shared();
        let tool = A2aListAgentsTool::new(registry);

        let result = tool.execute(serde_json::json!({})).await.unwrap();
        assert!(!result.is_error);
        assert!(result.output.contains("No remote A2A agents"));
    }

    #[tokio::test]
    async fn list_agents_with_agents() {
        let registry = A2aRegistry::new_shared();
        {
            let mut reg = registry.lock().await;
            reg.add(RemoteAgent {
                name: "alpha".to_string(),
                url: "http://alpha:8080".to_string(),
                secret: None,
                card: Some(sample_card()),
            });
            reg.add(RemoteAgent {
                name: "beta".to_string(),
                url: "http://beta:9090".to_string(),
                secret: None,
                card: None,
            });
        }

        let tool = A2aListAgentsTool::new(registry);
        let result = tool.execute(serde_json::json!({})).await.unwrap();
        assert!(!result.is_error);
        assert!(result.output.contains("alpha"));
        assert!(result.output.contains("beta"));
        assert!(result.output.contains("discovered"));
        assert!(result.output.contains("not yet discovered"));
    }

    #[tokio::test]
    async fn delegate_unknown_agent() {
        let registry = A2aRegistry::new_shared();
        let client = Arc::new(A2aClient::new().unwrap());
        let tool = A2aDelegateTool::new(registry, client);

        let result = tool
            .execute(serde_json::json!({
                "agent": "nonexistent",
                "message": "hello"
            }))
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(result.output.contains("unknown agent"));
    }

    #[test]
    fn extract_response_from_artifacts() {
        let task = sample_task("hello from agent");
        let text = extract_task_response(&task);
        assert_eq!(text, "hello from agent");
    }

    #[test]
    fn extract_response_fallback_to_status() {
        let task = Task {
            id: "task-1".to_string(),
            session_id: None,
            status: TaskStatus {
                state: TaskState::Completed,
                message: Some(A2aMessage {
                    role: A2aRole::Agent,
                    parts: vec![Part::Text {
                        text: "status message".to_string(),
                    }],
                    metadata: HashMap::new(),
                }),
                timestamp: None,
            },
            history: vec![],
            artifacts: vec![],
            metadata: HashMap::new(),
        };
        let text = extract_task_response(&task);
        assert_eq!(text, "status message");
    }

    #[test]
    fn extract_response_fallback_to_history() {
        let task = Task {
            id: "task-1".to_string(),
            session_id: None,
            status: TaskStatus {
                state: TaskState::Completed,
                message: None,
                timestamp: None,
            },
            history: vec![A2aMessage {
                role: A2aRole::Agent,
                parts: vec![Part::Text {
                    text: "from history".to_string(),
                }],
                metadata: HashMap::new(),
            }],
            artifacts: vec![],
            metadata: HashMap::new(),
        };
        let text = extract_task_response(&task);
        assert_eq!(text, "from history");
    }

    #[test]
    fn extract_response_no_text() {
        let task = Task {
            id: "task-1".to_string(),
            session_id: None,
            status: TaskStatus {
                state: TaskState::Completed,
                message: None,
                timestamp: None,
            },
            history: vec![],
            artifacts: vec![],
            metadata: HashMap::new(),
        };
        let text = extract_task_response(&task);
        assert_eq!(text, "(no text response)");
    }

    #[tokio::test]
    async fn discover_missing_url() {
        let registry = A2aRegistry::new_shared();
        let client = Arc::new(A2aClient::new().unwrap());
        let tool = A2aDiscoverTool::new(registry, client);

        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn delegate_missing_fields() {
        let registry = A2aRegistry::new_shared();
        let client = Arc::new(A2aClient::new().unwrap());
        let tool = A2aDelegateTool::new(registry, client);

        let result = tool.execute(serde_json::json!({"agent": "test"})).await;
        assert!(result.is_err());

        let result = tool
            .execute(serde_json::json!({"message": "hello"}))
            .await;
        assert!(result.is_err());
    }
}
