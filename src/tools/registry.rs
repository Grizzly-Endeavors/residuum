use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use tokio::sync::{Mutex, Notify};

use crate::actions::store::ActionStore;
use crate::background::BackgroundTaskSpawner;
use crate::bus::EndpointRegistry;
use crate::mcp::SharedMcpRegistry;
use crate::memory::search::HybridSearcher;
use crate::models::ToolDefinition;
use crate::projects::activation::SharedProjectState;
use crate::skills::SharedSkillState;

use super::{
    SharedFileTracker, SharedPathPolicy, SharedToolFilter, Tool, ToolError, ToolFilter, ToolResult,
    actions, background, edit, exec, inbox, memory_get, memory_search, ollama_web_search, projects,
    read, send_message, skills, web_fetch, write,
};

/// Registry of available tools.
pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
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
        Self { tools: Vec::new() }
    }

    /// Register a tool in the registry.
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.push(tool);
    }

    /// Get tool definitions for sending to the model, filtered by the tool filter.
    #[must_use]
    pub fn definitions(&self, filter: &ToolFilter) -> Vec<ToolDefinition> {
        self.tools
            .iter()
            .filter(|t| filter.is_available(t.name()))
            .map(|t| t.definition())
            .collect()
    }

    /// Execute a tool by name with the given arguments, respecting the tool filter.
    ///
    /// # Errors
    /// Returns `ToolError::NotFound` if no tool with the given name exists,
    /// or propagates execution errors from the tool.
    pub async fn execute(
        &self,
        name: &str,
        arguments: Value,
        filter: &ToolFilter,
    ) -> Result<ToolResult, ToolError> {
        let tool = self
            .tools
            .iter()
            .find(|t| t.name() == name)
            .ok_or_else(|| ToolError::NotFound(name.to_string()))?;

        if !filter.is_available(name) {
            return Ok(ToolResult::error(format!(
                "tool '{name}' is not available — activate a project that includes it"
            )));
        }

        tracing::debug!(tool = %name, "tool invocation");
        let result = tool.execute(arguments).await?;
        tracing::debug!(tool = %name, is_error = result.is_error, "tool result");
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
        self.register(Box::new(exec::ExecTool));
    }

    /// Register the `memory_search` tool with a shared hybrid searcher.
    pub fn register_search_tool(&mut self, searcher: Arc<HybridSearcher>) {
        self.register(Box::new(memory_search::MemorySearchTool::new(searcher)));
    }

    /// Register the `memory_get` tool for episode transcript retrieval.
    pub fn register_memory_get_tool(&mut self, episodes_dir: PathBuf) {
        self.register(Box::new(memory_get::MemoryGetTool::new(episodes_dir)));
    }

    /// Register project management tools.
    pub fn register_project_tools(
        &mut self,
        state: SharedProjectState,
        path_policy: SharedPathPolicy,
        tool_filter: SharedToolFilter,
        mcp_registry: SharedMcpRegistry,
        skill_state: SharedSkillState,
        tz: chrono_tz::Tz,
    ) {
        self.register(Box::new(projects::ProjectActivateTool::new(
            Arc::clone(&state),
            Arc::clone(&path_policy),
            Arc::clone(&tool_filter),
            Arc::clone(&mcp_registry),
            Arc::clone(&skill_state),
        )));
        self.register(Box::new(projects::ProjectDeactivateTool::new(
            Arc::clone(&state),
            path_policy,
            tool_filter,
            mcp_registry,
            skill_state,
            tz,
        )));
        self.register(Box::new(projects::ProjectCreateTool::new(
            Arc::clone(&state),
            tz,
        )));
        self.register(Box::new(projects::ProjectArchiveTool::new(
            Arc::clone(&state),
            tz,
        )));
        self.register(Box::new(projects::ProjectListTool::new(state)));
    }

    /// Register skill management tools (`skill_activate`, `skill_deactivate`).
    pub fn register_skill_tools(&mut self, state: SharedSkillState) {
        self.register(Box::new(skills::SkillActivateTool::new(Arc::clone(&state))));
        self.register(Box::new(skills::SkillDeactivateTool::new(state)));
    }

    /// Register inbox management tools (`inbox_list`, `inbox_read`, `inbox_archive`).
    pub fn register_inbox_tools(&mut self, inbox_dir: PathBuf, archive_dir: PathBuf) {
        self.register(Box::new(inbox::InboxListTool::new(inbox_dir.clone())));
        self.register(Box::new(inbox::InboxReadTool::new(inbox_dir.clone())));
        self.register(Box::new(inbox::InboxArchiveTool::new(
            inbox_dir,
            archive_dir,
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
        subagents_dir: std::path::PathBuf,
    ) {
        self.register(Box::new(background::SubagentSpawnTool::new(
            publisher,
            subagents_dir,
        )));
    }

    /// Build a tool registry for a background sub-agent.
    ///
    /// Includes all tools available to the main agent except `switch_endpoint`
    /// and `subagent_spawn`. Sub-agents get their own isolated project/skill
    /// state but share the same endpoint registry, action store, etc.
    #[expect(
        clippy::too_many_arguments,
        reason = "sub-agent registry needs all tool dependencies"
    )]
    #[must_use]
    pub fn build_subagent_registry(
        tracker: SharedFileTracker,
        path_policy: SharedPathPolicy,
        project_state: SharedProjectState,
        tool_filter: SharedToolFilter,
        mcp_registry: SharedMcpRegistry,
        skill_state: SharedSkillState,
        tz: chrono_tz::Tz,
        hybrid_searcher: Arc<HybridSearcher>,
        episodes_dir: std::path::PathBuf,
        inbox_dir: std::path::PathBuf,
        inbox_archive_dir: std::path::PathBuf,
        background_spawner: Arc<BackgroundTaskSpawner>,
        endpoint_registry: EndpointRegistry,
        publisher: crate::bus::Publisher,
        action_store: Arc<Mutex<ActionStore>>,
        action_notify: Arc<Notify>,
    ) -> Self {
        let mut registry = Self::new();

        // Core I/O tools
        registry.register_defaults(tracker, Arc::clone(&path_policy));

        // Full project tools (activate, deactivate, create, archive, list)
        registry.register_project_tools(
            project_state,
            path_policy,
            tool_filter,
            mcp_registry,
            Arc::clone(&skill_state),
            tz,
        );

        // Skill tools: activate, deactivate
        registry.register_skill_tools(skill_state);

        // Memory tools
        registry.register_search_tool(hybrid_searcher);
        registry.register_memory_get_tool(episodes_dir);

        // Inbox tools
        registry.register_inbox_tools(inbox_dir, inbox_archive_dir);

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
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::tools::{FileTracker, PathPolicy};

    fn no_filter() -> ToolFilter {
        ToolFilter::new(HashSet::new())
    }

    #[tokio::test]
    async fn registry_not_found() {
        let registry = ToolRegistry::new();
        let result = registry
            .execute("nonexistent", Value::Null, &no_filter())
            .await;
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
            registry.definitions(&no_filter()).is_empty(),
            "empty registry should have no definitions"
        );
    }

    #[test]
    fn registry_with_defaults() {
        let mut registry = ToolRegistry::new();
        let policy = PathPolicy::new_shared(std::path::PathBuf::from("/tmp"));
        registry.register_defaults(FileTracker::new_shared(), policy);
        let defs = registry.definitions(&no_filter());
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

    #[test]
    fn tool_filter_definitions_filtered() {
        let mut registry = ToolRegistry::new();
        let policy = PathPolicy::new_shared(std::path::PathBuf::from("/tmp"));
        registry.register_defaults(FileTracker::new_shared(), policy);

        // Gate exec artificially to test filtering logic
        let filter_with_gate = ToolFilter::new(HashSet::from(["exec"]));
        let defs = registry.definitions(&filter_with_gate);
        assert_eq!(defs.len(), 3, "gated tool should be filtered out");
        assert!(
            defs.iter().all(|d| d.name != "exec"),
            "gated tool should not appear in definitions"
        );
    }

    #[tokio::test]
    async fn tool_filter_blocks_execution() {
        let mut registry = ToolRegistry::new();
        let policy = PathPolicy::new_shared(std::path::PathBuf::from("/tmp"));
        registry.register_defaults(FileTracker::new_shared(), policy);

        // Gate exec artificially to test blocking logic
        let filter_with_gate = ToolFilter::new(HashSet::from(["exec"]));
        let result = registry
            .execute(
                "exec",
                serde_json::json!({"command": "echo test"}),
                &filter_with_gate,
            )
            .await
            .unwrap();
        assert!(result.is_error, "gated tool should return error");
        assert!(
            result.output.contains("not available"),
            "error should mention unavailability"
        );
    }
}
