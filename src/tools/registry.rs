use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use tokio::sync::{Mutex, Notify};

use crate::actions::store::ActionStore;
use crate::background::BackgroundTaskSpawner;
use crate::bus::EndpointRegistry;
use crate::inference::ToolDefinition;
use crate::memory::search::HybridSearcher;
use crate::skills::SharedSkillState;

use super::{
    SharedFileTracker, SharedPathPolicy, SharedToolsPath, Tool, ToolError, ToolResult, actions,
    background, edit, exec, file_bug_report, inbox, memory_get, memory_search, ollama_web_search,
    read, send_message, skills, submit_feedback, web_fetch, write,
};

/// Registry of available tools.
pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
    /// Effective `PATH` handle injected into the `exec` tool at registration.
    tools_path: Option<SharedToolsPath>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry {
    /// Create a new empty tool registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            tools: Vec::new(),
            tools_path: None,
        }
    }

    /// Set the effective `PATH` handle injected into the `exec` tool.
    ///
    /// Call before [`register_defaults`](Self::register_defaults) so the `exec`
    /// tool prepends the configured tool directories to spawned commands.
    pub fn set_tools_path(&mut self, tools_path: SharedToolsPath) {
        self.tools_path = Some(tools_path);
    }

    /// Register a tool in the registry.
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.push(tool);
    }

    /// Get tool definitions for sending to the model.
    #[must_use]
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.iter().map(|t| t.definition()).collect()
    }

    /// Names of all registered tools.
    ///
    /// Used to reserve the built-in tool namespace in the MCP registry so a
    /// colliding MCP tool is shadowed visibly rather than silently.
    #[must_use]
    pub fn tool_names(&self) -> Vec<String> {
        self.tools.iter().map(|t| t.name().to_string()).collect()
    }

    /// Execute a tool by name with the given arguments.
    ///
    /// # Errors
    /// Returns `ToolError::NotFound` if no tool with the given name exists,
    /// or propagates execution errors from the tool.
    #[tracing::instrument(skip_all, fields(tool.name = %name))]
    pub async fn execute(&self, name: &str, arguments: Value) -> Result<ToolResult, ToolError> {
        let tool = self
            .tools
            .iter()
            .find(|t| t.name() == name)
            .ok_or_else(|| ToolError::NotFound(name.to_string()))?;

        tracing::debug!("tool invocation");
        let result = tool.execute(arguments).await?;
        tracing::debug!(is_error = result.is_error, "tool result");
        Ok(result)
    }

    /// Register the default set of tools (read, write, edit, exec).
    pub fn register_defaults(&mut self, tracker: SharedFileTracker, policy: SharedPathPolicy) {
        self.register(Box::new(read::ReadTool::new(Arc::clone(&tracker))));
        self.register(Box::new(write::WriteTool::new(
            Arc::clone(&tracker),
            Arc::clone(&policy),
        )));
        self.register(Box::new(edit::EditTool::new(tracker, policy)));
        self.register(Box::new(exec::ExecTool::new(self.tools_path.clone())));
    }

    /// Register the `memory_search` tool with a shared hybrid searcher.
    pub fn register_search_tool(&mut self, searcher: Arc<HybridSearcher>) {
        self.register(Box::new(memory_search::MemorySearchTool::new(searcher)));
    }

    /// Register the `memory_get` tool for episode transcript retrieval.
    pub fn register_memory_get_tool(&mut self, episodes_dir: PathBuf) {
        self.register(Box::new(memory_get::MemoryGetTool::new(episodes_dir)));
    }

    /// Register skill management tools (`skill_activate`, `skill_deactivate`).
    pub fn register_skill_tools(&mut self, state: SharedSkillState) {
        self.register(Box::new(skills::SkillActivateTool::new(Arc::clone(&state))));
        self.register(Box::new(skills::SkillDeactivateTool::new(state)));
    }

    /// Register inbox management tools (`inbox_list`, `inbox_read`, `inbox_archive`, `user_inbox_add`).
    pub fn register_inbox_tools(
        &mut self,
        agent_inbox_dir: PathBuf,
        agent_archive_dir: PathBuf,
        user_inbox_dir: PathBuf,
        user_inbox_attachments_dir: PathBuf,
        tz: chrono_tz::Tz,
    ) {
        self.register(Box::new(inbox::InboxListTool::new(agent_inbox_dir.clone())));
        self.register(Box::new(inbox::InboxReadTool::new(agent_inbox_dir.clone())));
        self.register(Box::new(inbox::InboxArchiveTool::new(
            agent_inbox_dir,
            agent_archive_dir,
        )));
        self.register(Box::new(inbox::UserInboxAddTool::new(
            user_inbox_dir,
            user_inbox_attachments_dir,
            tz,
        )));
    }

    /// Register the `list_endpoints` tool for querying available endpoints.
    pub fn register_list_endpoints_tool(&mut self, registry: EndpointRegistry) {
        self.register(Box::new(super::list_endpoints::ListEndpointsTool::new(
            registry,
        )));
    }

    /// Register the `switch_endpoint` tool for changing the active output endpoint.
    pub fn register_switch_endpoint_tool(
        &mut self,
        registry: EndpointRegistry,
        override_tx: tokio::sync::watch::Sender<Option<crate::bus::EndpointName>>,
        publisher: crate::bus::Publisher,
    ) {
        self.register(Box::new(super::switch_endpoint::SwitchEndpointTool::new(
            registry,
            override_tx,
            publisher,
        )));
    }

    /// Register the `send_message` tool for proactive message delivery.
    pub fn register_send_message_tool(
        &mut self,
        registry: EndpointRegistry,
        publisher: crate::bus::Publisher,
    ) {
        self.register(Box::new(send_message::SendMessageTool::new(
            registry, publisher,
        )));
    }

    /// Register background task management tools (`stop_agent`, `list_agents`).
    pub fn register_background_tools(&mut self, spawner: Arc<BackgroundTaskSpawner>) {
        self.register(Box::new(background::StopAgentTool::new(Arc::clone(
            &spawner,
        ))));
        self.register(Box::new(background::ListAgentsTool::new(spawner)));
    }

    /// Register the `subagent_spawn` tool for on-demand sub-agent delegation.
    pub(crate) fn register_spawn_tool(
        &mut self,
        publisher: crate::bus::Publisher,
        skill_state: crate::skills::SharedSkillState,
    ) {
        self.register(Box::new(background::SubagentSpawnTool::new(
            publisher,
            skill_state,
        )));
    }

    /// Build a tool registry for a background sub-agent.
    ///
    /// Includes all tools available to the main agent except `switch_endpoint`
    /// and `subagent_spawn`. Sub-agents get their own isolated skill state but
    /// share the same endpoint registry, action store, etc.
    #[expect(
        clippy::too_many_arguments,
        reason = "sub-agent registry needs all tool dependencies"
    )]
    #[must_use]
    pub fn build_subagent_registry(
        tracker: SharedFileTracker,
        path_policy: SharedPathPolicy,
        skill_state: SharedSkillState,
        tz: chrono_tz::Tz,
        hybrid_searcher: Arc<HybridSearcher>,
        episodes_dir: std::path::PathBuf,
        agent_inbox_dir: std::path::PathBuf,
        agent_inbox_archive_dir: std::path::PathBuf,
        user_inbox_dir: std::path::PathBuf,
        user_inbox_attachments_dir: std::path::PathBuf,
        background_spawner: Arc<BackgroundTaskSpawner>,
        endpoint_registry: EndpointRegistry,
        publisher: crate::bus::Publisher,
        action_store: Arc<Mutex<ActionStore>>,
        action_notify: Arc<Notify>,
    ) -> Self {
        let mut registry = Self::new();

        // Core I/O tools
        registry.register_defaults(tracker, path_policy);

        // Skill tools: activate, deactivate
        registry.register_skill_tools(skill_state);

        // Memory tools
        registry.register_search_tool(hybrid_searcher);
        registry.register_memory_get_tool(episodes_dir);

        // Inbox tools
        registry.register_inbox_tools(
            agent_inbox_dir,
            agent_inbox_archive_dir,
            user_inbox_dir,
            user_inbox_attachments_dir,
            tz,
        );

        // Background task management (stop_agent, list_agents — NOT subagent_spawn)
        registry.register_background_tools(background_spawner);

        // Messaging tools
        registry.register_send_message_tool(endpoint_registry.clone(), publisher);
        registry.register_list_endpoints_tool(endpoint_registry);

        // Web fetch
        registry.register_web_fetch_tool();

        // Action scheduling tools
        registry.register_action_tools(action_store, action_notify, tz);

        registry
    }

    /// Register the `web_fetch` tool for fetching web page content.
    pub fn register_web_fetch_tool(&mut self) {
        self.register(Box::new(web_fetch::WebFetchTool::new()));
    }

    /// Register the `file_bug_report` and `submit_feedback` tools.
    ///
    /// Both go through the shared `TracingService`. The bug-report tool
    /// also captures a snapshot of the runtime client context so each
    /// submission carries version/model/OS metadata.
    pub fn register_feedback_tools(
        &mut self,
        service: Arc<crate::tracing_service::TracingService>,
        client_context: Arc<crate::tracing_service::ClientContext>,
    ) {
        self.register(Box::new(file_bug_report::FileBugReportTool::new(
            Arc::clone(&service),
            client_context,
        )));
        self.register(Box::new(submit_feedback::SubmitFeedbackTool::new(service)));
    }

    /// Register the `ollama_web_search` tool for Ollama Cloud web search.
    pub fn register_ollama_web_search_tool(&mut self, api_key: String, base_url: String) {
        self.register(Box::new(ollama_web_search::OllamaWebSearchTool::new(
            api_key, base_url,
        )));
    }

    /// Register action scheduling tools (`schedule_action`, `list_actions`, `cancel_action`).
    pub fn register_action_tools(
        &mut self,
        store: Arc<Mutex<ActionStore>>,
        notify: Arc<Notify>,
        tz: chrono_tz::Tz,
    ) {
        self.register(Box::new(actions::ScheduleActionTool::new(
            Arc::clone(&store),
            Arc::clone(&notify),
            tz,
        )));
        self.register(Box::new(actions::ListActionsTool::new(
            Arc::clone(&store),
            tz,
        )));
        self.register(Box::new(actions::CancelActionTool::new(store, notify)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{FileTracker, PathPolicy};

    #[tokio::test]
    async fn registry_not_found() {
        let registry = ToolRegistry::new();
        let result = registry.execute("nonexistent", Value::Null).await;
        assert!(result.is_err(), "should error on unknown tool");
        assert!(
            matches!(result.unwrap_err(), ToolError::NotFound(_)),
            "should be NotFound"
        );
    }

    #[test]
    fn registry_definitions_empty() {
        let registry = ToolRegistry::new();
        assert!(
            registry.definitions().is_empty(),
            "empty registry should have no definitions"
        );
    }

    #[test]
    fn registry_with_defaults() {
        let mut registry = ToolRegistry::new();
        let policy = PathPolicy::new_shared();
        registry.register_defaults(FileTracker::new_shared(), policy);
        let defs = registry.definitions();
        assert!(
            defs.iter().any(|d| d.name == "read_file"),
            "should have read_file tool"
        );
        assert!(
            defs.iter().any(|d| d.name == "write_file"),
            "should have write_file tool"
        );
        assert!(
            defs.iter().any(|d| d.name == "edit_file"),
            "should have edit_file tool"
        );
        assert!(
            defs.iter().any(|d| d.name == "exec"),
            "should have exec tool"
        );
    }
}
