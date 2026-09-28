//! Agent-to-agent listener settings.

use super::super::constants::DEFAULT_A2A_PORT;
use super::super::deserialize::A2aConfigFile;
use super::super::types::{A2aConfig, A2aVisibility};

/// Resolve `Agent2Agent` (A2A) protocol configuration from the TOML section.
///
/// Enabled by default: every request the listener answers still goes
/// through the auth layer, so turning it on by default costs nothing until
/// a caller key or sibling instance actually exists to use it.
///
/// An invalid `visibility` value falls back to the default visibility with
/// a notice rather than failing the whole config — a2a itself still comes
/// up, just not with the visibility the user intended until they fix it.
pub(super) fn resolve_a2a_config(
    section: Option<&A2aConfigFile>,
    notices: &mut Vec<String>,
) -> A2aConfig {
    let default = A2aConfig::default();
    let Some(section) = section else {
        return default;
    };

    let visibility = match section.visibility.as_deref().map(str::trim) {
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
    };

    let public_url = section
        .public_url
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string);

    A2aConfig {
        enabled: section.enabled.unwrap_or(default.enabled),
        port: section.port.unwrap_or(DEFAULT_A2A_PORT),
        public_url,
        visibility,
    }
}
