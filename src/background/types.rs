//! Session execution types: what a session runs with, and the outcome of a run.

use std::sync::Arc;

use tokio::sync::{Mutex, Notify};

use crate::actions::store::ActionStore;
use crate::agent::HopCounter;
use crate::bus::{EndpointRegistry, Publisher, SessionAddress};
use crate::config::BackgroundModelTier;
use crate::inference::{ImageData, MessageSender};
use crate::interfaces::types::InboundMessage;
use crate::memory::merge_writer::MemoryMergeWriter;
use crate::memory::observer::Observer;
use crate::memory::search::HybridSearcher;
use crate::workspace::identity::IdentityFiles;
use crate::workspace::layout::WorkspaceLayout;

use super::messaging::AgentMessenger;
use super::registry::SessionCategory;

/// Configuration for a single session run's turn.
#[derive(Debug, Clone)]
pub struct SubAgentConfig {
    /// The prompt/instructions for the session.
    pub prompt: String,
    /// Additional context to prepend to the session's prompt.
    pub context: Option<String>,
    /// Which model tier to use.
    pub model_tier: BackgroundModelTier,
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
    /// `crate::background::listener::race_guard_interrupt`), its content can
    /// still be delivered into the winning run as `Interrupt::UserMessage`
    /// with correct sender attribution, instead of a plain agent message
    /// misattributed to `main`. `None` for every other trigger.
    pub inbound: Option<InboundMessage>,
    /// Images attached to this run's kickoff message, carried into its
    /// first turn's opening message. Populated for a conversation-triggered
    /// spawn or resume; empty for every other trigger, which have no images
    /// of their own.
    pub images: Vec<ImageData>,
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
    /// This session's own observer instance, for per-run threshold checks
    /// and extraction.
    pub observer: Arc<Observer>,
    /// The single serialized writer for global memory, shared with the main
    /// agent.
    pub merge_writer: Arc<MemoryMergeWriter>,
    /// Token floor below which a completed run with nothing staged produces
    /// no episode.
    pub episode_skip_token_floor: usize,
    /// This new session's own address, recorded as the spawner on any
    /// session it forks in turn via its own `subagent_spawn` tool, and
    /// reused as the identity its `message_agent` tool reports to the
    /// agents it messages.
    pub own_address: SessionAddress,
    /// This new session's own depth from the main agent (main = 0).
    pub own_depth: u32,
    /// Maximum depth a `subagent_spawn`-created session may have.
    pub subagent_depth_cap: u32,
    /// This session's category, for `message_agent` to report alongside
    /// `own_address`.
    pub session_category: SessionCategory,
    /// What triggered this session, carried into `SubagentToolDeps` so a
    /// tool running in it can tell what started it.
    pub trigger: crate::bus::EventTrigger,
    /// The conversation this session replies to, for a conversation-triggered
    /// session, carried into `SubagentToolDeps`. `None` for every other
    /// trigger.
    pub conversation_target: Option<crate::bus::ConversationTarget>,
    /// Shared agent-messaging service, for the session's `message_agent` tool.
    pub messenger: std::sync::Arc<AgentMessenger>,
    /// This session's current-turn hop counter, seeded from the input hop
    /// count that started its first turn (see [`SubAgentConfig::hop_count`]),
    /// and shared with the session's `message_agent`/`subagent_spawn` tools
    /// so they read the same value the runtime updates as the run's turns
    /// progress.
    pub hop_counter: HopCounter,
    /// Shared tracing service backing this session's own
    /// `file_bug_report`/`submit_feedback` tools, the same instance the main
    /// agent's tools register against.
    pub tracing_service: Arc<crate::tracing_service::TracingService>,
    /// Snapshot of the runtime client context for this session's bug-report
    /// submissions.
    pub tracing_client_context: Arc<crate::tracing_service::ClientContext>,
    /// Standalone web search backend config, if one is configured — mirrors
    /// `cfg.web_search.standalone_backend` so the session's tool registry can
    /// gate `ollama_web_search` the same way the main agent's does.
    pub web_search_backend: Option<crate::config::StandaloneBackendConfig>,
    /// Main's live tool `PATH` (see [`crate::tools::SubagentToolDeps::tools_path`]).
    pub tools_path: crate::tools::SharedToolsPath,
    /// Main's write policy (see [`crate::tools::SubagentToolDeps::path_policy`]).
    pub path_policy: crate::tools::SharedPathPolicy,
    /// The shared agent key store.
    pub agent_keys: crate::agent_keys::SharedAgentKeys,
    /// Remote A2A agents this instance's client can reach, shared with main.
    pub a2a_hub: Arc<crate::a2a::A2aClientHub>,
    /// Outbound A2A tasks this instance started on other agents, shared with
    /// main.
    pub a2a_tracker: Arc<crate::a2a::RemoteTaskTracker>,
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
