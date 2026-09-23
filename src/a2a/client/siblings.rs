//! Sibling discovery: on every tunnel (re)connect, and every
//! [`REFRESH_INTERVAL`] while it stays connected, fetch the relay's per-user
//! A2A directory (`GET {origin}/a2a/agents`) and register every *other*
//! instance of the same user in [`A2aClientHub`] as a `Sibling`-sourced
//! agent, per `docs/systems-usage/a2a.md`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tokio::sync::watch;

use crate::tunnel::TunnelStatus;

use super::hub::A2aClientHub;

/// How often the discovery loop re-fetches the directory while the tunnel
/// stays connected (it also re-fetches immediately on every reconnect, since
/// the sibling token is minted fresh per connection).
const REFRESH_INTERVAL: Duration = Duration::from_secs(600);
/// Backoff bounds for a failed fetch, so a flaky relay doesn't wait a full
/// [`REFRESH_INTERVAL`] for the next attempt, but also doesn't hammer it.
const MIN_RETRY_BACKOFF: Duration = Duration::from_secs(5);
const MAX_RETRY_BACKOFF: Duration = Duration::from_secs(300);
/// How long a single directory fetch may take before it's treated as a
/// failure.
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest accepted sibling slug, matching the relay's own slug validation.
const MAX_SLUG_LEN: usize = 24;

/// The loop's timing knobs, split out of the constants above so tests can
/// drive the same logic on a millisecond timescale instead of waiting on the
/// real backoff/refresh durations.
#[derive(Clone, Copy)]
struct DiscoveryTimings {
    refresh: Duration,
    min_backoff: Duration,
    max_backoff: Duration,
    fetch_timeout: Duration,
}

impl Default for DiscoveryTimings {
    fn default() -> Self {
        Self {
            refresh: REFRESH_INTERVAL,
            min_backoff: MIN_RETRY_BACKOFF,
            max_backoff: MAX_RETRY_BACKOFF,
            fetch_timeout: FETCH_TIMEOUT,
        }
    }
}

/// One entry in the relay's `GET {origin}/a2a/agents` response. Only `slug`
/// is used; the other fields the relay sends (`display_name`, `card_url`,
/// `online`) aren't needed to register a sibling and are ignored by serde's
/// default "extra fields are fine" behavior.
#[derive(Debug, Deserialize)]
struct DirectoryEntry {
    slug: String,
}

#[derive(Debug, Deserialize)]
struct DirectoryResponse {
    agents: Vec<DirectoryEntry>,
}

/// The tunnel facts sibling discovery needs, extracted from
/// [`TunnelStatus::Connected`]. Absent whenever any of the three is missing —
/// e.g. an older relay that doesn't send `instance`/`a2a_token` yet — in
/// which case discovery simply has nothing to do.
#[derive(Clone, PartialEq, Eq)]
struct Connection {
    origin: String,
    instance: String,
    token: String,
}

fn connection_from(status: &TunnelStatus) -> Option<Connection> {
    if let TunnelStatus::Connected {
        origin: Some(origin),
        instance: Some(instance),
        a2a_token: Some(token),
        ..
    } = status
    {
        Some(Connection {
            origin: origin.trim_end_matches('/').to_string(),
            instance: instance.clone(),
            token: token.clone(),
        })
    } else {
        None
    }
}

/// A slug shaped like the relay's own `validate_slug`: 1–24 lowercase
/// letters, digits, and hyphens, never leading or trailing with a hyphen.
/// Guards against registering a malformed name from a buggy or compromised
/// relay as an `a2a:<name>` address the model would see.
fn is_valid_sibling_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= MAX_SLUG_LEN
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !slug.starts_with('-')
        && !slug.ends_with('-')
}

/// Spawn the background task that keeps `hub`'s sibling agents in sync with
/// the relay directory. Runs for the process lifetime (it exits only if
/// `tunnel_status_tx` is ever dropped, which happens at process shutdown);
/// both `hub` and `tunnel_status_rx` survive a config reload, so one
/// long-lived task spawned at gateway startup stays correct across reloads
/// without needing to be respawned.
pub(crate) fn spawn_sibling_discovery(
    hub: Arc<A2aClientHub>,
    tunnel_status_rx: watch::Receiver<TunnelStatus>,
) {
    crate::util::spawn_monitored("a2a-sibling-discovery", async move {
        run(hub, tunnel_status_rx, DiscoveryTimings::default()).await;
    });
}

async fn run(
    hub: Arc<A2aClientHub>,
    mut tunnel_status_rx: watch::Receiver<TunnelStatus>,
    timings: DiscoveryTimings,
) {
    let client = match reqwest::Client::builder()
        .timeout(timings.fetch_timeout)
        .build()
    {
        Ok(client) => client,
        Err(e) => {
            tracing::error!(
                error = %e,
                "failed to build the a2a sibling discovery http client; sibling discovery is disabled"
            );
            return;
        }
    };

    let mut backoff = timings.min_backoff;
    let mut failing = false;

    loop {
        let current_connection = connection_from(&tunnel_status_rx.borrow());
        let wait = if let Some(conn) = current_connection {
            match discover_once(&client, &hub, &conn).await {
                Ok(()) => {
                    if failing {
                        tracing::info!(origin = %conn.origin, "a2a sibling discovery recovered");
                    }
                    failing = false;
                    backoff = timings.min_backoff;
                    timings.refresh
                }
                Err(e) => {
                    if !failing {
                        tracing::warn!(
                            origin = %conn.origin,
                            error = %e,
                            "a2a sibling discovery failed, retrying with backoff"
                        );
                    }
                    failing = true;
                    let this_wait = backoff;
                    backoff = (backoff * 2).min(timings.max_backoff);
                    this_wait
                }
            }
        } else {
            timings.refresh
        };

        tokio::select! {
            biased;
            changed = tunnel_status_rx.changed() => {
                if changed.is_err() {
                    // The sender was dropped: the gateway is shutting down.
                    return;
                }
            }
            () = tokio::time::sleep(wait) => {}
        }
    }
}

/// One fetch-and-register pass: `GET {origin}/a2a/agents` with the sibling
/// bearer token, then replace the hub's sibling set with every entry except
/// this instance's own slug.
async fn discover_once(
    client: &reqwest::Client,
    hub: &A2aClientHub,
    conn: &Connection,
) -> Result<(), String> {
    let url = format!("{}/a2a/agents", conn.origin);
    let response = client
        .get(&url)
        .bearer_auth(&conn.token)
        .send()
        .await
        .map_err(|e| format!("request to {url} failed: {e}"))?;

    let status = response.status();
    if !status.is_success() {
        return Err(format!("{url} returned {status}"));
    }

    let body: DirectoryResponse = response
        .json()
        .await
        .map_err(|e| format!("malformed a2a directory response from {url}: {e}"))?;

    let siblings = body
        .agents
        .into_iter()
        .filter(|entry| entry.slug != conn.instance)
        .filter_map(|entry| {
            if !is_valid_sibling_slug(&entry.slug) {
                tracing::warn!(
                    slug = %entry.slug,
                    "a2a directory returned a malformed sibling slug, skipping"
                );
                return None;
            }
            let mut headers = HashMap::with_capacity(1);
            headers.insert(
                "Authorization".to_string(),
                format!("Bearer {}", conn.token),
            );
            Some((
                entry.slug.clone(),
                format!("{}/a2a/{}", conn.origin, entry.slug),
                headers,
            ))
        })
        .collect();

    hub.set_siblings(siblings).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap as StdHashMap;
    use std::sync::Mutex;

    use axum::extract::{Path, State};
    use axum::http::HeaderMap;
    use axum::response::IntoResponse;
    use axum::routing::get;
    use axum::{Json, Router};

    use super::*;
    use crate::a2a::client::hub::{AgentSnapshot, AgentSource, AgentStatus};

    const TEST_TIMEOUT: Duration = Duration::from_secs(5);
    const FAST_TIMINGS: DiscoveryTimings = DiscoveryTimings {
        refresh: Duration::from_millis(300),
        min_backoff: Duration::from_millis(20),
        max_backoff: Duration::from_millis(80),
        fetch_timeout: Duration::from_secs(2),
    };

    /// Shared state behind the fake relay/instance stand-in: the directory
    /// body to serve, and every `Authorization` header seen on a card fetch
    /// (`GET /a2a/{slug}/.well-known/agent-card.json`), keyed by slug.
    #[derive(Default)]
    struct FakeRelay {
        agents: Mutex<Vec<&'static str>>,
        card_auth_seen: Mutex<StdHashMap<String, String>>,
        card_fetch_should_fail: Mutex<bool>,
    }

    async fn directory_handler(State(relay): State<Arc<FakeRelay>>) -> impl IntoResponse {
        let slugs = relay.agents.lock().unwrap().clone();
        let agents: Vec<serde_json::Value> = slugs
            .into_iter()
            .map(|slug| {
                serde_json::json!({
                    "slug": slug,
                    "display_name": slug,
                    "card_url": format!("http://ignored/a2a/{slug}/.well-known/agent-card.json"),
                    "online": true,
                })
            })
            .collect();
        Json(serde_json::json!({ "agents": agents }))
    }

    async fn card_handler(
        State(relay): State<Arc<FakeRelay>>,
        Path(slug): Path<String>,
        headers: HeaderMap,
    ) -> axum::response::Response {
        if *relay.card_fetch_should_fail.lock().unwrap() {
            return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "nope").into_response();
        }
        let auth = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        relay
            .card_auth_seen
            .lock()
            .unwrap()
            .insert(slug.clone(), auth);
        let body = serde_json::json!({
            "name": slug,
            "description": "a fake sibling",
            "version": "1.0",
            "supportedInterfaces": [
                {"url": format!("http://placeholder/a2a/{slug}/"), "protocolBinding": "JSONRPC", "protocolVersion": "1.0"}
            ],
            "capabilities": {"streaming": true},
            "defaultInputModes": ["text/plain"],
            "defaultOutputModes": ["text/plain"],
            "skills": [],
        });
        Json(body).into_response()
    }

    /// Spawn the fake relay on a loopback port and return its base origin
    /// plus the shared state used to inspect what it saw.
    async fn spawn_fake_relay(initial_agents: Vec<&'static str>) -> (String, Arc<FakeRelay>) {
        let relay = Arc::new(FakeRelay {
            agents: Mutex::new(initial_agents),
            card_auth_seen: Mutex::new(StdHashMap::new()),
            card_fetch_should_fail: Mutex::new(false),
        });
        let app = Router::new()
            .route("/a2a/agents", get(directory_handler))
            .route("/a2a/{slug}/.well-known/agent-card.json", get(card_handler))
            .with_state(Arc::clone(&relay));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.ok();
        });
        (format!("http://{addr}"), relay)
    }

    fn connected(origin: &str, instance: &str, token: &str) -> TunnelStatus {
        TunnelStatus::Connected {
            user_id: "u1".to_string(),
            origin: Some(origin.to_string()),
            workbench_origin: None,
            instance: Some(instance.to_string()),
            a2a_token: Some(token.to_string()),
        }
    }

    /// Poll `hub.snapshot()` until `pred` matches, bounded by
    /// [`TEST_TIMEOUT`].
    async fn wait_for(hub: &A2aClientHub, pred: impl Fn(&[AgentSnapshot]) -> bool) {
        tokio::time::timeout(TEST_TIMEOUT, async {
            loop {
                let snap = hub.snapshot().await;
                if pred(&snap) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("condition was never met within the timeout");
    }

    #[tokio::test]
    async fn connect_registers_siblings_excluding_self_with_the_token_header() {
        let (origin, relay) = spawn_fake_relay(vec!["alpha", "beta", "gamma"]).await;
        let hub = Arc::new(A2aClientHub::new());
        let (tx, rx) = watch::channel(TunnelStatus::Disconnected);
        let task = tokio::spawn(run(Arc::clone(&hub), rx, FAST_TIMINGS));

        tx.send(connected(&origin, "alpha", "tok1")).ok();

        wait_for(&hub, |snap| {
            snap.len() == 2 && snap.iter().all(|a| matches!(a.status, AgentStatus::Ok(_)))
        })
        .await;

        let snap = hub.snapshot().await;
        let names: Vec<&str> = snap.iter().map(|a| a.name.as_str()).collect();
        assert!(names.contains(&"beta"), "got {names:?}");
        assert!(names.contains(&"gamma"), "got {names:?}");
        assert!(
            !names.contains(&"alpha"),
            "self must be excluded: {names:?}"
        );
        assert!(snap.iter().all(|a| a.source == AgentSource::Sibling));

        let seen = relay.card_auth_seen.lock().unwrap().clone();
        assert_eq!(seen.get("beta").map(String::as_str), Some("Bearer tok1"));
        assert_eq!(seen.get("gamma").map(String::as_str), Some("Bearer tok1"));

        task.abort();
    }

    #[tokio::test]
    async fn reconnect_with_a_new_token_updates_the_header() {
        let (origin, relay) = spawn_fake_relay(vec!["alpha", "beta"]).await;
        let hub = Arc::new(A2aClientHub::new());
        let (tx, rx) = watch::channel(TunnelStatus::Disconnected);
        let task = tokio::spawn(run(Arc::clone(&hub), rx, FAST_TIMINGS));

        tx.send(connected(&origin, "alpha", "tok1")).ok();
        wait_for(&hub, |snap| {
            snap.iter()
                .any(|a| a.name == "beta" && matches!(a.status, AgentStatus::Ok(_)))
        })
        .await;
        assert_eq!(
            relay.card_auth_seen.lock().unwrap().get("beta").cloned(),
            Some("Bearer tok1".to_string())
        );

        tx.send(connected(&origin, "alpha", "tok2")).ok();
        tokio::time::timeout(TEST_TIMEOUT, async {
            loop {
                if relay
                    .card_auth_seen
                    .lock()
                    .unwrap()
                    .get("beta")
                    .map(String::as_str)
                    == Some("Bearer tok2")
                {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("header was never updated to the new token");

        task.abort();
    }

    #[tokio::test]
    async fn config_entry_name_collision_means_config_wins() {
        let (origin, _relay) = spawn_fake_relay(vec!["alpha", "beta"]).await;
        let hub = Arc::new(A2aClientHub::new());

        // A config/a2a.json entry named "beta" already exists before
        // discovery ever runs.
        let config_url = format!("{origin}/a2a/beta");
        hub.register_external(
            "beta".to_string(),
            config_url.clone(),
            HashMap::new(),
            AgentSource::Config,
        )
        .await;

        let (tx, rx) = watch::channel(TunnelStatus::Disconnected);
        let task = tokio::spawn(run(Arc::clone(&hub), rx, FAST_TIMINGS));
        tx.send(connected(&origin, "alpha", "tok1")).ok();

        wait_for(&hub, |snap| {
            snap.iter()
                .any(|a| a.name == "beta" && matches!(a.status, AgentStatus::Ok(_)))
        })
        .await;

        let snap = hub.snapshot().await;
        let beta = snap
            .iter()
            .find(|a| a.name == "beta")
            .expect("beta present");
        assert_eq!(
            beta.source,
            AgentSource::Config,
            "the config entry must win the name collision"
        );

        task.abort();
    }

    #[tokio::test]
    async fn directory_failure_does_not_panic_and_retries() {
        let hub = Arc::new(A2aClientHub::new());
        let (tx, rx) = watch::channel(TunnelStatus::Disconnected);
        let task = tokio::spawn(run(Arc::clone(&hub), rx, FAST_TIMINGS));

        // An address nothing listens on: every fetch fails immediately.
        tx.send(connected("http://127.0.0.1:1", "alpha", "tok1"))
            .ok();

        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(
            !task.is_finished(),
            "the discovery task must not panic or exit on failure"
        );
        assert!(
            hub.snapshot().await.is_empty(),
            "a failed fetch must not register any siblings"
        );

        task.abort();
    }

    #[tokio::test]
    async fn disconnect_keeps_previously_registered_siblings() {
        let (origin, _relay) = spawn_fake_relay(vec!["alpha", "beta"]).await;
        let hub = Arc::new(A2aClientHub::new());
        let (tx, rx) = watch::channel(TunnelStatus::Disconnected);
        let task = tokio::spawn(run(Arc::clone(&hub), rx, FAST_TIMINGS));

        tx.send(connected(&origin, "alpha", "tok1")).ok();
        wait_for(&hub, |snap| !snap.is_empty()).await;

        tx.send(TunnelStatus::Disconnected).ok();
        tokio::time::sleep(Duration::from_millis(100)).await;

        assert!(
            !hub.snapshot().await.is_empty(),
            "siblings registered before a disconnect must stay registered"
        );

        task.abort();
    }

    #[test]
    fn slug_validation() {
        for good in ["laptop", "my-server-1", "a", "1x"] {
            assert!(is_valid_sibling_slug(good), "'{good}' should be accepted");
        }
        for bad in [
            "",
            "-leading",
            "trailing-",
            "Has-Caps",
            "has space",
            &"a".repeat(25),
        ] {
            assert!(!is_valid_sibling_slug(bad), "'{bad}' should be rejected");
        }
    }
}
