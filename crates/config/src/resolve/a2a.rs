//! Agent-to-agent listener settings.
//!
//! The listener itself (`enabled`/`port`/`public_url`) is hub-level: one
//! process, one A2A port. Visibility is per agent.

use super::super::constants::DEFAULT_A2A_PORT;
use super::super::deserialize::{AgentA2aConfigFile, HubA2aConfigFile};
use super::super::hub_types::HubConfig;
use super::super::types::{A2aConfig, A2aVisibility};

/// Resolve the hub-level A2A listener settings from `hub/config.toml`'s
/// `[a2a]` section.
///
/// Enabled by default: every request the listener answers still goes
/// through the auth layer, so turning it on by default costs nothing until
/// a caller key or sibling instance actually exists to use it.
pub(super) fn resolve_hub_a2a_config(
    section: Option<&HubA2aConfigFile>,
) -> (bool, u16, Option<String>) {
    let default = A2aConfig::default();
    let Some(section) = section else {
        return (default.enabled, default.port, default.public_url);
    };

    let public_url = section
        .public_url
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string);

    (
        section.enabled.unwrap_or(default.enabled),
        section.port.unwrap_or(DEFAULT_A2A_PORT),
        public_url,
    )
}

/// An agent's full A2A config: the hub's listener settings plus this
/// agent's own visibility.
pub(super) fn resolve_agent_a2a_config(
    hub: &HubConfig,
    section: Option<&AgentA2aConfigFile>,
    notices: &mut Vec<String>,
) -> A2aConfig {
    A2aConfig {
        enabled: hub.a2a.enabled,
        port: hub.a2a.port,
        public_url: hub.a2a.public_url.clone(),
        visibility: resolve_agent_a2a_visibility(section, notices),
    }
}

/// Resolve this agent's A2A visibility from its own config's `[a2a]` section.
///
/// An invalid `visibility` value falls back to the default visibility with
/// a notice rather than failing the whole config — a2a itself still comes
/// up, just not with the visibility the user intended until they fix it.
pub(super) fn resolve_agent_a2a_visibility(
    section: Option<&AgentA2aConfigFile>,
    notices: &mut Vec<String>,
) -> A2aVisibility {
    match section.and_then(|s| s.visibility.as_deref()).map(str::trim) {
        None | Some("") => A2aVisibility::default(),
        Some("public") => A2aVisibility::Public,
        Some("private") => A2aVisibility::Private,
        Some(other) => {
            tracing::warn!(
                value = other,
                "[a2a] visibility is invalid, falling back to the default"
            );
            notices.push(format!(
                "[a2a] visibility must be \"public\" or \"private\", got \"{other}\" — using the default visibility until you fix it."
            ));
            A2aVisibility::default()
        }
    }
}
