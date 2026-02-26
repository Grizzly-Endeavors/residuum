use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use tokio::sync::{Mutex, Notify};

use crate::background::BackgroundTaskSpawner;
use crate::background::spawn_context::SpawnContext;
use crate::cron::store::CronStore;
use crate::mcp::SharedMcpRegistry;
use crate::memory::search::HybridSearcher;
use crate::models::ToolDefinition;
use crate::projects::activation::SharedProjectState;
use crate::skills::SharedSkillState;

use super::{
    SharedFileTracker, SharedPathPolicy, SharedToolFilter, Tool, ToolError, ToolFilter, ToolResult,
    background, cron, edit, exec, inbox, memory_get, memory_search, projects, read, skills, write,
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

        tool.execute(arguments).await
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

    /// Register inbox management tools (`inbox_list`, `inbox_read`, `inbox_add`, `inbox_archive`).
    pub fn register_inbox_tools(
        &mut self,
        inbox_dir: PathBuf,
        archive_dir: PathBuf,
        tz: chrono_tz::Tz,
    ) {
        self.register(Box::new(inbox::InboxListTool::new(inbox_dir.clone())));
        self.register(Box::new(inbox::InboxReadTool::new(inbox_dir.clone())));
        self.register(Box::new(inbox::InboxAddTool::new(inbox_dir.clone(), tz)));
        self.register(Box::new(inbox::InboxArchiveTool::new(
            inbox_dir,
            archive_dir,
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
        spawner: Arc<BackgroundTaskSpawner>,
        spawn_context: Arc<SpawnContext>,
        project_state: SharedProjectState,
        skill_state: SharedSkillState,
        mcp_registry: SharedMcpRegistry,
        valid_external_channels: HashSet<String>,
    ) {
        self.register(Box::new(background::SubAgentSpawnTool::new(
            spawner,
            spawn_context,
            project_state,
            skill_state,
            mcp_registry,
            valid_external_channels,
        )));
    }

    /// Build a tool registry for a background sub-agent.
    ///
    /// Includes core tools (read, write, edit, exec), project tools (activate,
    /// deactivate, list — not create or archive), and skill tools (activate,
    /// deactivate). Excludes cron, inbox, memory, and background management
    /// tools which are not appropriate for isolated sub-agent turns.
    #[must_use]
    pub fn build_subagent_registry(
        tracker: SharedFileTracker,
        path_policy: SharedPathPolicy,
        project_state: SharedProjectState,
        tool_filter: SharedToolFilter,
        mcp_registry: SharedMcpRegistry,
        skill_state: SharedSkillState,
        tz: chrono_tz::Tz,
    ) -> Self {
        let mut registry = Self::new();

        // Core I/O tools
        registry.register_defaults(tracker, Arc::clone(&path_policy));

        // Project tools: activate, deactivate, list (not create/archive)
        registry.register(Box::new(projects::ProjectActivateTool::new(
            Arc::clone(&project_state),
            Arc::clone(&path_policy),
            Arc::clone(&tool_filter),
            Arc::clone(&mcp_registry),
            Arc::clone(&skill_state),
        )));
        registry.register(Box::new(projects::ProjectDeactivateTool::new(
            Arc::clone(&project_state),
            path_policy,
            tool_filter,
            mcp_registry,
            Arc::clone(&skill_state),
            tz,
        )));
        registry.register(Box::new(projects::ProjectListTool::new(project_state)));

        // Skill tools: activate, deactivate
        registry.register_skill_tools(skill_state);

        registry
    }

    /// Register cron management tools (`cron_add`, `cron_list`, `cron_update`, `cron_remove`).
    pub fn register_cron_tools(
        &mut self,
        store: Arc<Mutex<CronStore>>,
        notify: Arc<Notify>,
        tz: chrono_tz::Tz,
    ) {
        self.register(Box::new(cron::CronAddTool::new(
            Arc::clone(&store),
            Arc::clone(&notify),
            tz,
        )));
        self.register(Box::new(cron::CronListTool::new(Arc::clone(&store))));
        self.register(Box::new(cron::CronUpdateTool::new(
            Arc::clone(&store),
            Arc::clone(&notify),
            tz,
        )));
        self.register(Box::new(cron::CronRemoveTool::new(store, notify)));
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
        assert_eq!(defs.len(), 4, "should have read, write, edit, exec tools");
    }

    #[test]
    fn tool_filter_definitions_filtered() {
        let mut registry = ToolRegistry::new();
        let policy = PathPolicy::new_shared(std::path::PathBuf::from("/tmp"));
        registry.register_defaults(FileTracker::new_shared(), policy);

        let filter_exec = ToolFilter::new(HashSet::from(["exec"]));
        let defs = registry.definitions(&filter_exec);
        assert_eq!(defs.len(), 3, "exec should be filtered out");
        assert!(
            defs.iter().all(|d| d.name != "exec"),
            "exec should not appear in definitions"
        );
    }

    #[tokio::test]
    async fn tool_filter_blocks_execution() {
        let mut registry = ToolRegistry::new();
        let policy = PathPolicy::new_shared(std::path::PathBuf::from("/tmp"));
        registry.register_defaults(FileTracker::new_shared(), policy);

        let filter_exec = ToolFilter::new(HashSet::from(["exec"]));
        let result = registry
            .execute(
                "exec",
                serde_json::json!({"command": "echo test"}),
                &filter_exec,
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
