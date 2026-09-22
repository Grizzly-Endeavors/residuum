//! Spawn context: parameters needed to construct providers and `SubAgentResources`
//! for a new session run (pulse, action, webhook, or on-demand spawn).

use std::sync::Arc;

use anyhow::Context as _;
use tokio::sync::{Mutex, Notify};

use crate::actions::store::ActionStore;
use crate::agent::context::loading::{load_observations, load_recent_context_narrative};
use crate::background::registry::{SessionCategory, SessionRegistry};
use crate::background::runtime::SessionRuntime;
use crate::bus::{EndpointRegistry, Publisher, SessionAddress};
use crate::config::ProviderSpec;
use crate::config::{BackgroundConfig, BackgroundModelTier};
use crate::inference::retry::RetryConfig;
use crate::inference::{CompletionOptions, SharedHttpClient, build_provider_chain};
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

/// Everything needed to fork a new session run from the gateway event loop.
pub(crate) struct SpawnContext {
    pub(crate) background_config: BackgroundConfig,
    pub(crate) main_provider_specs: Vec<ProviderSpec>,
    pub(crate) http_client: SharedHttpClient,
    pub(crate) max_tokens: u32,
    pub(crate) retry_config: RetryConfig,
    pub(crate) options: CompletionOptions,
    pub(crate) layout: WorkspaceLayout,
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
/// `address` and `category` identify the session itself (not its
/// spawner) — carried into `SubAgentBuildConfig` so the fork's
/// `message_agent` tool can name itself as the sender of any message it
/// sends.
///
/// # Errors
/// Returns an error if provider construction fails (e.g. missing API key), the
/// identity files cannot be read, or `skill` names a skill that does not resolve.
#[tracing::instrument(skip_all, fields(tier = ?tier, skill = skill.unwrap_or("none")))]
pub(crate) async fn build_spawn_resources(
    ctx: &SpawnContext,
    tier: &BackgroundModelTier,
    skill: Option<&str>,
    address: &SessionAddress,
    category: SessionCategory,
) -> Result<SubAgentResources, anyhow::Error> {
    let specs = ctx
        .background_config
        .models
        .resolve_tier(tier, &ctx.main_provider_specs);

    let provider = build_provider_chain(
        &specs,
        ctx.max_tokens,
        ctx.http_client.clone(),
        ctx.retry_config.clone(),
    )
    .with_context(|| format!("failed to build provider chain for tier {tier:?}"))?;

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

    // Load identity fresh per fork so sessions see current SOUL.md/AGENTS.md/
    // etc. A read failure fails the spawn — the caller logs it loudly.
    let identity = IdentityFiles::load(&ctx.layout)
        .await
        .context("failed to load identity files for session fork")?;

    // Snapshot memory at fork time. A read failure here degrades gracefully
    // (the run starts with no memory context) rather than failing the spawn:
    // an agent missing background is better than no background work at all.
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
        Ok(ctx) => ctx,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load recent-context snapshot for session fork");
            None
        }
    };

    let build_config = SubAgentBuildConfig {
        workspace_layout: ctx.layout.clone(),
        identity,
        options,
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
        session_address: address.clone(),
        session_category: category,
        messenger: Arc::clone(&ctx.messenger),
    };

    build_subagent_resources(
        provider,
        &ctx.skill_state,
        Arc::clone(&ctx.mcp_registry),
        build_config,
    )
    .await
}
