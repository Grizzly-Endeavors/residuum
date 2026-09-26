//! `A2aClientHub`: the remote agents this instance's A2A client can reach —
//! loaded from `config/a2a.json`, plus any registered programmatically (used
//! by relay-sibling discovery). Resolves and caches each agent's card, and
//! builds A2A protocol clients on demand. See `docs/systems-usage/a2a.md`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use a2a_client::A2AClientFactory;
use a2a_client::auth::AuthInterceptor;
use tokio::sync::RwLock;

use crate::agent_keys::SharedAgentKeys;

use super::config::{A2aAgentEntry, load_a2a_agents_map};

/// Minimum delay before retrying a failed card fetch, doubled on every
/// consecutive failure up to [`MAX_RETRY_BACKOFF`].
const MIN_RETRY_BACKOFF: Duration = Duration::from_secs(5);
/// Longest delay between retries of a failed card fetch.
const MAX_RETRY_BACKOFF: Duration = Duration::from_secs(60);
/// How often the background refresh loop checks for agents due a retry.
const REFRESH_TICK: Duration = Duration::from_secs(5);

/// Where a tracked agent entry came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSource {
    /// Listed in `config/a2a.json`.
    Config,
    /// One of this user's own other instances, registered by relay-sibling
    /// discovery.
    Sibling,
}

/// Current status of an agent's card.
#[derive(Debug, Clone)]
pub enum AgentStatus {
    /// A card fetch hasn't completed yet.
    Pending,
    /// The agent's card, from the last successful fetch.
    Ok(Arc<a2a::AgentCard>),
    /// The last card fetch (or refetch) failed; the message is user-facing.
    Error(String),
}

#[derive(Clone)]
struct AgentRecord {
    url: String,
    headers: HashMap<String, String>,
    source: AgentSource,
    status: AgentStatus,
    retry_backoff: Duration,
    next_retry_at: Instant,
}

/// A point-in-time view of one registered agent, for `list_agents` and the
/// web settings API.
#[derive(Debug, Clone)]
pub struct AgentSnapshot {
    pub name: String,
    pub url: String,
    pub source: AgentSource,
    pub status: AgentStatus,
}

impl AgentSnapshot {
    /// The agent's card, if the last fetch succeeded.
    #[must_use]
    pub fn card(&self) -> Option<&a2a::AgentCard> {
        match &self.status {
            AgentStatus::Ok(card) => Some(card),
            AgentStatus::Pending | AgentStatus::Error(_) => None,
        }
    }
}

/// A client call against a remote agent could not be prepared.
#[derive(Debug, Clone, thiserror::Error)]
pub enum HubError {
    /// No agent by that name is registered.
    #[error("no remote agent named 'a2a:{0}'. Check config/a2a.json or list_agents.")]
    Unknown(String),
    /// The agent is registered but its card hasn't resolved successfully.
    #[error("remote agent a2a:{0} isn't reachable right now: {1}")]
    Offline(String, String),
    /// The card resolved, but a client transport couldn't be built from it.
    #[error("couldn't set up a connection to remote agent a2a:{0}: {1}")]
    ClientBuild(String, String),
    /// A connected call to the agent (e.g. `CancelTask`) failed.
    #[error("remote agent a2a:{0} couldn't complete the request: {1}")]
    RequestFailed(String, String),
}

/// The negotiated A2A client type [`A2AClientFactory::create_from_card`]
/// returns.
pub type NegotiatedClient = a2a_client::A2AClient<Box<dyn a2a_client::Transport>>;

/// Registry of remote A2A agents this instance's client can reach.
pub struct A2aClientHub {
    agents: RwLock<HashMap<String, AgentRecord>>,
}

impl Default for A2aClientHub {
    fn default() -> Self {
        Self::new()
    }
}

impl A2aClientHub {
    /// Create an empty hub.
    #[must_use]
    pub fn new() -> Self {
        Self {
            agents: RwLock::new(HashMap::new()),
        }
    }

    /// [`new`](Self::new), wrapped for sharing.
    #[must_use]
    pub fn new_shared() -> Arc<Self> {
        Arc::new(Self::new())
    }

    /// Whether `name` is currently registered (config or sibling).
    pub async fn agent_exists(&self, name: &str) -> bool {
        self.agents.read().await.contains_key(name)
    }

    /// Every registered agent's current state.
    pub async fn snapshot(&self) -> Vec<AgentSnapshot> {
        let agents = self.agents.read().await;
        let mut out: Vec<AgentSnapshot> = agents
            .iter()
            .map(|(name, rec)| AgentSnapshot {
                name: name.clone(),
                url: rec.url.clone(),
                source: rec.source,
                status: rec.status.clone(),
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// (Re)load agents from `config/a2a.json`. Config-sourced agents no
    /// longer in the file are removed. A name also registered as a sibling
    /// (relay-sibling discovery) is overridden by the config entry — config
    /// always wins a name collision, logged once at debug. A new or changed
    /// entry is queued for an immediate card fetch. A parse/read failure
    /// keeps the current agents and logs a warning, matching
    /// `crate::workspace::config::load_mcp_servers_map`'s reload behavior.
    pub async fn reload_from_file(&self, path: &std::path::Path, agent_keys: &SharedAgentKeys) {
        let snapshot = match agent_keys.snapshot().await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "a2a.json reload skipped: agent key store unavailable, keeping current agents"
                );
                return;
            }
        };
        let desired = match load_a2a_agents_map(path, &snapshot.store) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    path = %path.display(),
                    "failed to reload a2a.json, keeping current agents"
                );
                return;
            }
        };

        let mut to_refresh: Vec<String> = Vec::new();
        {
            let mut agents = self.agents.write().await;
            agents.retain(|name, rec| {
                rec.source != AgentSource::Config || desired.contains_key(name)
            });
            for (name, entry) in desired {
                let shadows_sibling = matches!(
                    agents.get(&name),
                    Some(rec) if rec.source == AgentSource::Sibling
                );
                if shadows_sibling {
                    tracing::debug!(
                        agent = %name,
                        "config/a2a.json entry overrides a relay sibling of the same name"
                    );
                }
                let changed = shadows_sibling
                    || match agents.get(&name) {
                        Some(existing) => {
                            existing.url != entry.url || existing.headers != entry.headers
                        }
                        None => true,
                    };
                if changed {
                    agents.insert(name.clone(), fresh_record(entry, AgentSource::Config));
                    to_refresh.push(name);
                }
            }
        }
        for name in to_refresh {
            self.refresh_card(&name).await;
        }
    }

    /// Register (or update) one externally-sourced agent — e.g. a relay
    /// sibling, or any other programmatic registration — and queue an
    /// immediate card fetch.
    pub async fn register_external(
        &self,
        name: String,
        url: String,
        headers: HashMap<String, String>,
        source: AgentSource,
    ) {
        {
            let mut agents = self.agents.write().await;
            agents.insert(
                name.clone(),
                fresh_record(
                    A2aAgentEntry {
                        name: name.clone(),
                        url,
                        headers,
                    },
                    source,
                ),
            );
        }
        self.refresh_card(&name).await;
    }

    /// Replace every `Sibling`-sourced agent with exactly this set —
    /// entries missing from `siblings` are removed, present ones are
    /// registered (or refreshed if changed). A name already claimed by a
    /// `config/a2a.json` entry is skipped: config always wins a name
    /// collision, logged once here per call.
    pub async fn set_siblings(&self, siblings: Vec<(String, String, HashMap<String, String>)>) {
        let accepted: Vec<(String, String, HashMap<String, String>)> = {
            let agents = self.agents.read().await;
            siblings
                .into_iter()
                .filter(|(name, _, _)| {
                    let shadowed_by_config =
                        matches!(agents.get(name), Some(rec) if rec.source == AgentSource::Config);
                    if shadowed_by_config {
                        tracing::debug!(
                            agent = %name,
                            "relay sibling shadowed by a config/a2a.json entry of the same name"
                        );
                    }
                    !shadowed_by_config
                })
                .collect()
        };

        let names: HashSet<String> = accepted.iter().map(|(name, _, _)| name.clone()).collect();
        {
            let mut agents = self.agents.write().await;
            agents.retain(|name, rec| rec.source != AgentSource::Sibling || names.contains(name));
        }
        for (name, url, headers) in accepted {
            self.register_external(name, url, headers, AgentSource::Sibling)
                .await;
        }
    }

    /// Drop a registered agent entirely (any source).
    pub async fn remove(&self, name: &str) {
        self.agents.write().await.remove(name);
    }

    async fn refresh_card(&self, name: &str) {
        let Some((url, headers)) = ({
            let agents = self.agents.read().await;
            agents
                .get(name)
                .map(|rec| (rec.url.clone(), rec.headers.clone()))
        }) else {
            return;
        };

        let result = fetch_card(&url, &headers).await;
        let mut agents = self.agents.write().await;
        let Some(rec) = agents.get_mut(name) else {
            return;
        };
        match result {
            Ok(card) => {
                rec.status = AgentStatus::Ok(Arc::new(card));
                rec.retry_backoff = MIN_RETRY_BACKOFF;
            }
            Err(e) => {
                tracing::warn!(agent = %name, error = %e, "failed to fetch a2a agent card");
                rec.next_retry_at = Instant::now() + rec.retry_backoff;
                rec.retry_backoff = (rec.retry_backoff * 2).min(MAX_RETRY_BACKOFF);
                rec.status = AgentStatus::Error(e);
            }
        }
    }

    /// Spawn the background loop that retries card fetches for agents not
    /// currently `Ok`, honoring each agent's own backoff. Fire-and-forget:
    /// runs for the process lifetime, like
    /// `crate::gateway::file_server::FileRegistry::spawn_cleanup_task`.
    pub fn spawn_background_refresh(self: &Arc<Self>) {
        let hub = Arc::clone(self);
        crate::util::spawn_monitored("a2a-hub-refresh", async move {
            let mut interval = tokio::time::interval(REFRESH_TICK);
            loop {
                interval.tick().await;
                let due: Vec<String> = {
                    let agents = hub.agents.read().await;
                    let now = Instant::now();
                    agents
                        .iter()
                        .filter(|(_, rec)| {
                            !matches!(rec.status, AgentStatus::Ok(_)) && now >= rec.next_retry_at
                        })
                        .map(|(name, _)| name.clone())
                        .collect()
                };
                for name in due {
                    hub.refresh_card(&name).await;
                }
            }
        });
    }

    /// The agent's currently cached card, if the last fetch succeeded.
    pub async fn card_for(&self, name: &str) -> Option<Arc<a2a::AgentCard>> {
        let agents = self.agents.read().await;
        match &agents.get(name)?.status {
            AgentStatus::Ok(card) => Some(Arc::clone(card)),
            AgentStatus::Pending | AgentStatus::Error(_) => None,
        }
    }

    /// Build a fresh protocol client for `name`'s currently cached card,
    /// carrying its configured headers as auth interceptors.
    ///
    /// # Errors
    /// Returns [`HubError::Unknown`] if no such agent is registered,
    /// [`HubError::Offline`] if its card hasn't resolved successfully, and
    /// [`HubError::ClientBuild`] if a client transport couldn't be
    /// negotiated from the card.
    pub async fn client_for(
        &self,
        name: &str,
    ) -> Result<(NegotiatedClient, Arc<a2a::AgentCard>), HubError> {
        let (card, headers) = {
            let agents = self.agents.read().await;
            let rec = agents
                .get(name)
                .ok_or_else(|| HubError::Unknown(name.to_string()))?;
            match &rec.status {
                AgentStatus::Ok(card) => (Arc::clone(card), rec.headers.clone()),
                AgentStatus::Pending => {
                    return Err(HubError::Offline(
                        name.to_string(),
                        "still resolving its agent card".to_string(),
                    ));
                }
                AgentStatus::Error(e) => {
                    return Err(HubError::Offline(name.to_string(), e.clone()));
                }
            }
        };

        let mut builder = A2AClientFactory::builder();
        for (header_name, header_value) in &headers {
            builder = builder.with_interceptor(Arc::new(AuthInterceptor::custom(
                header_name.clone(),
                header_value.clone(),
            )));
        }
        let factory = builder.build();
        let client = factory
            .create_from_card(&card)
            .await
            .map_err(|e| HubError::ClientBuild(name.to_string(), e.to_string()))?;
        Ok((client, card))
    }
}

fn fresh_record(entry: A2aAgentEntry, source: AgentSource) -> AgentRecord {
    AgentRecord {
        url: entry.url,
        headers: entry.headers,
        source,
        status: AgentStatus::Pending,
        retry_backoff: MIN_RETRY_BACKOFF,
        next_retry_at: Instant::now(),
    }
}

/// Fetch `url`'s agent card, using a reqwest client that sends `headers` as
/// default headers on every request — including the card fetch itself, so a
/// private remote agent works from its configured headers.
async fn fetch_card(
    url: &str,
    headers: &HashMap<String, String>,
) -> Result<a2a::AgentCard, String> {
    let client = headers_client(headers)?;
    let resolver = a2a_client::agent_card::AgentCardResolver::new(Some(client));
    resolver.resolve(url).await.map_err(|e| e.to_string())
}

fn headers_client(headers: &HashMap<String, String>) -> Result<reqwest::Client, String> {
    let mut map = reqwest::header::HeaderMap::new();
    for (key, value) in headers {
        let name = reqwest::header::HeaderName::try_from(key.as_str())
            .map_err(|e| format!("invalid header name '{key}': {e}"))?;
        let val = reqwest::header::HeaderValue::from_str(value)
            .map_err(|e| format!("invalid header value for '{key}': {e}"))?;
        map.insert(name, val);
    }
    reqwest::Client::builder()
        .default_headers(map)
        .build()
        .map_err(|e| format!("failed to build http client: {e}"))
}

/// The client-facing name for a task state, used in outbound tool responses
/// and the tracked-task record. Distinct from the wire enum's Serialize
/// impl (`TASK_STATE_*`), so a user reading `list_agents` or `outbound.json`
/// sees a plain word rather than a protocol constant.
#[must_use]
pub(crate) fn task_state_str(state: &a2a::TaskState) -> &'static str {
    match state {
        a2a::TaskState::Unspecified => "unspecified",
        a2a::TaskState::Submitted => "submitted",
        a2a::TaskState::Working => "working",
        a2a::TaskState::Completed => "completed",
        a2a::TaskState::Failed => "failed",
        a2a::TaskState::Canceled => "canceled",
        a2a::TaskState::InputRequired => "input_required",
        a2a::TaskState::Rejected => "rejected",
        a2a::TaskState::AuthRequired => "auth_required",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_keys::AgentKeys;

    fn agent_keys() -> (tempfile::TempDir, SharedAgentKeys) {
        let dir = tempfile::tempdir().unwrap();
        let keys = AgentKeys::new_shared(dir.path());
        (dir, keys)
    }

    async fn spawn_card_server(name: &'static str) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let body = format!(
            r#"{{"name":"{name}","description":"a test agent","version":"1.0",
                "supportedInterfaces":[{{"url":"http://{addr}/","protocolBinding":"JSONRPC","protocolVersion":"1.0"}}],
                "capabilities":{{"streaming":true}},"defaultInputModes":["text/plain"],
                "defaultOutputModes":["text/plain"],"skills":[]}}"#
        );
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let body = body.clone();
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = [0_u8; 4096];
                    let _read_result = socket.read(&mut buf).await;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        body.len(),
                        body,
                    );
                    socket.write_all(response.as_bytes()).await.ok();
                });
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn unknown_agent_is_an_error() {
        let hub = A2aClientHub::new();
        let Err(err) = hub.client_for("nope").await else {
            panic!("expected an error for an unregistered agent");
        };
        assert!(matches!(err, HubError::Unknown(name) if name == "nope"));
    }

    #[tokio::test]
    async fn register_external_fetches_card_and_becomes_ok() {
        let hub = A2aClientHub::new();
        let url = spawn_card_server("laptop-agent").await;
        hub.register_external(
            "laptop".to_string(),
            url,
            HashMap::new(),
            AgentSource::Config,
        )
        .await;
        let snap = hub.snapshot().await;
        let [only] = snap.as_slice() else {
            panic!("expected exactly one agent, got {snap:?}");
        };
        assert_eq!(only.name, "laptop");
        assert!(matches!(only.status, AgentStatus::Ok(_)));
        assert_eq!(only.card().unwrap().name, "laptop-agent");
    }

    #[tokio::test]
    async fn client_for_builds_a_client_once_card_is_ok() {
        let hub = A2aClientHub::new();
        let url = spawn_card_server("laptop-agent").await;
        hub.register_external(
            "laptop".to_string(),
            url,
            HashMap::new(),
            AgentSource::Config,
        )
        .await;
        let (client, card) = hub.client_for("laptop").await.unwrap();
        assert_eq!(card.name, "laptop-agent");
        drop(client);
    }

    #[tokio::test]
    async fn unreachable_agent_reports_offline() {
        let hub = A2aClientHub::new();
        hub.register_external(
            "ghost".to_string(),
            "http://127.0.0.1:1".to_string(),
            HashMap::new(),
            AgentSource::Config,
        )
        .await;
        let Err(err) = hub.client_for("ghost").await else {
            panic!("expected an error for an unreachable agent");
        };
        assert!(matches!(err, HubError::Offline(name, _) if name == "ghost"));
    }

    #[tokio::test]
    async fn reload_from_file_removes_config_agents_no_longer_present() {
        let hub = A2aClientHub::new();
        let (_keys_dir, keys) = agent_keys();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a2a.json");
        let url = spawn_card_server("laptop-agent").await;
        std::fs::write(
            &path,
            format!(r#"{{"agents":{{"laptop":{{"url":"{url}"}}}}}}"#),
        )
        .unwrap();
        hub.reload_from_file(&path, &keys).await;
        assert_eq!(hub.snapshot().await.len(), 1);

        std::fs::write(&path, r#"{"agents":{}}"#).unwrap();
        hub.reload_from_file(&path, &keys).await;
        assert!(hub.snapshot().await.is_empty());
    }

    #[tokio::test]
    async fn reload_from_file_leaves_sibling_agents_untouched() {
        let hub = A2aClientHub::new();
        let (_keys_dir, keys) = agent_keys();
        let sibling_url = spawn_card_server("sibling-agent").await;
        hub.register_external(
            "beta".to_string(),
            sibling_url,
            HashMap::new(),
            AgentSource::Sibling,
        )
        .await;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a2a.json");
        std::fs::write(&path, r#"{"agents":{}}"#).unwrap();
        hub.reload_from_file(&path, &keys).await;

        let snap = hub.snapshot().await;
        let [only] = snap.as_slice() else {
            panic!("expected exactly one agent, got {snap:?}");
        };
        assert_eq!(
            only.source,
            AgentSource::Sibling,
            "sibling agent must survive a config reload"
        );
    }

    #[tokio::test]
    async fn set_siblings_replaces_the_sibling_set() {
        let hub = A2aClientHub::new();
        let url_a = spawn_card_server("alpha").await;
        let url_b = spawn_card_server("beta").await;
        hub.set_siblings(vec![
            ("alpha".to_string(), url_a, HashMap::new()),
            ("beta".to_string(), url_b.clone(), HashMap::new()),
        ])
        .await;
        assert_eq!(hub.snapshot().await.len(), 2);

        hub.set_siblings(vec![("beta".to_string(), url_b, HashMap::new())])
            .await;
        let snap = hub.snapshot().await;
        let [only] = snap.as_slice() else {
            panic!("expected exactly one agent, got {snap:?}");
        };
        assert_eq!(only.name, "beta");
    }

    #[tokio::test]
    async fn set_siblings_does_not_shadow_a_config_entry_of_the_same_name() {
        let hub = A2aClientHub::new();
        let config_url = spawn_card_server("laptop-config").await;
        hub.register_external(
            "laptop".to_string(),
            config_url.clone(),
            HashMap::new(),
            AgentSource::Config,
        )
        .await;

        let sibling_url = spawn_card_server("laptop-sibling").await;
        hub.set_siblings(vec![("laptop".to_string(), sibling_url, HashMap::new())])
            .await;

        let snap = hub.snapshot().await;
        let [only] = snap.as_slice() else {
            panic!("expected exactly one agent, got {snap:?}");
        };
        assert_eq!(only.name, "laptop");
        assert_eq!(
            only.source,
            AgentSource::Config,
            "a config entry must win a name collision with a sibling"
        );
        assert_eq!(only.url, config_url, "the config entry's url must be kept");
    }

    #[tokio::test]
    async fn reload_from_file_overrides_a_sibling_of_the_same_name() {
        let hub = A2aClientHub::new();
        let (_keys_dir, keys) = agent_keys();
        let sibling_url = spawn_card_server("laptop-sibling").await;
        hub.register_external(
            "laptop".to_string(),
            sibling_url,
            HashMap::new(),
            AgentSource::Sibling,
        )
        .await;

        let config_url = spawn_card_server("laptop-config").await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a2a.json");
        std::fs::write(
            &path,
            format!(r#"{{"agents":{{"laptop":{{"url":"{config_url}"}}}}}}"#),
        )
        .unwrap();
        hub.reload_from_file(&path, &keys).await;

        let snap = hub.snapshot().await;
        let [only] = snap.as_slice() else {
            panic!("expected exactly one agent, got {snap:?}");
        };
        assert_eq!(only.name, "laptop");
        assert_eq!(
            only.source,
            AgentSource::Config,
            "a config entry must override a sibling of the same name"
        );
        assert_eq!(only.url, config_url);
    }

    #[test]
    fn task_state_strings() {
        assert_eq!(
            task_state_str(&a2a::TaskState::InputRequired),
            "input_required"
        );
        assert_eq!(task_state_str(&a2a::TaskState::Completed), "completed");
    }
}
