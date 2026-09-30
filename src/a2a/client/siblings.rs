//! Sibling discovery: on every tunnel (re)connect, and every
//! [`REFRESH_INTERVAL`] while it stays connected, fetch the relay's per-user
//! A2A directory (`GET {origin}/a2a/agents`) and register every agent of every
//! *other* instance of the same user as a `Sibling`-sourced agent in every
//! hosted agent's [`A2aClientHub`], per `docs/systems-usage/a2a.md`. A
//! sibling is named `<instance>/<agent>` and lives at
//! `{origin}/a2a/{instance}/{agent}`.
//!
//! Discovery runs once per hub process. [`SiblingFanout`] is the registry of
//! the per-agent client hubs it fans each result out to. The hub's own
//! agents are teammates, reachable directly, so they never appear as
//! siblings: every entry of the process's own instance is filtered out of
//! every result.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tokio::sync::{Mutex, watch};

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
/// Longest accepted instance or agent slug, matching the relay's own slug
/// validation.
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

/// One entry in the relay's `GET {origin}/a2a/agents` response: one agent of
/// one instance. Only `instance` and `agent` are used; the other fields the
/// relay sends (`slug`, `display_name`, `card_url`, `online`) aren't needed to
/// register a sibling and are ignored by serde's default "extra fields are
/// fine" behavior.
#[derive(Debug, Deserialize)]
struct DirectoryEntry {
    instance: String,
    agent: String,
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
/// Applied to both halves of an `<instance>/<agent>` name. Guards against
/// registering a malformed name from a buggy or compromised relay as an
/// `a2a:<instance>/<agent>` address the model would see.
fn is_valid_sibling_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= MAX_SLUG_LEN
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !slug.starts_with('-')
        && !slug.ends_with('-')
}

/// One discovered sibling: name, A2A base URL, and request headers.
type SiblingEntry = (String, String, HashMap<String, String>);

#[derive(Default)]
struct FanoutInner {
    hubs: BTreeMap<String, Arc<A2aClientHub>>,
    /// The last discovery result, replayed to hubs that register later.
    last: Option<Vec<SiblingEntry>>,
}

/// The per-agent [`A2aClientHub`]s that sibling discovery fans its results
/// out to, keyed by agent name.
///
/// The agent host registers an agent's hub when the agent starts and
/// unregisters it when the agent stops. A hub registered after a discovery
/// pass receives that pass's result immediately, so a late-starting agent
/// doesn't wait for the next refresh.
#[derive(Default)]
pub struct SiblingFanout {
    inner: Mutex<FanoutInner>,
}

impl SiblingFanout {
    /// An empty registry.
    #[must_use]
    pub fn new_shared() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Add (or replace) `agent`'s client hub, and give it the most recent
    /// discovery result.
    pub async fn register(&self, agent: &str, hub: Arc<A2aClientHub>) {
        let mut inner = self.inner.lock().await;
        if let Some(last) = inner.last.clone() {
            hub.set_siblings(last).await;
        }
        inner.hubs.insert(agent.to_string(), hub);
    }

    /// Stop fanning results out to `agent`'s hub.
    pub async fn unregister(&self, agent: &str) {
        self.inner.lock().await.hubs.remove(agent);
    }

    /// Replace the sibling set in every registered hub.
    async fn set_siblings(&self, siblings: Vec<SiblingEntry>) {
        let mut inner = self.inner.lock().await;
        for hub in inner.hubs.values() {
            hub.set_siblings(siblings.clone()).await;
        }
        inner.last = Some(siblings);
    }
}

/// Spawn the background task that keeps every registered agent's sibling
/// entries in sync with the relay directory. Runs for the process lifetime
/// (it exits only if `tunnel_status_tx` is ever dropped, which happens at
/// process shutdown); `fanout` and `tunnel_status_rx` survive a config
/// reload, so one long-lived task spawned at gateway startup stays correct
/// across reloads without needing to be respawned.
pub(crate) fn spawn_sibling_discovery(
    fanout: Arc<SiblingFanout>,
    tunnel_status_rx: watch::Receiver<TunnelStatus>,
) {
    crate::util::spawn_monitored("a2a-sibling-discovery", async move {
        run(fanout, tunnel_status_rx, DiscoveryTimings::default()).await;
    });
}

async fn run(
    fanout: Arc<SiblingFanout>,
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
            match discover_once(&client, &fanout, &conn).await {
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
/// bearer token, then replace every registered hub's sibling set with every
/// agent except those of this instance (this hub's agents are teammates).
async fn discover_once(
    client: &reqwest::Client,
    fanout: &SiblingFanout,
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
        .filter(|entry| entry.instance != conn.instance)
        .filter_map(|entry| {
            if !is_valid_sibling_slug(&entry.instance) || !is_valid_sibling_slug(&entry.agent) {
                tracing::warn!(
                    instance = %entry.instance,
                    agent = %entry.agent,
                    "a2a directory returned a malformed sibling name, skipping"
                );
                return None;
            }
            let mut headers = HashMap::with_capacity(1);
            headers.insert(
                "Authorization".to_string(),
                format!("Bearer {}", conn.token),
            );
            Some((
                format!("{}/{}", entry.instance, entry.agent),
                format!("{}/a2a/{}/{}", conn.origin, entry.instance, entry.agent),
                headers,
            ))
        })
        .collect();

    fanout.set_siblings(siblings).await;
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

    /// A fanout with `hub` registered as agent `solo`.
    async fn fanout_of(hub: &Arc<A2aClientHub>) -> Arc<SiblingFanout> {
        let fanout = SiblingFanout::new_shared();
        fanout.register("solo", Arc::clone(hub)).await;
        fanout
    }
    const FAST_TIMINGS: DiscoveryTimings = DiscoveryTimings {
        refresh: Duration::from_millis(300),
        min_backoff: Duration::from_millis(20),
        max_backoff: Duration::from_millis(80),
        fetch_timeout: Duration::from_secs(2),
    };

    /// One directory entry the fake relay serves: `(instance, agent)`.
    type FakeEntry = (&'static str, &'static str);

    /// Shared state behind the fake relay/instance stand-in: the directory
    /// body to serve, and every `Authorization` header seen on a card fetch
    /// (`GET /a2a/{instance}/{agent}/.well-known/agent-card.json`), keyed by
    /// `<instance>/<agent>`.
    #[derive(Default)]
    struct FakeRelay {
        agents: Mutex<Vec<FakeEntry>>,
        card_auth_seen: Mutex<StdHashMap<String, String>>,
        card_fetch_should_fail: Mutex<bool>,
    }

    async fn directory_handler(State(relay): State<Arc<FakeRelay>>) -> impl IntoResponse {
        let entries = relay.agents.lock().unwrap().clone();
        let agents: Vec<serde_json::Value> = entries
            .into_iter()
            .map(|(instance, agent)| {
                serde_json::json!({
                    "slug": format!("{instance}/{agent}"),
                    "instance": instance,
                    "agent": agent,
                    "display_name": agent,
                    "card_url": format!("http://ignored/a2a/{instance}/{agent}/.well-known/agent-card.json"),
                    "online": true,
                })
            })
            .collect();
        Json(serde_json::json!({ "agents": agents }))
    }

    async fn card_handler(
        State(relay): State<Arc<FakeRelay>>,
        Path((instance, agent)): Path<(String, String)>,
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
        let slug = format!("{instance}/{agent}");
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
                {"url": format!("http://placeholder/a2a/{instance}/{agent}/"), "protocolBinding": "JSONRPC", "protocolVersion": "1.0"}
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
    async fn spawn_fake_relay(initial_agents: Vec<FakeEntry>) -> (String, Arc<FakeRelay>) {
        let relay = Arc::new(FakeRelay {
            agents: Mutex::new(initial_agents),
            card_auth_seen: Mutex::new(StdHashMap::new()),
            card_fetch_should_fail: Mutex::new(false),
        });
        let app = Router::new()
            .route("/a2a/agents", get(directory_handler))
            .route(
                "/a2a/{instance}/{agent}/.well-known/agent-card.json",
                get(card_handler),
            )
            .with_state(Arc::clone(&relay));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        crate::util::spawn_in_span(async move {
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
        let (origin, relay) = spawn_fake_relay(vec![
            ("alpha", "scout"),
            ("alpha", "writer"),
            ("beta", "atlas"),
            ("gamma", "nova"),
        ])
        .await;
        let hub = Arc::new(A2aClientHub::new());
        let (tx, rx) = watch::channel(TunnelStatus::Disconnected);
        let task = crate::util::spawn_in_span(run(fanout_of(&hub).await, rx, FAST_TIMINGS));

        tx.send(connected(&origin, "alpha", "tok1")).ok();

        wait_for(&hub, |snap| {
            snap.len() == 2 && snap.iter().all(|a| matches!(a.status, AgentStatus::Ok(_)))
        })
        .await;

        let snap = hub.snapshot().await;
        assert_eq!(sibling_names(&snap), ["beta/atlas", "gamma/nova"]);
        assert!(snap.iter().all(|a| a.source == AgentSource::Sibling));

        let seen = relay.card_auth_seen.lock().unwrap().clone();
        assert_eq!(
            seen.get("beta/atlas").map(String::as_str),
            Some("Bearer tok1")
        );
        assert_eq!(
            seen.get("gamma/nova").map(String::as_str),
            Some("Bearer tok1")
        );
        assert!(
            !seen.keys().any(|slug| slug.starts_with("alpha/")),
            "the hub's own agents must never be fetched: {seen:?}"
        );

        task.abort();
    }

    #[tokio::test]
    async fn reconnect_with_a_new_token_updates_the_header() {
        let (origin, relay) = spawn_fake_relay(vec![("alpha", "scout"), ("beta", "atlas")]).await;
        let hub = Arc::new(A2aClientHub::new());
        let (tx, rx) = watch::channel(TunnelStatus::Disconnected);
        let task = crate::util::spawn_in_span(run(fanout_of(&hub).await, rx, FAST_TIMINGS));

        tx.send(connected(&origin, "alpha", "tok1")).ok();
        wait_for(&hub, |snap| {
            snap.iter()
                .any(|a| a.name == "beta/atlas" && matches!(a.status, AgentStatus::Ok(_)))
        })
        .await;
        assert_eq!(
            relay
                .card_auth_seen
                .lock()
                .unwrap()
                .get("beta/atlas")
                .cloned(),
            Some("Bearer tok1".to_string())
        );

        tx.send(connected(&origin, "alpha", "tok2")).ok();
        tokio::time::timeout(TEST_TIMEOUT, async {
            loop {
                if relay
                    .card_auth_seen
                    .lock()
                    .unwrap()
                    .get("beta/atlas")
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
        let (origin, _relay) = spawn_fake_relay(vec![("alpha", "scout"), ("beta", "atlas")]).await;
        let hub = Arc::new(A2aClientHub::new());

        // A config entry under the sibling's name already exists before
        // discovery ever runs.
        let config_url = format!("{origin}/a2a/beta/atlas");
        hub.register_external(
            "beta/atlas".to_string(),
            config_url.clone(),
            HashMap::new(),
            AgentSource::Config,
        )
        .await;

        let (tx, rx) = watch::channel(TunnelStatus::Disconnected);
        let task = crate::util::spawn_in_span(run(fanout_of(&hub).await, rx, FAST_TIMINGS));
        tx.send(connected(&origin, "alpha", "tok1")).ok();

        wait_for(&hub, |snap| {
            snap.iter()
                .any(|a| a.name == "beta/atlas" && matches!(a.status, AgentStatus::Ok(_)))
        })
        .await;

        let snap = hub.snapshot().await;
        let beta = snap
            .iter()
            .find(|a| a.name == "beta/atlas")
            .expect("beta/atlas present");
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
        let task = crate::util::spawn_in_span(run(fanout_of(&hub).await, rx, FAST_TIMINGS));

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
        let (origin, _relay) = spawn_fake_relay(vec![("alpha", "scout"), ("beta", "atlas")]).await;
        let hub = Arc::new(A2aClientHub::new());
        let (tx, rx) = watch::channel(TunnelStatus::Disconnected);
        let task = crate::util::spawn_in_span(run(fanout_of(&hub).await, rx, FAST_TIMINGS));

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

    fn sibling_names(snap: &[AgentSnapshot]) -> Vec<&str> {
        let mut names: Vec<&str> = snap.iter().map(|a| a.name.as_str()).collect();
        names.sort_unstable();
        names
    }

    #[tokio::test]
    async fn discovery_fans_out_to_every_agent_and_excludes_the_hubs_own_instance() {
        // "alpha" is this hub's own instance, so its agents are teammates.
        let (origin, _relay) = spawn_fake_relay(vec![
            ("alpha", "scout"),
            ("alpha", "writer"),
            ("beta", "atlas"),
            ("beta", "ledger"),
            ("gamma", "nova"),
        ])
        .await;
        let scout = Arc::new(A2aClientHub::new());
        let writer = Arc::new(A2aClientHub::new());
        let fanout = SiblingFanout::new_shared();
        fanout.register("scout", Arc::clone(&scout)).await;
        fanout.register("writer", Arc::clone(&writer)).await;

        let (tx, rx) = watch::channel(TunnelStatus::Disconnected);
        let task = crate::util::spawn_in_span(run(fanout, rx, FAST_TIMINGS));
        tx.send(connected(&origin, "alpha", "tok1")).ok();

        let expected = ["beta/atlas", "beta/ledger", "gamma/nova"];
        wait_for(&scout, |snap| snap.len() == 3).await;
        wait_for(&writer, |snap| snap.len() == 3).await;
        assert_eq!(sibling_names(&scout.snapshot().await), expected);
        assert_eq!(sibling_names(&writer.snapshot().await), expected);

        task.abort();
    }

    #[tokio::test]
    async fn a_hub_registered_after_discovery_gets_the_last_result() {
        let (origin, _relay) = spawn_fake_relay(vec![("alpha", "scout"), ("beta", "atlas")]).await;
        let early = Arc::new(A2aClientHub::new());
        let fanout = fanout_of(&early).await;

        let (tx, rx) = watch::channel(TunnelStatus::Disconnected);
        let task = crate::util::spawn_in_span(run(Arc::clone(&fanout), rx, FAST_TIMINGS));
        tx.send(connected(&origin, "alpha", "tok1")).ok();
        wait_for(&early, |snap| snap.len() == 1).await;

        let late = Arc::new(A2aClientHub::new());
        fanout.register("late", Arc::clone(&late)).await;
        assert_eq!(sibling_names(&late.snapshot().await), ["beta/atlas"]);

        task.abort();
    }

    #[tokio::test]
    async fn an_unregistered_hub_stops_receiving_results() {
        let (origin, _relay) = spawn_fake_relay(vec![("alpha", "scout"), ("beta", "atlas")]).await;
        let stopped = Arc::new(A2aClientHub::new());
        let running = Arc::new(A2aClientHub::new());
        let fanout = SiblingFanout::new_shared();
        fanout.register("stopped", Arc::clone(&stopped)).await;
        fanout.register("running", Arc::clone(&running)).await;
        fanout.unregister("stopped").await;

        let (tx, rx) = watch::channel(TunnelStatus::Disconnected);
        let task = crate::util::spawn_in_span(run(fanout, rx, FAST_TIMINGS));
        tx.send(connected(&origin, "alpha", "tok1")).ok();
        wait_for(&running, |snap| snap.len() == 1).await;

        assert!(stopped.snapshot().await.is_empty());
        task.abort();
    }

    #[tokio::test]
    async fn a_malformed_instance_or_agent_name_is_skipped() {
        let (origin, _relay) = spawn_fake_relay(vec![
            ("beta", "atlas"),
            ("Bad-Caps", "nova"),
            ("gamma", "has space"),
            ("delta", "-lead"),
        ])
        .await;
        let hub = Arc::new(A2aClientHub::new());
        let (tx, rx) = watch::channel(TunnelStatus::Disconnected);
        let task = crate::util::spawn_in_span(run(fanout_of(&hub).await, rx, FAST_TIMINGS));
        tx.send(connected(&origin, "alpha", "tok1")).ok();

        wait_for(&hub, |snap| !snap.is_empty()).await;
        assert_eq!(sibling_names(&hub.snapshot().await), ["beta/atlas"]);

        task.abort();
    }

    #[tokio::test]
    async fn the_sibling_url_addresses_the_agent_under_its_instance() {
        let (origin, _relay) = spawn_fake_relay(vec![("beta", "atlas")]).await;
        let hub = Arc::new(A2aClientHub::new());
        let (tx, rx) = watch::channel(TunnelStatus::Disconnected);
        let task = crate::util::spawn_in_span(run(fanout_of(&hub).await, rx, FAST_TIMINGS));
        tx.send(connected(&origin, "alpha", "tok1")).ok();

        wait_for(&hub, |snap| {
            snap.iter().any(|a| matches!(a.status, AgentStatus::Ok(_)))
        })
        .await;
        let client = hub.client_for("beta/atlas").await;
        assert!(
            client.is_ok(),
            "the sibling must be callable by its full name"
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
