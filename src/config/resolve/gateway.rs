//! Gateway bind, workspace directory, timezone, and cloud tunnel settings.

use std::path::PathBuf;

use crate::util::FatalError;

use super::super::bootstrap::default_workspace_dir;
use super::super::constants::DEFAULT_CLOUD_RELAY_URL;
use super::super::deserialize::{CloudConfigFile, ConfigFile, GatewayConfigFile};
use super::super::secrets::SecretStore;
use super::super::types::{CloudConfig, GatewayConfig};

/// Resolve the workspace root directory: env var, then config file, then the
/// platform default. `~` in an explicit value is expanded.
///
/// # Errors
/// Returns `FatalError::Config` if the default workspace directory can't be
/// determined (no config value was given to fall back from).
pub(super) fn resolve_workspace_dir_setting(
    file: Option<&ConfigFile>,
) -> Result<PathBuf, FatalError> {
    std::env::var("RESIDUUM_WORKSPACE")
        .ok()
        .or_else(|| file.and_then(|f| f.workspace_dir.clone()))
        .map(|s| {
            let expanded = shellexpand::tilde(&s);
            PathBuf::from(expanded.as_ref())
        })
        .map_or_else(default_workspace_dir, Ok)
}

/// Resolve the timezone from env var or config file.
///
/// # Errors
/// Returns `FatalError::Config` if no timezone is set or the value is not a
/// valid IANA timezone name.
pub(super) fn resolve_timezone(file: Option<&ConfigFile>) -> Result<chrono_tz::Tz, FatalError> {
    let tz_name = std::env::var("RESIDUUM_TIMEZONE")
        .ok()
        .or_else(|| file.and_then(|f| f.timezone.clone()))
        .ok_or_else(|| {
            FatalError::Config(
                "timezone is required: set RESIDUUM_TIMEZONE env var or 'timezone' in config.toml \
                 (IANA name, e.g. \"America/New_York\")"
                    .to_string(),
            )
        })?;
    tz_name.parse().map_err(|_err| {
        FatalError::Config(format!(
            "invalid timezone '{tz_name}': expected IANA name like 'America/New_York' or 'UTC'"
        ))
    })
}

/// Resolve gateway configuration from environment variables and defaults only.
///
/// Used by the setup server which runs before any config file exists.
#[must_use]
pub(crate) fn resolve_default_gateway_config() -> GatewayConfig {
    resolve_gateway_config(None)
}

/// Resolve gateway configuration from TOML section and environment variables.
pub(super) fn resolve_gateway_config(section: Option<&GatewayConfigFile>) -> GatewayConfig {
    let mut cfg = GatewayConfig::default();

    // Env > file > default for bind
    if let Ok(val) = std::env::var("RESIDUUM_GATEWAY_BIND") {
        tracing::debug!(env = "RESIDUUM_GATEWAY_BIND", value = %val, "gateway bind overridden by env var");
        cfg.bind = val;
    } else if let Some(val) = section.and_then(|s| s.bind.clone()) {
        cfg.bind = val;
    }

    // Env > file > default for port
    match std::env::var("RESIDUUM_GATEWAY_PORT") {
        Ok(val) => match val.parse::<u16>() {
            Ok(p) => {
                tracing::debug!(
                    env = "RESIDUUM_GATEWAY_PORT",
                    value = p,
                    "gateway port overridden by env var"
                );
                cfg.port = p;
            }
            Err(e) => {
                tracing::warn!(%val, error = %e, "RESIDUUM_GATEWAY_PORT is not a valid port");
            }
        },
        Err(_) => {
            if let Some(p) = section.and_then(|s| s.port) {
                cfg.port = p;
            }
        }
    }

    cfg
}

/// Resolve cloud tunnel configuration from TOML section and environment.
///
/// Token resolution: `RESIDUUM_CLOUD_TOKEN` env > `token` field in TOML (with
/// `${ENV_VAR}` / `secret:name` expansion) > `None` if section is absent, disabled,
/// or no token found.
pub(super) fn resolve_cloud_config(
    section: Option<&CloudConfigFile>,
    secrets: &SecretStore,
    gateway: &GatewayConfig,
) -> Option<CloudConfig> {
    let section = section?;

    // If explicitly disabled, return None.
    if section.enabled == Some(false) {
        return None;
    }

    let token = super::channels::resolve_bot_token(
        "RESIDUUM_CLOUD_TOKEN",
        section.token.as_deref(),
        secrets,
    );

    if let Some(tok) = token {
        let relay_url = section
            .relay_url
            .clone()
            .unwrap_or_else(|| DEFAULT_CLOUD_RELAY_URL.to_string());
        let local_port = section.local_port.unwrap_or(gateway.port);
        Some(CloudConfig {
            relay_url,
            token: tok,
            local_port,
        })
    } else {
        tracing::warn!(
            section = "cloud",
            "section present but no token found; set RESIDUUM_CLOUD_TOKEN or token in config"
        );
        None
    }
}
