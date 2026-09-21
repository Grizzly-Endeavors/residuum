//! Session execution types: what a session runs with, and the outcome of a run.

use std::sync::Arc;

use tokio::sync::{Mutex, Notify};

use crate::actions::store::ActionStore;
use crate::bus::{EndpointRegistry, Publisher};
use crate::config::BackgroundModelTier;
use crate::memory::search::HybridSearcher;
use crate::workspace::identity::IdentityFiles;
use crate::workspace::layout::WorkspaceLayout;

/// Configuration for a single session run's turn.
#[derive(Debug, Clone)]
pub struct SubAgentConfig {
    /// The prompt/instructions for the session.
    pub prompt: String,
    /// Additional context to prepend to the session's prompt.
    pub context: Option<String>,
    /// Which model tier to use.
    pub model_tier: BackgroundModelTier,
}

/// Extract a truncated (120-char) preview from a prompt string, for display
/// as a session's `purpose`.
#[must_use]
pub(crate) fn truncate_prompt_preview(prompt: &str) -> String {
    prompt.chars().take(120).collect()
}

/// Configuration passed to [`build_subagent_resources`](super::build_subagent_resources)
/// that groups constructor arguments.
pub struct SubAgentBuildConfig {
    /// Workspace layout (used to set the path policy root).
    pub workspace_layout: WorkspaceLayout,
    /// Identity files for the system prompt.
    pub identity: IdentityFiles,
    /// LLM completion options for the session turn.
    pub options: crate::inference::CompletionOptions,
    /// Timezone used by inbox and action-scheduling tools.
    pub tz: chrono_tz::Tz,
    /// Skill to activate for this session, if any. Its body becomes the
    /// session's role instructions through the normal active-skill path.
    pub skill: Option<String>,
    /// Snapshot of the global observation log, taken at fork time.
    pub observations: Option<String>,
    /// Snapshot of the recent-context narrative, taken at fork time.
    pub recent_context: Option<String>,
    // ── Session tool dependencies ────────────────────────────────────
    /// Session registry for `stop_agent` / `list_agents` tools.
    pub session_registry: Arc<super::registry::SessionRegistry>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_tier_default_is_medium() {
        assert_eq!(
            BackgroundModelTier::default(),
            BackgroundModelTier::Medium,
            "default tier should be medium"
        );
    }

    #[test]
    fn truncate_prompt_preview_truncates_at_120_chars() {
        let long_prompt = "x".repeat(200);
        let preview = truncate_prompt_preview(&long_prompt);
        assert_eq!(preview.len(), 120, "preview should be capped at 120 chars");
    }
}
