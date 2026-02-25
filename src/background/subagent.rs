//! Sub-agent execution for background tasks.

use std::path::Path;
use std::sync::Arc;

use crate::agent::context::{ProjectsContext, SkillsContext, build_subagent_system_content};
use crate::agent::interrupt::dead_interrupt_rx;
use crate::agent::recent_messages::RecentMessages;
use crate::agent::turn::execute_turn;
use crate::channels::null::NullDisplay;
use crate::mcp::SharedMcpRegistry;
use crate::models::{CompletionOptions, Message, ModelProvider};
use crate::projects::activation::{ProjectState, SharedProjectState};
use crate::skills::{SharedSkillState, SkillState};
use crate::tools::path_policy::PathPolicy;
use crate::tools::{FileTracker, SharedPathPolicy, SharedToolFilter, ToolFilter, ToolRegistry};
use crate::workspace::identity::IdentityFiles;
use crate::workspace::layout::WorkspaceLayout;

use super::types::SubAgentConfig;

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
}

/// Configuration passed to [`build_resources`] that groups constructor arguments.
pub struct SubAgentBuildConfig {
    /// Gated tool names (e.g. `"exec"`) — passed to the isolated `ToolFilter`.
    pub gated_tools: std::collections::HashSet<&'static str>,
    /// Workspace layout (used to set the path policy root).
    pub workspace_layout: WorkspaceLayout,
    /// Identity files for the system prompt.
    pub identity: IdentityFiles,
    /// LLM completion options for the sub-agent turn.
    pub options: CompletionOptions,
    /// Timezone used by project management tools.
    pub tz: chrono_tz::Tz,
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
        workspace_layout,
        identity,
        options,
        tz,
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

    // Fresh isolated tool filter
    let tool_filter = ToolFilter::new_shared(gated_tools);

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
    }
}

/// Execute a sub-agent background task.
///
/// Builds a minimal system prompt, reads any context files, and runs a single
/// agent turn loop. Returns the final text response.
///
/// After the turn completes, any project left active is force-deactivated and
/// a warning is logged (sub-agents are expected to call `project_deactivate`
/// before finishing).
///
/// # Errors
/// Returns an error if file reading or the model call fails.
pub(crate) async fn execute_subagent(
    task_id: &str,
    config: &SubAgentConfig,
    resources: &SubAgentResources,
) -> Result<String, anyhow::Error> {
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

    let system_content =
        build_subagent_system_content(&resources.identity, &projects_ctx, &skills_ctx);

    // Build user message: system content + context files + prompt
    let mut user_parts = Vec::new();

    if !system_content.is_empty() {
        user_parts.push(system_content);
    }

    if let Some(ctx) = &config.context {
        user_parts.push(ctx.clone());
    }

    for path in &config.context_files {
        match read_context_file(path).await {
            Ok(content) => {
                let filename = path.file_name().map_or_else(
                    || path.display().to_string(),
                    |n| n.to_string_lossy().to_string(),
                );
                user_parts.push(format!(
                    "<context_file name=\"{filename}\">\n{content}\n</context_file>"
                ));
            }
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "failed to read context file for sub-agent");
            }
        }
    }

    user_parts.push(config.prompt.clone());

    let combined_prompt = user_parts.join("\n\n");
    let mut recent_messages = RecentMessages::new();
    recent_messages.push(Message::user(combined_prompt));

    let display = NullDisplay;
    let mut interrupt_rx = dead_interrupt_rx();

    let memory_ctx = crate::agent::context::MemoryContext {
        observations: None,
        recent_context: None,
    };

    let texts: Vec<String> = execute_turn(
        &*resources.provider,
        &resources.tools,
        &resources.tool_filter,
        &resources.mcp_registry,
        &resources.identity,
        &resources.options,
        &memory_ctx,
        &projects_ctx,
        &skills_ctx,
        &mut recent_messages,
        &display,
        None,
        &mut interrupt_rx,
    )
    .await?;

    // Mandatory cleanup: force-deactivate any project the sub-agent left active
    let active_name = resources
        .project_state
        .lock()
        .await
        .active_project_name()
        .map(str::to_string);
    if let Some(name) = active_name {
        let prompt_preview: String = config.prompt.chars().take(200).collect();
        tracing::warn!(
            task_id = %task_id,
            project = %name,
            "[auto] SubAgent {task_id} completed without deactivating. Task: {prompt_preview}"
        );
        resources
            .mcp_registry
            .write()
            .await
            .force_deactivate_project(&name)
            .await;
        resources.path_policy.write().await.set_active_project(None);
        resources.tool_filter.write().await.clear_enabled();
    }

    Ok(texts.last().cloned().unwrap_or_default())
}

/// Read a context file from disk.
async fn read_context_file(path: &Path) -> Result<String, anyhow::Error> {
    let content = tokio::fs::read_to_string(path).await?;
    Ok(content)
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
        }
    }

    #[tokio::test]
    async fn subagent_returns_summary() {
        let resources = make_resources("3 new emails found");

        let config = SubAgentConfig {
            prompt: "check emails".to_string(),
            context: None,
            context_files: Vec::new(),
            model_tier: crate::config::BackgroundModelTier::Medium,
        };

        let result = execute_subagent("bg-001", &config, &resources)
            .await
            .unwrap();
        assert_eq!(result, "3 new emails found");
    }

    #[tokio::test]
    async fn subagent_includes_tools_md() {
        let mut resources = make_resources("done");
        resources.identity = IdentityFiles {
            tools: Some("You have access to exec tool.".to_string()),
            ..IdentityFiles::default()
        };

        let config = SubAgentConfig {
            prompt: "do something".to_string(),
            context: None,
            context_files: Vec::new(),
            model_tier: crate::config::BackgroundModelTier::Small,
        };

        let result = execute_subagent("bg-002", &config, &resources)
            .await
            .unwrap();
        assert_eq!(result, "done");
    }

    #[tokio::test]
    async fn subagent_excludes_soul_and_identity() {
        let identity = IdentityFiles {
            soul: Some("I am a test soul.".to_string()),
            identity: Some("Self-evolving identity.".to_string()),
            tools: Some("exec tool".to_string()),
            user: Some("User likes Rust".to_string()),
            ..IdentityFiles::default()
        };

        let projects_ctx = ProjectsContext {
            index: Some("project index"),
            active_context: None,
        };
        let content =
            build_subagent_system_content(&identity, &projects_ctx, &SkillsContext::none());

        assert!(!content.contains("test soul"), "should not include SOUL.md");
        assert!(
            !content.contains("Self-evolving"),
            "should not include IDENTITY.md"
        );
        assert!(content.contains("exec tool"), "should include TOOLS.md");
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
            tools: Some("exec tool".to_string()),
            ..IdentityFiles::default()
        };
        let projects_ctx = ProjectsContext::none();
        let skills_ctx = SkillsContext {
            index: Some("<available_skills><skill>pdf</skill></available_skills>"),
            active_instructions: None,
        };
        let content = build_subagent_system_content(&identity, &projects_ctx, &skills_ctx);
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
    fn subagent_system_content_excludes_active_skills_instructions() {
        // Sub-agents don't get active skill instructions in the system prompt
        // (skills are activated mid-turn and appear in tool results instead)
        let identity = IdentityFiles::default();
        let projects_ctx = ProjectsContext::none();
        let skills_ctx = SkillsContext {
            index: None,
            active_instructions: Some("<active_skill name=\"pdf\">Do PDFs.</active_skill>"),
        };
        let content = build_subagent_system_content(&identity, &projects_ctx, &skills_ctx);
        assert!(
            !content.contains("Do PDFs"),
            "active skill instructions should not appear in subagent system prompt"
        );
    }

    #[tokio::test]
    async fn subagent_no_active_project_no_cleanup_needed() {
        // Run a sub-agent with no active project — verify it completes cleanly
        let resources = make_resources("finished");
        let config = SubAgentConfig {
            prompt: "do something quick".to_string(),
            context: None,
            context_files: Vec::new(),
            model_tier: crate::config::BackgroundModelTier::Small,
        };

        let result = execute_subagent("bg-003", &config, &resources)
            .await
            .unwrap();
        assert_eq!(result, "finished");
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
