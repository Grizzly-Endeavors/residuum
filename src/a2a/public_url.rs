//! This instance's current A2A public URL: `[a2a] public_url` when set,
//! otherwise the relay tunnel's origin while connected, otherwise a local
//! fallback. See `docs/systems-usage/a2a.md`.

use std::sync::Arc;

use crate::config::A2aConfig;
use crate::tunnel::TunnelStatus;

/// The address other agents can use to reach this agent, when one is known:
/// the configured `[a2a] public_url`, else the relay URL while the tunnel is
/// connected. `None` when neither applies.
#[must_use]
pub(crate) fn known_a2a_public_url(
    a2a: &A2aConfig,
    tunnel_status: &TunnelStatus,
) -> Option<String> {
    if let Some(url) = &a2a.public_url {
        return Some(url.clone());
    }
    if let TunnelStatus::Connected {
        origin: Some(origin),
        instance: Some(instance),
        ..
    } = tunnel_status
    {
        return Some(format!("{}/a2a/{instance}", origin.trim_end_matches('/')));
    }
    None
}

/// The URL the Agent Card advertises: [`known_a2a_public_url`], falling back
/// to the local listener address.
#[must_use]
pub(crate) fn resolve_a2a_public_url(
    a2a: &A2aConfig,
    gateway_bind: &str,
    tunnel_status: &TunnelStatus,
) -> String {
    known_a2a_public_url(a2a, tunnel_status)
        .unwrap_or_else(|| format!("http://{gateway_bind}:{}", a2a.port))
}

pub(crate) struct A2aPublicUrl {
    a2a: A2aConfig,
    gateway_bind: String,
    tunnel_status_rx: tokio::sync::watch::Receiver<TunnelStatus>,
}

/// Shared handle to [`A2aPublicUrl`].
pub(crate) type SharedA2aPublicUrl = Arc<A2aPublicUrl>;

impl A2aPublicUrl {
    /// Build a handle from the current `[a2a]` config, the gateway's bind
    /// address, and a live tunnel status receiver.
    #[must_use]
    pub(crate) fn new(
        a2a: A2aConfig,
        gateway_bind: String,
        tunnel_status_rx: tokio::sync::watch::Receiver<TunnelStatus>,
    ) -> SharedA2aPublicUrl {
        Arc::new(Self {
            a2a,
            gateway_bind,
            tunnel_status_rx,
        })
    }

    /// The current public URL, per [`resolve_a2a_public_url`]'s precedence.
    #[must_use]
    pub(crate) fn current(&self) -> String {
        resolve_a2a_public_url(
            &self.a2a,
            &self.gateway_bind,
            &self.tunnel_status_rx.borrow(),
        )
    }
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
    fn explicit_public_url_always_wins() {
        let status = TunnelStatus::Connected {
            user_id: "u1".to_string(),
            origin: Some("https://example.com".to_string()),
            workbench_origin: None,
            instance: Some("laptop".to_string()),
            a2a_token: None,
        };
        let url =
            resolve_a2a_public_url(&cfg(Some("https://own.example/a2a")), "127.0.0.1", &status);
        assert_eq!(url, "https://own.example/a2a");
    }

    #[test]
    fn connected_tunnel_with_instance_and_origin_wins_over_local_fallback() {
        let status = TunnelStatus::Connected {
            user_id: "u1".to_string(),
            origin: Some("https://example.agent-residuum.com/".to_string()),
            workbench_origin: None,
            instance: Some("laptop".to_string()),
            a2a_token: None,
        };
        let url = resolve_a2a_public_url(&cfg(None), "127.0.0.1", &status);
        assert_eq!(url, "https://example.agent-residuum.com/a2a/laptop");
    }

    #[test]
    fn disconnected_tunnel_falls_back_to_local_bind() {
        let url = resolve_a2a_public_url(&cfg(None), "127.0.0.1", &TunnelStatus::Disconnected);
        assert_eq!(url, "http://127.0.0.1:7702");
    }

    #[test]
    fn connected_tunnel_missing_instance_falls_back_to_local_bind() {
        let status = TunnelStatus::Connected {
            user_id: "u1".to_string(),
            origin: Some("https://example.com".to_string()),
            workbench_origin: None,
            instance: None,
            a2a_token: None,
        };
        let url = resolve_a2a_public_url(&cfg(None), "127.0.0.1", &status);
        assert_eq!(url, "http://127.0.0.1:7702");
    }

    #[tokio::test]
    async fn shared_handle_reflects_current_tunnel_status() {
        let (tx, rx) = tokio::sync::watch::channel(TunnelStatus::Disconnected);
        let handle = A2aPublicUrl::new(cfg(None), "127.0.0.1".to_string(), rx);
        assert_eq!(handle.current(), "http://127.0.0.1:7702");

        tx.send(TunnelStatus::Connected {
            user_id: "u1".to_string(),
            origin: Some("https://example.com".to_string()),
            workbench_origin: None,
            instance: Some("laptop".to_string()),
            a2a_token: None,
        })
        .ok();
        assert_eq!(handle.current(), "https://example.com/a2a/laptop");
    }
}
