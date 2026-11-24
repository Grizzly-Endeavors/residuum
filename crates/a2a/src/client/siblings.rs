use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tokio::sync::{Mutex, watch};

use residuum_net::remote_access::SecureDiscovery;

use super::hub::A2aClientHub;

const REFRESH_INTERVAL: Duration = Duration::from_secs(600);
const MIN_RETRY_BACKOFF: Duration = Duration::from_secs(5);
const MAX_RETRY_BACKOFF: Duration = Duration::from_secs(300);
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_SLUG_LEN: usize = 24;

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

#[derive(Debug, Deserialize)]
struct DirectoryEntry {
    instance: String,
    agent: String,
}

#[derive(Debug, Deserialize)]
struct DirectoryResponse {
    agents: Vec<DirectoryEntry>,
}

fn is_valid_sibling_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= MAX_SLUG_LEN
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !slug.starts_with('-')
        && !slug.ends_with('-')
}

type SiblingEntry = (String, String, HashMap<String, String>);

#[derive(Default)]
struct FanoutInner {
    hubs: BTreeMap<String, Arc<A2aClientHub>>,
    last: Option<Vec<SiblingEntry>>,
}

#[derive(Default)]
pub struct SiblingFanout {
    inner: Mutex<FanoutInner>,
}

impl SiblingFanout {
    #[must_use]
    pub fn new_shared() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub async fn register(&self, agent: &str, hub: Arc<A2aClientHub>) {
        let last = {
            let mut inner = self.inner.lock().await;
            inner.hubs.insert(agent.to_string(), Arc::clone(&hub));
            inner.last.clone()
        };
        if let Some(last) = last {
            let to_refresh = hub.apply_siblings(last).await;
            if !to_refresh.is_empty() {
                residuum_core::util::spawn_in_span(async move {
                    hub.refresh_cards(to_refresh).await;
                });
            }
        }
    }

    pub async fn unregister(&self, agent: &str) {
        self.inner.lock().await.hubs.remove(agent);
    }

    async fn set_siblings(&self, siblings: Vec<SiblingEntry>) {
        let hubs: Vec<Arc<A2aClientHub>> = {
            let mut inner = self.inner.lock().await;
            inner.last = Some(siblings.clone());
            inner.hubs.values().cloned().collect()
        };
        futures_util::future::join_all(hubs.iter().map(|hub| hub.set_siblings(siblings.clone())))
            .await;
    }
}

pub fn spawn_sibling_discovery(
    fanout: Arc<SiblingFanout>,
    secure_rx: watch::Receiver<Option<Arc<SecureDiscovery>>>,
) {
    residuum_core::util::spawn_monitored("a2a-sibling-discovery", async move {
        run(fanout, secure_rx, DiscoveryTimings::default()).await;
    });
}

async fn run(
    fanout: Arc<SiblingFanout>,
    mut secure_rx: watch::Receiver<Option<Arc<SecureDiscovery>>>,
    timings: DiscoveryTimings,
) {
    let client = match reqwest::Client::builder()
        .timeout(timings.fetch_timeout)
        .redirect(reqwest::redirect::Policy::none())
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
    let mut had_directory = false;

    loop {
        let secure = secure_rx.borrow_and_update().clone();
        if secure.is_none() && had_directory {
            tracing::debug!(
                "a2a sibling discovery has no directory to read; the siblings already found stay registered"
            );
        }
        had_directory = secure.is_some();
        let wait = match secure {
            Some(secure) => match discover(&client, &fanout, &secure).await {
                Ok(()) => {
                    if failing {
                        tracing::info!(directory = %secure.directory_url, "a2a sibling discovery recovered");
                    }
                    failing = false;
                    backoff = timings.min_backoff;
                    timings.refresh
                }
                Err(e) => {
                    if !failing {
                        tracing::warn!(
                            directory = %secure.directory_url,
                            error = %e,
                            "a2a sibling discovery failed, retrying with backoff"
                        );
                    }
                    failing = true;
                    let this_wait = backoff;
                    backoff = (backoff * 2).min(timings.max_backoff);
                    this_wait
                }
            },
            None => timings.refresh,
        };

        tokio::select! {
            biased;
            changed = secure_rx.changed() => {
                if changed.is_err() {
                    return;
                }
            }
            () = tokio::time::sleep(wait) => {}
        }
    }
}

async fn discover(
    client: &reqwest::Client,
    fanout: &SiblingFanout,
    secure: &SecureDiscovery,
) -> Result<(), String> {
    let url = &secure.directory_url;
    let mut request = client.get(url);
    if let Some(token) = &secure.directory_token {
        request = request.bearer_auth(token);
    }
    let response = request
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
        .filter(|entry| entry.instance != secure.own_slug)
        .filter(|entry| secure.siblings.contains_key(&entry.instance))
        .filter_map(|entry| {
            if !is_valid_sibling_slug(&entry.instance) || !is_valid_sibling_slug(&entry.agent) {
                tracing::warn!(
                    instance = %entry.instance,
                    agent = %entry.agent,
                    "a2a directory returned a malformed sibling name, skipping"
                );
                return None;
            }
            let key = secure.siblings.get(&entry.instance)?;
            let mut headers = HashMap::with_capacity(1);
            headers.insert("Authorization".to_string(), format!("Bearer {key}"));
            let origin = (secure.origin_for)(&entry.instance);
            Some((
                format!("{}/{}", entry.instance, entry.agent),
                format!("{}/a2a/{}", origin.trim_end_matches('/'), entry.agent),
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
    use crate::client::hub::{AgentSnapshot, AgentSource, AgentStatus};
    use residuum_core::testing::wait;

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

    type FakeEntry = (&'static str, &'static str);

    #[derive(Default)]
    struct FakeRelay {
        agents: Mutex<Vec<FakeEntry>>,
        card_auth_seen: Mutex<StdHashMap<String, String>>,
        card_fetch_should_fail: Mutex<bool>,
        directory_auth_seen: Mutex<Vec<String>>,
    }

    async fn directory_handler(
        State(relay): State<Arc<FakeRelay>>,
        headers: HeaderMap,
    ) -> impl IntoResponse {
        relay.directory_auth_seen.lock().unwrap().push(
            headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string(),
        );
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

    async fn spawn_fake_relay(initial_agents: Vec<FakeEntry>) -> (String, Arc<FakeRelay>) {
        let relay = Arc::new(FakeRelay {
            agents: Mutex::new(initial_agents),
            card_auth_seen: Mutex::new(StdHashMap::new()),
            card_fetch_should_fail: Mutex::new(false),
            directory_auth_seen: Mutex::new(Vec::new()),
        });
        let app = Router::new()
            .route("/a2a/agents", get(directory_handler))
            .route(
                "/i/{instance}/a2a/{agent}/.well-known/agent-card.json",
                get(card_handler),
            )
            .with_state(Arc::clone(&relay));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        residuum_core::util::spawn_in_span(async move {
            axum::serve(listener, app).await.ok();
        });
        (format!("http://{addr}"), relay)
    }

    async fn wait_for(hub: &A2aClientHub, pred: impl Fn(&[AgentSnapshot]) -> bool) {
        let pred = &pred;
        wait::until("the hub snapshot to match", || async move {
            pred(&hub.snapshot().await).then_some(())
        })
        .await;
    }

    fn sibling_names(snap: &[AgentSnapshot]) -> Vec<&str> {
        let mut names: Vec<&str> = snap.iter().map(|a| a.name.as_str()).collect();
        names.sort_unstable();
        names
    }

    fn secure(origin: &str, own: &str, joined: &[(&str, &str)]) -> Arc<SecureDiscovery> {
        let base = origin.to_string();
        Arc::new(SecureDiscovery {
            own_slug: own.to_string(),
            directory_url: format!("{origin}/a2a/agents"),
            directory_token: Some("rsa_directory".to_string()),
            siblings: joined
                .iter()
                .map(|(slug, key)| ((*slug).to_string(), (*key).to_string()))
                .collect(),
            origin_for: Arc::new(move |slug| format!("{base}/i/{slug}")),
        })
    }

    fn start(
        fanout: Arc<SiblingFanout>,
    ) -> (
        watch::Sender<Option<Arc<SecureDiscovery>>>,
        tokio::task::JoinHandle<()>,
    ) {
        let (tx, rx) = watch::channel(None);
        let task = residuum_core::util::spawn_in_span(run(fanout, rx, FAST_TIMINGS));
        (tx, task)
    }

    #[tokio::test]
    async fn only_joined_siblings_are_registered_at_derived_hosts_with_their_keys() {
        let (origin, relay) = spawn_fake_relay(vec![
            ("alpha", "scout"),
            ("beta", "atlas"),
            ("gamma", "nova"),
        ])
        .await;
        let hub = Arc::new(A2aClientHub::new());
        let (tx, task) = start(fanout_of(&hub).await);

        tx.send(Some(secure(
            &origin,
            "alpha",
            &[("beta", "rsdm_sib_beta_key")],
        )))
        .ok();
        wait_for(&hub, |snap| {
            snap.len() == 1 && snap.iter().all(|a| matches!(a.status, AgentStatus::Ok(_)))
        })
        .await;

        let snap = hub.snapshot().await;
        assert_eq!(
            sibling_names(&snap),
            ["beta/atlas"],
            "gamma never joined and alpha is this instance"
        );
        assert!(snap.iter().all(|a| a.source == AgentSource::Sibling));
        let seen = relay.card_auth_seen.lock().unwrap().clone();
        assert_eq!(
            seen.get("beta/atlas").map(String::as_str),
            Some("Bearer rsdm_sib_beta_key")
        );
        assert!(
            !seen.keys().any(|slug| slug.starts_with("alpha/")),
            "the hub's own agents must never be fetched: {seen:?}"
        );
        assert!(!seen.contains_key("gamma/nova"));
        assert!(
            relay
                .directory_auth_seen
                .lock()
                .unwrap()
                .iter()
                .all(|auth| auth == "Bearer rsa_directory")
        );
        task.abort();
    }

    #[tokio::test]
    async fn nobody_is_registered_until_a_join_exists_and_a_removed_sibling_is_dropped() {
        let (origin, relay) = spawn_fake_relay(vec![("beta", "atlas")]).await;
        let hub = Arc::new(A2aClientHub::new());
        let (tx, task) = start(fanout_of(&hub).await);

        tx.send(Some(secure(&origin, "alpha", &[]))).ok();
        wait::until_true("discovery to read the directory twice", || {
            relay.directory_auth_seen.lock().unwrap().len() >= 2
        })
        .await;
        assert!(
            hub.snapshot().await.is_empty(),
            "an unjoined sibling is ignored"
        );

        tx.send(Some(secure(
            &origin,
            "alpha",
            &[("beta", "rsdm_sib_beta_key")],
        )))
        .ok();
        wait_for(&hub, |snap| snap.len() == 1).await;

        tx.send(Some(secure(&origin, "alpha", &[]))).ok();
        wait_for(&hub, <[AgentSnapshot]>::is_empty).await;
        task.abort();
    }

    #[tokio::test]
    async fn a_new_key_for_a_sibling_updates_the_header() {
        let (origin, relay) = spawn_fake_relay(vec![("alpha", "scout"), ("beta", "atlas")]).await;
        let hub = Arc::new(A2aClientHub::new());
        let (tx, task) = start(fanout_of(&hub).await);

        tx.send(Some(secure(&origin, "alpha", &[("beta", "key1")])))
            .ok();
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
            Some("Bearer key1".to_string())
        );

        tx.send(Some(secure(&origin, "alpha", &[("beta", "key2")])))
            .ok();
        wait::until_true("the sibling's card request to carry the new key", || {
            relay
                .card_auth_seen
                .lock()
                .unwrap()
                .get("beta/atlas")
                .map(String::as_str)
                == Some("Bearer key2")
        })
        .await;

        task.abort();
    }

    #[tokio::test]
    async fn config_entry_name_collision_means_config_wins() {
        let (origin, _relay) = spawn_fake_relay(vec![("alpha", "scout"), ("beta", "atlas")]).await;
        let hub = Arc::new(A2aClientHub::new());

        let config_url = format!("{origin}/i/beta/a2a/atlas");
        hub.register_external(
            "beta/atlas".to_string(),
            config_url.clone(),
            HashMap::new(),
            AgentSource::Config,
        )
        .await;

        let (tx, task) = start(fanout_of(&hub).await);
        tx.send(Some(secure(&origin, "alpha", &[("beta", "key1")])))
            .ok();

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
        let (tx, task) = start(fanout_of(&hub).await);

        let refusing = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", refusing.local_addr().unwrap());
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = Arc::clone(&attempts);
        residuum_core::util::spawn_in_span(async move {
            while let Ok((connection, _)) = refusing.accept().await {
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                drop(connection);
            }
        });
        tx.send(Some(secure(&address, "alpha", &[("beta", "key1")])))
            .ok();

        wait::until_true("discovery to try the directory again", || {
            attempts.load(std::sync::atomic::Ordering::SeqCst) >= 2
        })
        .await;
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
    async fn losing_the_directory_address_keeps_previously_registered_siblings() {
        let (origin, _relay) = spawn_fake_relay(vec![("alpha", "scout"), ("beta", "atlas")]).await;
        let hub = Arc::new(A2aClientHub::new());
        let log = residuum_core::testing::log::EventLog::default();
        let _capture = log.capture();
        let (tx, task) = start(fanout_of(&hub).await);

        tx.send(Some(secure(&origin, "alpha", &[("beta", "key1")])))
            .ok();
        wait_for(&hub, |snap| !snap.is_empty()).await;

        tx.send(None).ok();
        wait::until_true("discovery to notice the directory is gone", || {
            !log.matching("no directory to read").is_empty()
        })
        .await;

        assert!(
            !hub.snapshot().await.is_empty(),
            "siblings registered before a disconnect must stay registered"
        );

        task.abort();
    }

    #[tokio::test]
    async fn discovery_fans_out_to_every_agent_and_excludes_the_hubs_own_instance() {
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

        let (tx, task) = start(fanout);
        tx.send(Some(secure(
            &origin,
            "alpha",
            &[("alpha", "own"), ("beta", "k1"), ("gamma", "k2")],
        )))
        .ok();

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

        let (tx, task) = start(Arc::clone(&fanout));
        tx.send(Some(secure(&origin, "alpha", &[("beta", "k1")])))
            .ok();
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

        let (tx, task) = start(fanout);
        tx.send(Some(secure(&origin, "alpha", &[("beta", "k1")])))
            .ok();
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
        let (tx, task) = start(fanout_of(&hub).await);
        tx.send(Some(secure(
            &origin,
            "alpha",
            &[
                ("beta", "k1"),
                ("Bad-Caps", "k2"),
                ("gamma", "k3"),
                ("delta", "k4"),
            ],
        )))
        .ok();

        wait_for(&hub, |snap| !snap.is_empty()).await;
        assert_eq!(sibling_names(&hub.snapshot().await), ["beta/atlas"]);

        task.abort();
    }

    #[tokio::test]
    async fn the_sibling_is_callable_by_its_full_name() {
        let (origin, _relay) = spawn_fake_relay(vec![("beta", "atlas")]).await;
        let hub = Arc::new(A2aClientHub::new());
        let (tx, task) = start(fanout_of(&hub).await);
        tx.send(Some(secure(&origin, "alpha", &[("beta", "k1")])))
            .ok();

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

    #[tokio::test]
    async fn register_does_not_block_on_stalled_sibling_card_fetch() {
        let stalled_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let stalled_addr = stalled_listener.local_addr().unwrap();
        let accepted = Arc::new(tokio::sync::Notify::new());
        let accepted_signal = Arc::clone(&accepted);
        let hold_task = residuum_core::util::spawn_in_span(async move {
            let mut held = Vec::new();
            while let Ok((socket, _)) = stalled_listener.accept().await {
                held.push(socket);
                accepted_signal.notify_waiters();
            }
        });

        let first = Arc::new(A2aClientHub::new());
        let fanout = fanout_of(&first).await;
        let fanout_for_set = Arc::clone(&fanout);
        let set_task = residuum_core::util::spawn_in_span(async move {
            fanout_for_set
                .set_siblings(vec![(
                    "beta/atlas".to_string(),
                    format!("http://{stalled_addr}/a2a/atlas"),
                    HashMap::new(),
                )])
                .await;
        });

        accepted.notified().await;

        let second = Arc::new(A2aClientHub::new());
        wait::guarded(
            "registering a new agent hub while a sibling card fetch is stalled",
            fanout.register("second", Arc::clone(&second)),
        )
        .await;

        let snap = second.snapshot().await;
        assert_eq!(sibling_names(&snap), ["beta/atlas"]);
        assert!(matches!(
            snap.first().map(|a| &a.status),
            Some(AgentStatus::Pending)
        ));

        set_task.abort();
        hold_task.abort();
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
