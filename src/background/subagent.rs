//! Sub-agent execution for background tasks.

use anyhow::Context as _;
use std::sync::Arc;

use crate::agent::context::{PromptContext, SkillsContext, build_subagent_system_content};
use crate::agent::interrupt::dead_interrupt_rx;
use crate::agent::recent_messages::RecentMessages;
use crate::agent::turn::{EventContext, TurnResources, execute_turn};
use crate::bus::Publisher;
use crate::mcp::SharedMcpRegistry;
use crate::models::{CompletionOptions, Message, ModelProvider};
use crate::skills::{SharedSkillState, SkillState};
use crate::tools::path_policy::PathPolicy;
use crate::tools::{FileTracker, ToolRegistry};
use crate::workspace::identity::IdentityFiles;

use super::types::{SubAgentBuildConfig, SubAgentConfig};

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
    /// Shared MCP registry (ref-counted, not isolated).
    pub(crate) mcp_registry: SharedMcpRegistry,
    /// Sub-agent's own isolated skill state.
    pub(crate) skill_state: SharedSkillState,
    pub(crate) identity: IdentityFiles,
    pub(crate) options: CompletionOptions,
    /// Formatted skill index for the system prompt (built at spawn time).
    pub(crate) skills_index: Option<String>,
    /// Opt-in (from preset frontmatter) to render SOUL.md, AGENTS.md, and
    /// MEMORY.md in the subagent's system prompt.
    pub(crate) include_identity: bool,
}

/// Build isolated sub-agent resources from the main agent's shared state.
///
/// Clones the skill index so the sub-agent starts with the same view of
/// available skills, but operates on its own independent copies of
/// `SkillState` and `PathPolicy`. The `McpRegistry` is shared (ref-counted)
/// so servers are not duplicated.
///
/// When `config.skill` is set, that skill is activated on the sub-agent's own
/// skill state so its body arrives as the sub-agent's role instructions.
///
/// # Errors
/// Returns an error if `config.skill` names a skill that cannot be resolved or
/// read — a sub-agent without the instructions that define its job is not worth
/// running, so the spawn fails instead.
#[tracing::instrument(skip_all)]
pub async fn build_subagent_resources(
    provider: Box<dyn ModelProvider>,
    main_skill_state: &SharedSkillState,
    mcp_registry: SharedMcpRegistry,
    config: SubAgentBuildConfig,
) -> anyhow::Result<SubAgentResources> {
    let SubAgentBuildConfig {
        workspace_layout,
        identity,
        options,
        tz,
        skill,
        include_identity,
        background_spawner,
        endpoint_registry,
        publisher,
        action_store,
        action_notify,
        hybrid_searcher,
    } = config;

    // Clone skill index and dirs for an isolated SkillState (no active skills)
    let (cloned_skill_index, skill_dirs) = {
        let guard = main_skill_state.lock().await;
        (guard.index().clone(), guard.dirs().to_vec())
    };
    let skill_state = SkillState::new_shared(cloned_skill_index, skill_dirs);

    // Activate the requested skill up front so its body renders as this
    // sub-agent's role instructions through the normal active-skill path.
    // A name that doesn't resolve fails the spawn rather than silently
    // running a sub-agent without the instructions that define its job.
    if let Some(name) = &skill {
        let mut guard = skill_state.lock().await;
        guard
            .activate(name)
            .await
            .with_context(|| format!("failed to activate skill '{name}' for sub-agent"))?;
    }

    // Fresh isolated path policy
    let path_policy = PathPolicy::new_shared();

    // Fresh file tracker (tracks reads within this sub-agent turn only)
    let tracker = FileTracker::new_shared();

    // Build the formatted index for the system prompt
    let skills_index = {
        let guard = skill_state.lock().await;
        let idx = guard.format_index_for_prompt();
        if idx.is_empty() { None } else { Some(idx) }
    };

    let tools = ToolRegistry::build_subagent_registry(
        tracker,
        Arc::clone(&path_policy),
        Arc::clone(&skill_state),
        tz,
        hybrid_searcher,
        workspace_layout.episodes_dir(),
        workspace_layout.agent_inbox_dir(),
        workspace_layout.agent_inbox_archive_dir(),
        workspace_layout.user_inbox_dir(),
        workspace_layout.user_inbox_attachments_dir(),
        background_spawner,
        endpoint_registry,
        publisher,
        action_store,
        action_notify,
    );

    Ok(SubAgentResources {
        provider,
        tools,
        mcp_registry,
        skill_state,
        identity,
        options,
        skills_index,
        include_identity,
    })
}

/// Execute a sub-agent background task.
///
/// Builds a minimal system prompt, reads any context files, and runs a single
/// agent turn loop. Returns the final text response.
///
/// # Errors
/// Returns an error if file reading or the model call fails.
#[tracing::instrument(skip_all, fields(task.id = %task_id))]
pub(crate) async fn execute_subagent(
    task_id: &str,
    config: &SubAgentConfig,
    resources: &SubAgentResources,
) -> Result<SubAgentOutput, anyhow::Error> {
    // Build skills context from the sub-agent's isolated skill state
    let active_instructions: Option<String> = {
        let guard = resources.skill_state.lock().await;
        guard.format_active_for_prompt()
    };
    let skills_ctx = SkillsContext {
        index: resources.skills_index.as_deref(),
        active_instructions: active_instructions.as_deref(),
    };

    let system_content =
        build_subagent_system_content(&resources.identity, &skills_ctx, resources.include_identity);

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

    // No broker needed: sub-agents pass `None` for both endpoints, so
    // streaming events are never published.  A noop publisher satisfies
    // the type without spawning a background task.
    let publisher = Publisher::noop();
    let mut interrupt_rx = dead_interrupt_rx();

    let memory_ctx = crate::agent::context::MemoryContext {
        observations: None,
        recent_context: None,
    };

    let prompt_ctx = PromptContext { skills: skills_ctx };

    let turn_resources = TurnResources {
        provider: &*resources.provider,
        tools: &resources.tools,
        mcp_registry: &resources.mcp_registry,
        identity: &resources.identity,
        options: &resources.options,
    };

    let events = EventContext {
        publisher: &publisher,
        output_endpoint: None,
        tool_activity_endpoint: None,
        correlation_id: "",
    };
    // Sub-agent turns are not watched by the subconscious (main agent only).
    let mut texts: Vec<String> = execute_turn(
        &turn_resources,
        &memory_ctx,
        &prompt_ctx,
        &mut recent_messages,
        &events,
        None,
        &mut interrupt_rx,
        None,
    )
    .await?;

    if texts.is_empty() {
        tracing::warn!(task_id = %task_id, "sub-agent turn produced no text output");
    }
    let summary = texts.pop().unwrap_or_default();
    let messages = recent_messages.messages().to_vec();
    Ok(SubAgentOutput { summary, messages })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::McpRegistry;
    use crate::models::{ModelError, ModelResponse, ToolDefinition};
    use crate::skills::{SkillIndex, SkillState};
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
        let skill_state = SkillState::new_shared(SkillIndex::default(), vec![]);
        let mcp_registry = McpRegistry::new_shared();
        SubAgentResources {
            provider: Box::new(MockSubAgentProvider {
                response: response.to_string(),
            }),
            tools: ToolRegistry::new(),
            mcp_registry,
            skill_state,
            identity: IdentityFiles::default(),
            options: CompletionOptions::default(),
            skills_index: None,
            include_identity: false,
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
        let content = build_subagent_system_content(&identity, &SkillsContext::default(), false);
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

        let content = build_subagent_system_content(&identity, &SkillsContext::default(), false);

        assert!(!content.contains("test soul"), "should not include SOUL.md");
        assert!(
            content.contains("exec tool"),
            "should include ENVIRONMENT.md"
        );
        assert!(
            content.contains("User likes Rust"),
            "should include USER.md"
        );
    }

    #[test]
    fn subagent_system_content_includes_skills_index() {
        let identity = IdentityFiles {
            environment: Some("exec tool".to_string()),
            ..IdentityFiles::default()
        };
        let skills_ctx = SkillsContext {
            index: Some("<available_skills><skill>pdf</skill></available_skills>"),
            active_instructions: None,
        };
        let content = build_subagent_system_content(&identity, &skills_ctx, false);
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
        let skills_ctx = SkillsContext {
            index: None,
            active_instructions: Some("<active_skill name=\"pdf\">Do PDFs.</active_skill>"),
        };
        let content = build_subagent_system_content(&identity, &skills_ctx, false);
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
        assert!(
            output.messages.len() >= 2,
            "transcript should contain at least user + assistant messages, got {}",
            output.messages.len()
        );
        let first = output.messages.first().unwrap();
        assert_eq!(first.role, crate::models::Role::User);
        assert!(
            first.content.contains("do work"),
            "user message should contain the prompt"
        );
        let last = output.messages.last().unwrap();
        assert_eq!(last.role, crate::models::Role::Assistant);
        assert_eq!(last.content, "done");
    }

    #[tokio::test]
    async fn subagent_includes_context_in_user_message() {
        let resources = make_resources("result");

        let config = SubAgentConfig {
            prompt: "check emails".to_string(),
            context: Some("extra context".to_string()),
            model_tier: crate::config::BackgroundModelTier::Medium,
        };

        let output = execute_subagent("bg-ctx", &config, &resources)
            .await
            .unwrap();
        let first = output.messages.first().unwrap();
        assert_eq!(first.role, crate::models::Role::User);
        assert!(
            first.content.contains("extra context"),
            "user message should contain the context"
        );
        assert!(
            first.content.contains("check emails"),
            "user message should contain the prompt"
        );
    }
}
