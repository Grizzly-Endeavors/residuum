//! Core tunnel connection logic with automatic reconnection.

use std::sync::Arc;
use std::time::Duration;

use rand::Rng;
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::http as ws_http;
use tracing::{debug, error, info, warn};

use super::TunnelStatus;
use super::v2::frames::AgentInfo;
use super::v2::{self, ConnectOutcome, SessionEnd, SessionHandler};
use crate::config::CloudConfig;

/// Header listing optional features this tunnel client supports.
const CAPABILITIES_HEADER: &str = "x-residuum-capabilities";

/// Capability: this client speaks tunnel v2 and terminates TLS itself. The
/// relay refuses registration without it.
const TLS_PASSTHROUGH_CAPABILITY: &str = "tls-passthrough";

/// Minimum backoff duration between reconnection attempts.
const MIN_BACKOFF: Duration = Duration::from_secs(1);

/// Maximum backoff duration between reconnection attempts.
pub(super) const MAX_BACKOFF: Duration = Duration::from_mins(1);

/// Calculate the next backoff duration by doubling the current value, capped at
/// [`MAX_BACKOFF`], with random jitter (0.5x–1.5x) to avoid thundering herd.
#[must_use]
fn next_backoff(current: Duration) -> Duration {
    let doubled = current.saturating_mul(2);
    let base = doubled.min(MAX_BACKOFF);
    let jitter = rand::thread_rng().gen_range(0.5_f64..1.5);
    base.mul_f64(jitter)
}

/// Start the tunnel client, maintaining a persistent connection to the cloud
/// relay with exponential backoff reconnection.
///
/// `handler` decides whether to trust the relay and receives the browser
/// streams the relay hands over. `agents_rx` carries the hub's full agent
/// list. It is sent to the relay after every (re)connect and again on every
/// change.
///
/// Runs until the shutdown signal is received. Transient connection errors are
/// logged and retried automatically.
#[tracing::instrument(skip_all, fields(relay_url = %cfg.relay_url))]
pub(crate) async fn start_tunnel(
    cfg: CloudConfig,
    mut agents_rx: watch::Receiver<Vec<AgentInfo>>,
    mut shutdown_rx: watch::Receiver<bool>,
    status_tx: Arc<watch::Sender<TunnelStatus>>,
    handler: Arc<dyn SessionHandler>,
) {
    let mut backoff = MIN_BACKOFF;

    loop {
        if *shutdown_rx.borrow() {
            info!("tunnel shutting down before reconnect");
            status_tx.send(TunnelStatus::Disconnected).ok();
            return;
        }

        status_tx
            .send(TunnelStatus::Connecting)
            .unwrap_or_else(|_| {
                debug!("status receiver dropped");
            });
        debug!(url = %cfg.relay_url, "connecting to relay");

        let attempt = Attempt {
            cfg: &cfg,
            handler: &handler,
        };
        match attempt
            .run(&mut agents_rx, &mut shutdown_rx, &status_tx)
            .await
        {
            Step::Done => return,
            Step::Reconnect => backoff = MIN_BACKOFF,
            Step::Wait(wait) => {
                sleep_unless_shutdown(wait.unwrap_or(backoff), &mut shutdown_rx).await;
                backoff = next_backoff(backoff);
            }
        }
    }
}

/// What to do after one connection attempt.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Step {
    /// Shutdown was requested and handled.
    Done,
    /// The session ended after a successful connection; reconnect now.
    Reconnect,
    /// Wait (the given time, or the current backoff) and try again.
    Wait(Option<Duration>),
}

/// Sleep for `wait`, or until shutdown is requested.
async fn sleep_unless_shutdown(wait: Duration, shutdown_rx: &mut watch::Receiver<bool>) {
    tokio::select! {
        () = tokio::time::sleep(wait) => {}
        _ = shutdown_rx.wait_for(|stop| *stop) => {}
    }
}

/// What one attempt on the relay's registration endpoint needs to know.
pub(super) struct Attempt<'a> {
    pub(super) cfg: &'a CloudConfig,
    pub(super) handler: &'a Arc<dyn SessionHandler>,
}

impl Attempt<'_> {
    /// Connect once and run the session until it ends.
    pub(super) async fn run(
        &self,
        agents_rx: &mut watch::Receiver<Vec<AgentInfo>>,
        shutdown_rx: &mut watch::Receiver<bool>,
        status_tx: &watch::Sender<TunnelStatus>,
    ) -> Step {
        let Some(url) = v2::register_url(&self.cfg.relay_url) else {
            error!(
                url = %self.cfg.relay_url,
                "[cloud] relay_url must end in /tunnel/v2/register; the tunnel won't connect until it does"
            );
            status_tx.send(TunnelStatus::Disconnected).ok();
            return Step::Wait(Some(MAX_BACKOFF));
        };
        let request = match build_request(self.cfg, &url) {
            Ok(r) => r,
            Err(e) => {
                error!(error = %e, "failed to build WebSocket request");
                return Step::Wait(None);
            }
        };
        let ws = match v2::connect(request).await {
            ConnectOutcome::Connected(ws) => *ws,
            ConnectOutcome::Unsupported(status) => {
                error!(
                    status,
                    url = %url,
                    "the relay does not offer end-to-end encrypted remote access; update the relay or check [cloud] relay_url"
                );
                self.handler.on_relay_unsupported();
                status_tx.send(TunnelStatus::Disconnected).ok();
                return Step::Wait(None);
            }
            ConnectOutcome::Failed(e) => {
                warn!(error = %e, "failed to connect to relay, will retry");
                return Step::Wait(None);
            }
        };
        let end = v2::run_session(
            ws,
            v2::SessionInputs {
                handler: self.handler,
                agents_rx,
                shutdown_rx,
                status_tx,
            },
        )
        .await;
        match end {
            SessionEnd::Shutdown => Step::Done,
            SessionEnd::Refused => {
                status_tx.send(TunnelStatus::Disconnected).ok();
                Step::Wait(Some(MAX_BACKOFF))
            }
            SessionEnd::Lost { reason, accepted } => {
                warn!(reason = %reason, accepted, "disconnected from relay, reconnecting");
                if accepted {
                    Step::Reconnect
                } else {
                    Step::Wait(None)
                }
            }
        }
    }
}

/// Build the upgrade request for the registration endpoint at `url`.
fn build_request(cfg: &CloudConfig, url: &str) -> Result<ws_http::Request<()>, ws_http::Error> {
    let host = url::Url::parse(url)
        .ok()
        .and_then(|u| {
            let h = u.host_str()?.to_string();
            Some(match u.port() {
                Some(port) => format!("{h}:{port}"),
                None => h,
            })
        })
        .unwrap_or_else(|| "localhost".to_string());

    let mut request = ws_http::Request::builder()
        .uri(url)
        .header("Host", host)
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header(
            "Sec-WebSocket-Key",
            tokio_tungstenite::tungstenite::handshake::client::generate_key(),
        )
        .body(())?;
    request.headers_mut().insert(
        ws_http::header::AUTHORIZATION,
        ws_http::HeaderValue::from_str(&format!("Bearer {}", cfg.token))?,
    );
    request.headers_mut().insert(
        CAPABILITIES_HEADER,
        ws_http::HeaderValue::from_static(TLS_PASSTHROUGH_CAPABILITY),
    );
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RemoteAccessSettings;

    fn cfg() -> CloudConfig {
        CloudConfig {
            relay_url: "wss://relay.example.com/tunnel/v2/register".to_string(),
            token: "tok".to_string(),
            remote: RemoteAccessSettings::default(),
        }
    }

    #[test]
    fn backoff_doubles_with_jitter() {
        let current = Duration::from_secs(2);
        let next = next_backoff(current);
        // Doubled = 4s, jitter range 0.5..1.5 → result in 2s..6s
        assert!(
            next >= Duration::from_secs(2) && next <= Duration::from_secs(6),
            "backoff from 2s should be in 2s..6s (4s ± jitter), got {next:?}"
        );
    }

    #[test]
    fn backoff_caps_at_max_with_jitter() {
        let current = Duration::from_secs(45);
        let next = next_backoff(current);
        // Base is capped at MAX_BACKOFF (60s), jitter range → 30s..90s
        let min_with_jitter = MAX_BACKOFF.mul_f64(0.5);
        let max_with_jitter = MAX_BACKOFF.mul_f64(1.5);
        assert!(
            next >= min_with_jitter,
            "backoff should be at least {min_with_jitter:?}, got {next:?}"
        );
        assert!(
            next <= max_with_jitter,
            "backoff should not exceed {max_with_jitter:?}, got {next:?}"
        );
    }

    #[test]
    fn backoff_floor_is_never_below_half_current() {
        let mut current = MIN_BACKOFF;
        for _ in 0..5 {
            let next = next_backoff(current);
            let floor = current.mul_f64(0.5);
            assert!(
                next >= floor,
                "backoff should not drop below {floor:?}, got {next:?} from {current:?}"
            );
            current = next;
        }
    }

    #[test]
    fn upgrade_request_carries_the_token_and_only_the_passthrough_capability() {
        let url = "wss://relay.example.com/tunnel/v2/register";
        let request = build_request(&cfg(), url).unwrap();
        assert_eq!(request.uri(), url);
        assert_eq!(request.headers()["host"], "relay.example.com");
        assert_eq!(request.headers()["authorization"], "Bearer tok");
        assert_eq!(request.headers()[CAPABILITIES_HEADER], "tls-passthrough");
    }

    #[test]
    fn upgrade_request_host_keeps_an_explicit_port() {
        let request = build_request(&cfg(), "ws://127.0.0.1:8080/tunnel/v2/register").unwrap();
        assert_eq!(request.headers()["host"], "127.0.0.1:8080");
    }

    #[test]
    fn upgrade_request_for_a_malformed_url_is_refused_not_sent_to_localhost() {
        assert!(build_request(&cfg(), "not a url").is_err());
    }
}
