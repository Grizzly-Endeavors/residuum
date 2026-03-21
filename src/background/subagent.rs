//! Sub-agent execution for background tasks.

use std::sync::Arc;

use tokio::sync::{Mutex, Notify};

use crate::actions::store::ActionStore;
use crate::agent::context::{
    ProjectsContext, PromptContext, SkillsContext, SubagentsContext, build_subagent_system_content,
};
use crate::agent::interrupt::dead_interrupt_rx;
use crate::agent::recent_messages::RecentMessages;
use crate::agent::turn::{TurnResources, execute_turn};
use crate::background::BackgroundTaskSpawner;
use crate::bus::{self, EndpointRegistry, Publisher};
use crate::mcp::SharedMcpRegistry;
use crate::memory::search::HybridSearcher;
use crate::models::{CompletionOptions, Message, ModelProvider};
use crate::projects::activation::{ProjectState, SharedProjectState};
use crate::skills::{SharedSkillState, SkillState};
use crate::tools::path_policy::PathPolicy;
use crate::tools::{FileTracker, SharedPathPolicy, SharedToolFilter, ToolFilter, ToolRegistry};
use crate::workspace::identity::IdentityFiles;
use crate::workspace::layout::WorkspaceLayout;

use super::types::SubAgentConfig;

/// Output from a completed sub-agent execution.
pub(crate) struct SubAgentOutput {
    /// The final text response (last assistant message).
    pub summary: String,
    /// Full conversation transcript (all messages exchanged during the turn).
    pub messages: Vec<Message>,
}

/// Everything needed to run a sub-agent turn, gathered at spawn time.
pub struct SubAgentResources {
    pub(crate) provider: Box<dyn ModelProvider>,
    pub(crate) tools: ToolRegistry,
    /// Sub-agent's own isolated tool filter (not shared with main agent).
    pub(crate) tool_filter: SharedToolFilter,
    /// Shared MCP registry (ref-counted, not isolated).
    pub(crate) mcp_registry: SharedMcpRegistry,
    /// Sub-agent's own isolated project state.
    pub(crate) project_state: SharedProjectState,
    /// Sub-agent's own isolated skill state.
    pub(crate) skill_state: SharedSkillState,
    /// Sub-agent's own isolated path policy.
    pub(crate) path_policy: SharedPathPolicy,
    pub(crate) identity: IdentityFiles,
    pub(crate) options: CompletionOptions,
    pub(crate) projects_ctx_index: Option<String>,
    /// Formatted skill index for the system prompt (built at spawn time).
    pub(crate) skills_index: Option<String>,
    /// Preset-specific instructions to prepend to the subagent system prompt.
    pub(crate) preset_instructions: Option<String>,
}

/// Preset-derived tool restriction for a sub-agent.
pub enum PresetToolRestriction {
    /// Tools permanently blocked (from `denied_tools` frontmatter).
    Denied(std::collections::HashSet<String>),
    /// Only listed tools are available (from `allowed_tools` frontmatter).
    AllowedOnly(std::collections::HashSet<String>),
}

/// Configuration passed to [`build_resources`] that groups constructor arguments.
pub struct SubAgentBuildConfig {
    /// Gated tool names — passed to the isolated `ToolFilter` (currently empty).
    pub gated_tools: std::collections::HashSet<&'static str>,
    /// Optional preset-level tool restriction (denied or allowed-only).
    pub preset_tool_restriction: Option<PresetToolRestriction>,
    /// Workspace layout (used to set the path policy root).
    pub workspace_layout: WorkspaceLayout,
    /// Identity files for the system prompt.
    pub identity: IdentityFiles,
    /// LLM completion options for the sub-agent turn.
    pub options: CompletionOptions,
    /// Timezone used by project management tools.
    pub tz: chrono_tz::Tz,
    /// Preset-specific instructions to inject into the subagent system prompt.
    pub preset_instructions: Option<String>,
    // ── Sub-agent tool dependencies ────────────────────────────────────
    /// Background task spawner for `stop_agent` / `list_agents` tools.
    pub background_spawner: Arc<BackgroundTaskSpawner>,
    /// Endpoint registry for `send_message` / `list_endpoints` tools.
    pub endpoint_registry: EndpointRegistry,
    /// Bus publisher for `send_message` tool.
    pub publisher: Publisher,
    /// Scheduled action store for action tools.
    pub action_store: Arc<Mutex<ActionStore>>,
    /// Notify handle for action tools.
    pub action_notify: Arc<Notify>,
    /// Hybrid searcher for `memory_search` tool.
    pub hybrid_searcher: Arc<HybridSearcher>,
}

/// Build isolated sub-agent resources from the main agent's shared state.
///
/// Clones the project and skill indices so the sub-agent starts with the same
/// view of available projects/skills, but operates on its own independent
/// copies of `ProjectState`, `SkillState`, `PathPolicy`, and `ToolFilter`.
/// The `McpRegistry` is shared (ref-counted) so servers are not duplicated.
pub async fn build_resources(
    provider: Box<dyn ModelProvider>,
    main_project_state: &SharedProjectState,
    main_skill_state: &SharedSkillState,
    mcp_registry: SharedMcpRegistry,
    config: SubAgentBuildConfig,
) -> SubAgentResources {
    let SubAgentBuildConfig {
        gated_tools,
        preset_tool_restriction,
        workspace_layout,
        identity,
        options,
        tz,
        preset_instructions,
        background_spawner,
        endpoint_registry,
        publisher,
        action_store,
        action_notify,
        hybrid_searcher,
    } = config;
    // Clone project index and layout for an isolated ProjectState
    let (project_index, layout_clone) = {
        let guard = main_project_state.lock().await;
        (guard.index().clone(), guard.layout().clone())
    };
    let project_state = ProjectState::new_shared(project_index, layout_clone);

    // Clone skill index and dirs for an isolated SkillState (no active skills)
    let (cloned_skill_index, skill_dirs) = {
        let guard: tokio::sync::MutexGuard<SkillState> = main_skill_state.lock().await;
        (guard.index().clone(), guard.dirs().to_vec())
    };
    let skill_state = SkillState::new_shared(cloned_skill_index, skill_dirs);

    // Fresh isolated path policy rooted at the workspace
    let path_policy = PathPolicy::new_shared(workspace_layout.root().to_path_buf());

    // Fresh isolated tool filter — apply preset restrictions if any
    let tool_filter = match preset_tool_restriction {
        Some(PresetToolRestriction::AllowedOnly(allowed)) => {
            ToolFilter::new_shared_allowed_only(allowed)
        }
        Some(PresetToolRestriction::Denied(denied)) => {
            ToolFilter::new_shared_with_denied(gated_tools, denied)
        }
        None => ToolFilter::new_shared(gated_tools),
    };

    // Fresh file tracker (tracks reads within this sub-agent turn only)
    let tracker = FileTracker::new_shared();

    // Build the formatted indices for the system prompt
    let projects_ctx_index = {
        let guard = project_state.lock().await;
        let idx = guard.format_index_for_prompt();
        if idx.is_empty() { None } else { Some(idx) }
    };
    let skills_index = {
        let guard: tokio::sync::MutexGuard<SkillState> = skill_state.lock().await;
        let idx = guard.format_index_for_prompt();
        if idx.is_empty() { None } else { Some(idx) }
    };

    let tools = ToolRegistry::build_subagent_registry(
        tracker,
        Arc::clone(&path_policy),
        Arc::clone(&project_state),
        Arc::clone(&tool_filter),
        Arc::clone(&mcp_registry),
        Arc::clone(&skill_state),
        tz,
        hybrid_searcher,
        workspace_layout.episodes_dir(),
        workspace_layout.inbox_dir(),
        workspace_layout.inbox_archive_dir(),
        background_spawner,
        endpoint_registry,
        publisher,
        action_store,
        action_notify,
    );

    SubAgentResources {
        provider,
        tools,
        tool_filter,
        mcp_registry,
        project_state,
        skill_state,
        path_policy,
        identity,
        options,
        projects_ctx_index,
        skills_index,
        preset_instructions,
    }
}

/// Execute a sub-agent background task.
///
/// Builds a minimal system prompt, reads any context files, and runs a single
/// agent turn loop. Returns the final text response.
///
/// After the turn completes, if a project is still active the sub-agent gets
/// one more turn with a deactivation prompt so it can call `project_deactivate`
/// with a proper session log. If it still fails to deactivate, the project is
/// deactivated manually (ref-count decremented, no session log).
///
/// # Errors
/// Returns an error if file reading or the model call fails.
pub(crate) async fn execute_subagent(
    task_id: &str,
    config: &SubAgentConfig,
    resources: &SubAgentResources,
) -> Result<SubAgentOutput, anyhow::Error> {
    let projects_ctx = ProjectsContext {
        index: resources.projects_ctx_index.as_deref(),
        active_context: None,
    };

    // Build skills context from the sub-agent's isolated skill state
    let active_instructions: Option<String> = {
        let guard: tokio::sync::MutexGuard<SkillState> = resources.skill_state.lock().await;
        guard.format_active_for_prompt()
    };
    let skills_ctx = SkillsContext {
        index: resources.skills_index.as_deref(),
        active_instructions: active_instructions.as_deref(),
    };

    let system_content = build_subagent_system_content(
        &resources.identity,
        &projects_ctx,
        &skills_ctx,
        resources.preset_instructions.as_deref(),
    );

    // Build user message: system content + context files + prompt
    let mut user_parts = Vec::new();

    if !system_content.is_empty() {
        user_parts.push(system_content);
    }

    if let Some(ctx) = &config.context {
        user_parts.push(ctx.clone());
    }

    user_parts.push(config.prompt.clone());

    let combined_prompt = user_parts.join("\n\n");
    let mut recent_messages = RecentMessages::new();
    recent_messages.push(Message::user(combined_prompt));

    let bus_handle = bus::spawn_broker();
    let publisher = bus_handle.publisher();
    let mut interrupt_rx = dead_interrupt_rx();

    let memory_ctx = crate::agent::context::MemoryContext {
        observations: None,
        recent_context: None,
    };

    let prompt_ctx = PromptContext {
        projects: projects_ctx,
        skills: skills_ctx,
        subagents: SubagentsContext::none(),
    };

    let turn_resources = TurnResources {
        provider: &*resources.provider,
        tools: &resources.tools,
        tool_filter: &resources.tool_filter,
        mcp_registry: &resources.mcp_registry,
        identity: &resources.identity,
        options: &resources.options,
    };

    let mut texts: Vec<String> = execute_turn(
        &turn_resources,
        &memory_ctx,
        &prompt_ctx,
        &mut recent_messages,
        &publisher,
        None,
        None,
        &mut interrupt_rx,
    )
    .await?;

    ensure_project_deactivated(
        task_id,
        config,
        resources,
        &memory_ctx,
        &prompt_ctx,
        &mut recent_messages,
        &publisher,
    )
    .await;

    if texts.is_empty() {
        tracing::warn!(task_id = %task_id, "sub-agent turn produced no text output");
    }
    let summary = texts.pop().unwrap_or_default();
    let messages = recent_messages.messages().to_vec();
    Ok(SubAgentOutput { summary, messages })
}

/// If a project is still active after the main turn, give the sub-agent one
/// more turn with a deactivation prompt so it can write a proper session log.
/// If the retry turn also fails, fall back to a manual ref-count decrement.
async fn ensure_project_deactivated(
    task_id: &str,
    config: &SubAgentConfig,
    resources: &SubAgentResources,
    memory_ctx: &crate::agent::context::MemoryContext<'_>,
    prompt_ctx: &PromptContext<'_>,
    recent_messages: &mut RecentMessages,
    publisher: &bus::Publisher,
) {
    let active_name = resources
        .project_state
        .lock()
        .await
        .active_project_name()
        .map(str::to_string);

    let Some(name) = active_name else {
        return;
    };

    // Retry: prompt the sub-agent to call project_deactivate with a session log
    tracing::info!(
        task_id = %task_id,
        project = %name,
        "sub-agent left project active, prompting deactivation turn"
    );

    recent_messages.push(Message::system(format!(
        "You completed your task but left project \"{name}\" active. \
         Call project_deactivate now with a session log summarizing \
         the work you did."
    )));

    let turn_resources = TurnResources {
        provider: &*resources.provider,
        tools: &resources.tools,
        tool_filter: &resources.tool_filter,
        mcp_registry: &resources.mcp_registry,
        identity: &resources.identity,
        options: &resources.options,
    };

    let mut deactivation_interrupt_rx = dead_interrupt_rx();
    if let Err(err) = execute_turn(
        &turn_resources,
        memory_ctx,
        prompt_ctx,
        recent_messages,
        publisher,
        None,
        None,
        &mut deactivation_interrupt_rx,
    )
    .await
    {
        tracing::warn!(task_id = %task_id, error = %err, "deactivation turn failed");
    }

    // Safety net: if the retry didn't clean up, decrement the ref manually
    let still_active = resources
        .project_state
        .lock()
        .await
        .active_project_name()
        .map(str::to_string);
    if let Some(still_name) = still_active {
        let prompt_preview: String = config.prompt.chars().take(200).collect();
        tracing::warn!(
            task_id = %task_id,
            project = %still_name,
            prompt_preview = %prompt_preview,
            "sub-agent completed without deactivating project after retry"
        );
        resources
            .mcp_registry
            .write()
            .await
            .deactivate_project(&still_name)
            .await;
        resources.path_policy.write().await.set_active_project(None);
        resources.tool_filter.write().await.clear_enabled();
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use std::collections::HashSet;
    use std::path::PathBuf;

    use super::*;
    use crate::mcp::McpRegistry;
    use crate::models::{ModelError, ModelResponse, ToolDefinition};
    use crate::projects::activation::ProjectState;
    use crate::projects::scanner::ProjectIndex;
    use crate::skills::{SkillIndex, SkillState};
    use crate::tools::ToolFilter;
    use crate::workspace::layout::WorkspaceLayout;
    use async_trait::async_trait;

    struct MockSubAgentProvider {
        response: String,
    }

    #[async_trait]
    impl ModelProvider for MockSubAgentProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<ModelResponse, ModelError> {
            Ok(ModelResponse::new(self.response.clone(), vec![]))
        }

        fn model_name(&self) -> &'static str {
            "mock-subagent"
        }
    }

    fn make_resources(response: &str) -> SubAgentResources {
        let project_state = ProjectState::new_shared(
            ProjectIndex::default(),
            WorkspaceLayout::new(PathBuf::from("/tmp")),
        );
        let skill_state = SkillState::new_shared(SkillIndex::default(), vec![]);
        let path_policy = PathPolicy::new_shared(PathBuf::from("/tmp"));
        let tool_filter = ToolFilter::new_shared(HashSet::new());
        let mcp_registry = McpRegistry::new_shared();
        SubAgentResources {
            provider: Box::new(MockSubAgentProvider {
                response: response.to_string(),
            }),
            tools: ToolRegistry::new(),
            tool_filter,
            mcp_registry,
            project_state,
            skill_state,
            path_policy,
            identity: IdentityFiles::default(),
            options: CompletionOptions::default(),
            projects_ctx_index: None,
            skills_index: None,
            preset_instructions: None,
        }
    }

    #[tokio::test]
    async fn subagent_returns_summary() {
        let resources = make_resources("3 new emails found");

        let config = SubAgentConfig {
            prompt: "check emails".to_string(),
            context: None,
            model_tier: crate::config::BackgroundModelTier::Medium,
        };

        let output = execute_subagent("bg-001", &config, &resources)
            .await
            .unwrap();
        assert_eq!(output.summary, "3 new emails found");
    }

    #[test]
    fn subagent_system_content_includes_environment() {
        // Directly verify build_subagent_system_content includes ENVIRONMENT.md
        // content. (The execute_subagent mock ignores message contents, so
        // testing at that level can't catch a silent drop of identity fields.)
        let identity = IdentityFiles {
            environment: Some("You have access to exec tool.".to_string()),
            ..IdentityFiles::default()
        };
        let content = build_subagent_system_content(
            &identity,
            &ProjectsContext::none(),
            &SkillsContext::none(),
            None,
        );
        assert!(
            content.contains("You have access to exec tool."),
            "should include ENVIRONMENT.md content"
        );
    }

    #[tokio::test]
    async fn subagent_excludes_soul() {
        let identity = IdentityFiles {
            soul: Some("I am a test soul.".to_string()),
            environment: Some("exec tool".to_string()),
            user: Some("User likes Rust".to_string()),
            ..IdentityFiles::default()
        };

        let projects_ctx = ProjectsContext {
            index: Some("project index"),
            active_context: None,
        };
        let content =
            build_subagent_system_content(&identity, &projects_ctx, &SkillsContext::none(), None);

        assert!(!content.contains("test soul"), "should not include SOUL.md");
        assert!(
            content.contains("exec tool"),
            "should include ENVIRONMENT.md"
        );
        assert!(
            content.contains("User likes Rust"),
            "should include USER.md"
        );
        assert!(
            content.contains("project index"),
            "should include projects index"
        );
    }

    #[test]
    fn subagent_system_content_includes_skills_index() {
        let identity = IdentityFiles {
            environment: Some("exec tool".to_string()),
            ..IdentityFiles::default()
        };
        let projects_ctx = ProjectsContext::none();
        let skills_ctx = SkillsContext {
            index: Some("<available_skills><skill>pdf</skill></available_skills>"),
            active_instructions: None,
        };
        let content = build_subagent_system_content(&identity, &projects_ctx, &skills_ctx, None);
        assert!(
            content.contains("<SKILLS_INDEX>"),
            "should include skills index section"
        );
        assert!(
            content.contains("pdf"),
            "should include skill name from index"
        );
    }

    #[test]
    fn subagent_system_content_includes_active_skills_instructions() {
        // Sub-agents now include active skill instructions in the system prompt
        let identity = IdentityFiles::default();
        let projects_ctx = ProjectsContext::none();
        let skills_ctx = SkillsContext {
            index: None,
            active_instructions: Some("<active_skill name=\"pdf\">Do PDFs.</active_skill>"),
        };
        let content = build_subagent_system_content(&identity, &projects_ctx, &skills_ctx, None);
        assert!(
            content.contains("Do PDFs"),
            "active skill instructions should appear in subagent system prompt"
        );
        assert!(
            content.contains("<ACTIVE_SKILLS>"),
            "should include active skills section"
        );
    }

    #[tokio::test]
    async fn subagent_captures_full_transcript() {
        let resources = make_resources("done");

        let config = SubAgentConfig {
            prompt: "do work".to_string(),
            context: None,
            model_tier: crate::config::BackgroundModelTier::Small,
        };

        let output = execute_subagent("bg-002", &config, &resources)
            .await
            .unwrap();
        assert_eq!(output.summary, "done");
        // At minimum: the initial user message + the assistant response
        assert!(
            output.messages.len() >= 2,
            "transcript should contain at least user + assistant messages, got {}",
            output.messages.len()
        );
    }

    #[tokio::test]
    async fn subagent_no_active_project_no_cleanup_needed() {
        // Run a sub-agent with no active project — verify it completes cleanly
        let resources = make_resources("finished");
        let config = SubAgentConfig {
            prompt: "do something quick".to_string(),
            context: None,
            model_tier: crate::config::BackgroundModelTier::Small,
        };

        let output = execute_subagent("bg-003", &config, &resources)
            .await
            .unwrap();
        assert_eq!(output.summary, "finished");
        // After the turn, project state should still have no active project
        assert!(
            resources
                .project_state
                .lock()
                .await
                .active_project_name()
                .is_none(),
            "no project should be active after clean turn"
        );
    }
}
