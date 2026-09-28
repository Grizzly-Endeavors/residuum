//! Session limits and per-tier model assignments.

use std::collections::HashMap;

use crate::util::FatalError;

use super::super::deserialize::{BackgroundConfigFile, BackgroundModelsFile, ProviderEntryFile};
use super::super::secrets::SecretStore;
use super::super::types::BackgroundConfig;

/// Resolve background task configuration.
///
/// Reads `max_concurrent` from `config.toml`'s `[background]` section, and
/// model tiers from `providers.toml`'s `[background.models]` section.
///
/// # Errors
/// Returns `FatalError::Config` if a model tier string cannot be resolved.
pub(super) fn resolve_background_config(
    section: Option<&BackgroundConfigFile>,
    models_section: Option<&BackgroundModelsFile>,
    providers_map: Option<&HashMap<String, ProviderEntryFile>>,
    secrets: &SecretStore,
    role_overrides: &mut HashMap<String, super::super::types::RoleOverrides>,
) -> Result<BackgroundConfig, FatalError> {
    let mut cfg = BackgroundConfig::default();

    if let Some(section) = section {
        if let Some(v) = section.max_concurrent {
            cfg.max_concurrent = v;
        }
        if let Some(v) = section.idle_timeout_scheduled_minutes {
            cfg.idle_timeout_scheduled = std::time::Duration::from_secs(v.saturating_mul(60));
        }
        if let Some(v) = section.idle_timeout_spawned_minutes {
            cfg.idle_timeout_spawned = std::time::Duration::from_secs(v.saturating_mul(60));
        }
        if let Some(v) = section.idle_timeout_external_minutes {
            cfg.idle_timeout_external = std::time::Duration::from_secs(v.saturating_mul(60));
        }
        if let Some(v) = section.idle_timeout_artifact_minutes {
            cfg.idle_timeout_artifact = std::time::Duration::from_secs(v.saturating_mul(60));
        }
        if let Some(v) = section.episode_skip_token_floor {
            cfg.episode_skip_token_floor = v;
        }
        if let Some(v) = section.subagent_depth_cap {
            cfg.subagent_depth_cap = v;
        }
        if let Some(v) = section.hop_soft_limit {
            cfg.hop_soft_limit = v;
        }
        if let Some(v) = section.hop_hard_limit {
            cfg.hop_hard_limit = v;
        }
    }

    if let Some(models_section) = models_section {
        cfg.models.small = resolve_bg_tier(
            models_section.small.clone(),
            "bg_small",
            providers_map,
            secrets,
            role_overrides,
        )?;
        cfg.models.medium = resolve_bg_tier(
            models_section.medium.clone(),
            "bg_medium",
            providers_map,
            secrets,
            role_overrides,
        )?;
        cfg.models.large = resolve_bg_tier(
            models_section.large.clone(),
            "bg_large",
            providers_map,
            secrets,
            role_overrides,
        )?;
    }

    Ok(cfg)
}

/// Resolve a single background tier assignment, extracting overrides.
fn resolve_bg_tier(
    assignment: Option<super::super::deserialize::ModelAssignment>,
    role_key: &str,
    providers_map: Option<&HashMap<String, ProviderEntryFile>>,
    secrets: &SecretStore,
    role_overrides: &mut HashMap<String, super::super::types::RoleOverrides>,
) -> Result<Option<Vec<super::super::provider::ProviderSpec>>, FatalError> {
    let Some(spec) = assignment else {
        return Ok(None);
    };
    super::models::extract_role_overrides(role_key, &spec, role_overrides)?;
    Ok(Some(super::models::resolve_assignment_chain(
        spec,
        providers_map,
        secrets,
    )?))
}
