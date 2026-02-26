//! Background task spawn helpers for the gateway event loop.
//!
//! Groups the parameters needed to construct providers and `SubAgentResources`
//! for pulse and cron background tasks.

use std::collections::HashSet;

use crate::background::subagent::{SubAgentBuildConfig, SubAgentResources, build_resources};
use crate::config::ProviderSpec;
use crate::config::{BackgroundConfig, BackgroundModelTier};
use crate::mcp::SharedMcpRegistry;
use crate::models::retry::RetryConfig;
use crate::models::{CompletionOptions, SharedHttpClient, build_provider_from_provider_spec};
use crate::projects::activation::SharedProjectState;
use crate::skills::SharedSkillState;
use crate::workspace::identity::IdentityFiles;
use crate::workspace::layout::WorkspaceLayout;

/// Everything needed to spawn background tasks from the gateway event loop.
pub(super) struct SpawnContext {
    pub(super) background_config: BackgroundConfig,
    pub(super) main_provider_spec: ProviderSpec,
    pub(super) http_client: SharedHttpClient,
    pub(super) max_tokens: u32,
    pub(super) retry_config: RetryConfig,
    pub(super) identity: IdentityFiles,
    pub(super) options: CompletionOptions,
    pub(super) layout: WorkspaceLayout,
    pub(super) tz: chrono_tz::Tz,
}

/// Build isolated `SubAgentResources` for a background task at a given tier.
///
/// Resolves the model tier to a concrete provider spec, constructs the provider,
/// and calls `background::build_resources()` with fresh isolated state.
///
/// # Errors
/// Returns an error if provider construction fails (e.g. missing API key).
pub(super) async fn build_spawn_resources(
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
