//! Session execution types: what a session runs with, and the outcome of a run.

use std::sync::Arc;

use residuum_agent::HopCounter;
use residuum_bus::MessageEvent;
use residuum_bus::SessionAddress;
use residuum_bus::session::SessionCategory;
use residuum_config::ModelSelector;
use residuum_config::WorkspaceLayout;
use residuum_inference::{ImageData, MessageSender};
use residuum_memory::merge_writer::MemoryMergeWriter;
use residuum_memory::observer::Observer;
use residuum_workspace::identity::IdentityFiles;

use super::session_tools::SessionToolFactory;

/// Configuration for a single session run's turn.
#[derive(Debug, Clone)]
pub struct SubAgentConfig {
    /// The prompt/instructions for the session.
    pub prompt: String,
    /// Additional context to prepend to the session's prompt.
    pub context: Option<String>,
    /// The model tier or explicit model to run on.
    pub model: ModelSelector,
    /// The `provider/model` id the run started on: the first model of the
    /// tier's failover chain, or the explicit model. `None` for a run started
    /// without building a provider (tests).
    pub resolved_model: Option<String>,
    /// Hop count of this run's first turn: `0` for a `scheduled`/`external`
    /// trigger, one more than the spawning turn's highest input hop count
    /// for an agent-initiated spawn, or the hop count of the message that
    /// triggered a resume.
    pub hop_count: u32,
    /// Who sent this run's opening message, for a conversation-triggered
    /// session — carried onto the fork's kickoff message so it shows the
    /// same `[From: name via interface (location)]` attribution the main
    /// agent shows. `None` for every other trigger.
    pub sender: Option<MessageSender>,
    /// The original inbound message that triggered this run, for a
    /// conversation-triggered spawn — carried so that if this run loses the
    /// race to register its own address (see
    /// `crate::listener::race_guard_interrupt`), its content can
    /// still be delivered into the winning run as `Interrupt::UserMessage`
    /// with correct sender attribution, instead of a plain agent message
    /// misattributed to `main`. `None` for every other trigger.
    pub inbound: Option<MessageEvent>,
    /// Images attached to this run's kickoff message, carried into its
    /// first turn's opening message. Populated for a conversation-triggered
    /// spawn or resume; empty for every other trigger, which have no images
    /// of their own.
    pub images: Vec<ImageData>,
    /// Main's saved conversation, for a fork of it. See
    /// [`residuum_bus::SpawnRequestEvent::carried_history`].
    pub carried_history: Vec<residuum_inference::Message>,
    /// The agent messages a resume delivers as this run's opening input. See
    /// [`residuum_bus::SpawnRequestEvent::delivered_messages`].
    pub delivered_messages: Vec<residuum_bus::AgentMessageEvent>,
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
    /// Workspace layout, for the completion memory pipeline.
    pub workspace_layout: WorkspaceLayout,
    /// Identity files for the system prompt.
    pub identity: IdentityFiles,
    /// LLM completion options for the session turn.
    pub options: residuum_inference::CompletionOptions,
    /// Maximum tool-call iterations for this session's turns before a turn
    /// stops itself gracefully. `None` means unlimited.
    pub max_tool_iterations: Option<usize>,
    /// Guards against a model repeating the exact same tool call in this
    /// session's turns.
    pub repeat_call_guard: residuum_config::RepeatCallGuardConfig,
    /// Skill to activate for this session, if any. Its body becomes the
    /// session's role instructions through the normal active-skill path.
    pub skill: Option<String>,
    /// Snapshot of the global observation log, taken at fork time.
    pub observations: Option<String>,
    /// Snapshot of the recent-context narrative, taken at fork time.
    pub recent_context: Option<String>,
    /// This session's own observer instance, for per-run threshold checks
    /// and extraction.
    pub observer: Arc<Observer>,
    /// The single serialized writer for global memory, shared with the main
    /// agent.
    pub merge_writer: Arc<MemoryMergeWriter>,
    /// Token floor below which a completed run with nothing staged produces
    /// no episode.
    pub episode_skip_token_floor: usize,
    // ── The session's tools ──────────────────────────────────────────
    /// Builds the session's tool registry, once its own skill state exists.
    pub session_tools: Arc<dyn SessionToolFactory>,
    /// This new session's own address, for its tools'
    /// [`SessionToolScope`](super::session_tools::SessionToolScope).
    pub own_address: SessionAddress,
    /// This new session's own depth from the main agent (main = 0).
    pub own_depth: u32,
    /// This session's category, for its tools' scope.
    pub session_category: SessionCategory,
    /// What triggered this session, for its tools' scope.
    pub trigger: residuum_bus::EventTrigger,
    /// The conversation this session replies to, for a conversation-triggered
    /// session, for its tools' scope. `None` for every other trigger.
    pub conversation_target: Option<residuum_bus::ConversationTarget>,
    /// This session's current-turn hop counter, seeded from the input hop
    /// count that started its first turn (see [`SubAgentConfig::hop_count`]),
    /// and shared with the session's `message_agent`/`subagent_spawn` tools
    /// so they read the same value the runtime updates as the run's turns
    /// progress.
    pub hop_counter: HopCounter,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_tier_default_is_medium() {
        assert_eq!(
            ModelSelector::default(),
            ModelSelector::MEDIUM,
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
