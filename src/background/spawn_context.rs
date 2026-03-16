//! Spawn context: parameters needed to construct providers and `SubAgentResources`
//! for background tasks (pulse, actions, and on-demand sub-agents).

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

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
use crate::projects::activation::SharedProjectState;
use crate::skills::SharedSkillState;
use crate::workspace::identity::IdentityFiles;
use crate::workspace::layout::WorkspaceLayout;

use crate::subagents::types::SubagentPresetFrontmatter;

use super::subagent::{
    PresetToolRestriction, SubAgentBuildConfig, SubAgentResources, build_resources,
};

/// Everything needed to spawn background tasks from the gateway event loop.
pub(crate) struct SpawnContext {
    pub(crate) background_config: BackgroundConfig,
    pub(crate) main_provider_specs: Vec<ProviderSpec>,
    pub(crate) http_client: SharedHttpClient,
    pub(crate) max_tokens: u32,
    pub(crate) retry_config: RetryConfig,
    pub(crate) identity: IdentityFiles,
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
}

/// Build isolated `SubAgentResources` for a background task at a given tier.
///
/// Resolves the model tier to a concrete provider spec, constructs the provider,
/// and calls `background::build_resources()` with fresh isolated state.
///
/// If `preset` is provided, its tool restrictions and instructions are applied.
///
/// # Errors
/// Returns an error if provider construction fails (e.g. missing API key).
pub(crate) async fn build_spawn_resources(
    ctx: &SpawnContext,
    tier: &BackgroundModelTier,
    project_state: &SharedProjectState,
    skill_state: &SharedSkillState,
    mcp_registry: SharedMcpRegistry,
    preset: Option<(&SubagentPresetFrontmatter, String)>,
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
    )?;

    let (preset_tool_restriction, preset_instructions) = match preset {
        Some((fm, body)) => {
            let restriction = match (&fm.allowed_tools, &fm.denied_tools) {
                (Some(allowed), _) => Some(PresetToolRestriction::AllowedOnly(
                    allowed.iter().cloned().collect(),
                )),
                (None, Some(denied)) => Some(PresetToolRestriction::Denied(
                    denied.iter().cloned().collect(),
                )),
                (None, None) => None,
            };
            let instructions = if body.is_empty() { None } else { Some(body) };
            (restriction, instructions)
        }
        None => (None, None),
    };

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

    let build_config = SubAgentBuildConfig {
        gated_tools: HashSet::new(),
        preset_tool_restriction,
        workspace_layout: ctx.layout.clone(),
        identity: ctx.identity.clone(),
        options,
        tz: ctx.tz,
        preset_instructions,
        background_spawner: Arc::clone(&ctx.background_spawner),
        endpoint_registry: ctx.endpoint_registry.clone(),
        publisher: ctx.publisher.clone(),
        action_store: Arc::clone(&ctx.action_store),
        action_notify: Arc::clone(&ctx.action_notify),
        hybrid_searcher: Arc::clone(&ctx.hybrid_searcher),
    };

    Ok(build_resources(
        provider,
        project_state,
        skill_state,
        mcp_registry,
        build_config,
    )
    .await)
}

/// Load a sub-agent preset and resolve its model tier.
///
/// Returns the resolved tier and the preset frontmatter+body (if loaded successfully).
/// Used by pulse, scheduled actions, and on-demand spawning.
///
/// # Errors
/// Returns an error if the preset index cannot be scanned or the preset cannot be loaded.
pub(crate) async fn load_preset_for_spawn(
    subagents_dir: &Path,
    preset_name: &str,
    fallback_tier: BackgroundModelTier,
) -> Result<
    (
        BackgroundModelTier,
        Option<(SubagentPresetFrontmatter, String)>,
    ),
    anyhow::Error,
> {
    let index = crate::subagents::SubagentPresetIndex::scan(subagents_dir).await?;
    let (fm, body) = index.load_preset(preset_name).await?;

    let tier: BackgroundModelTier = match fm.model_tier.as_deref() {
        Some(s) => s.parse().unwrap_or_else(|_| {
            tracing::warn!(preset = %preset_name, model_tier = %s, "unknown model_tier, using fallback");
            fallback_tier
        }),
        None => fallback_tier,
    };

    Ok((tier, Some((fm, body))))
}
