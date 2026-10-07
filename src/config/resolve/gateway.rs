//! Gateway bind, timezone, and cloud tunnel settings (hub-level).

use crate::util::FatalError;

use super::super::constants::DEFAULT_CLOUD_RELAY_URL;
use super::super::deserialize::{CloudConfigFile, GatewayConfigFile, HubConfigFile};
use super::super::secrets::SecretStore;
use super::super::types::{
    ACME_PRODUCTION_DIRECTORY, ACME_STAGING_DIRECTORY, CloudConfig, GatewayConfig,
    RemoteAccessSettings,
};

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
            remote: resolve_remote_settings(section),
        })
    } else {
        tracing::warn!(
            section = "cloud",
            "section present but no token found; set RESIDUUM_CLOUD_TOKEN or token in config"
        );
        None
    }
}

/// Resolve the remote-access settings of the `[cloud]` section. A value that
/// can't be used is logged and replaced by its default, so a typo never takes
/// the tunnel down.
fn resolve_remote_settings(section: &CloudConfigFile) -> RemoteAccessSettings {
    let mut settings = RemoteAccessSettings::default();
    if let Some(enabled) = section.remote_access {
        settings.enabled = enabled;
    }
    if let Some(base) = section.base_domain.as_deref().map(str::trim) {
        if is_valid_base_domain(base) {
            settings.base_domain = base.to_ascii_lowercase();
        } else {
            tracing::warn!(base_domain = %base, "[cloud] base_domain isn't a valid domain name; using the default");
        }
    }
    if let Some(directory) = section.acme_directory.as_deref().map(str::trim) {
        match directory {
            "production" => settings.acme_directory = ACME_PRODUCTION_DIRECTORY.to_string(),
            "staging" => settings.acme_directory = ACME_STAGING_DIRECTORY.to_string(),
            url if url.starts_with("https://") || url.starts_with("http://") => {
                settings.acme_directory = url.to_string();
            }
            other => {
                tracing::warn!(acme_directory = %other, "[cloud] acme_directory must be \"production\", \"staging\" or a URL; using production");
            }
        }
    }
    settings.acme_root_ca = section
        .acme_root_ca
        .as_deref()
        .map(|path| std::path::PathBuf::from(shellexpand::tilde(path).into_owned()));
    if let Some(url) = section.pin_service_url.as_deref().map(str::trim) {
        if url.starts_with("https://") || url.starts_with("http://") {
            settings.pin_service_url = url.trim_end_matches('/').to_string();
        } else {
            tracing::warn!(pin_service_url = %url, "[cloud] pin_service_url must be a URL; using the default");
        }
    }
    if let Some(resolver) = section.caa_resolver.as_deref().map(str::trim) {
        match resolver.parse() {
            Ok(addr) => settings.caa_resolver = addr,
            Err(e) => {
                tracing::warn!(caa_resolver = %resolver, error = %e, "[cloud] caa_resolver must be ip:port; using the default");
            }
        }
    }
    settings
}

/// Whether `domain` is a plain DNS name of at least two labels.
fn is_valid_base_domain(domain: &str) -> bool {
    domain.split('.').count() >= 2
        && domain.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

#[cfg(test)]
mod remote_settings_tests {
    use super::*;

    fn section(toml_text: &str) -> CloudConfigFile {
        toml::from_str(toml_text).unwrap()
    }

    #[test]
    fn an_empty_section_gets_the_production_defaults() {
        let settings = resolve_remote_settings(&section(""));
        assert_eq!(settings, RemoteAccessSettings::default());
        assert!(settings.enabled);
        assert_eq!(settings.acme_directory, ACME_PRODUCTION_DIRECTORY);
    }

    #[test]
    fn staging_and_custom_directories_are_accepted() {
        let staging = resolve_remote_settings(&section("acme_directory = \"staging\""));
        assert_eq!(staging.acme_directory, ACME_STAGING_DIRECTORY);
        let custom = resolve_remote_settings(&section(
            "acme_directory = \"https://localhost:14000/dir\"\nacme_root_ca = \"/tmp/pebble.pem\"\nbase_domain = \"Relay.Test\"\npin_service_url = \"http://127.0.0.1:9/\"\ncaa_resolver = \"127.0.0.1:8053\"\nremote_access = false",
        ));
        assert_eq!(custom.acme_directory, "https://localhost:14000/dir");
        assert_eq!(custom.base_domain, "relay.test");
        assert_eq!(custom.pin_service_url, "http://127.0.0.1:9");
        assert_eq!(custom.caa_resolver.port(), 8053);
        assert!(!custom.enabled);
    }

    #[test]
    fn unusable_values_fall_back_to_defaults() {
        let settings = resolve_remote_settings(&section(
            "acme_directory = \"letsencrypt\"\nbase_domain = \"nodots\"\ncaa_resolver = \"cloudflare\"\npin_service_url = \"ftp://x\"",
        ));
        assert_eq!(settings, RemoteAccessSettings::default());
    }
}
