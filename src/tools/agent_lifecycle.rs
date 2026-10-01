//! Teammate lifecycle tools: `agent_create` and `agent_delete`.
//!
//! Both call the hub's [`crate::hub::AgentDirectory`] as the calling agent,
//! so the host publishes the event that names it, which becomes a toast and a
//! team event. Neither has an approval gate: the user sees what happened and
//! can restore a deleted agent from its checkpoint.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use crate::agent::HopCounter;
use crate::config::paths::MAX_AGENT_NAME_LEN;
use crate::hub::{
    Actor, AgentState, AgentSummary, CreateAgentRequest, DirectoryHandle, LifecycleError,
};
use crate::inference::ToolDefinition;

use super::{Tool, ToolError, ToolResult};

/// How an agent's tools reach the hub: the directory, and the name the hub
/// knows this agent by.
#[derive(Clone)]
pub struct LifecycleAccess {
    directory: DirectoryHandle,
    agent: String,
}

impl LifecycleAccess {
    /// Access to `directory` on behalf of the agent named `agent`.
    #[must_use]
    pub fn new(directory: DirectoryHandle, agent: impl Into<String>) -> Self {
        Self {
            directory,
            agent: agent.into(),
        }
    }
}

const HUB_UNAVAILABLE: &str = "the hub is shutting down, so agents can't be created or deleted right now. Try again after it restarts.";

fn lifecycle_error_text(error: &LifecycleError, action: &str) -> String {
    match error {
        LifecycleError::InvalidName(rule) => format!(
            "can't {action}: {rule}. Names are 1-{MAX_AGENT_NAME_LEN} characters of lowercase letters, digits, and hyphens, with no leading or trailing hyphen."
        ),
        LifecycleError::AlreadyExists(name) => format!(
            "can't {action}: an agent named '{name}' already exists. Pick a different name, or message the existing agent at agent:{name}."
        ),
        LifecycleError::NoDeletedAgent(name) => {
            format!("can't {action}: there is no deleted agent named '{name}'.")
        }
        LifecycleError::NotFound(name) => format!(
            "can't {action}: there is no agent named '{name}'. Check the name against the team roster."
        ),
        LifecycleError::InvalidRequest(reason)
        | LifecycleError::Failed(reason)
        | LifecycleError::ShuttingDown(reason) => {
            format!("can't {action}: {reason}")
        }
        LifecycleError::NotRunning { .. } => format!("can't {action}: {error}"),
    }
}

/// What the creating agent is told about the new agent: where to reach it,
/// or, when it failed to start, that it exists but isn't running.
fn created_text(summary: &AgentSummary) -> String {
    let name = &summary.name;
    if summary.state == AgentState::Failed {
        let reason = summary
            .last_error
            .as_ref()
            .map_or("no reason was recorded", |err| err.message.as_str());
        return format!(
            "Created agent '{name}', but it failed to start: {reason}\nIt exists on disk and did not receive a first message. The user can fix its settings and start it from their team view."
        );
    }
    format!(
        "Created agent '{name}' ({state}). Reach it with message_agent at agent:{name}.",
        state = summary.state
    )
}

/// Tool that creates a new teammate agent.
pub struct AgentCreateTool {
    access: LifecycleAccess,
    hop_counter: HopCounter,
}

impl AgentCreateTool {
    /// Create the tool for the agent `access` names. `hop_counter` is the
    /// calling turn's, so the new agent's first message continues the
    /// creator's message chain.
    #[must_use]
    pub fn new(access: LifecycleAccess, hop_counter: HopCounter) -> Self {
        Self {
            access,
            hop_counter,
        }
    }
}

#[derive(Deserialize)]
struct CreateArgs {
    name: String,
    #[serde(default)]
    description: Option<String>,
}

#[async_trait]
impl Tool for AgentCreateTool {
    fn name(&self) -> &'static str {
        "agent_create"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: format!(
                "Create a new teammate: a long-lived agent with its own workspace, memory, and conversation. \
                 Use it when the work needs a durable specialist (a researcher, an inbox triager) that keeps \
                 its own notes and improves over time. For a one-off task whose result you need now, use \
                 subagent_spawn instead.\n\n\
                 It inherits your model settings and A2A visibility. Write a `description`: it is delivered as \
                 the teammate's first message and becomes its SOUL.md notes and team role page. State its \
                 purpose, how it should work, and what to hand it. Message the teammate afterwards at \
                 agent:<name>.\n\n\
                 Names are 1-{MAX_AGENT_NAME_LEN} characters of lowercase letters, digits, and hyphens, with \
                 no leading or trailing hyphen."
            ),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "The new agent's name, for example 'research-desk'"
                    },
                    "description": {
                        "type": "string",
                        "description": "What the agent is for and how it should work. Becomes its own SOUL.md notes and role page."
                    }
                },
                "required": ["name"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let args: CreateArgs = serde_json::from_value(arguments)
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;
        let Some(directory) = self.access.directory.get() else {
            return Ok(ToolResult::error(HUB_UNAVAILABLE));
        };
        let own = match directory.summary(&self.access.agent) {
            Ok(summary) => summary,
            Err(e) => {
                tracing::error!(agent = %self.access.agent, error = %e, "agent_create couldn't read the caller's own settings");
                return Ok(ToolResult::error(format!(
                    "can't create an agent: couldn't read your own settings to pass them on ({e})."
                )));
            }
        };
        let request = CreateAgentRequest {
            name: args.name,
            description: args.description,
            models_from: Some(self.access.agent.clone()),
            providers_toml: None,
            a2a_visibility: Some(own.a2a_visibility),
            creator_hop: self.hop_counter.outgoing(),
        };
        let by = Actor::Agent(self.access.agent.clone());
        // Its own task: cancelling this turn must not drop the creation
        // half-way through writing the new agent's directory.
        let created =
            crate::util::spawn_in_span(async move { directory.create(request, by).await })
                .await
                .map_err(|e| ToolError::Execution(format!("agent creation task failed: {e}")))?;
        match created {
            Ok(summary) => Ok(ToolResult::success(created_text(&summary))),
            Err(e) => Ok(ToolResult::error(lifecycle_error_text(
                &e,
                "create the agent",
            ))),
        }
    }
}

/// Tool that deletes a teammate agent.
pub struct AgentDeleteTool {
    access: LifecycleAccess,
}

impl AgentDeleteTool {
    /// Create the tool for the agent `access` names.
    #[must_use]
    pub fn new(access: LifecycleAccess) -> Self {
        Self { access }
    }
}

#[derive(Deserialize)]
struct DeleteArgs {
    name: String,
}

#[async_trait]
impl Tool for AgentDeleteTool {
    fn name(&self) -> &'static str {
        "agent_delete"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: "Stop a teammate and remove its directory, role page, and roster entry. Its files \
                 are checkpointed first; the result gives the checkpoint id. The user can restore the \
                 teammate from the team view's recently deleted list or with `residuum agent restore <name>`; \
                 you have no tool to restore one.\n\n\
                 You can delete yourself. That stops your turn the moment this call returns, so send your \
                 messages and save your files first, and call it last."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "The name of the agent to delete"
                    }
                },
                "required": ["name"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let args: DeleteArgs = serde_json::from_value(arguments)
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;
        let Some(directory) = self.access.directory.get() else {
            return Ok(ToolResult::error(HUB_UNAVAILABLE));
        };
        let by = Actor::Agent(self.access.agent.clone());
        let name = args.name;

        if name == self.access.agent {
            // Deleting stops this agent, which cancels the turn running this
            // call. Awaiting the delete here would drop it half-way, after the
            // agent stopped and before its directory was checkpointed and
            // removed. A detached task finishes the delete on its own; a
            // failure reaches the user as a hub notice (see `AgentHost`).
            let told = format!(
                "Deleting yourself now. You are stopped as soon as this call returns; your files are checkpointed first, and the user can restore you from the team view or with `residuum agent restore {name}`."
            );
            crate::util::spawn_in_span(async move {
                if let Err(e) = directory.delete(&name, by).await {
                    tracing::error!(agent = %name, error = %e, "self-delete failed");
                }
            });
            return Ok(ToolResult::success(told));
        }

        let target = name.clone();
        let deleted =
            crate::util::spawn_in_span(async move { directory.delete(&target, by).await })
                .await
                .map_err(|e| ToolError::Execution(format!("agent deletion task failed: {e}")))?;
        match deleted {
            Ok(outcome) => {
                let restore = outcome.checkpoint_id.map_or_else(
                    || "No checkpoint could be recorded, so it can't be restored.".to_string(),
                    |id| {
                        format!(
                            "Its files were checkpointed as {id}; the user can restore it from the team view or with `residuum agent restore {name}`."
                        )
                    },
                );
                Ok(ToolResult::success(format!(
                    "Deleted agent '{name}'. {restore}"
                )))
            }
            Err(e) => Ok(ToolResult::error(lifecycle_error_text(
                &e,
                "delete the agent",
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use crate::hub::{A2aVisibility, AgentErrorKind, AgentLastError};

    fn summary(state: AgentState, last_error: Option<&str>) -> AgentSummary {
        AgentSummary {
            name: "nova".to_string(),
            state,
            last_error: last_error.map(|message| AgentLastError {
                message: message.to_string(),
                kind: AgentErrorKind::Config,
                reason: "bad model settings".to_string(),
                at: Utc::now(),
            }),
            autostart: true,
            role: None,
            a2a_visibility: A2aVisibility::Private,
        }
    }

    #[test]
    fn a_running_teammate_is_reported_with_how_to_reach_it() {
        let text = created_text(&summary(AgentState::Running, None));
        assert!(text.contains("'nova' (running)"), "{text}");
        assert!(text.contains("agent:nova"), "{text}");
    }

    #[test]
    fn a_teammate_that_failed_to_start_is_reported_plainly_with_the_team_view() {
        let text = created_text(&summary(
            AgentState::Failed,
            Some("nova couldn't start: bad model settings"),
        ));
        assert!(text.contains("failed to start"), "{text}");
        assert!(text.contains("bad model settings"), "{text}");
        assert!(text.contains("team view"), "{text}");
        assert!(!text.contains("agent:nova"), "{text}");
    }
}
