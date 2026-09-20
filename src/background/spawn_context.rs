//! Spawn context: parameters needed to construct providers and `SubAgentResources`
//! for background tasks (pulse, actions, and on-demand sub-agents).

use std::sync::Arc;

use anyhow::Context as _;
use tokio::sync::{Mutex, Notify};

use crate::actions::store::ActionStore;
use crate::background::BackgroundTaskSpawner;
use crate::bus::{EndpointRegistry, Publisher};
use crate::config::ProviderSpec;
use crate::config::{BackgroundConfig, BackgroundModelTier};
use crate::mcp::SharedMcpRegistry;
use crate::memory::search::HybridSearcher;
use crate::models::retry::RetryConfig;
use crate::models::{CompletionOptions, SharedHttpClient, build_provider_chain};
use crate::skills::SharedSkillState;
use crate::workspace::identity::IdentityFiles;
use crate::workspace::layout::WorkspaceLayout;

use super::subagent::{SubAgentResources, build_subagent_resources};
use super::types::SubAgentBuildConfig;

/// Everything needed to spawn background tasks from the gateway event loop.
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
    // ── Sub-agent tool dependencies ────────────────────────────────────
    pub(crate) background_spawner: Arc<BackgroundTaskSpawner>,
    pub(crate) endpoint_registry: EndpointRegistry,
    pub(crate) publisher: Publisher,
    pub(crate) action_store: Arc<Mutex<ActionStore>>,
    pub(crate) action_notify: Arc<Notify>,
    pub(crate) hybrid_searcher: Arc<HybridSearcher>,
    /// Main agent skill state — cloned per spawn into isolated sub-agent state.
    pub(crate) skill_state: SharedSkillState,
    /// Shared MCP registry (ref-counted across sub-agents).
    pub(crate) mcp_registry: SharedMcpRegistry,
}

/// Build isolated `SubAgentResources` for a background task at a given tier.
///
/// Resolves the model tier to a concrete provider spec, constructs the provider,
/// and builds fresh isolated state. When `skill` is set, that skill is activated
/// on the sub-agent's own skill state so its body becomes the sub-agent's role
/// instructions.
///
/// # Errors
/// Returns an error if provider construction fails (e.g. missing API key), the
/// identity files cannot be read, or `skill` names a skill that does not resolve.
#[tracing::instrument(skip_all, fields(tier = ?tier, skill = skill.unwrap_or("none")))]
pub(crate) async fn build_spawn_resources(
    ctx: &SpawnContext,
    tier: &BackgroundModelTier,
    skill: Option<&str>,
    include_identity: bool,
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

    // Load identity fresh per spawn so sub-agents see current SOUL.md/AGENTS.md/
    // etc. A read failure fails the spawn — the caller logs it loudly.
    let identity = IdentityFiles::load(&ctx.layout)
        .await
        .context("failed to load identity files for sub-agent spawn")?;

    let build_config = SubAgentBuildConfig {
        workspace_layout: ctx.layout.clone(),
        identity,
        options,
        tz: ctx.tz,
        skill: skill.map(str::to_string),
        include_identity,
        background_spawner: Arc::clone(&ctx.background_spawner),
        endpoint_registry: ctx.endpoint_registry.clone(),
        publisher: ctx.publisher.clone(),
        action_store: Arc::clone(&ctx.action_store),
        action_notify: Arc::clone(&ctx.action_notify),
        hybrid_searcher: Arc::clone(&ctx.hybrid_searcher),
    };

    build_subagent_resources(
        provider,
        &ctx.skill_state,
        Arc::clone(&ctx.mcp_registry),
        build_config,
    )
    .await
}
