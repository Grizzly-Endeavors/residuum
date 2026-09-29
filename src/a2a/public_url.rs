//! An agent's A2A base URL: `[a2a] public_url` plus `/agents/<name>` when
//! set, otherwise the local listener address plus `/agents/<name>`. See
//! `docs/systems-usage/a2a.md`.

use crate::config::A2aConfig;

/// The path prefix the hub's A2A listener serves each agent under.
pub(crate) const AGENTS_PATH_PREFIX: &str = "/agents";

/// The address other agents can use to reach `agent_name`, when one is
/// configured: `[a2a] public_url` plus `/agents/<name>`. `None` when
/// `public_url` is unset.
#[must_use]
pub(crate) fn known_a2a_public_url(a2a: &A2aConfig, agent_name: &str) -> Option<String> {
    a2a.public_url.as_ref().map(|url| {
        format!(
            "{}{AGENTS_PATH_PREFIX}/{agent_name}",
            url.trim_end_matches('/')
        )
    })
}

/// The URL `agent_name`'s Agent Card advertises: [`known_a2a_public_url`],
/// falling back to the local listener address.
#[must_use]
pub(crate) fn resolve_a2a_public_url(
    a2a: &A2aConfig,
    gateway_bind: &str,
    agent_name: &str,
) -> String {
    known_a2a_public_url(a2a, agent_name).unwrap_or_else(|| {
        format!(
            "http://{gateway_bind}:{}{AGENTS_PATH_PREFIX}/{agent_name}",
            a2a.port
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::A2aVisibility;

    fn cfg(public_url: Option<&str>) -> A2aConfig {
        A2aConfig {
            enabled: true,
            port: 7702,
            public_url: public_url.map(str::to_string),
            visibility: A2aVisibility::Public,
        }
    }

    #[test]
    fn explicit_public_url_gets_the_agent_path_appended() {
        let url =
            resolve_a2a_public_url(&cfg(Some("https://own.example/a2a")), "127.0.0.1", "scout");
        assert_eq!(url, "https://own.example/a2a/agents/scout");
    }

    #[test]
    fn trailing_slash_on_the_public_url_is_not_doubled() {
        let url = resolve_a2a_public_url(&cfg(Some("https://own.example/")), "127.0.0.1", "scout");
        assert_eq!(url, "https://own.example/agents/scout");
    }

    #[test]
    fn without_a_public_url_the_local_address_is_used() {
        let url = resolve_a2a_public_url(&cfg(None), "127.0.0.1", "scout");
        assert_eq!(url, "http://127.0.0.1:7702/agents/scout");
        assert_eq!(known_a2a_public_url(&cfg(None), "scout"), None);
    }
}
