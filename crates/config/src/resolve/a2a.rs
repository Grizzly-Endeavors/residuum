use super::super::constants::DEFAULT_A2A_PORT;
use super::super::deserialize::{AgentA2aConfigFile, HubA2aConfigFile};
use super::super::hub_types::HubConfig;
use super::super::types::{A2aConfig, A2aVisibility};

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
