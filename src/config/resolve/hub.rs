//! Hub-level config resolution: timezone, gateway, cloud, the A2A listener,
//! tracing, and the shared background limits — everything that lives in
//! `hub/config.toml` rather than an agent's own config.

use std::path::Path;

use crate::util::FatalError;

use super::super::deserialize::{HubBackgroundConfigFile, HubConfigFile};
use super::super::hub_types::{HubA2aConfig, HubBackgroundConfig, HubConfig};
use super::gateway;
use super::load_secrets_degraded;
use super::tracing_config;

/// Build a `HubConfig` from an optional hub config file and environment
/// variables.
///
/// # Errors
/// Returns `FatalError::Config` if the timezone is missing/invalid or the
/// tracing log level string is invalid.
pub(crate) fn from_file_and_env(
    file: Option<&HubConfigFile>,
    hub_dir: &Path,
) -> Result<HubConfig, FatalError> {
    let mut notices: Vec<String> = Vec::new();
    let secrets = load_secrets_degraded(hub_dir, &mut notices);

    let timezone = gateway::resolve_timezone(file)?;
    let gateway = gateway::resolve_gateway_config(file.and_then(|f| f.gateway.as_ref()));
    let cloud =
        gateway::resolve_cloud_config(file.and_then(|f| f.cloud.as_ref()), &secrets, &gateway);
    let tracing = tracing_config::resolve_tracing_config(file.and_then(|f| f.tracing.as_ref()))?;
    let (a2a_enabled, a2a_port, a2a_public_url) =
        super::a2a::resolve_hub_a2a_config(file.and_then(|f| f.a2a.as_ref()));
    let a2a = HubA2aConfig {
        enabled: a2a_enabled,
        port: a2a_port,
        public_url: a2a_public_url,
    };
    let background = resolve_hub_background_config(file.and_then(|f| f.background.as_ref()));

    Ok(HubConfig {
        timezone,
        gateway,
        cloud,
        a2a,
        tracing,
        background,
        config_dir: hub_dir.to_path_buf(),
        load_notices: notices,
    })
}

/// Resolve the hub's shared background limits (session budget, hop limits).
fn resolve_hub_background_config(section: Option<&HubBackgroundConfigFile>) -> HubBackgroundConfig {
    let mut cfg = HubBackgroundConfig::default();
    let Some(section) = section else {
        return cfg;
    };
    if let Some(v) = section.max_concurrent {
        cfg.max_concurrent = v;
    }
    if let Some(v) = section.hop_soft_limit {
        cfg.hop_soft_limit = v;
    }
    if let Some(v) = section.hop_hard_limit {
        cfg.hop_hard_limit = v;
    }
    cfg
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(toml: &str) -> HubConfigFile {
        toml::from_str(toml).unwrap()
    }

    fn test_dir() -> std::path::PathBuf {
        std::env::temp_dir().join("residuum-test-hub-config")
    }

    #[test]
    fn timezone_required() {
        let result = from_file_and_env(None, &test_dir());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("timezone"));
    }

    #[test]
    fn defaults_resolve() {
        let file = parse("timezone = \"UTC\"\n");
        let cfg = from_file_and_env(Some(&file), &test_dir()).unwrap();
        assert_eq!(cfg.timezone, chrono_tz::UTC);
        assert!(cfg.a2a.enabled);
        assert_eq!(cfg.a2a.port, crate::config::DEFAULT_A2A_PORT);
        assert_eq!(cfg.background.max_concurrent, 3);
        assert_eq!(cfg.background.hop_soft_limit, 8);
        assert_eq!(cfg.background.hop_hard_limit, 32);
        assert!(cfg.cloud.is_none());
    }

    #[test]
    fn gateway_and_a2a_and_background_parse() {
        let file = parse(
            r#"
timezone = "UTC"

[gateway]
bind = "0.0.0.0"
port = 8080

[a2a]
enabled = false
port = 9999
public_url = "https://example.com/a2a/laptop"

[background]
max_concurrent = 10
hop_soft_limit = 4
hop_hard_limit = 16
"#,
        );
        let cfg = from_file_and_env(Some(&file), &test_dir()).unwrap();
        assert_eq!(cfg.gateway.bind, "0.0.0.0");
        assert_eq!(cfg.gateway.port, 8080);
        assert!(!cfg.a2a.enabled);
        assert_eq!(cfg.a2a.port, 9999);
        assert_eq!(
            cfg.a2a.public_url.as_deref(),
            Some("https://example.com/a2a/laptop")
        );
        assert_eq!(cfg.background.max_concurrent, 10);
        assert_eq!(cfg.background.hop_soft_limit, 4);
        assert_eq!(cfg.background.hop_hard_limit, 16);
    }
}
