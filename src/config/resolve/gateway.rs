//! Gateway bind, timezone, and cloud tunnel settings (hub-level).

use crate::util::FatalError;

use super::super::constants::DEFAULT_CLOUD_RELAY_URL;
use super::super::deserialize::{CloudConfigFile, GatewayConfigFile, HubConfigFile};
use super::super::secrets::SecretStore;
use super::super::types::{CloudConfig, GatewayConfig};

/// Resolve the timezone from env var or hub config file.
///
/// # Errors
/// Returns `FatalError::Config` if no timezone is set or the value is not a
/// valid IANA timezone name. A missing timezone is [`is_missing_timezone`];
/// onboarding uses that to start without one (see
/// [`super::hub::from_file_and_env_for_onboarding`]).
pub(super) fn resolve_timezone(file: Option<&HubConfigFile>) -> Result<chrono_tz::Tz, FatalError> {
    let Some(tz_name) = std::env::var("RESIDUUM_TIMEZONE")
        .ok()
        .filter(|name| !name.is_empty())
        .or_else(|| {
            file.and_then(|f| f.timezone.clone())
                .filter(|name| !name.is_empty())
        })
    else {
        return Err(FatalError::Config(
            "timezone is required: set RESIDUUM_TIMEZONE env var or 'timezone' in hub/config.toml \
             (IANA name, e.g. \"America/New_York\")"
                .to_string(),
        ));
    };
    tz_name.parse().map_err(|_err| {
        FatalError::Config(format!(
            "invalid timezone '{tz_name}': expected IANA name like 'America/New_York' or 'UTC'"
        ))
    })
}

/// Whether `err` is a missing timezone rather than an invalid name or any
/// other config problem.
pub(super) fn is_missing_timezone(err: &FatalError) -> bool {
    matches!(err, FatalError::Config(msg) if msg.starts_with("timezone is required:"))
}

/// The machine's IANA timezone, or UTC when it can't be read or parsed.
///
/// Onboarding uses this only as a stand-in so the hub can run before the
/// setup wizard has written a timezone. It is not saved to `config.toml`.
pub(super) fn machine_timezone() -> chrono_tz::Tz {
    iana_time_zone::get_timezone()
        .ok()
        .and_then(|name| name.parse().ok())
        .unwrap_or(chrono_tz::UTC)
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

    let token = std::env::var("RESIDUUM_CLOUD_TOKEN")
        .ok()
        .or_else(|| super::channels::resolve_bot_token(section.token.as_deref(), secrets));

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
