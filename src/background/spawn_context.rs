//! Spawn context: parameters needed to construct providers and `SubAgentResources`
//! for a new session run (pulse, action, webhook, or on-demand spawn).

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use anyhow::Context as _;
use tokio::sync::{Mutex, Notify};

use crate::actions::store::ActionStore;
use crate::agent::HopCounter;
use crate::agent::context::loading::{load_observations, load_recent_context_narrative};
use crate::background::registry::{SessionCategory, SessionRegistry};
use crate::background::runtime::SessionRuntime;
use crate::bus::{EndpointRegistry, Publisher, SessionAddress};
use crate::config::ProviderSpec;
use crate::config::{BackgroundConfig, BackgroundModelTier};
use crate::inference::retry::RetryConfig;
use crate::inference::{
    CompletionOptions, SharedHttpClient, build_provider_chain_with_shared_notices,
};
use crate::mcp::SharedMcpRegistry;
use crate::memory::merge_writer::MemoryMergeWriter;
use crate::memory::observer::Observer;
use crate::memory::search::HybridSearcher;
use crate::skills::SharedSkillState;
use crate::workspace::identity::IdentityFiles;
use crate::workspace::layout::WorkspaceLayout;

use super::messaging::AgentMessenger;
use super::subagent::{SubAgentResources, build_subagent_resources};
use super::types::SubAgentBuildConfig;

/// Shared failover-transition counters for the background model tiers.
///
/// A background tier's provider chain is rebuilt fresh for every session
/// spawn (see [`build_spawn_resources`]), unlike the main model's or the
/// subconscious's, which are built once and live until the next config
/// reload. Sharing one counter per tier here — held on `SpawnContext` for
/// that tier's whole lifetime, across every spawn that uses it — means a
/// transition notices exactly once when the tier actually fails over or
/// recovers, rather than once per spawn made while it's already degraded.
#[derive(Clone, Default)]
pub(crate) struct BackgroundTierActiveIndex {
    small: Arc<AtomicUsize>,
    medium: Arc<AtomicUsize>,
    large: Arc<AtomicUsize>,
}

impl BackgroundTierActiveIndex {
    /// This tier's shared counter, cloned (the `Arc`, not the count).
    fn for_tier(&self, tier: BackgroundModelTier) -> Arc<AtomicUsize> {
        match tier {
            BackgroundModelTier::Small => Arc::clone(&self.small),
            BackgroundModelTier::Medium => Arc::clone(&self.medium),
            BackgroundModelTier::Large => Arc::clone(&self.large),
        }
    }
}

/// Everything needed to fork a new session run from the gateway event loop.
pub(crate) struct SpawnContext {
    pub(crate) background_config: BackgroundConfig,
    pub(crate) main_provider_specs: Vec<ProviderSpec>,
    pub(crate) http_client: SharedHttpClient,
    pub(crate) max_tokens: u32,
    pub(crate) retry_config: RetryConfig,
    pub(crate) options: CompletionOptions,
    /// Maximum tool-call iterations for a session turn before it stops
    /// itself gracefully, mirroring `cfg.agent.max_tool_iterations`. `None`
    /// means unlimited.
    pub(crate) max_tool_iterations: Option<usize>,
    /// Guards against a model repeating the exact same tool call, mirroring
    /// `cfg.agent.repeat_call_guard`.
    pub(crate) repeat_call_guard: crate::config::RepeatCallGuardConfig,
    pub(crate) layout: WorkspaceLayout,
    /// The app config directory (`~/.residuum/`), for a session's
    /// `write_file`/`edit_file` tools to recognize `config.toml`/
    /// `providers.toml` and diagnose them the same way main's do.
    pub(crate) config_dir: std::path::PathBuf,
    pub(crate) tz: chrono_tz::Tz,
    pub(crate) role_overrides: std::collections::HashMap<String, crate::config::RoleOverrides>,
    // ── Session tool dependencies ────────────────────────────────────
    pub(crate) session_runtime: Arc<SessionRuntime>,
    pub(crate) session_registry: Arc<SessionRegistry>,
    pub(crate) endpoint_registry: EndpointRegistry,
    pub(crate) publisher: Publisher,
    pub(crate) action_store: Arc<Mutex<ActionStore>>,
    pub(crate) action_notify: Arc<Notify>,
    pub(crate) hybrid_searcher: Arc<HybridSearcher>,
    /// Main agent skill state — cloned per fork into isolated session state.
    pub(crate) skill_state: SharedSkillState,
    /// Shared MCP registry (ref-counted across sessions).
    pub(crate) mcp_registry: SharedMcpRegistry,
    /// A session's own observer instance, built from the same `[observer]`
    /// config the main agent uses, for per-run threshold checks and
    /// extraction. Independent from the main agent's `Observer` instance so
    /// a session fork never contends with a main config-reload swap.
    pub(crate) observer: Arc<Observer>,
    /// The single serialized writer for global memory, shared with the main
    /// agent so episode numbering and log appends never race between a
    /// session's completion pipeline and the main agent's own observations.
    pub(crate) merge_writer: Arc<MemoryMergeWriter>,
    /// Shared agent-messaging service, threaded into every fork so its
    /// `message_agent` tool can identify itself as the sender.
    pub(crate) messenger: Arc<AgentMessenger>,
    /// Shared tracing service, threaded into every fork's own
    /// `file_bug_report`/`submit_feedback` tools — the same instance the
    /// main agent's tools register against, so config updates (see
    /// `crate::gateway::reload::reload_tracing`) reach sessions too since
    /// they share the `Arc`.
    pub(crate) tracing_service: Arc<crate::tracing_service::TracingService>,
    /// Runtime client context snapshot for forks' bug-report submissions,
    /// rebuilt alongside the rest of `SpawnContext` on every config reload
    /// (see `crate::gateway::reload::build_spawn_context`) so it reflects
    /// the currently active model/provider.
    pub(crate) tracing_client_context: Arc<crate::tracing_service::ClientContext>,
    /// Standalone web search backend config, if one is configured — mirrors
    /// `cfg.web_search.standalone_backend`, rebuilt on every config reload so
    /// a newly forked session gates `ollama_web_search` the same way the
    /// main agent's own (startup-time) check does.
    pub(crate) web_search_backend: Option<crate::config::StandaloneBackendConfig>,
    /// Main's live tool `PATH`, shared so a session's `exec` resolves the
    /// same binaries and sees config reloads.
    pub(crate) tools_path: crate::tools::SharedToolsPath,
    /// Main's write policy, shared so a session is blocked from the same
    /// config and credential files and sees config reloads.
    pub(crate) path_policy: crate::tools::SharedPathPolicy,
    /// The shared agent key store.
    pub(crate) agent_keys: crate::agent_keys::SharedAgentKeys,
    /// Remote A2A agents this instance's client can reach, shared with main.
    pub(crate) a2a_hub: Arc<crate::a2a::A2aClientHub>,
    /// Outbound A2A tasks this instance started on other agents, shared with
    /// main.
    pub(crate) a2a_tracker: Arc<crate::a2a::RemoteTaskTracker>,
    /// Workspace and config checkpoint repositories, shared with main —
    /// backs the `workspace_history`/`workspace_restore` tools.
    pub(crate) checkpoints: Arc<crate::checkpoints::CheckpointEngine>,
    /// Shared failover-transition counters for the background model tiers —
    /// see [`BackgroundTierActiveIndex`].
    pub(crate) bg_tier_active_index: BackgroundTierActiveIndex,
}

/// Identity and origin of the session [`build_spawn_resources`] is forking,
/// grouped into one struct to keep that function's argument count down.
pub(crate) struct NewSessionContext {
    /// This new session's own address, threaded into its `subagent_spawn`
    /// tool so any session it spawns in turn records the right spawner.
    pub(crate) own_address: SessionAddress,
    /// This new session's own depth from the main agent (main = 0).
    pub(crate) own_depth: u32,
    /// This session's category, carried into `SubAgentBuildConfig` so its
    /// `message_agent` tool can report it.
    pub(crate) category: SessionCategory,
    /// Hop count of this session's first turn (see
    /// [`super::types::SubAgentConfig::hop_count`]).
    pub(crate) hop_count: u32,
    /// What triggered this session, carried into its `SubagentToolDeps`.
    pub(crate) trigger: crate::bus::EventTrigger,
    /// The conversation this session replies to, for a conversation-triggered
    /// session, carried into its `SubagentToolDeps`. `None` for every other
    /// trigger.
    pub(crate) conversation_target: Option<crate::bus::ConversationTarget>,
}

/// Build isolated `SubAgentResources` for a new session run at a given tier.
///
/// Resolves the model tier to a concrete provider spec, constructs the provider,
/// and builds fresh isolated state. When `skill` is set, that skill is activated
/// on the session's own skill state so its body becomes the session's role
/// instructions. Also snapshots the global observation log and recent-context
/// narrative at fork time, per the design's "Fork contents": a session never
/// sees merges that happen after it forked.
///
/// `session.own_address` and `session.own_depth` are this new session's own
/// address and depth, threaded into its `subagent_spawn` tool so any session
/// it spawns in turn records the right spawner and depth (see "Nesting" in
/// the design). `session.own_address` and `session.category` are also
/// carried into `SubAgentBuildConfig` so the fork's `message_agent` tool can
/// name itself as the sender of any message it sends. `session.trigger` and
/// `session.conversation_target` are carried the same way into the session's
/// `SubagentToolDeps`, so a tool running in this session can tell what
/// started it and, for a conversation-triggered session, which endpoint and
/// conversation it replies to.
///
/// # Errors
/// Returns an error if provider construction fails (e.g. missing API key), the
/// identity files cannot be read, or `skill` names a skill that does not resolve.
/// Log a warning for each fallback provider dropped from a background
/// tier's chain — visible in diagnostics without stopping the spawn, since
/// the tier's primary (or another fallback) still built successfully.
fn log_dropped_fallbacks(tier: BackgroundModelTier, dropped: &[crate::inference::DroppedFallback]) {
    for fallback in dropped {
        tracing::warn!(
            tier = ?tier,
            provider = %fallback.name,
            error = %fallback.error,
            "dropped an unbuildable fallback provider for this background tier"
        );
    }
}

/// Load identity fresh per fork and snapshot memory at fork time.
///
/// Identity (SOUL.md/AGENTS.md/etc.) is loaded fresh per fork so sessions
/// see the current files; a read failure here fails the spawn — the caller
/// logs it loudly. The observation log and recent-context narrative are
/// snapshotted at fork time per the design's "Fork contents": a session
/// never sees merges that happen after it forked. A read failure snapshotting
/// either degrades gracefully (the run starts with no memory context) rather
/// than failing the spawn: an agent missing background is better than no
/// background work at all.
async fn load_fork_identity_and_memory(
    ctx: &SpawnContext,
) -> Result<(IdentityFiles, Option<String>, Option<String>), anyhow::Error> {
    let identity = IdentityFiles::load(&ctx.layout)
        .await
        .context("failed to load identity files for session fork")?;

    let observations = match load_observations(&ctx.layout.observations_json()).await {
        Ok(obs) => obs,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load observation snapshot for session fork");
            None
        }
    };
    let recent_context = match load_recent_context_narrative(&ctx.layout.recent_context_json())
        .await
    {
        Ok(narrative) => narrative,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load recent-context snapshot for session fork");
            None
        }
    };

    Ok((identity, observations, recent_context))
}

#[tracing::instrument(skip_all, fields(tier = ?tier, skill = skill.unwrap_or("none")))]
pub(crate) async fn build_spawn_resources(
    ctx: &SpawnContext,
    tier: &BackgroundModelTier,
    skill: Option<&str>,
    session: NewSessionContext,
) -> Result<SubAgentResources, anyhow::Error> {
    let NewSessionContext {
        own_address,
        own_depth,
        category,
        hop_count,
        trigger,
        conversation_target,
    } = session;
    let specs = ctx
        .background_config
        .models
        .resolve_tier(tier, &ctx.main_provider_specs);

    let (provider, dropped) = build_provider_chain_with_shared_notices(
        &specs,
        ctx.max_tokens,
        ctx.http_client.clone(),
        ctx.retry_config.clone(),
        ctx.publisher.clone(),
        format!("background sessions ({tier} tier)"),
        ctx.bg_tier_active_index.for_tier(*tier),
    )
    .with_context(|| format!("failed to build provider chain for tier {tier:?}"))?;
    log_dropped_fallbacks(*tier, &dropped);

    // Apply per-tier overrides over global options
    let tier_key = match tier {
        BackgroundModelTier::Small => "bg_small",
        BackgroundModelTier::Medium => "bg_medium",
        BackgroundModelTier::Large => "bg_large",
    };
    let ov = ctx.role_overrides.get(tier_key);
    let options = CompletionOptions {
        max_tokens: Some(ctx.max_tokens),
        temperature: ov.and_then(|o| o.temperature).or(ctx.options.temperature),
        thinking: ov
            .and_then(|o| o.thinking.clone())
            .or(ctx.options.thinking.clone()),
        ..CompletionOptions::default()
    };

    let (identity, observations, recent_context) = load_fork_identity_and_memory(ctx).await?;

    let build_config = SubAgentBuildConfig {
        workspace_layout: ctx.layout.clone(),
        config_dir: ctx.config_dir.clone(),
        identity,
        options,
        max_tool_iterations: ctx.max_tool_iterations,
        repeat_call_guard: ctx.repeat_call_guard,
        tz: ctx.tz,
        skill: skill.map(str::to_string),
        observations,
        recent_context,
        session_registry: Arc::clone(&ctx.session_registry),
        endpoint_registry: ctx.endpoint_registry.clone(),
        publisher: ctx.publisher.clone(),
        action_store: Arc::clone(&ctx.action_store),
        action_notify: Arc::clone(&ctx.action_notify),
        hybrid_searcher: Arc::clone(&ctx.hybrid_searcher),
        observer: Arc::clone(&ctx.observer),
        merge_writer: Arc::clone(&ctx.merge_writer),
        episode_skip_token_floor: ctx.background_config.episode_skip_token_floor,
        own_address,
        own_depth,
        subagent_depth_cap: ctx.background_config.subagent_depth_cap,
        session_category: category,
        trigger,
        conversation_target,
        messenger: Arc::clone(&ctx.messenger),
        hop_counter: HopCounter::new(hop_count),
        tracing_service: Arc::clone(&ctx.tracing_service),
        tracing_client_context: Arc::clone(&ctx.tracing_client_context),
        web_search_backend: ctx.web_search_backend.clone(),
        tools_path: Arc::clone(&ctx.tools_path),
        path_policy: Arc::clone(&ctx.path_policy),
        agent_keys: Arc::clone(&ctx.agent_keys),
        a2a_hub: Arc::clone(&ctx.a2a_hub),
        a2a_tracker: Arc::clone(&ctx.a2a_tracker),
        checkpoints: Arc::clone(&ctx.checkpoints),
    };

    build_subagent_resources(
        provider,
        &ctx.skill_state,
        Arc::clone(&ctx.mcp_registry),
        build_config,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_tier_gets_its_own_stable_counter() {
        let counters = BackgroundTierActiveIndex::default();

        let small_a = counters.for_tier(BackgroundModelTier::Small);
        let small_b = counters.for_tier(BackgroundModelTier::Small);
        let medium = counters.for_tier(BackgroundModelTier::Medium);
        let large = counters.for_tier(BackgroundModelTier::Large);

        // Same tier, two lookups: the same underlying counter (an `Arc`
        // clone), so a store through one is visible through the other —
        // this is what lets many short-lived spawns for one tier share
        // transition state.
        small_a.store(1, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(small_b.load(std::sync::atomic::Ordering::SeqCst), 1);

        // Different tiers never share a counter.
        assert_eq!(medium.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(large.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
}
