//! Spawn context: parameters needed to construct providers and `SubAgentResources`
//! for background tasks (pulse, cron, and on-demand sub-agents).

use std::collections::HashSet;

use crate::config::ProviderSpec;
use crate::config::{BackgroundConfig, BackgroundModelTier};
use crate::mcp::SharedMcpRegistry;
use crate::models::retry::RetryConfig;
use crate::models::{CompletionOptions, SharedHttpClient, build_provider_from_provider_spec};
use crate::projects::activation::SharedProjectState;
use crate::skills::SharedSkillState;
use crate::workspace::identity::IdentityFiles;
use crate::workspace::layout::WorkspaceLayout;

use super::subagent::{SubAgentBuildConfig, SubAgentResources, build_resources};

/// Everything needed to spawn background tasks from the gateway event loop.
pub(crate) struct SpawnContext {
    pub(crate) background_config: BackgroundConfig,
    pub(crate) main_provider_spec: ProviderSpec,
    pub(crate) http_client: SharedHttpClient,
    pub(crate) max_tokens: u32,
    pub(crate) retry_config: RetryConfig,
    pub(crate) identity: IdentityFiles,
    pub(crate) options: CompletionOptions,
    pub(crate) layout: WorkspaceLayout,
    pub(crate) tz: chrono_tz::Tz,
}

/// Build isolated `SubAgentResources` for a background task at a given tier.
///
/// Resolves the model tier to a concrete provider spec, constructs the provider,
/// and calls `background::build_resources()` with fresh isolated state.
///
/// # Errors
/// Returns an error if provider construction fails (e.g. missing API key).
pub(crate) async fn build_spawn_resources(
    ctx: &SpawnContext,
    tier: &BackgroundModelTier,
    project_state: &SharedProjectState,
    skill_state: &SharedSkillState,
    mcp_registry: SharedMcpRegistry,
) -> Result<SubAgentResources, anyhow::Error> {
    let spec = ctx
        .background_config
        .models
        .resolve_tier(tier, &ctx.main_provider_spec);

    let provider = build_provider_from_provider_spec(
        &spec,
        ctx.max_tokens,
        ctx.http_client.clone(),
        ctx.retry_config.clone(),
    )?;

    let build_config = SubAgentBuildConfig {
        gated_tools: HashSet::from(["exec"]),
        workspace_layout: ctx.layout.clone(),
        identity: ctx.identity.clone(),
        options: ctx.options.clone(),
        tz: ctx.tz,
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
