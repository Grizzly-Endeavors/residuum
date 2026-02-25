//! Sub-agent execution for background tasks.

use std::path::Path;

use crate::agent::context::{ProjectsContext, SkillsContext, build_subagent_system_content};
use crate::agent::interrupt::dead_interrupt_rx;
use crate::agent::recent_messages::RecentMessages;
use crate::agent::turn::execute_turn;
use crate::channels::null::NullDisplay;
use crate::mcp::SharedMcpRegistry;
use crate::models::{CompletionOptions, Message, ModelProvider};
use crate::tools::{SharedToolFilter, ToolRegistry};
use crate::workspace::identity::IdentityFiles;

use super::types::SubAgentConfig;

/// Everything needed to run a sub-agent turn, gathered at spawn time.
pub struct SubAgentResources {
    pub(crate) provider: Box<dyn ModelProvider>,
    pub(crate) tools: ToolRegistry,
    pub(crate) tool_filter: SharedToolFilter,
    pub(crate) mcp_registry: SharedMcpRegistry,
    pub(crate) identity: IdentityFiles,
    pub(crate) options: CompletionOptions,
    pub(crate) projects_ctx_index: Option<String>,
}

/// Execute a sub-agent background task.
///
/// Builds a minimal system prompt, reads any context files, and runs a single
/// agent turn loop. Returns the final text response.
///
/// # Errors
/// Returns an error if file reading or the model call fails.
pub(crate) async fn execute_subagent(
    _task_id: &str,
    config: &SubAgentConfig,
    resources: &SubAgentResources,
) -> Result<String, anyhow::Error> {
    let projects_ctx = ProjectsContext {
        index: resources.projects_ctx_index.as_deref(),
        active_context: None,
    };

    let system_content = build_subagent_system_content(&resources.identity, &projects_ctx);

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
        &SkillsContext::none(),
        &mut recent_messages,
        &display,
        None,
        &mut interrupt_rx,
    )
    .await?;

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
    use super::*;
    use crate::mcp::McpRegistry;
    use crate::models::{ModelError, ModelResponse, ToolDefinition};
    use crate::tools::ToolFilter;
    use async_trait::async_trait;
    use std::collections::HashSet;

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

    #[tokio::test]
    async fn subagent_returns_summary() {
        let resources = SubAgentResources {
            provider: Box::new(MockSubAgentProvider {
                response: "3 new emails found".to_string(),
            }),
            tools: ToolRegistry::new(),
            tool_filter: ToolFilter::new_shared(HashSet::new()),
            mcp_registry: McpRegistry::new_shared(),
            identity: IdentityFiles::default(),
            options: CompletionOptions::default(),
            projects_ctx_index: None,
        };

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
        let identity = IdentityFiles {
            tools: Some("You have access to exec tool.".to_string()),
            ..IdentityFiles::default()
        };

        let resources = SubAgentResources {
            provider: Box::new(MockSubAgentProvider {
                response: "done".to_string(),
            }),
            tools: ToolRegistry::new(),
            tool_filter: ToolFilter::new_shared(HashSet::new()),
            mcp_registry: McpRegistry::new_shared(),
            identity,
            options: CompletionOptions::default(),
            projects_ctx_index: None,
        };

        let config = SubAgentConfig {
            prompt: "do something".to_string(),
            context: None,
            context_files: Vec::new(),
            model_tier: crate::config::BackgroundModelTier::Small,
        };

        // The test verifies it doesn't panic and returns successfully
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
        let content = build_subagent_system_content(&identity, &projects_ctx);

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
}
