//! Agent messaging tool: sends text from this agent to another by address,
//! including a remote A2A agent (`a2a:<name>`).

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::a2a::{A2aClientHub, RemoteTaskTracker};
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

/// Tool for sending a message to another agent by address — main, any
/// session (live or completed), or a remote A2A agent (`a2a:<name>`).
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
    a2a_hub: Arc<A2aClientHub>,
    a2a_tracker: Arc<RemoteTaskTracker>,
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
        a2a_hub: Arc<A2aClientHub>,
        a2a_tracker: Arc<RemoteTaskTracker>,
    ) -> Self {
        Self {
            self_address,
            self_category,
            messenger,
            hop_counter,
            a2a_hub,
            a2a_tracker,
        }
    }

    async fn send_to_remote_agent(
        &self,
        agent_name: &str,
        message: &str,
        skill: Option<&str>,
    ) -> Result<ToolResult, ToolError> {
        if !self.a2a_hub.agent_exists(agent_name).await {
            return Ok(ToolResult::error(format!(
                "no remote agent named 'a2a:{agent_name}'. Check config/a2a.json or list_agents \
                 for known remote agents."
            )));
        }

        let sender = self.self_address.as_ref();
        let (client, _card) = match self.a2a_hub.client_for(agent_name).await {
            Ok(c) => c,
            Err(e) => return Ok(ToolResult::error(e.to_string())),
        };

        // A follow-up to an open INPUT_REQUIRED/AUTH_REQUIRED task goes to
        // that task; otherwise start a new task in the persisted (sender,
        // agent) context, if one exists.
        let open_task = self
            .a2a_tracker
            .awaiting_reply_task_for(sender, agent_name)
            .await;
        let context_id = match &open_task {
            Some(task) => Some(task.context_id.clone()),
            None => self.a2a_tracker.context_for(sender, agent_name).await,
        };

        let mut msg = a2a::Message::new(a2a::Role::User, vec![a2a::Part::text(message)]);
        msg.task_id = open_task.as_ref().map(|t| t.task_id.clone());
        msg.context_id = context_id.clone();
        if let Some(skill) = skill {
            let mut meta = HashMap::new();
            meta.insert("skill".to_string(), Value::String(skill.to_string()));
            msg.metadata = Some(meta);
        }

        let hop = self.hop_counter.outgoing();
        let req = a2a::SendMessageRequest {
            message: msg,
            configuration: Some(a2a::SendMessageConfiguration {
                accepted_output_modes: None,
                task_push_notification_config: None,
                history_length: None,
                return_immediately: Some(true),
            }),
            metadata: None,
            tenant: None,
        };

        match client.send_message(&req).await {
            Ok(a2a::SendMessageResponse::Message(reply)) => {
                // A2A lets an agent answer directly without opening a task;
                // there is nothing to track, so the reply is the result.
                Ok(ToolResult::success(format!(
                    "Remote agent a2a:{agent_name} replied directly:\n{}",
                    message_text(&reply)
                )))
            }
            Ok(resp) => {
                let Some(task_id) = response_task_id(&resp) else {
                    return Ok(ToolResult::error(format!(
                        "remote agent a2a:{agent_name} replied without starting a task"
                    )));
                };
                let ctx = response_context_id(&resp)
                    .or(context_id)
                    .unwrap_or_else(|| task_id.clone());
                let state = response_state_str(&resp).unwrap_or("submitted");
                self.a2a_tracker
                    .track(
                        &self.self_address,
                        agent_name,
                        task_id.clone(),
                        ctx,
                        state,
                        hop,
                    )
                    .await;
                Ok(ToolResult::success(format!(
                    "Sent to remote agent a2a:{agent_name} (task {task_id}). Its reply will \
                     arrive as an agent message."
                )))
            }
            Err(e) => Ok(ToolResult::error(format!(
                "remote agent a2a:{agent_name} couldn't complete the request: {e}"
            ))),
        }
    }
}

/// The text parts of a direct reply, joined; non-text parts are named so the
/// agent knows something was left out.
fn message_text(reply: &a2a::Message) -> String {
    let parts: Vec<String> = reply
        .parts
        .iter()
        .map(|part| match &part.content {
            a2a::PartContent::Text(text) => text.clone(),
            a2a::PartContent::Data(value) => value.to_string(),
            a2a::PartContent::Raw(_) | a2a::PartContent::Url(_) => format!(
                "[non-text part{} not shown]",
                part.filename
                    .as_deref()
                    .map(|name| format!(" '{name}'"))
                    .unwrap_or_default()
            ),
        })
        .collect();
    parts.join("\n")
}

fn response_task_id(resp: &a2a::SendMessageResponse) -> Option<String> {
    match resp {
        a2a::SendMessageResponse::Task(t) => Some(t.id.clone()),
        a2a::SendMessageResponse::Message(_) => None,
    }
}

fn response_context_id(resp: &a2a::SendMessageResponse) -> Option<String> {
    match resp {
        a2a::SendMessageResponse::Task(t) => Some(t.context_id.clone()),
        a2a::SendMessageResponse::Message(m) => m.context_id.clone(),
    }
}

fn response_state_str(resp: &a2a::SendMessageResponse) -> Option<&'static str> {
    match resp {
        a2a::SendMessageResponse::Task(t) => {
            Some(crate::a2a::client::hub::task_state_str(&t.status.state))
        }
        a2a::SendMessageResponse::Message(_) => None,
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
            description: "Send a text message to another agent by address — main, any session \
                          (running, idle, or previously completed), or a remote agent reachable \
                          over A2A (address \"a2a:<name>\"). A running session sees it as an \
                          interrupt at its next tool-call boundary; an idle one starts a new turn \
                          with it; a completed one is resumed as a new run at the same address. \
                          A remote agent's reply does not arrive immediately — it comes back \
                          later as an agent message from \"a2a:<name>\", once its task reaches a \
                          state that needs your attention. Every delivered message names your \
                          own address and category so the recipient can reply. Use list_agents to \
                          find addresses and remote agents."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "to": {
                        "type": "string",
                        "description": "Address to message: \"main\", a session address from list_agents, or \"a2a:<name>\" for a remote agent."
                    },
                    "message": {
                        "type": "string",
                        "description": "The message body."
                    },
                    "skill": {
                        "type": "string",
                        "description": "Only meaningful when \"to\" is a remote agent: the id of one of its advertised skills to invoke, sent as message metadata."
                    }
                },
                "required": ["to", "message"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let to = super::require_str(&arguments, "to")?;
        let message = super::require_str(&arguments, "message")?;
        let skill = arguments.get("skill").and_then(Value::as_str);

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

        if let Some(agent_name) = to.strip_prefix("a2a:") {
            return self.send_to_remote_agent(agent_name, message, skill).await;
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
    use crate::a2a::AgentSource;
    use crate::background::HopLimits;
    use crate::background::registry::SessionRegistry;
    use crate::background::store::SessionStore;

    async fn make_tool(
        self_address: &str,
        self_category: &str,
    ) -> (MessageAgentTool, tempfile::TempDir) {
        let (tool, _hub, dir) = make_tool_with_hub(self_address, self_category).await;
        (tool, dir)
    }

    async fn make_tool_with_hub(
        self_address: &str,
        self_category: &str,
    ) -> (MessageAgentTool, Arc<A2aClientHub>, tempfile::TempDir) {
        let bus_handle = crate::bus::spawn_broker();
        let registry = Arc::new(SessionRegistry::new());
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SessionStore::new(dir.path().join("sessions")));
        let messenger = Arc::new(AgentMessenger::new(
            registry,
            bus_handle.publisher(),
            store,
            HopLimits { soft: 8, hard: 32 },
        ));
        let hub = A2aClientHub::new_shared();
        let tracker = RemoteTaskTracker::load(
            dir.path().join("outbound.json"),
            Arc::clone(&hub),
            Arc::clone(&messenger),
            dir.path().join("inbox"),
        )
        .await;
        let tool = MessageAgentTool::new(
            SessionAddress::from(self_address),
            self_category.to_string(),
            messenger,
            HopCounter::new(0),
            Arc::clone(&hub),
            tracker,
        );
        (tool, hub, dir)
    }

    #[tokio::test]
    async fn message_required() {
        let (tool, _dir) = make_tool("main", "main").await;
        let result = tool.execute(serde_json::json!({ "to": "main" })).await;
        assert!(result.is_err(), "should error on missing message");
    }

    #[tokio::test]
    async fn empty_message_rejected() {
        let (tool, _dir) = make_tool("main", "main").await;
        let result = tool
            .execute(serde_json::json!({ "to": "spawned-x-0001", "message": "  " }))
            .await;
        assert!(result.is_err(), "should error on empty message");
    }

    #[tokio::test]
    async fn cannot_message_self() {
        let (tool, _dir) = make_tool("spawned-researcher-0001", "spawned").await;
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
        let (tool, _dir) = make_tool("main", "main").await;
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
        let (tool, _dir) = make_tool("spawned-researcher-0001", "spawned").await;
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
        let (tool, _dir) = make_tool("artifact-wiki-0001", "artifact").await;
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
        let (tool, _dir) = make_tool("artifact-wiki-0001", "artifact").await;
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

    #[tokio::test]
    async fn messaging_an_unknown_remote_agent_is_a_plain_language_error() {
        let (tool, _dir) = make_tool("main", "main").await;
        let result = tool
            .execute(serde_json::json!({ "to": "a2a:nope", "message": "hello" }))
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.output.contains("a2a:nope"), "got: {}", result.output);
        assert!(
            result.output.contains("config/a2a.json"),
            "got: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn messaging_a_registered_but_unreachable_remote_agent_reports_offline() {
        let (tool, hub, _dir) = make_tool_with_hub("main", "main").await;
        hub.register_external(
            "laptop".to_string(),
            "http://127.0.0.1:1".to_string(),
            std::collections::HashMap::new(),
            AgentSource::Config,
        )
        .await;
        let result = tool
            .execute(serde_json::json!({ "to": "a2a:laptop", "message": "hello" }))
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(
            result.output.contains("laptop") && result.output.contains("reachable"),
            "got: {}",
            result.output
        );
    }
}
