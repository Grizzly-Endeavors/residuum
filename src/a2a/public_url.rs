//! An agent's A2A base URL, in order of precedence: `[a2a] public_url` plus
//! `/agents/<name>` when set; otherwise the tunnel's address for the hub while
//! the tunnel is connected (`{instance origin}/a2a/<name>`); otherwise the local listener address plus `/agents/<name>`.
//! See `docs/systems-usage/a2a.md`.

use crate::config::A2aConfig;
use crate::tunnel::TunnelStatus;

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

/// The address every agent of this hub is reachable under through the tunnel,
/// once it is connected: the instance's own host, `{instance origin}/a2a`. An
/// agent's own address appends `/<name>`.
#[must_use]
pub(crate) fn relay_a2a_base(status: &TunnelStatus) -> Option<String> {
    let TunnelStatus::Connected {
        instance_origin: Some(instance_origin),
        ..
    } = status
    else {
        return None;
    };
    Some(format!("{}/a2a", instance_origin.trim_end_matches('/')))
}

/// `agent_name`'s address on the relay: `relay_base` (see
/// [`relay_a2a_base`]) plus `/<name>`.
#[must_use]
pub(crate) fn relay_agent_url(relay_base: &str, agent_name: &str) -> String {
    format!("{}/{agent_name}", relay_base.trim_end_matches('/'))
}

/// The URL `agent_name`'s Agent Card advertises: [`known_a2a_public_url`],
/// else the agent's address under `relay_base`, else the local listener
/// address.
#[must_use]
pub(crate) fn resolve_a2a_public_url(
    a2a: &A2aConfig,
    gateway_bind: &str,
    agent_name: &str,
    relay_base: Option<&str>,
) -> String {
    known_a2a_public_url(a2a, agent_name)
        .or_else(|| relay_base.map(|base| relay_agent_url(base, agent_name)))
        .unwrap_or_else(|| {
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

    const RELAY_BASE: &str = "https://laptop.bear.agent-residuum.com/a2a";

    fn connected(instance_origin: Option<&str>) -> TunnelStatus {
        TunnelStatus::Connected {
            user_id: "bear".to_string(),
            origin: Some("https://bear.agent-residuum.com".to_string()),
            workbench_origin: None,
            instance: Some("laptop".to_string()),
            instance_origin: instance_origin.map(str::to_string),
        }
    }

    #[test]
    fn the_tunnel_uses_the_instance_host() {
        let status = connected(Some("https://laptop.bear.agent-residuum.com/"));
        assert_eq!(relay_a2a_base(&status), Some(RELAY_BASE.to_string()));
        let url = resolve_a2a_public_url(
            &cfg(None),
            "127.0.0.1",
            "scout",
            relay_a2a_base(&status).as_deref(),
        );
        assert_eq!(url, "https://laptop.bear.agent-residuum.com/a2a/scout");
    }

    #[test]
    fn explicit_public_url_gets_the_agent_path_appended() {
        let url = resolve_a2a_public_url(
            &cfg(Some("https://own.example/a2a")),
            "127.0.0.1",
            "scout",
            None,
        );
        assert_eq!(url, "https://own.example/a2a/agents/scout");
    }

    #[test]
    fn trailing_slash_on_the_public_url_is_not_doubled() {
        let url = resolve_a2a_public_url(
            &cfg(Some("https://own.example/")),
            "127.0.0.1",
            "scout",
            None,
        );
        assert_eq!(url, "https://own.example/agents/scout");
    }

    #[test]
    fn without_a_public_url_or_relay_the_local_address_is_used() {
        let url = resolve_a2a_public_url(&cfg(None), "127.0.0.1", "scout", None);
        assert_eq!(url, "http://127.0.0.1:7702/agents/scout");
        assert_eq!(known_a2a_public_url(&cfg(None), "scout"), None);
    }

    #[test]
    fn the_relay_address_is_used_when_no_public_url_is_set() {
        let url = resolve_a2a_public_url(&cfg(None), "127.0.0.1", "scout", Some(RELAY_BASE));
        assert_eq!(url, "https://laptop.bear.agent-residuum.com/a2a/scout");
    }

    #[test]
    fn an_explicit_public_url_beats_the_relay_address() {
        let url = resolve_a2a_public_url(
            &cfg(Some("https://own.example")),
            "127.0.0.1",
            "scout",
            Some(RELAY_BASE),
        );
        assert_eq!(url, "https://own.example/agents/scout");
    }

    #[test]
    fn the_relay_base_needs_a_connected_tunnel_with_an_instance_origin() {
        assert_eq!(relay_a2a_base(&connected(None)), None);
        assert_eq!(relay_a2a_base(&TunnelStatus::Connecting), None);
        assert_eq!(relay_a2a_base(&TunnelStatus::Disconnected), None);
    }
}
